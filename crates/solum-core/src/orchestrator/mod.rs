//! The Agent Orchestrator — the local "brain" that wires the pieces together
//! (ARCHITECTURE.md §3). It owns the store, the extractor, the importance rule
//! table, the proactivity config, the HITL guard, and a small tool registry.
//!
//! The Phase 1 closed loop lives in [`Orchestrator::ingest`]:
//! `raw input → intent routing → extraction → classification → scheduling →
//! persistence`, all offline and deterministic given an injected `now`.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};

use crate::classify::{NotificationMatchKind, NotificationPriorityRule, RuleTable};
use crate::error::{CoreError, Result};
use crate::extract::{
    might_request_widget, route_intent, Extractor, Intent, Reasoner, RuleBasedExtractor,
};
use crate::guard::{ExecutionToken, Grant, Guard, PendingConfirmation, Tool, ToolCtx};
use crate::journal::{
    checkin_due, strip_status_prefix, BehaviorEntry, BehaviorKind, CHECKIN_QUESTION,
};
use crate::llm::{ChatContext, ChatTurn, MAX_HISTORY_TURNS};
use crate::memory::MemoryFact;
use crate::model::{fmt_ts, Event, EventKind, MemoryEntry, MemoryLayer, Notification, RiskLevel};
use crate::notification_intelligence::{
    batch_triage_prompt, content_hash, decide_intake, parse_batch_triage, priority_presets,
    ActionProposalState, CaptureLane, CaptureState, FilterProposalState, IntakeDecision,
    LlmTriageDecision, NotificationActionIntent, NotificationActionProposal, NotificationCapture,
    NotificationCaptureRecord, NotificationFilterProposal, NotificationFilterRule,
    NotificationIntelligenceConfig, DEDUP_WINDOW_MINUTES,
};
use crate::persona::{PersonaDraft, PersonaProfile};
use crate::proactivity::{ProactivityConfig, ProactivityDimension, ProactivityLevel};
use crate::recall::{Candidate, Snippet, SnippetLayer};
use crate::routine::Routine;
use crate::schedule::plan_notifications;
use crate::store::{new_guid, AuditRow, Store};
use crate::suggest::{self, Suggestion, SuggestionKind, SuggestionStatus};
use crate::wearable::HealthSample;
use crate::widget::{WidgetDefinition, WidgetDefinitionDraft, WidgetImportOutcome, WidgetRecord};

/// A validated definition awaiting an explicit human confirmation. The random
/// id carries no write power by itself: only this orchestrator instance can
/// turn it into a persistent definition, and it is consumed on confirmation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WidgetPreview {
    pub preview_id: String,
    pub definition: WidgetDefinitionDraft,
}

#[derive(Debug, Clone)]
struct PendingWidgetDefinition {
    definition: WidgetDefinitionDraft,
    raw_schema: String,
}

/// The result of ingesting one utterance.
#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub intent: Intent,
    pub raw_input_id: i64,
    /// The persisted event (id populated), if one was extracted.
    pub event: Option<Event>,
    /// Persisted notifications (ids populated) for the event.
    pub notifications: Vec<Notification>,
    /// A human-readable summary of what the agent did.
    pub message: String,
    /// Optional generative UI (F18, §3.9): an offline template for extracted
    /// events, or an LLM-assembled envelope for chat. Ephemeral by design —
    /// rendered once, never persisted (对话即焚).
    pub ui: Option<crate::genui::UiEnvelope>,
    /// F19 uses a separate persistent-data renderer, not the F18 envelope.
    /// A definition appears here only after strict validation and before the
    /// explicit confirmation that is required to write it.
    pub widget_preview: Option<WidgetPreview>,
}

/// Outcome of the two gates in front of auto-creating an event from a
/// notification. Distinct variants because the user-facing explanations differ:
/// "this app was never allowed to" vs "it was, but it has done too much today".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoEventVerdict {
    Allowed,
    NotGranted,
    RateLimited,
}

/// Device-local counters for notifications that never made it into the store.
const CAPTURE_OVERFLOW_KEY: &str = "capture_spool_overflow";

/// The receipt name for one overflow marker. Recording and releasing must
/// derive it the same way, or a release silently matches nothing and the
/// guarantee quietly degrades to the age-based behaviour this replaced.
fn overflow_receipt(name: impl AsRef<str>) -> String {
    format!("overflow:{}", name.as_ref())
}

pub struct Orchestrator {
    store: Store,
    extractor: RuleBasedExtractor,
    guard: Guard,
    rule_table: RuleTable,
    proactivity: ProactivityConfig,
    /// The active persona version, cached (like the rule table / proactivity).
    persona: Option<PersonaProfile>,
    tools: HashMap<String, Box<dyn Tool>>,
    /// Optional cloud reasoner (§3.6). `None` = fully offline; every use site
    /// must degrade gracefully so the offline floor keeps working (F16).
    reasoner: Option<Box<dyn Reasoner>>,
    /// M1 (§3.10): the last few chat turns, held in process memory only —
    /// never persisted, never synced, gone when the host exits.
    chat_history: VecDeque<ChatTurn>,
    /// Validated component definitions that have been previewed but not saved.
    pending_widgets: HashMap<String, PendingWidgetDefinition>,
}

impl Orchestrator {
    /// Open an orchestrator backed by a store at `path`.
    pub fn open(path: &str) -> Result<Self> {
        Self::from_store(Store::open(path)?)
    }

    /// Open an ephemeral in-memory orchestrator (tests / dry runs).
    pub fn in_memory() -> Result<Self> {
        Self::from_store(Store::open_in_memory()?)
    }

    fn from_store(store: Store) -> Result<Self> {
        let rule_table = store.load_rule_table()?;
        let proactivity = store.load_proactivity()?;
        let persona = store.active_persona()?;
        let mut tools: HashMap<String, Box<dyn Tool>> = HashMap::new();
        for t in builtin_tools() {
            tools.insert(t.name().to_string(), t);
        }
        Ok(Orchestrator {
            store,
            extractor: RuleBasedExtractor::new(),
            guard: Guard::new(),
            rule_table,
            proactivity,
            persona,
            tools,
            reasoner: None,
            chat_history: VecDeque::new(),
            pending_widgets: HashMap::new(),
        })
    }

    /// Attach a cloud reasoner (chat replies + extraction fallback).
    pub fn set_reasoner(&mut self, r: Box<dyn Reasoner>) {
        self.reasoner = Some(r);
    }

    /// Detach the cloud reasoner — back to fully offline (F16 floor). Used
    /// when the account session ends and no direct-key config exists.
    pub fn clear_reasoner(&mut self) {
        self.reasoner = None;
    }

    /// One sync round against the relay (F17, §3.8): push local changes,
    /// pull + merge everyone else's. Merged rows may replace the cached
    /// persona / rule table / proactivity, so the caches are reloaded.
    pub fn sync_now(
        &mut self,
        transport: &dyn crate::sync::SyncTransport,
        cfg: &crate::sync::SyncConfig,
    ) -> Result<crate::sync::SyncOutcome> {
        let outcome = crate::sync::sync_once(&self.store, transport, cfg)?;
        if outcome.applied > 0 {
            self.reload_caches()?;
        }
        Ok(outcome)
    }

    /// Re-read the state this orchestrator caches in memory.
    ///
    /// Needed by any caller that mutated the database *behind* this
    /// orchestrator — notably the desktop shell, which runs sync on its own
    /// connection so network latency never holds the orchestrator lock, and
    /// then calls this to pick up whatever the merge changed.
    pub fn reload_caches(&mut self) -> Result<()> {
        self.rule_table = self.store.load_rule_table()?;
        self.proactivity = self.store.load_proactivity()?;
        self.persona = self.store.active_persona()?;
        Ok(())
    }

    /// Ops held because this build could not interpret them, as
    /// `(still held, dropped for overflow)` — see §3.8 forward compatibility.
    pub fn sync_quarantine_stats(&self) -> Result<(i64, i64)> {
        self.store.sync_quarantine_stats()
    }

    /// Notifications the native listener had to drop because the capture spool
    /// was full — data the user was told would be captured and was not, so it
    /// is durable and surfaced rather than logged and forgotten.
    ///
    /// Only the *overflow* tally lives here. Payloads this side could not hand
    /// to the store are **not** counted with a counter: those files are
    /// retained on disk forever, so the honest figure is a live census of that
    /// directory, taken by the shell. Modelling a permanent directory as an
    /// event tally was wrong twice over — the count could miss a file when the
    /// process died between parking and counting, and it could double-count the
    /// same file once its idempotency receipt aged out from under it.
    pub fn capture_overflow_count(&self) -> Result<i64> {
        Ok(self
            .store
            .sync_state(CAPTURE_OVERFLOW_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// Count spool-overflow markers, **once each**.
    ///
    /// `names` are the marker filenames. Recording the count and deleting the
    /// files cannot be one atomic step (database + filesystem), so the receipt
    /// keyed on each name is what makes a crash in between harmless: a re-scan
    /// of the same markers adds nothing. Without that the total could exceed
    /// the true number of drops, and a figure that can overshoot is not the
    /// lower bound the UI presents it as.
    ///
    /// Returns how many were newly counted.
    pub fn note_capture_overflow(&self, names: &[String]) -> Result<usize> {
        let receipts: Vec<String> = names.iter().map(overflow_receipt).collect();
        self.store
            .record_capture_loss(CAPTURE_OVERFLOW_KEY, &receipts)
    }

    /// Retire the receipts for markers the caller has **already deleted**.
    ///
    /// Pass only names whose file removal succeeded. The receipt is what stops a
    /// still-present marker from being counted twice, so releasing one early —
    /// or on a timer, as this used to do — reopens that hole for any marker
    /// whose deletion keeps failing. Holding a receipt too long costs one row.
    pub fn release_capture_overflow(&self, deleted_names: &[String]) -> Result<usize> {
        let receipts: Vec<String> = deleted_names.iter().map(overflow_receipt).collect();
        self.store.release_capture_loss_receipts(&receipts)
    }

    /// Clear the overflow tally — the user has seen and acknowledged it.
    /// Explicit, because nothing else can know they have.
    ///
    /// Receipts are intentionally **not** cleared: they guard against
    /// re-counting marker files that may still be on disk, which has nothing to
    /// do with whether the user has read the notice. Nor is anything in
    /// `failed/` touched — that is retained data, not a notice.
    pub fn acknowledge_capture_losses(&self) -> Result<()> {
        self.store.clear_sync_state(CAPTURE_OVERFLOW_KEY)
    }

    /// Unopenable pulled blobs: `(still held, dropped for overflow)`.
    pub fn bad_blob_stats(&self) -> Result<(i64, i64)> {
        self.store.bad_blob_stats()
    }

    /// The sticky "this device missed swept history" marker, if set.
    pub fn sync_history_gap(&self) -> Result<Option<String>> {
        self.store.sync_state(crate::sync::HISTORY_GAP_KEY)
    }

    /// Why the push half of the last sync round could not finish, if it
    /// could not. Cleared automatically by the next round that pushes cleanly.
    pub fn sync_push_block(&self) -> Result<Option<String>> {
        self.store.sync_state(crate::sync::PUSH_BLOCKED_KEY)
    }

    /// Local ops parked as too large to ever upload (see `hold_push_op`).
    pub fn push_hold_count(&self) -> Result<i64> {
        self.store.push_hold_count()
    }

    /// This device's sync identity (shown in status output).
    pub fn sync_device_id(&self) -> Result<String> {
        self.store.device_id()
    }

    pub fn has_reasoner(&self) -> bool {
        self.reasoner.is_some()
    }


}


mod closed_loop;
mod queries;
mod routines_insights;
mod notifications;
mod widgets_persona;
pub(crate) mod tools;
pub(crate) use tools::*;

#[cfg(test)]
mod tests;

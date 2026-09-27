//! F20 notification intelligence: deterministic intake/routing primitives and
//! defensive parsing for LLM batch triage. Persistence and side effects live
//! in `store`/`orchestrator`; this module stays pure and clock-injected.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use chrono::NaiveDateTime;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::classify::{NotificationMatchKind, NotificationPriorityRule};
use crate::error::{CoreError, Result};
use crate::model::{parse_ts, Event, EventKind};
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use capture::{
    truncate_field, FilterProposalState, NotificationCapture, NotificationCaptureRecord,
    MAX_BATCH_CHARS, MAX_FIELD_CHARS,
};
pub use config::{
    CaptureLane, CaptureState, NotificationCaptureMode, NotificationFilterRule,
    NotificationIntelligenceConfig, DEDUP_WINDOW_MINUTES, MAX_AUTO_EVENTS_PER_APP_PER_DAY,
    MAX_BATCH_INTERVAL_MINUTES, MIN_BATCH_INTERVAL_MINUTES,
};
pub use proposals::{
    content_hash, decide_intake, priority_presets, starter_patterns_for_package,
    ActionProposalState, AppStarterRuleSpec, IntakeDecision, NotificationActionIntent,
    NotificationActionKind, NotificationActionProposal, NotificationFilterProposal,
    ACTION_PROPOSAL_TTL_HOURS, DEFAULT_STARTER_PATTERNS,
};
pub use triage::{batch_triage_prompt, fit_batch, parse_batch_triage, LlmTriageDecision};

mod capture;
mod config;
mod proposals;
#[cfg(test)]
mod tests;
mod triage;

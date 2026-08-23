//! Platform-neutral additions carried by `solum-export` v3.
//!
//! The existing v2 `_restore` section remains the authority for guid-bearing
//! SQLite rows. This module covers persistence that v2 cannot represent and
//! validates it without writing any store or platform storage.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::classify::NotificationMatchKind;
use crate::error::{CoreError, Result};
use crate::model::{fmt_ts, parse_ts};
use crate::notification_intelligence::{ActionProposalState, CaptureState, NotificationActionKind};
use crate::store::Store;

pub const PORTABLE_EXPORT_VERSION: u32 = 3;
pub const PORTABLE_FORMAT: &str = "solum-portable";
pub const PORTABLE_SCHEMA_VERSION: u32 = 1;

const MAX_CHAT_SESSIONS: usize = 40;
const MAX_MESSAGES_PER_SESSION: usize = 240;
const MAX_CHAT_TEXT_CHARS: usize = 2_400_000;
const MAX_MESSAGE_CHARS: usize = 12_000;
const MAX_TITLE_CHARS: usize = 80;
const MAX_DOCUMENTS: usize = 30;
const MAX_DOCUMENT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_DOCUMENT_TOTAL_BYTES: u64 = 600 * 1024 * 1024;
const MAX_PENDING_NOTIFICATIONS: usize = 10_000;
const MAX_NOTIFICATION_FIELD_CHARS: usize = 2_000;
const MAX_AUDIT_ROWS: usize = 100_000;
const MAX_JOURNAL_ROWS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPortableData {
    pub chat: PortableChat,
    pub documents: Vec<DocumentManifest>,
    pub pending_notifications: Vec<PendingNotificationInput>,
    #[serde(default)]
    pub llm: Option<PortableLlmSettings>,
    pub external_restore_intents: ExternalRestoreIntents,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableChat {
    pub sessions: Vec<PortableChatSession>,
    pub drafts: Vec<PortableChatDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableChatSession {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub messages: Vec<PortableChatMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortableChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableChatMessage {
    pub role: PortableChatRole,
    pub content: String,
    #[serde(default)]
    pub intent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableChatDraft {
    pub session_id: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentManifest {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub plaintext_size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingNotificationInput {
    pub package_name: String,
    pub title: String,
    pub body: String,
    pub received_at: String,
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableLlmSettings {
    pub base_url: String,
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalRestoreIntents {
    pub email_accounts: Vec<EmailRestoreIntent>,
    #[serde(default)]
    pub soulous: Option<SoulousRestoreIntent>,
    pub health_sources: Vec<String>,
    pub background_reminders: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailRestoreIntent {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoulousRestoreIntent {
    pub server_url: String,
    pub push_schedule_events: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSection {
    pub format: String,
    pub version: u32,
    pub notification_journal: PortableNotificationJournal,
    pub audit_log: Vec<PortableAuditRow>,
    pub chat: PortableChat,
    pub documents: Vec<DocumentManifest>,
    pub pending_notifications: Vec<PortablePendingNotification>,
    pub agent_settings: PortableAgentSettings,
    pub restore_intents: PortableRestoreIntents,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableNotificationJournal {
    pub captures: Vec<PortableCaptureRow>,
    pub filter_proposals: Vec<PortableFilterProposalRow>,
    pub action_proposals: Vec<PortableActionProposalRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableCaptureRow {
    pub id: String,
    pub raw_input_guid: String,
    pub event_guid: Option<String>,
    pub package_name: String,
    pub title: String,
    pub body: String,
    pub received_at: String,
    pub content_hash: String,
    pub local_only: bool,
    pub lane: String,
    pub source_state: CaptureState,
    pub restore_state: CaptureState,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableFilterProposalRow {
    pub id: String,
    pub package_name: Option<String>,
    pub pattern: String,
    pub matcher: NotificationMatchKind,
    pub reason: String,
    pub state: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableActionProposalRow {
    pub id: String,
    pub capture_id: String,
    pub kind: NotificationActionKind,
    pub event_guid: Option<String>,
    pub event_title: String,
    pub event_start: String,
    pub new_start: Option<String>,
    pub reason: String,
    pub source_state: ActionProposalState,
    pub restore_state: ActionProposalState,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableAuditRow {
    pub id: String,
    pub ts: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
    pub decision: String,
    pub token_id: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePendingNotification {
    pub id: String,
    pub package_name: String,
    pub title: String,
    pub body: String,
    pub received_at: String,
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableAgentSettings {
    pub llm: Option<PortableLlmSettings>,
    pub notification: PortableNotificationSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableNotificationSettings {
    pub batch_interval_minutes: u16,
    pub filter_rules: Vec<PortableNotificationFilterRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableNotificationFilterRule {
    pub id: String,
    pub pattern: String,
    pub package_name: Option<String>,
    pub matcher: NotificationMatchKind,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRestoreIntents {
    pub notification_cloud_was_enabled: bool,
    pub notification_packages: Vec<String>,
    pub auto_event_packages: Vec<String>,
    pub email_accounts: Vec<EmailRestoreIntent>,
    pub soulous: Option<SoulousRestoreIntent>,
    pub health_sources: Vec<String>,
    pub background_reminders: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRecoveryPlan {
    pub counts: Vec<(String, usize)>,
    pub attachment_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortableStoreSnapshot {
    pub counts: Vec<(String, usize)>,
    pub fingerprint: String,
}

/// Produce a deterministic logical snapshot of every SQLite-backed domain
/// represented by the portable export. Platform-owned data is deliberately
/// empty: callers fingerprint that storage independently in its owning
/// runtime. This function does not expose row contents to its caller.
pub fn snapshot_portable_store(store: &Store) -> Result<PortableStoreSnapshot> {
    let fixed_time = NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .ok_or_else(|| invalid("portable snapshot timestamp is invalid"))?;
    let document = build_portable_export(
        store,
        fixed_time,
        PlatformPortableData {
            chat: PortableChat::default(),
            documents: Vec::new(),
            pending_notifications: Vec::new(),
            llm: None,
            external_restore_intents: ExternalRestoreIntents::default(),
        },
    )?;
    let plan = crate::export::plan_import(&document)?;
    let encoded = serde_json::to_vec(&document)?;
    let fingerprint = Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(PortableStoreSnapshot {
        counts: plan.counts,
        fingerprint,
    })
}

/// Build v3 from the current Store plus a same-profile platform snapshot.
pub fn build_portable_export(
    store: &Store,
    now: NaiveDateTime,
    platform: PlatformPortableData,
) -> Result<Value> {
    validate_platform_input(&platform)?;
    let notification_config = store.notification_intelligence_config()?;
    let section = PortableSection {
        format: PORTABLE_FORMAT.to_string(),
        version: PORTABLE_SCHEMA_VERSION,
        notification_journal: build_notification_journal(store)?,
        audit_log: build_audit_log(store)?,
        chat: platform.chat,
        documents: platform.documents,
        pending_notifications: build_pending_notifications(platform.pending_notifications)?,
        agent_settings: PortableAgentSettings {
            llm: platform.llm,
            notification: PortableNotificationSettings {
                batch_interval_minutes: notification_config.batch_interval_minutes,
                filter_rules: notification_config
                    .filter_rules
                    .into_iter()
                    .map(|rule| PortableNotificationFilterRule {
                        id: rule.id,
                        pattern: rule.pattern,
                        package_name: rule.package_name,
                        matcher: rule.matcher,
                        reason: rule.reason,
                    })
                    .collect(),
            },
        },
        restore_intents: PortableRestoreIntents {
            notification_cloud_was_enabled: store.notif_cloud_enabled()?,
            notification_packages: notification_config.allowed_packages,
            auto_event_packages: notification_config.auto_event_packages,
            email_accounts: platform.external_restore_intents.email_accounts,
            soulous: platform.external_restore_intents.soulous,
            health_sources: platform.external_restore_intents.health_sources,
            background_reminders: platform.external_restore_intents.background_reminders,
        },
    };

    let mut document = crate::export::build_export(store, now)?;
    document["version"] = json!(PORTABLE_EXPORT_VERSION);
    document["_portable"] = serde_json::to_value(section)?;
    validate_portable_document(&document)?;
    Ok(document)
}

pub fn validate_portable_document(document: &Value) -> Result<PortableRecoveryPlan> {
    if document.get("format").and_then(Value::as_str) != Some(crate::export::FORMAT)
        || document.get("version").and_then(Value::as_u64) != Some(PORTABLE_EXPORT_VERSION as u64)
    {
        return Err(invalid("document is not solum-export v3"));
    }
    let section: PortableSection = serde_json::from_value(
        document
            .get("_portable")
            .cloned()
            .ok_or_else(|| invalid("missing _portable"))?,
    )
    .map_err(|_| invalid("invalid _portable shape"))?;
    if section.format != PORTABLE_FORMAT || section.version != PORTABLE_SCHEMA_VERSION {
        return Err(invalid("unsupported _portable format or version"));
    }
    validate_section(&section, document.get("_restore"))?;

    let counts = vec![
        (
            "portable.notification_captures".into(),
            section.notification_journal.captures.len(),
        ),
        (
            "portable.filter_proposals".into(),
            section.notification_journal.filter_proposals.len(),
        ),
        (
            "portable.action_proposals".into(),
            section.notification_journal.action_proposals.len(),
        ),
        ("portable.audit_log".into(), section.audit_log.len()),
        ("portable.chat_sessions".into(), section.chat.sessions.len()),
        ("portable.chat_drafts".into(), section.chat.drafts.len()),
        ("portable.documents".into(), section.documents.len()),
        (
            "portable.pending_notifications".into(),
            section.pending_notifications.len(),
        ),
    ]
    .into_iter()
    .filter(|(_, count)| *count != 0)
    .collect();
    Ok(PortableRecoveryPlan {
        counts,
        attachment_bytes: section
            .documents
            .iter()
            .map(|item| item.plaintext_size)
            .sum(),
    })
}

fn build_notification_journal(store: &Store) -> Result<PortableNotificationJournal> {
    let mut captures = store.list_notification_captures()?;
    captures.sort_by_key(|row| row.id.unwrap_or_default());
    if captures.len() > MAX_JOURNAL_ROWS {
        return Err(invalid("notification capture count exceeds limit"));
    }
    let mut capture_ids = HashMap::new();
    let mut capture_ordinals = BTreeMap::new();
    let mut portable_captures = Vec::with_capacity(captures.len());
    for capture in captures {
        let local_id = capture
            .id
            .ok_or_else(|| invalid("capture missing local id"))?;
        let raw_input_guid = store.raw_input_guid_for_backup(capture.raw_input_id)?;
        let event_guid = capture
            .event_id
            .map(|id| store.event_guid_for_backup(id))
            .transpose()?
            .flatten();
        let identity = json!({ "raw_input_guid": raw_input_guid });
        let id = next_stable_id("capture", &identity, &mut capture_ordinals)?;
        capture_ids.insert(local_id, id.clone());
        let restore_state = if capture.state == CaptureState::Queued {
            CaptureState::NeedsReview
        } else {
            capture.state
        };
        portable_captures.push(PortableCaptureRow {
            id,
            raw_input_guid,
            event_guid,
            package_name: capture.package_name,
            title: capture.title,
            body: capture.body,
            received_at: fmt_ts(&capture.received_at),
            content_hash: capture.content_hash,
            local_only: capture.local_only,
            lane: capture.lane.as_str().to_string(),
            source_state: capture.state,
            restore_state,
            reason: capture.reason,
            created_at: fmt_ts(&capture.created_at),
        });
    }

    let mut filters = store.list_notification_filter_proposals()?;
    filters.sort_by_key(|row| row.id.unwrap_or_default());
    if filters.len() > MAX_JOURNAL_ROWS {
        return Err(invalid("filter proposal count exceeds limit"));
    }
    let mut filter_ordinals = BTreeMap::new();
    let mut portable_filters = Vec::with_capacity(filters.len());
    for row in filters {
        let created_at = fmt_ts(&row.created_at);
        let identity = json!({
            "package_name": row.package_name,
            "pattern": row.pattern,
            "matcher": row.matcher,
            "reason": row.reason,
            "created_at": created_at,
        });
        portable_filters.push(PortableFilterProposalRow {
            id: next_stable_id("filter-proposal", &identity, &mut filter_ordinals)?,
            package_name: row.package_name,
            pattern: row.pattern,
            matcher: row.matcher,
            reason: row.reason,
            state: row.state.as_str().to_string(),
            created_at,
        });
    }

    let mut actions = store.list_notification_action_proposals()?;
    actions.sort_by_key(|row| row.id.unwrap_or_default());
    if actions.len() > MAX_JOURNAL_ROWS {
        return Err(invalid("action proposal count exceeds limit"));
    }
    let mut action_ordinals = BTreeMap::new();
    let mut portable_actions = Vec::with_capacity(actions.len());
    for row in actions {
        let capture_id = capture_ids
            .get(&row.capture_id)
            .cloned()
            .ok_or_else(|| invalid("action proposal has no capture"))?;
        let event_guid = store
            .event_guid_for_backup(row.event_id)?
            .or_else(|| is_lower_hex(&row.event_guid, 32).then(|| row.event_guid.clone()));
        let event_start = fmt_ts(&row.event_start);
        let new_start = row.new_start.map(|value| fmt_ts(&value));
        let created_at = fmt_ts(&row.created_at);
        let identity = json!({
            "capture_id": capture_id,
            "kind": row.kind,
            "event_guid": event_guid,
            "event_title": row.event_title,
            "event_start": event_start,
            "new_start": new_start,
            "reason": row.reason,
            "created_at": created_at,
        });
        let restore_state = if row.state == ActionProposalState::Pending {
            ActionProposalState::Stale
        } else {
            row.state
        };
        portable_actions.push(PortableActionProposalRow {
            id: next_stable_id("action-proposal", &identity, &mut action_ordinals)?,
            capture_id,
            kind: row.kind,
            event_guid,
            event_title: row.event_title,
            event_start,
            new_start,
            reason: row.reason,
            source_state: row.state,
            restore_state,
            created_at,
        });
    }

    Ok(PortableNotificationJournal {
        captures: portable_captures,
        filter_proposals: portable_filters,
        action_proposals: portable_actions,
    })
}

fn build_audit_log(store: &Store) -> Result<Vec<PortableAuditRow>> {
    let rows = store.list_audit()?;
    if rows.len() > MAX_AUDIT_ROWS {
        return Err(invalid("audit row count exceeds limit"));
    }
    let mut ordinals = BTreeMap::new();
    rows.into_iter()
        .map(|row| {
            let identity = json!({
                "ts": row.ts,
                "tool": row.tool,
                "risk": row.risk,
                "summary": row.summary,
                "decision": row.decision,
                "token_id": row.token_id,
                "detail": row.detail,
            });
            Ok(PortableAuditRow {
                id: next_stable_id("audit", &identity, &mut ordinals)?,
                ts: row.ts,
                tool: row.tool,
                risk: row.risk,
                summary: row.summary,
                decision: row.decision,
                token_id: row.token_id,
                detail: row.detail,
            })
        })
        .collect()
}

fn build_pending_notifications(
    rows: Vec<PendingNotificationInput>,
) -> Result<Vec<PortablePendingNotification>> {
    if rows.len() > MAX_PENDING_NOTIFICATIONS {
        return Err(invalid("pending notification count exceeds limit"));
    }
    let mut ordinals = BTreeMap::new();
    rows.into_iter()
        .map(|row| {
            let identity = json!({
                "package_name": row.package_name,
                "title": row.title,
                "body": row.body,
                "received_at": row.received_at,
            });
            Ok(PortablePendingNotification {
                id: next_stable_id("pending-notification", &identity, &mut ordinals)?,
                package_name: row.package_name,
                title: row.title,
                body: row.body,
                received_at: row.received_at,
                failed: row.failed,
            })
        })
        .collect()
}

fn validate_platform_input(platform: &PlatformPortableData) -> Result<()> {
    validate_chat(&platform.chat)?;
    validate_documents(&platform.documents)?;
    validate_pending_inputs(&platform.pending_notifications)?;
    if let Some(llm) = &platform.llm {
        validate_llm(llm)?;
    }
    validate_external_intents(&platform.external_restore_intents)
}

fn validate_section(section: &PortableSection, restore: Option<&Value>) -> Result<()> {
    validate_chat(&section.chat)?;
    validate_documents(&section.documents)?;
    if let Some(llm) = &section.agent_settings.llm {
        validate_llm(llm)?;
    }
    validate_notification_settings(&section.agent_settings.notification)?;
    validate_restore_intents(&section.restore_intents)?;

    let restore = restore
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("_restore must be an object"))?;
    let raw_guids = restore_guids(restore.get("raw_inputs"))?;
    validate_journal(&section.notification_journal, &raw_guids)?;
    validate_audit(&section.audit_log)?;
    validate_pending_rows(&section.pending_notifications)
}

pub(crate) fn validate_chat(chat: &PortableChat) -> Result<()> {
    if chat.sessions.len() > MAX_CHAT_SESSIONS || chat.drafts.len() > MAX_CHAT_SESSIONS {
        return Err(invalid("chat count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut total_chars = 0usize;
    for session in &chat.sessions {
        validate_local_id(&session.id)?;
        if !ids.insert(session.id.as_str()) {
            return Err(invalid("duplicate chat session id"));
        }
        validate_rfc3339(&session.updated_at)?;
        if session.title.chars().count() > MAX_TITLE_CHARS
            || session.messages.len() > MAX_MESSAGES_PER_SESSION
        {
            return Err(invalid("chat session exceeds limit"));
        }
        total_chars = total_chars.saturating_add(session.title.chars().count());
        for message in &session.messages {
            let length = message.content.chars().count();
            if length > MAX_MESSAGE_CHARS || message.intent.chars().count() > 128 {
                return Err(invalid("chat message exceeds limit"));
            }
            total_chars = total_chars.saturating_add(length);
        }
    }
    let mut draft_ids = BTreeSet::new();
    for draft in &chat.drafts {
        if !ids.contains(draft.session_id.as_str())
            || !draft_ids.insert(draft.session_id.as_str())
            || draft.content.chars().count() > MAX_MESSAGE_CHARS
        {
            return Err(invalid("invalid chat draft"));
        }
        total_chars = total_chars.saturating_add(draft.content.chars().count());
    }
    if total_chars > MAX_CHAT_TEXT_CHARS {
        return Err(invalid("chat text exceeds total limit"));
    }
    Ok(())
}

pub(crate) fn validate_documents(documents: &[DocumentManifest]) -> Result<()> {
    if documents.len() > MAX_DOCUMENTS {
        return Err(invalid("document count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut total = 0u64;
    for document in documents {
        validate_local_id(&document.id)?;
        if !ids.insert(document.id.as_str())
            || document.name.is_empty()
            || document.name.chars().count() > 255
            || document.media_type.chars().count() > 127
            || document.plaintext_size > MAX_DOCUMENT_BYTES
            || !is_lower_hex(&document.sha256, 64)
        {
            return Err(invalid("invalid document manifest"));
        }
        total = total
            .checked_add(document.plaintext_size)
            .ok_or_else(|| invalid("document bytes overflow"))?;
    }
    if total > MAX_DOCUMENT_TOTAL_BYTES {
        return Err(invalid("document bytes exceed total limit"));
    }
    Ok(())
}

fn validate_pending_inputs(rows: &[PendingNotificationInput]) -> Result<()> {
    if rows.len() > MAX_PENDING_NOTIFICATIONS {
        return Err(invalid("pending notification count exceeds limit"));
    }
    for row in rows {
        validate_pending_fields(&row.package_name, &row.title, &row.body, &row.received_at)?;
    }
    Ok(())
}

fn validate_pending_rows(rows: &[PortablePendingNotification]) -> Result<()> {
    if rows.len() > MAX_PENDING_NOTIFICATIONS {
        return Err(invalid("pending notification count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in rows {
        validate_pending_fields(&row.package_name, &row.title, &row.body, &row.received_at)?;
        let identity = json!({
            "package_name": row.package_name,
            "title": row.title,
            "body": row.body,
            "received_at": row.received_at,
        });
        let expected = next_stable_id("pending-notification", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid pending notification id"));
        }
    }
    Ok(())
}

fn validate_pending_fields(
    package: &str,
    title: &str,
    body: &str,
    received_at: &str,
) -> Result<()> {
    if package.is_empty()
        || package.chars().count() > 255
        || title.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
        || body.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
    {
        return Err(invalid("pending notification field exceeds limit"));
    }
    validate_rfc3339(received_at)
}

pub(crate) fn validate_llm(llm: &PortableLlmSettings) -> Result<()> {
    crate::net::validate_endpoint(&llm.base_url, "backup LLM base_url")?;
    if llm.model.trim().is_empty()
        || llm.model.chars().count() > 200
        || llm.timeout_secs < 3
        || llm.timeout_secs > 600
        || llm.max_tokens == Some(0)
        || llm.temperature.is_some_and(|value| !value.is_finite())
    {
        return Err(invalid("invalid LLM settings"));
    }
    Ok(())
}

fn validate_external_intents(intents: &ExternalRestoreIntents) -> Result<()> {
    validate_email_intents(&intents.email_accounts)?;
    validate_soulous_intent(intents.soulous.as_ref())?;
    validate_string_set(&intents.health_sources, 32, 128)
}

pub(crate) fn validate_restore_intents(intents: &PortableRestoreIntents) -> Result<()> {
    validate_string_set(&intents.notification_packages, 256, 255)?;
    validate_string_set(&intents.auto_event_packages, 256, 255)?;
    if !intents
        .auto_event_packages
        .iter()
        .all(|item| intents.notification_packages.contains(item))
    {
        return Err(invalid("auto-event intent is not a capture intent"));
    }
    validate_email_intents(&intents.email_accounts)?;
    validate_soulous_intent(intents.soulous.as_ref())?;
    validate_string_set(&intents.health_sources, 32, 128)
}

fn validate_email_intents(accounts: &[EmailRestoreIntent]) -> Result<()> {
    if accounts.len() > 12 {
        return Err(invalid("email intent count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    for account in accounts {
        validate_local_id(&account.id)?;
        if !ids.insert(account.id.as_str())
            || account.label.is_empty()
            || account.label.chars().count() > 80
            || account.provider.is_empty()
            || account.provider.chars().count() > 40
            || account.address.is_empty()
            || account.address.chars().count() > 320
        {
            return Err(invalid("invalid email restore intent"));
        }
    }
    Ok(())
}

fn validate_soulous_intent(intent: Option<&SoulousRestoreIntent>) -> Result<()> {
    if let Some(intent) = intent {
        crate::net::validate_endpoint(&intent.server_url, "backup Soulous server_url")?;
    }
    Ok(())
}

fn validate_notification_settings(settings: &PortableNotificationSettings) -> Result<()> {
    if !(crate::notification_intelligence::MIN_BATCH_INTERVAL_MINUTES
        ..=crate::notification_intelligence::MAX_BATCH_INTERVAL_MINUTES)
        .contains(&settings.batch_interval_minutes)
        || settings.filter_rules.len() > 1_000
    {
        return Err(invalid("invalid notification settings"));
    }
    let mut ids = BTreeSet::new();
    for rule in &settings.filter_rules {
        validate_local_id(&rule.id)?;
        if !ids.insert(rule.id.as_str())
            || rule.pattern.is_empty()
            || rule.pattern.chars().count() > 160
            || rule.reason.chars().count() > 500
        {
            return Err(invalid("invalid notification filter rule"));
        }
    }
    Ok(())
}

fn validate_journal(
    journal: &PortableNotificationJournal,
    raw_guids: &BTreeSet<String>,
) -> Result<()> {
    if journal.captures.len() > MAX_JOURNAL_ROWS
        || journal.filter_proposals.len() > MAX_JOURNAL_ROWS
        || journal.action_proposals.len() > MAX_JOURNAL_ROWS
    {
        return Err(invalid("notification journal count exceeds limit"));
    }
    let mut capture_ids = BTreeSet::new();
    let mut capture_ordinals = BTreeMap::new();
    for row in &journal.captures {
        if !raw_guids.contains(&row.raw_input_guid)
            || !is_lower_hex(&row.raw_input_guid, 32)
            || row
                .event_guid
                .as_ref()
                .is_some_and(|id| !is_lower_hex(id, 32))
            || row.package_name.is_empty()
            || row.title.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
            || row.body.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
            || !is_lower_hex(&row.content_hash, 16)
            || !matches!(row.lane.as_str(), "urgent" | "batch")
            || row.restore_state
                != if row.source_state == CaptureState::Queued {
                    CaptureState::NeedsReview
                } else {
                    row.source_state
                }
        {
            return Err(invalid("invalid portable capture"));
        }
        parse_ts(&row.received_at)?;
        parse_ts(&row.created_at)?;
        let expected = next_stable_id(
            "capture",
            &json!({ "raw_input_guid": row.raw_input_guid }),
            &mut capture_ordinals,
        )?;
        if row.id != expected || !capture_ids.insert(row.id.as_str()) {
            return Err(invalid("invalid portable capture id"));
        }
    }

    validate_filter_rows(&journal.filter_proposals)?;
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in &journal.action_proposals {
        if !capture_ids.contains(row.capture_id.as_str())
            || row
                .event_guid
                .as_ref()
                .is_some_and(|id| !is_lower_hex(id, 32))
            || row.restore_state
                != if row.source_state == ActionProposalState::Pending {
                    ActionProposalState::Stale
                } else {
                    row.source_state
                }
        {
            return Err(invalid("invalid portable action proposal"));
        }
        parse_ts(&row.event_start)?;
        row.new_start.as_deref().map(parse_ts).transpose()?;
        parse_ts(&row.created_at)?;
        let identity = json!({
            "capture_id": row.capture_id,
            "kind": row.kind,
            "event_guid": row.event_guid,
            "event_title": row.event_title,
            "event_start": row.event_start,
            "new_start": row.new_start,
            "reason": row.reason,
            "created_at": row.created_at,
        });
        let expected = next_stable_id("action-proposal", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid portable action proposal id"));
        }
    }
    Ok(())
}

fn validate_filter_rows(rows: &[PortableFilterProposalRow]) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in rows {
        parse_ts(&row.created_at)?;
        if !matches!(row.state.as_str(), "pending" | "accepted" | "dismissed") {
            return Err(invalid("invalid filter proposal state"));
        }
        let identity = json!({
            "package_name": row.package_name,
            "pattern": row.pattern,
            "matcher": row.matcher,
            "reason": row.reason,
            "created_at": row.created_at,
        });
        let expected = next_stable_id("filter-proposal", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid filter proposal id"));
        }
    }
    Ok(())
}

fn validate_audit(rows: &[PortableAuditRow]) -> Result<()> {
    if rows.len() > MAX_AUDIT_ROWS {
        return Err(invalid("audit row count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in rows {
        parse_ts(&row.ts)?;
        let identity = json!({
            "ts": row.ts,
            "tool": row.tool,
            "risk": row.risk,
            "summary": row.summary,
            "decision": row.decision,
            "token_id": row.token_id,
            "detail": row.detail,
        });
        let expected = next_stable_id("audit", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid audit id"));
        }
    }
    Ok(())
}

fn restore_guids(rows: Option<&Value>) -> Result<BTreeSet<String>> {
    rows.and_then(Value::as_array)
        .ok_or_else(|| invalid("missing restore table"))?
        .iter()
        .map(|row| {
            row.get("guid")
                .and_then(Value::as_str)
                .filter(|guid| is_lower_hex(guid, 32))
                .map(ToOwned::to_owned)
                .ok_or_else(|| invalid("invalid restore guid"))
        })
        .collect()
}

fn next_stable_id(
    domain: &str,
    identity: &Value,
    ordinals: &mut BTreeMap<Vec<u8>, u64>,
) -> Result<String> {
    let canonical = canonical_json(identity)?;
    let ordinal = ordinals.entry(canonical.clone()).or_default();
    let mut hasher = Sha256::new();
    hasher.update(b"solum-export:v3:");
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(&canonical);
    hasher.update([0]);
    hasher.update(ordinal.to_string().as_bytes());
    let digest = hasher.finalize();
    *ordinal += 1;
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    fn write(value: &Value, out: &mut Vec<u8>) -> Result<()> {
        match value {
            Value::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    write(item, out)?;
                }
                out.push(b']');
            }
            Value::Object(object) => {
                out.push(b'{');
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)?;
                    out.push(b':');
                    write(&object[key], out)?;
                }
                out.push(b'}');
            }
            scalar => serde_json::to_writer(out, scalar)?,
        }
        Ok(())
    }
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

fn validate_local_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._:".contains(&byte))
    {
        return Err(invalid("invalid portable local id"));
    }
    Ok(())
}

fn validate_rfc3339(value: &str) -> Result<()> {
    DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| invalid("invalid RFC3339 timestamp"))
}

fn validate_string_set(values: &[String], max_count: usize, max_chars: usize) -> Result<()> {
    if values.len() > max_count {
        return Err(invalid("string set exceeds count limit"));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        if value.is_empty() || value.chars().count() > max_chars || !unique.insert(value.as_str()) {
            return Err(invalid("invalid string set"));
        }
    }
    Ok(())
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(message: &str) -> CoreError {
    CoreError::Invalid(format!("solum-export v3: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{AuditEntry, Decision};
    use crate::model::{Event, EventKind, RiskLevel};
    use crate::notification_intelligence::{
        CaptureLane, NotificationActionProposal, NotificationCapture,
    };
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 23)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    pub(crate) fn sample_platform() -> PlatformPortableData {
        PlatformPortableData {
            chat: PortableChat {
                sessions: vec![PortableChatSession {
                    id: "session-public-fixture".into(),
                    title: "Synthetic session".into(),
                    updated_at: "2026-08-23T02:00:00.000Z".into(),
                    messages: vec![
                        PortableChatMessage {
                            role: PortableChatRole::User,
                            content: "Plan a synthetic meeting".into(),
                            intent: "chat".into(),
                        },
                        PortableChatMessage {
                            role: PortableChatRole::Assistant,
                            content: "Synthetic reply".into(),
                            intent: String::new(),
                        },
                    ],
                }],
                drafts: vec![PortableChatDraft {
                    session_id: "session-public-fixture".into(),
                    content: "Synthetic draft".into(),
                }],
            },
            documents: vec![DocumentManifest {
                id: "document-public-fixture".into(),
                name: "synthetic.txt".into(),
                media_type: "text/plain".into(),
                plaintext_size: 17,
                sha256: "00".repeat(32),
            }],
            pending_notifications: vec![PendingNotificationInput {
                package_name: "dev.solum.synthetic".into(),
                title: "Synthetic notification".into(),
                body: "No personal data".into(),
                received_at: "2026-08-23T02:00:00.000Z".into(),
                failed: true,
            }],
            llm: Some(PortableLlmSettings {
                base_url: "https://example.com/v1".into(),
                model: "synthetic-model".into(),
                temperature: Some(0.3),
                max_tokens: Some(512),
                timeout_secs: 30,
            }),
            external_restore_intents: ExternalRestoreIntents {
                email_accounts: vec![EmailRestoreIntent {
                    id: "mail-public-fixture".into(),
                    label: "Synthetic mail".into(),
                    provider: "custom".into(),
                    address: "fixture@example.invalid".into(),
                }],
                soulous: Some(SoulousRestoreIntent {
                    server_url: "https://example.invalid".into(),
                    push_schedule_events: false,
                }),
                health_sources: vec!["synthetic-health".into()],
                background_reminders: true,
            },
        }
    }

    #[test]
    fn portable_store_snapshot_is_stable_and_tracks_logical_changes() {
        let store = Store::open(":memory:").unwrap();
        let first = snapshot_portable_store(&store).unwrap();
        assert_eq!(first, snapshot_portable_store(&store).unwrap());
        assert_eq!(first.fingerprint.len(), 64);

        store.set_notif_cloud_enabled(true).unwrap();
        let changed = snapshot_portable_store(&store).unwrap();
        assert_ne!(first.fingerprint, changed.fingerprint);
    }

    #[test]
    fn v3_builds_a_strict_recovery_plan_without_secrets_or_partial_store_import() {
        let source = Store::open_in_memory().unwrap();
        let document = build_portable_export(&source, now(), sample_platform()).unwrap();
        assert_eq!(document["version"], PORTABLE_EXPORT_VERSION);

        let plan = crate::export::plan_import(&document).unwrap();
        assert_eq!(plan.version, PORTABLE_EXPORT_VERSION);
        assert!(plan.counts.contains(&("portable.chat_sessions".into(), 1)));
        assert!(plan.counts.contains(&("portable.documents".into(), 1)));
        let serialized = serde_json::to_string(&document).unwrap();
        for forbidden in ["api_key", "access_token", "refresh_token", "password"] {
            assert!(
                !serialized.contains(forbidden),
                "v3 leaked field {forbidden}"
            );
        }

        let target = Store::open_in_memory().unwrap();
        let error = crate::export::import_document(&target, &document, now()).unwrap_err();
        assert!(error.to_string().contains("平台临时库恢复事务"));
        assert!(target.list_raw_inputs().unwrap().is_empty());
    }

    #[test]
    fn v3_stable_ids_and_safe_restore_states_are_deterministic() {
        let store = Store::open_in_memory().unwrap();
        let capture = store
            .insert_notification_capture(
                &NotificationCapture {
                    package_name: "dev.solum.synthetic".into(),
                    title: "Synthetic".into(),
                    body: "Fixture".into(),
                    received_at: now(),
                },
                true,
                CaptureLane::Batch,
                CaptureState::Queued,
                None,
            )
            .unwrap();
        let event = Event::new(
            "Synthetic event",
            EventKind::Meeting,
            now(),
            "fixture",
            now(),
        );
        let event_id = store.insert_event(&event, None).unwrap();
        let event_guid = store.event_guid_and_local_only(event_id).unwrap().0;
        store
            .insert_notification_action_proposal(&NotificationActionProposal {
                id: None,
                capture_id: capture.id.unwrap(),
                kind: NotificationActionKind::CancelEvent,
                event_id,
                event_title: event.title,
                event_guid,
                event_start: event.start,
                new_start: None,
                reason: "Synthetic reason".into(),
                state: ActionProposalState::Pending,
                created_at: now(),
            })
            .unwrap();
        store
            .append_audit(&AuditEntry {
                ts: now(),
                tool: "synthetic_tool".into(),
                risk: RiskLevel::Safe,
                summary: "Synthetic audit".into(),
                decision: Decision::Executed,
                token_id: None,
                detail: "fixture".into(),
            })
            .unwrap();

        let first = build_portable_export(&store, now(), sample_platform()).unwrap();
        let second = build_portable_export(&store, now(), sample_platform()).unwrap();
        assert_eq!(first["_portable"], second["_portable"]);
        let capture = &first["_portable"]["notification_journal"]["captures"][0];
        assert_eq!(capture["source_state"], "queued");
        assert_eq!(capture["restore_state"], "needs_review");
        assert!(is_lower_hex(capture["id"].as_str().unwrap(), 64));
        assert!(is_lower_hex(
            first["_portable"]["audit_log"][0]["id"].as_str().unwrap(),
            64
        ));
        let action = &first["_portable"]["notification_journal"]["action_proposals"][0];
        assert_eq!(action["source_state"], "pending");
        assert_eq!(action["restore_state"], "stale");
    }

    #[test]
    fn v3_rejects_unknown_fields_identity_tampering_and_limits() {
        let store = Store::open_in_memory().unwrap();
        let document = build_portable_export(&store, now(), sample_platform()).unwrap();

        let mut unknown = document.clone();
        unknown["_portable"]["secret"] = json!("must be rejected");
        assert!(validate_portable_document(&unknown).is_err());

        let mut secret_field = document.clone();
        secret_field["_portable"]["agent_settings"]["llm"]["api_key"] =
            json!("fixture-secret-must-not-be-accepted");
        assert!(validate_portable_document(&secret_field).is_err());

        let mut changed_id = document.clone();
        changed_id["_portable"]["pending_notifications"][0]["id"] = json!("00".repeat(32));
        assert!(validate_portable_document(&changed_id).is_err());

        let mut oversized = document.clone();
        oversized["_portable"]["documents"][0]["plaintext_size"] = json!(MAX_DOCUMENT_BYTES + 1);
        assert!(validate_portable_document(&oversized).is_err());

        let mut dangling_draft = document;
        dangling_draft["_portable"]["chat"]["drafts"][0]["session_id"] = json!("missing-session");
        assert!(validate_portable_document(&dangling_draft).is_err());
    }
}

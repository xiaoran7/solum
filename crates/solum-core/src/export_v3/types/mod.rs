//! `types` -- every shape the portable document is made of: chat, documents, pending notifications, journal and audit rows, settings, restore intents and the store snapshot.
//!
//! Part of the `export_v3` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

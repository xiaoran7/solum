//! `capture` -- one captured notification and its stored form. Field truncation is the privacy boundary: `MAX_FIELD_CHARS` caps what leaves the device in any single field, and it is applied on the way in rather than on the way out, so a truncated capture is never written to the database in the first place.
//!
//! Part of the `notification_intelligence` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCapture {
    pub package_name: String,
    pub title: String,
    pub body: String,
    pub received_at: NaiveDateTime,
}

/// Per-field ceiling for captured notification text.
///
/// A notification is written by another app, and nothing stops that app —
/// buggy or hostile — from posting a megabyte of text. Unbounded, that text is
/// written to the inbox file, read into memory, stored in SQLite, and then
/// concatenated into a cloud prompt: disk, memory, cloud spend, and the volume
/// of personal content leaving the device all scale with whatever a third
/// party decided to put there. No real notification needs this much.
pub const MAX_FIELD_CHARS: usize = 2_000;
/// Ceiling on the text of one batch handed to the model. 24 captures × 2 000
/// chars is already generous; past this the batch is trimmed rather than sent.
pub const MAX_BATCH_CHARS: usize = 24_000;

/// Truncate on a character boundary, marking that it happened so the user can
/// see the text was cut rather than silently wondering where it went.
pub fn truncate_field(value: &str) -> String {
    if value.chars().count() <= MAX_FIELD_CHARS {
        return value.to_string();
    }
    let mut out: String = value.chars().take(MAX_FIELD_CHARS).collect();
    out.push_str("…（已截断）");
    out
}

impl NotificationCapture {
    /// Apply the intake ceilings. Called at the boundary, before the text is
    /// hashed, stored, or shown — everything downstream then works with text
    /// that is known to be bounded.
    pub fn truncated(mut self) -> Self {
        self.title = truncate_field(&self.title);
        self.body = truncate_field(&self.body);
        self
    }

    pub fn text(&self) -> String {
        format!("{} {}", self.title.trim(), self.body.trim())
            .trim()
            .to_string()
    }

    pub fn raw_input(&self) -> String {
        format!("[通知·{}] {}", self.package_name, self.text())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCaptureRecord {
    pub id: Option<i64>,
    pub raw_input_id: i64,
    pub package_name: String,
    pub title: String,
    pub body: String,
    pub received_at: NaiveDateTime,
    pub content_hash: String,
    /// Copied from the originating raw input at capture time. Toggling the
    /// cloud setting later must not rewrite this provenance/sync decision.
    pub local_only: bool,
    pub lane: CaptureLane,
    pub state: CaptureState,
    pub reason: Option<String>,
    pub event_id: Option<i64>,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterProposalState {
    Pending,
    Accepted,
    Dismissed,
}

impl FilterProposalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
        }
    }
}

impl std::str::FromStr for FilterProposalState {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "dismissed" => Ok(Self::Dismissed),
            _ => Err(CoreError::Invalid(format!("未知通知过滤提议状态: {s}"))),
        }
    }
}

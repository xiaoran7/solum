//! Core domain model: events, risk levels, notifications, memory records.
//!
//! These types are storage- and UI-agnostic. Times are wall-clock
//! `NaiveDateTime` values (Phase 1 keeps timezone handling out of the core;
//! the caller injects "now" so the pure logic stays deterministic and
//! testable — see ARCHITECTURE.md §6 Phase 1 and docs/MISC.md).

use std::fmt;
use std::str::FromStr;

use chrono::NaiveDateTime;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::CoreError;

/// The canonical timestamp format used for storage and display.
pub const TS_FMT: &str = "%Y-%m-%dT%H:%M:%S";

/// Parse a timestamp in the canonical [`TS_FMT`] format.
pub fn parse_ts(s: &str) -> Result<NaiveDateTime, CoreError> {
    NaiveDateTime::parse_from_str(s, TS_FMT)
        .map_err(|e| CoreError::TimeParse(format!("{s:?}: {e}")))
}

/// Render a timestamp in the canonical [`TS_FMT`] format.
pub fn fmt_ts(dt: &NaiveDateTime) -> String {
    dt.format(TS_FMT).to_string()
}

/// Human-facing timestamp for conversational replies and ledger summaries:
/// minute precision, space separator — never used for storage or parsing
/// (that stays [`TS_FMT`]).
pub fn fmt_ts_human(dt: &NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

/// Kinds of schedulable event. The kind drives importance classification
/// (lead time + channels) — see [`crate::classify`] and ARCHITECTURE.md §3.1.
///
/// Six built-ins plus `Custom(slug)` for the categories a user creates in
/// settings. The wire form of a custom kind is the flat string
/// `custom:<slug>`, matching the Harmony client byte for byte — see
/// `classify.rs` for the surrounding document shape. Serde is implemented by
/// hand because the derived form for a newtype variant would be
/// `{"custom":"<slug>"}`, which the other client would not read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventKind {
    Exam,
    Meeting,
    Class,
    Deadline,
    Reminder,
    Other,
    Custom(String),
}

impl EventKind {
    /// The six built-in kinds, for iteration (e.g. seeding the default rule
    /// table). User-defined categories are *not* in here — they only exist
    /// inside a saved rule table, so every caller that iterates this array
    /// still sees a closed, exhaustive set.
    pub const BUILTINS: [EventKind; 6] = [
        EventKind::Exam,
        EventKind::Meeting,
        EventKind::Class,
        EventKind::Deadline,
        EventKind::Reminder,
        EventKind::Other,
    ];

    /// The wire/display string. Custom kinds carry their prefix in the slug
    /// match arm, so this really is the full `custom:<slug>` string.
    pub fn as_str(&self) -> &str {
        match self {
            EventKind::Exam => "exam",
            EventKind::Meeting => "meeting",
            EventKind::Class => "class",
            EventKind::Deadline => "deadline",
            EventKind::Reminder => "reminder",
            EventKind::Other => "other",
            EventKind::Custom(slug) => slug,
        }
    }

    /// Chinese display label for built-in kinds. A custom kind has no fixed
    /// label — the one the user typed lives in the rule table's
    /// `custom_kinds` entry, so fall back to the slug rather than inventing
    /// text here.
    pub fn label(&self) -> &str {
        match self {
            EventKind::Exam => "考试",
            EventKind::Meeting => "会议",
            EventKind::Class => "课程",
            EventKind::Deadline => "截止",
            EventKind::Reminder => "提醒",
            EventKind::Other => "事件",
            EventKind::Custom(_) => self.as_str(),
        }
    }

    /// The user-facing name of a custom kind, taken from the rule table.
    pub fn custom_label<'a>(&'a self, table: &'a crate::classify::RuleTable) -> Option<&'a str> {
        match self {
            EventKind::Custom(slug) => table.custom_label(slug),
            _ => None,
        }
    }

    /// True when the slug names a user-created category.
    pub fn is_custom(&self) -> bool {
        matches!(self, EventKind::Custom(_))
    }

    /// Built-in kinds, in the canonical order. `all()` is kept as the
    /// by-value spelling for the many `for kind in EventKind::all()` loops.
    pub fn all() -> [EventKind; 6] {
        Self::BUILTINS
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for EventKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EventKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        EventKind::from_str(&raw).map_err(|e| D::Error::custom(e.to_string()))
    }
}

impl FromStr for EventKind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "exam" => EventKind::Exam,
            "meeting" => EventKind::Meeting,
            "class" => EventKind::Class,
            "deadline" => EventKind::Deadline,
            "reminder" => EventKind::Reminder,
            "other" => EventKind::Other,
            other => {
                // 用户自建的分类是 `custom:<slug>`。这条分支很关键：events.kind
                // 是 TEXT 列，若这里对未知值直接报错，从别的端同步进来的自建
                // 分类会让整行事件读不出来（静默丢数据）。slug 的字符集与
                // 鸿蒙端 customKindSlug 一致，两端必须同步改。
                match custom_kind_key(other) {
                    Some(key) => EventKind::Custom(key),
                    None => {
                        return Err(CoreError::Invalid(format!("unknown event kind: {other}")))
                    }
                }
            }
        })
    }
}

/// Prefix marking a user-defined importance category.
pub const CUSTOM_KIND_PREFIX: &str = "custom:";

/// Longest accepted slug, matching the Harmony client.
const CUSTOM_KIND_SLUG_MAX: usize = 40;

/// Validate a bare slug: 1..=40 characters, CJK ideographs plus ASCII letters,
/// digits, `-` and `_`. Kept byte-identical to the Harmony `customKindSlug` so
/// the same names are accepted on both clients.
fn custom_kind_slug(bare: &str) -> Option<String> {
    if bare.is_empty() || bare.chars().count() > CUSTOM_KIND_SLUG_MAX {
        return None;
    }
    for c in bare.chars() {
        let han = matches!(c as u32, 0x4e00..=0x9fff);
        let ascii = c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if !han && !ascii {
            return None;
        }
    }
    Some(bare.to_string())
}

/// The **bare** slug for a user-created category, or `None` when the name
/// cannot be one. Either spelling is accepted on the way in.
///
/// Two forms exist on purpose, and mixing them up breaks sync silently:
///   * the bare slug — what the shared rule-table document stores, and what
///     `CustomKind::slug` holds;
///   * `custom:<slug>` — the wire form of `EventKind::Custom`, and the key
///     events are stored under.
pub fn custom_kind_slug_value(slug: &str) -> Option<String> {
    custom_kind_slug(slug.strip_prefix(CUSTOM_KIND_PREFIX).unwrap_or(slug))
}

/// The `EventKind::Custom` storage key. The `custom:` prefix is **required**
/// here: a bare word is not a category, it is a malformed event kind, and
/// accepting one would let any unknown word turn into a new category.
pub fn custom_kind_key(key: &str) -> Option<String> {
    let bare = key.strip_prefix(CUSTOM_KIND_PREFIX)?;
    custom_kind_slug(bare).map(|bare| format!("{CUSTOM_KIND_PREFIX}{bare}"))
}
/// Risk level of a tool/action. This is the backbone of the HITL guard
/// (ARCHITECTURE.md §3.3): `Dangerous` actions can never be executed without
/// an explicit human-confirmed, one-time token — no matter the proactivity
/// level or persona.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Safe,
    Sensitive,
    Dangerous,
}

impl RiskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskLevel::Safe => "safe",
            RiskLevel::Sensitive => "sensitive",
            RiskLevel::Dangerous => "dangerous",
        }
    }
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A structured, schedulable event extracted from natural language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Storage id; `None` until persisted.
    pub id: Option<i64>,
    pub title: String,
    pub kind: EventKind,
    pub start: NaiveDateTime,
    pub end: Option<NaiveDateTime>,
    pub location: Option<String>,
    #[serde(default)]
    pub people: Vec<String>,
    /// The raw utterance this event was extracted from (provenance for the
    /// memory ledger, F12).
    pub raw_input: String,
    /// When the event record was created (agent clock).
    pub created_at: NaiveDateTime,
}

impl Event {
    /// Construct a new (unpersisted) event.
    pub fn new(
        title: impl Into<String>,
        kind: EventKind,
        start: NaiveDateTime,
        raw_input: impl Into<String>,
        created_at: NaiveDateTime,
    ) -> Self {
        Event {
            id: None,
            title: title.into(),
            kind,
            start,
            end: None,
            location: None,
            people: Vec::new(),
            raw_input: raw_input.into(),
            created_at,
        }
    }
}

/// Delivery channel for a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Push,
    Banner,
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Channel::Push => "push",
            Channel::Banner => "banner",
        }
    }
}

/// Lifecycle status of a scheduled notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationStatus {
    Pending,
    Fired,
    Dismissed,
}

impl NotificationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationStatus::Pending => "pending",
            NotificationStatus::Fired => "fired",
            NotificationStatus::Dismissed => "dismissed",
        }
    }
}

impl FromStr for NotificationStatus {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pending" => NotificationStatus::Pending,
            "fired" => NotificationStatus::Fired,
            "dismissed" => NotificationStatus::Dismissed,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown notification status: {other}"
                )))
            }
        })
    }
}

/// A planned notification for an event: fire at `fire_at`, using `channels`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: Option<i64>,
    pub event_id: i64,
    pub fire_at: NaiveDateTime,
    /// Human-readable lead label, e.g. "3d" or "30m".
    pub lead_label: String,
    pub channels: Vec<Channel>,
    pub status: NotificationStatus,
    pub created_at: NaiveDateTime,
    pub fired_at: Option<NaiveDateTime>,
}

/// Which memory layer a ledger entry belongs to (ARCHITECTURE.md §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    /// Raw utterances as ingested.
    RawInput,
    /// Extracted events (episodic).
    Event,
    /// Scheduled notifications derived from events.
    Notification,
    /// Behavior-journal entries (status reports, check-ins, fired reminders).
    Behavior,
    /// Generated suggestions (F10).
    Suggestion,
    /// Wearable health samples (F5): heart rate / steps / sleep.
    Wearable,
    /// Semantic memory facts (§3.10): one-sentence user facts.
    Fact,
    /// Standing routines (F3 完全体): recurring reminders learned from habits.
    Routine,
    /// Captured third-party notifications and their F20 routing outcome.
    NotificationCapture,
}

impl MemoryLayer {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryLayer::RawInput => "raw_input",
            MemoryLayer::Event => "event",
            MemoryLayer::Notification => "notification",
            MemoryLayer::Behavior => "behavior",
            MemoryLayer::Suggestion => "suggestion",
            MemoryLayer::Wearable => "wearable",
            MemoryLayer::Fact => "fact",
            MemoryLayer::Routine => "routine",
            MemoryLayer::NotificationCapture => "notification_capture",
        }
    }
}

impl std::str::FromStr for MemoryLayer {
    type Err = crate::error::CoreError;

    /// Parse a layer name. Lives here rather than in the shell because the
    /// layer is now part of a Guard tool's arguments, and tool arguments are
    /// fingerprinted — the same string has to mean the same thing on both
    /// sides of the confirmation.
    fn from_str(s: &str) -> crate::error::Result<Self> {
        Ok(match s {
            "raw_input" => MemoryLayer::RawInput,
            "event" => MemoryLayer::Event,
            "notification" => MemoryLayer::Notification,
            "notification_capture" => MemoryLayer::NotificationCapture,
            "behavior" => MemoryLayer::Behavior,
            "suggestion" => MemoryLayer::Suggestion,
            "wearable" => MemoryLayer::Wearable,
            "fact" => MemoryLayer::Fact,
            "routine" => MemoryLayer::Routine,
            other => {
                return Err(crate::error::CoreError::Invalid(format!(
                    "未知记忆层：{other}"
                )))
            }
        })
    }
}

/// A unified, user-facing ledger entry (F12). Every entry is inspectable and
/// deletable — this is the privacy-trust surface, not an optional add-on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub layer: MemoryLayer,
    pub id: i64,
    pub summary: String,
    /// Where this memory came from (e.g. "raw_input#3").
    pub source: Option<String>,
    pub created_at: NaiveDateTime,
}

//! Core domain model: events, risk levels, notifications, memory records.
//!
//! These types are storage- and UI-agnostic. Times are wall-clock
//! `NaiveDateTime` values (Phase 1 keeps timezone handling out of the core;
//! the caller injects "now" so the pure logic stays deterministic and
//! testable — see ARCHITECTURE.md §6 Phase 1 and docs/MISC.md).

use std::fmt;
use std::str::FromStr;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Exam,
    Meeting,
    Class,
    Deadline,
    Reminder,
    Other,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Exam => "exam",
            EventKind::Meeting => "meeting",
            EventKind::Class => "class",
            EventKind::Deadline => "deadline",
            EventKind::Reminder => "reminder",
            EventKind::Other => "other",
        }
    }

    /// Chinese display label for user-facing surfaces (replies, ledger).
    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Exam => "考试",
            EventKind::Meeting => "会议",
            EventKind::Class => "课程",
            EventKind::Deadline => "截止",
            EventKind::Reminder => "提醒",
            EventKind::Other => "事件",
        }
    }

    /// All kinds, for iteration (e.g. seeding the default rule table).
    pub fn all() -> [EventKind; 6] {
        [
            EventKind::Exam,
            EventKind::Meeting,
            EventKind::Class,
            EventKind::Deadline,
            EventKind::Reminder,
            EventKind::Other,
        ]
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
            other => return Err(CoreError::Invalid(format!("unknown event kind: {other}"))),
        })
    }
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

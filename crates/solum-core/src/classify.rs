//! Importance classification: map an event kind to *when* and *how* to warn.
//!
//! ARCHITECTURE.md §3.1 specifies a user-editable rule table with an LLM
//! fallback for kinds the table doesn't cover. Phase 1 ships the rule table
//! (the deterministic, always-available floor); the LLM fallback plugs in via
//! [`crate::extract::Reasoner`] later.
//!
//! Design note (see docs/MISC.md): each kind carries a *list* of lead times so
//! layered reminders (e.g. exam at 3d **and** 1h) are expressible. The defaults
//! match the doc's single values exactly.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::model::{Channel, EventKind};

/// Matching form for an F20 important-notification routing rule. These rules
/// are only a UX/cost hint: they never authorize an action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationMatchKind {
    #[default]
    Substring,
    Regex,
}

/// A user-editable rule that routes matching captured notifications to the
/// immediate lane. It intentionally lives in the existing importance rule
/// table rather than introducing a second rules store (Phase 10 decision 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPriorityRule {
    pub id: String,
    pub pattern: String,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default = "default_notification_priority")]
    pub priority: u8,
    #[serde(default)]
    pub matcher: NotificationMatchKind,
}

const fn default_notification_priority() -> u8 {
    1
}

/// A single "warn me this far ahead" entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadTime {
    /// Human label such as "3d", "30m", "1h".
    pub label: String,
    /// Lead time in minutes.
    pub minutes: i64,
}

impl LeadTime {
    pub fn new(label: impl Into<String>, minutes: i64) -> Self {
        LeadTime {
            label: label.into(),
            minutes,
        }
    }

    /// Parse a compact label like "3d", "30m", "1h", "2w", "0".
    pub fn parse(label: &str) -> Result<Self> {
        let label = label.trim();
        if label == "0" {
            return Ok(LeadTime::new("0", 0));
        }
        let (num, unit) = label.split_at(
            label
                .find(|c: char| !c.is_ascii_digit())
                .ok_or_else(|| CoreError::Invalid(format!("no unit in lead time {label:?}")))?,
        );
        let n: i64 = num
            .parse()
            .map_err(|_| CoreError::Invalid(format!("bad number in lead time {label:?}")))?;
        let minutes = match unit {
            "m" | "min" => n,
            "h" | "hr" => n * 60,
            "d" => n * 60 * 24,
            "w" => n * 60 * 24 * 7,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown lead-time unit {other:?}"
                )))
            }
        };
        Ok(LeadTime::new(label, minutes))
    }
}

/// The importance rule for one event kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportanceRule {
    pub kind: EventKind,
    pub lead_times: Vec<LeadTime>,
    pub channels: Vec<Channel>,
}

/// The full, user-editable rule table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTable {
    rules: HashMap<EventKind, ImportanceRule>,
    #[serde(default)]
    notification_priority_rules: Vec<NotificationPriorityRule>,
}

impl RuleTable {
    /// The built-in defaults from ARCHITECTURE.md §3.1 (plus sensible entries
    /// for reminder/other, which the doc leaves to defaults).
    pub fn default_table() -> Self {
        let push = || vec![Channel::Push];
        let push_banner = || vec![Channel::Push, Channel::Banner];
        let mk = |kind, leads: &[(&str, i64)], channels: Vec<Channel>| {
            (
                kind,
                ImportanceRule {
                    kind,
                    lead_times: leads.iter().map(|(l, m)| LeadTime::new(*l, *m)).collect(),
                    channels,
                },
            )
        };
        let rules = HashMap::from([
            mk(EventKind::Exam, &[("3d", 3 * 24 * 60)], push_banner()),
            mk(EventKind::Meeting, &[("30m", 30)], push()),
            mk(EventKind::Class, &[("30m", 30)], push()),
            mk(EventKind::Deadline, &[("1d", 24 * 60)], push_banner()),
            mk(EventKind::Reminder, &[("0", 0)], push()),
            mk(EventKind::Other, &[("30m", 30)], push()),
        ]);
        RuleTable {
            rules,
            notification_priority_rules: Vec::new(),
        }
    }

    /// Look up the rule for a kind, falling back to a safe default.
    pub fn rule(&self, kind: EventKind) -> ImportanceRule {
        self.rules.get(&kind).cloned().unwrap_or(ImportanceRule {
            kind,
            lead_times: vec![LeadTime::new("30m", 30)],
            channels: vec![Channel::Push],
        })
    }

    /// Insert or replace a rule (user editing the table).
    pub fn set(&mut self, rule: ImportanceRule) {
        self.rules.insert(rule.kind, rule);
    }

    pub fn notification_priority_rules(&self) -> &[NotificationPriorityRule] {
        &self.notification_priority_rules
    }

    pub fn add_notification_priority_rule(&mut self, rule: NotificationPriorityRule) {
        if let Some(existing) = self
            .notification_priority_rules
            .iter_mut()
            .find(|existing| existing.id == rule.id)
        {
            *existing = rule;
        } else {
            self.notification_priority_rules.push(rule);
        }
    }

    pub fn remove_notification_priority_rule(&mut self, id: &str) -> bool {
        let before = self.notification_priority_rules.len();
        self.notification_priority_rules
            .retain(|rule| rule.id != id);
        self.notification_priority_rules.len() != before
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

impl Default for RuleTable {
    fn default() -> Self {
        Self::default_table()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_time_parsing() {
        assert_eq!(LeadTime::parse("3d").unwrap().minutes, 3 * 24 * 60);
        assert_eq!(LeadTime::parse("30m").unwrap().minutes, 30);
        assert_eq!(LeadTime::parse("1h").unwrap().minutes, 60);
        assert_eq!(LeadTime::parse("2w").unwrap().minutes, 2 * 7 * 24 * 60);
        assert_eq!(LeadTime::parse("0").unwrap().minutes, 0);
        assert!(LeadTime::parse("abc").is_err());
        assert!(LeadTime::parse("3x").is_err());
    }

    #[test]
    fn defaults_match_architecture() {
        let t = RuleTable::default_table();
        let exam = t.rule(EventKind::Exam);
        assert_eq!(exam.lead_times[0].label, "3d");
        assert_eq!(exam.channels, vec![Channel::Push, Channel::Banner]);

        let meeting = t.rule(EventKind::Meeting);
        assert_eq!(meeting.lead_times[0].minutes, 30);
        assert_eq!(meeting.channels, vec![Channel::Push]);

        let deadline = t.rule(EventKind::Deadline);
        assert_eq!(deadline.lead_times[0].label, "1d");
        assert_eq!(deadline.channels, vec![Channel::Push, Channel::Banner]);
    }

    #[test]
    fn user_can_override_and_layer() {
        let mut t = RuleTable::default_table();
        t.set(ImportanceRule {
            kind: EventKind::Exam,
            lead_times: vec![
                LeadTime::parse("3d").unwrap(),
                LeadTime::parse("1h").unwrap(),
            ],
            channels: vec![Channel::Push],
        });
        assert_eq!(t.rule(EventKind::Exam).lead_times.len(), 2);
    }

    #[test]
    fn round_trips_json() {
        let t = RuleTable::default_table();
        let restored = RuleTable::from_json(&t.to_json().unwrap()).unwrap();
        assert_eq!(t, restored);
    }
}

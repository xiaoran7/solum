//! Standing routines (F3 习惯闭环完全体, ARCHITECTURE.md §6 Phase 7 D4).
//!
//! A routine is a recurring daily reminder ("07:20 护肤") that usually starts
//! life as an accepted habit suggestion. Firing reuses the existing pipeline:
//! each day an occurrence is *materialized* as a normal `Event` (kind
//! `Reminder`) plus a `Notification` at the routine's time — so delivery,
//! the Android AlarmManager mirror, sync, and the F12 ledger all work
//! unchanged. Completion is tracked through the behavior journal: a `Status`
//! entry **whose `source` is this routine's tag** (see [`source_tag`]); a
//! routine that goes seven days without a single confirmation triggers a
//! *pause suggestion* — the anti-nag brake (proactivity must carry its own
//! off-switch).
//!
//! Completion used to be matched on the entry *content* instead — equal to, or
//! merely containing, the routine title. That quietly conflated "the user
//! pressed 完成 on this routine" with "the user happened to mention these
//! words": a routine named 护肤 was marked done by the sentence 我在护肤, and
//! the pause suggestion it should have produced was suppressed by it too.
//! Provenance is the only thing that actually means confirmation.

use chrono::{NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// One standing routine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routine {
    pub id: Option<i64>,
    /// What to remind ("护肤"). Also the completion-match key in the journal.
    pub title: String,
    /// Time of day, "HH:MM".
    pub time_of_day: String,
    /// Provenance, e.g. "suggestion#5" (F12).
    pub source: Option<String>,
    pub active: bool,
    pub created_at: NaiveDateTime,
    /// The furthest calendar date (inclusive) whose occurrence has been
    /// materialized. Prevents duplicate daily events.
    pub scheduled_until: Option<chrono::NaiveDate>,
}

impl Routine {
    pub fn time(&self) -> Result<NaiveTime> {
        parse_time_of_day(&self.time_of_day)
    }
}

/// The provenance tag that marks a row as belonging to routine `id`. Used for
/// both materialized events and completion entries in the behavior journal —
/// matching on this, never on free text, is what makes a completion a
/// completion.
pub fn source_tag(id: i64) -> String {
    format!("routine#{id}")
}

/// Was this journal entry a confirmation of routine `id`?
pub fn is_completion_of(entry: &crate::journal::BehaviorEntry, id: i64) -> bool {
    entry.source.as_deref() == Some(source_tag(id).as_str())
}

/// Shared machine format for every date-less clock time in Solum: exactly
/// `HH:MM`, including a leading zero. Custom widgets reuse this parser rather
/// than creating a second time-of-day convention.
pub fn parse_time_of_day(value: &str) -> Result<NaiveTime> {
    let parsed = NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|e| CoreError::Invalid(format!("time_of_day {:?} 不合法: {e}", value)))?;
    if parsed.format("%H:%M").to_string() != value {
        return Err(CoreError::Invalid(format!(
            "time_of_day {:?} 必须为 HH:MM",
            value
        )));
    }
    Ok(parsed)
}

/// Parse the machine-readable half of a habit suggestion's `source` field:
/// `habit:<HH:MM>:<title>` (set by `suggest::detect_habits`). Returns
/// `(time_of_day, title)`.
pub fn parse_habit_source(source: &str) -> Option<(String, String)> {
    let rest = source.strip_prefix("habit:")?;
    // The time itself contains a colon ("07:20"), so split at the *third*
    // colon-equivalent boundary: fixed-width "HH:MM" then ":<title>".
    let (time, title) = (rest.get(..5)?, rest.get(6..)?);
    if rest.as_bytes().get(5) != Some(&b':')
        || parse_time_of_day(time).is_err()
        || title.trim().is_empty()
    {
        return None;
    }
    Some((time.to_string(), title.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_habit_source() {
        assert_eq!(
            parse_habit_source("habit:07:20:护肤"),
            Some(("07:20".into(), "护肤".into()))
        );
        assert_eq!(parse_habit_source("event#3"), None);
        assert_eq!(parse_habit_source("habit:xx:yy:护肤"), None);
        assert_eq!(parse_habit_source("habit:07:20:"), None);
    }

    #[test]
    fn routine_time_parses() {
        let r = Routine {
            id: None,
            title: "护肤".into(),
            time_of_day: "07:20".into(),
            source: None,
            active: true,
            created_at: chrono::NaiveDate::from_ymd_opt(2026, 7, 6)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
            scheduled_until: None,
        };
        assert_eq!(
            r.time().unwrap(),
            NaiveTime::from_hms_opt(7, 20, 0).unwrap()
        );
        assert!(parse_time_of_day("7:20").is_err());
    }
}

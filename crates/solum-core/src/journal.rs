//! Behavior journal (F4) + proactive status check-ins (F3).
//!
//! Interactions automatically condense into a structured, local-only log:
//! status answers ("我在护肤"), the agent's own check-in questions, and fired
//! reminders. The log is a first-class memory layer — inspectable and deletable
//! from the F12 ledger — and is the raw material habit detection reads
//! (see [`crate::suggest`] once Phase 2 lands it).
//!
//! Check-in cadence is a pure policy function of the `status_checkins`
//! proactivity level and the injected clock, so it stays deterministic and
//! testable; the resident shell just calls [`checkin_due`] on its tick.

use chrono::{Duration, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::proactivity::ProactivityLevel;

/// What produced a behavior-log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorKind {
    /// The user reported what they're doing (a `StatusAnswer` utterance).
    Status,
    /// The agent proactively asked "现在在忙什么".
    CheckinAsked,
    /// A scheduled reminder fired.
    ReminderFired,
}

impl BehaviorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BehaviorKind::Status => "status",
            BehaviorKind::CheckinAsked => "checkin_asked",
            BehaviorKind::ReminderFired => "reminder_fired",
        }
    }
}

impl std::str::FromStr for BehaviorKind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "status" => BehaviorKind::Status,
            "checkin_asked" => BehaviorKind::CheckinAsked,
            "reminder_fired" => BehaviorKind::ReminderFired,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown behavior kind: {other}"
                )))
            }
        })
    }
}

/// One structured behavior-log record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorEntry {
    pub id: Option<i64>,
    pub ts: NaiveDateTime,
    pub kind: BehaviorKind,
    /// The activity/content, e.g. "护肤" or the reminder summary.
    pub content: String,
    /// Provenance, e.g. "raw_input#5" or "notification#3" (F12).
    pub source: Option<String>,
}

/// Strip the "我在/正在/…" lead-in from a status answer so the log stores the
/// activity itself ("我在护肤" → "护肤").
pub fn strip_status_prefix(text: &str) -> String {
    let t = text.trim();
    let lower = t.to_lowercase();
    for p in ["我在", "正在", "在做", "刚在"] {
        if let Some(rest) = t.strip_prefix(p) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    for p in ["i'm ", "im ", "currently "] {
        if lower.starts_with(p) {
            let rest = t[p.len()..].trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    t.to_string()
}

/// The fixed check-in question. Kept boring and predictable on purpose — the
/// LLM may rephrase it later, but the offline floor never depends on that.
pub const CHECKIN_QUESTION: &str = "现在在忙什么？随口回一句，我会记下来帮你总结习惯。";

/// Waking hours during which the agent may proactively ask: [08:00, 22:00).
pub fn in_waking_hours(t: NaiveDateTime) -> bool {
    (8..22).contains(&t.hour())
}

/// How often each proactivity level checks in. `Passive` never asks.
pub fn checkin_interval(level: ProactivityLevel) -> Option<Duration> {
    match level {
        ProactivityLevel::Passive => None,
        ProactivityLevel::Secretary => Some(Duration::hours(4)),
        ProactivityLevel::Butler => Some(Duration::hours(2)),
    }
}

/// Whether a check-in is due at `now`, given the last time one was asked.
/// Pure policy: waking hours only, and at least one interval since the last
/// ask (a never-asked user is due immediately within waking hours).
pub fn checkin_due(
    level: ProactivityLevel,
    last_asked: Option<NaiveDateTime>,
    now: NaiveDateTime,
) -> bool {
    let Some(interval) = checkin_interval(level) else {
        return false;
    };
    if !in_waking_hours(now) {
        return false;
    }
    match last_asked {
        None => true,
        Some(last) => now - last >= interval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 6)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn strips_status_prefixes() {
        assert_eq!(strip_status_prefix("我在护肤"), "护肤");
        assert_eq!(strip_status_prefix("正在写代码"), "写代码");
        assert_eq!(strip_status_prefix("I'm reading"), "reading");
        // No prefix → unchanged.
        assert_eq!(strip_status_prefix("护肤中"), "护肤中");
        // Prefix with nothing behind it → keep the original text.
        assert_eq!(strip_status_prefix("我在"), "我在");
    }

    #[test]
    fn passive_never_asks() {
        assert!(!checkin_due(ProactivityLevel::Passive, None, dt(10, 0)));
    }

    #[test]
    fn respects_waking_hours() {
        assert!(!checkin_due(ProactivityLevel::Butler, None, dt(7, 59)));
        assert!(checkin_due(ProactivityLevel::Butler, None, dt(8, 0)));
        assert!(checkin_due(ProactivityLevel::Butler, None, dt(21, 59)));
        assert!(!checkin_due(ProactivityLevel::Butler, None, dt(22, 0)));
    }

    #[test]
    fn interval_gates_repeat_asks() {
        let last = dt(10, 0);
        // Secretary: 4h.
        assert!(!checkin_due(
            ProactivityLevel::Secretary,
            Some(last),
            dt(13, 59)
        ));
        assert!(checkin_due(
            ProactivityLevel::Secretary,
            Some(last),
            dt(14, 0)
        ));
        // Butler: 2h.
        assert!(!checkin_due(
            ProactivityLevel::Butler,
            Some(last),
            dt(11, 59)
        ));
        assert!(checkin_due(ProactivityLevel::Butler, Some(last), dt(12, 0)));
    }

    #[test]
    fn behavior_kind_round_trips() {
        for k in [
            BehaviorKind::Status,
            BehaviorKind::CheckinAsked,
            BehaviorKind::ReminderFired,
        ] {
            assert_eq!(k.as_str().parse::<BehaviorKind>().unwrap(), k);
        }
        assert!("nope".parse::<BehaviorKind>().is_err());
    }
}

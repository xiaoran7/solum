//! `reschedule` -- "move this event" as `<target> <marker> <new time>`. Returns `None` unless the right-hand side resolves to at least a date or a clock time — "把会改一下" stays chat — and leaves the half the phrase did not specify at `None` for the orchestrator to resolve against the actual event.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// A parsed "move this event" request. `new_date` / `new_time` are the halves
/// the phrase actually specified; whichever is absent keeps the target event's
/// original value (that resolution needs the event, so it lives in the
/// orchestrator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescheduleRequest {
    /// What the user called the event ("明天的会", "期末考试").
    pub target: String,
    pub new_date: Option<chrono::NaiveDate>,
    pub new_time: Option<(u32, u32)>,
}

const RESCHEDULE_MARKERS: &[&str] = &[
    "改到",
    "改成",
    "改在",
    "推迟到",
    "延到",
    "延期到",
    "提前到",
    "挪到",
    "换到",
    "调到",
];

/// Rule-only parse of a reschedule utterance: `<target> <marker> <new time>`.
/// Returns `None` unless the right-hand side resolves to at least a date or a
/// clock time — "把会改一下" stays chat.
pub fn parse_reschedule(text: &str, now: NaiveDateTime) -> Option<RescheduleRequest> {
    for marker in RESCHEDULE_MARKERS {
        let Some(pos) = text.find(marker) else {
            continue;
        };
        let (new_date, new_time) =
            crate::time_parse::parse_date_time_parts(&text[pos + marker.len()..], now);
        if new_date.is_none() && new_time.is_none() {
            continue;
        }
        let target = text[..pos]
            .trim()
            .trim_start_matches(['把', '将', '帮', '我', '请'])
            .trim_end_matches('的')
            .trim()
            .to_string();
        return Some(RescheduleRequest {
            target,
            new_date,
            new_time,
        });
    }
    None
}

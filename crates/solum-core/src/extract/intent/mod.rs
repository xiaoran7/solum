//! `intent` -- the classification order, which is the whole argument of this file and cannot be read off any one rule: reschedule and cancel are tested before schedulability so a phrase carrying both an event word and a new time does not ingest a duplicate, and the memory check sits after the schedulable one because "记住明天3点开会" is an event while "记住我不吃辣" is a fact.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::eventwords::is_schedulable;
use super::widget::requests_widget;
use super::words::{DANGER_WORDS, STATUS_PREFIXES};
use super::*;

/// Classify an utterance. `now` is used so the router can tell whether the text
/// actually contains a resolvable time.
pub fn route_intent(text: &str, now: NaiveDateTime) -> Intent {
    let lower = text.to_lowercase();
    if DANGER_WORDS.iter().any(|w| lower.contains(*w)) {
        return Intent::DangerousCommand;
    }
    // Before the schedulable test: "把明天的会改到4点" carries both an event
    // word and a time, and must not ingest as a new event.
    if parse_reschedule(text, now).is_some() {
        return Intent::RescheduleEvent;
    }
    if parse_cancel(text).is_some() {
        return Intent::CancelEvent;
    }
    if requests_widget(text) {
        return Intent::CreateWidget;
    }
    if is_schedulable(text, now) {
        return Intent::IngestEvent;
    }
    // After the schedulable check on purpose: "记住明天3点开会" is an event
    // ("记住" doubles as a reminder filler), while "记住我不吃辣" is a fact.
    if crate::memory::extract_fact_content(text).is_some() {
        return Intent::MemoryWrite;
    }
    if STATUS_PREFIXES.iter().any(|p| lower.starts_with(*p)) {
        return Intent::StatusAnswer;
    }
    Intent::Chat
}

//! `widget` -- the offline widget route: nouns, creation verbs, and the two confidences — a high-confidence "组件" request must never fall through to event ingestion when the cloud is down, while an ambiguous chat-shaped one is allowed to.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::eventwords::is_schedulable;
use super::words::{WIDGET_CREATE_WORDS, WIDGET_NOUNS};
use super::*;
/// A high-confidence offline route for widget creation. The cloud classifier
/// can additionally handle ambiguous chat-shaped requests, but an explicit
/// “组件” request must never fall through to event ingestion when offline.
pub fn requests_widget(text: &str) -> bool {
    let lower = text.to_lowercase();
    WIDGET_NOUNS.iter().any(|noun| lower.contains(noun))
        && WIDGET_CREATE_WORDS.iter().any(|verb| lower.contains(verb))
}

/// Ambiguous phrasing eligible for the opt-in cloud router. This deliberately
/// excludes schedule/event vocabulary: the LLM can assist a user-authored
/// widget request, but never gets to reinterpret ordinary F1 ingestion.
pub fn might_request_widget(text: &str, now: NaiveDateTime) -> bool {
    let lower = text.to_lowercase();
    let creation = WIDGET_CREATE_WORDS.iter().any(|verb| lower.contains(verb));
    let shape = ["表", "清单", "追踪", "记录器", "收支", "tracker", "list"]
        .iter()
        .any(|hint| lower.contains(hint));
    creation && shape && !is_schedulable(text, now)
}

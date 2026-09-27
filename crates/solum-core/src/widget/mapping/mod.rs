//! `mapping` -- how one event field lands on one widget field. A snapshot, not a live binding: the UI says so, and the type is named for the fact that it is taken once.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// How a widget's fields line up with an event's, for the two snapshot
/// bridges in 设计稿 ⑦ (import from events, promote a record to an event).
///
/// Both directions are **copies, not links** — B (live querying `events`) was
/// rejected because it would need cross-table access rules and would collide
/// with F12: if the ledger deletes an event, does the widget row vanish too?
/// "Yes" needs cascades; "no" breaks 「删除即从语料消失」. A snapshot has
/// neither problem, as long as the UI says it is a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFieldMapping {
    /// First text field — receives the event title.
    pub title: String,
    /// First datetime field, else the first date field — receives the start.
    pub when: Option<String>,
    pub when_type: Option<WidgetFieldType>,
}

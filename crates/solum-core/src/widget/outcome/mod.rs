//! `outcome` -- what an import did: the ones that landed, the ones that were skipped and why, and the stat tiles. The skip reasons are capped because an import that skipped everything would otherwise produce an unbounded list saying the same thing over and over.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// How many individual skip reasons 「从日程导入」 reports. A long schedule
/// where nothing maps would otherwise produce an unbounded list that says the
/// same thing over and over.
pub const MAX_SKIP_REASONS: usize = 5;

/// What 「从日程导入」 actually did. A bare count cannot tell "you have no
/// schedule" apart from "every entry failed to map" — both render as 0, and
/// the second one is the case the user needs to act on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WidgetImportOutcome {
    pub imported: usize,
    pub skipped: usize,
    /// A bounded sample of why, in schedule order.
    pub reasons: Vec<WidgetImportSkip>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WidgetImportSkip {
    pub title: String,
    pub reason: String,
}

/// One tile of a `stat` view.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WidgetStat {
    pub field: String,
    pub label: String,
    pub op: StatOp,
    pub op_label: &'static str,
    /// Sums keep their fraction; counts are whole numbers.
    pub value: f64,
}

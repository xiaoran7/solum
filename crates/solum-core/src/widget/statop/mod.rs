//! `statop` -- the arithmetic a `stat` tile may do. An operation is a name from this list, never an expression — which is what lets a widget be data instead of code.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// The fixed aggregate a `stat` view shows for a field, derived from its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatOp {
    /// Numbers add up.
    Sum,
    /// Booleans count how many are true.
    CountTrue,
    /// Everything else counts how many records filled it in.
    CountFilled,
}

impl StatOp {
    pub fn for_field(field_type: WidgetFieldType) -> Self {
        match field_type {
            WidgetFieldType::Number => Self::Sum,
            WidgetFieldType::Bool => Self::CountTrue,
            _ => Self::CountFilled,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Sum => "合计",
            Self::CountTrue => "为是",
            Self::CountFilled => "已填",
        }
    }
}

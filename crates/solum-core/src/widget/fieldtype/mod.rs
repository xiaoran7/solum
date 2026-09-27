//! `fieldtype` -- the field kinds a schema may declare, and what each one's value has to look like. Kept with its `impl` — the four methods are the definition, and a reader checking one field type needs all of them.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetFieldType {
    Text,
    Number,
    Date,
    Datetime,
    Time,
    Bool,
    Enum,
}

impl WidgetFieldType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Date => "date",
            Self::Datetime => "datetime",
            Self::Time => "time",
            Self::Bool => "bool",
            Self::Enum => "enum",
        }
    }
}

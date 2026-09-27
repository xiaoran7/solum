//! `viewtype` -- the view kinds, and the per-kind ceiling on how many of them one widget may have. The ceiling lives with the enum because it is a property of the kind, not of the widget.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetViewType {
    Form,
    List,
    /// Same data binding as `list`, rendered as columns.
    Table,
    /// One aggregate tile per listed field. Deliberately no operator in the
    /// schema: the op follows from the field's type (设计稿 ②「聚合能力放视图
    /// 层的固定算子，不放 schema 层」), so a model cannot invent one and the
    /// number always means the same thing.
    Stat,
}

impl WidgetViewType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Form => "form",
            Self::List => "list",
            Self::Table => "table",
            Self::Stat => "stat",
        }
    }

    /// Views whose column on `widget_fields` holds this view's ordering.
    pub const ALL: &'static [Self] = &[Self::Form, Self::List, Self::Table, Self::Stat];

    pub fn ord_column(&self) -> &'static str {
        match self {
            Self::Form => "form_ord",
            Self::List => "list_ord",
            Self::Table => "table_ord",
            Self::Stat => "stat_ord",
        }
    }
}

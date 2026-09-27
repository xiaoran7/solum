//! Persistent custom widgets (F19, Phase 11 first vertical slice).
//!
//! Widgets are deliberately declarative data: this module accepts a small,
//! closed schema and validates record values against it. It never accepts
//! executable code, HTML, CSS, or arbitrary view descriptions.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use std::collections::HashSet;

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};

mod draft;
mod fieldtype;
mod helpers;
/// Hard limits are enforced before a definition can become persistent.
mod icons;
mod mapping;
mod outcome;
mod schema;
mod shapes;
mod statop;
#[cfg(test)]
mod tests;
mod validate;
mod viewtype;

// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use fieldtype::WidgetFieldType;
pub use outcome::{WidgetImportOutcome, WidgetImportSkip, MAX_SKIP_REASONS};
pub use shapes::{
    WidgetDefinition, WidgetDefinitionDraft, WidgetField, WidgetRecord, WidgetSchema,
    WidgetSchemaRejection, WidgetView,
};
pub use statop::StatOp;
pub use viewtype::WidgetViewType;

pub const MAX_FIELDS: usize = 12;
pub const MAX_VIEWS: usize = 4;
/// Total definitions on one device. Unlike the two above this cannot be
/// checked from a single schema, so it is enforced at the storage boundary.
/// The fixed “组件” tab renders a flat catalog with no search or grouping;
/// past this count the entry point stops being usable, and a model that keeps
/// proposing new widgets should be told no rather than allowed to accumulate.
pub const MAX_WIDGETS: usize = 8;
const MAX_SCHEMA_BYTES: usize = 12 * 1024;

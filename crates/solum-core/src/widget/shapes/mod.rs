//! `shapes` -- the data shapes, in the order they are produced: a draft as the model proposed it, the schema it was validated into, the definition that gets persisted, the record that comes back out, and the rejection explaining why a draft was refused. All `deny_unknown_fields` — a widget that gains a key it did not declare is a widget nobody validated.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{is_field_name, validate_field_options};
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetField {
    /// Machine key used in a record's JSON object.
    pub name: String,
    /// Human-facing label. The renderer always assigns it through textContent.
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: WidgetFieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl WidgetField {
    /// Validate one field on its own, for the add-a-field path. The whole-schema
    /// checks (uniqueness, view references) belong to [`WidgetSchema::validate`];
    /// this is only the part that a single field can answer by itself.
    pub fn validate_standalone(&self) -> Result<()> {
        if !is_field_name(&self.name) {
            return Err(CoreError::Invalid(format!(
                "字段名 {:?} 必须是小写 ASCII 标识符",
                self.name
            )));
        }
        if self.label.trim().is_empty() || self.label.trim().len() > 80 {
            return Err(CoreError::Invalid(format!(
                "字段 {:?} 的标签必须是 1 到 80 个字符",
                self.name
            )));
        }
        validate_field_options(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetView {
    #[serde(rename = "type")]
    pub view_type: WidgetViewType,
    /// Visible field keys, in display order. Empty is rejected rather than
    /// inferred, so the generated schema is always explicit and reviewable.
    pub fields: Vec<String>,
    /// Optional initial list sort. The UI also exposes a user-selected sort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSchema {
    pub fields: Vec<WidgetField>,
    pub views: Vec<WidgetView>,
}

/// The only shape a model may propose. `deny_unknown_fields` is intentional:
/// accepting an unfamiliar key today is silently accepting a future language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDefinitionDraft {
    pub name: String,
    pub icon: String,
    pub fields: Vec<WidgetField>,
    pub views: Vec<WidgetView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetDefinition {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub schema: WidgetSchema,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetRecord {
    pub id: i64,
    pub widget_id: i64,
    pub data: Value,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WidgetSchemaRejection {
    pub id: i64,
    pub schema_json: String,
    pub reason: String,
    pub created_at: NaiveDateTime,
}

//! Persistent custom widgets (F19, Phase 11 first vertical slice).
//!
//! Widgets are deliberately declarative data: this module accepts a small,
//! closed schema and validates record values against it. It never accepts
//! executable code, HTML, CSS, or arbitrary view descriptions.

use std::collections::HashSet;

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};

/// Hard limits are enforced before a definition can become persistent.
pub const MAX_FIELDS: usize = 12;
pub const MAX_VIEWS: usize = 4;
/// Total definitions on one device. Unlike the two above this cannot be
/// checked from a single schema, so it is enforced at the storage boundary.
/// The fixed “组件” tab renders a flat catalog with no search or grouping;
/// past this count the entry point stops being usable, and a model that keeps
/// proposing new widgets should be told no rather than allowed to accumulate.
pub const MAX_WIDGETS: usize = 8;
const MAX_SCHEMA_BYTES: usize = 12 * 1024;

/// Icons are names from the static, hand-drawn frontend vocabulary. A schema
/// supplies a name only; it cannot supply SVG markup.
const ALLOWED_ICONS: &[&str] = &[
    "calendar", "doc", "gauge", "journal", "memory", "rules", "watch",
];

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

impl WidgetDefinitionDraft {
    /// Parse a model response as a complete declarative definition. Markdown
    /// fences are tolerated, but no surrounding prose or partial JSON is.
    pub fn parse_generated(raw: &str) -> Result<Self> {
        if raw.len() > MAX_SCHEMA_BYTES {
            return Err(CoreError::Invalid(format!(
                "组件 schema 超过 {} 字节上限",
                MAX_SCHEMA_BYTES
            )));
        }
        let json = strip_json_fence(raw);
        let draft: Self = serde_json::from_str(json)
            .map_err(|e| CoreError::Invalid(format!("组件 schema 不是严格 JSON：{e}")))?;
        draft.validate()?;
        Ok(draft)
    }

    pub fn schema(&self) -> WidgetSchema {
        WidgetSchema {
            fields: self.fields.clone(),
            views: self.views.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.trim().len() > 80 {
            return Err(CoreError::Invalid("组件名称必须是 1 到 80 个字符".into()));
        }
        if self.name != self.name.trim() {
            return Err(CoreError::Invalid("组件名称不能有首尾空白".into()));
        }
        if !ALLOWED_ICONS.contains(&self.icon.as_str()) {
            return Err(CoreError::Invalid(format!(
                "组件图标 {:?} 不在允许目录中",
                self.icon
            )));
        }
        self.schema().validate()
    }
}

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

impl WidgetSchema {
    /// Which fields an event can be copied into. `None` when the widget has no
    /// text field at all — there would be nowhere to put the title, and
    /// guessing some other field would produce nonsense rows.
    pub fn event_mapping(&self) -> Option<EventFieldMapping> {
        let title = self
            .fields
            .iter()
            .find(|f| f.field_type == WidgetFieldType::Text)?;
        let when = self
            .fields
            .iter()
            .find(|f| f.field_type == WidgetFieldType::Datetime)
            .or_else(|| {
                self.fields
                    .iter()
                    .find(|f| f.field_type == WidgetFieldType::Date)
            });
        Some(EventFieldMapping {
            title: title.name.clone(),
            when: when.map(|f| f.name.clone()),
            when_type: when.map(|f| f.field_type),
        })
    }

    /// Build a record from an event, using [`Self::event_mapping`]. Required
    /// fields the mapping cannot fill are left absent on purpose — the caller
    /// validates, and a rejection is better than a fabricated value.
    pub fn record_from_event(&self, title: &str, start: NaiveDateTime) -> Option<Value> {
        let mapping = self.event_mapping()?;
        let mut data = serde_json::Map::new();
        data.insert(mapping.title, Value::String(title.to_string()));
        if let (Some(name), Some(kind)) = (mapping.when, mapping.when_type) {
            let formatted = match kind {
                WidgetFieldType::Datetime => start.format("%Y-%m-%dT%H:%M").to_string(),
                _ => start.format("%Y-%m-%d").to_string(),
            };
            data.insert(name, Value::String(formatted));
        }
        Some(Value::Object(data))
    }

    /// Read a record back as `(title, when)` for promotion to a schedule
    /// entry. `None` when the record has no usable title.
    pub fn event_from_record(&self, data: &Value) -> Option<(String, Option<NaiveDateTime>)> {
        let mapping = self.event_mapping()?;
        let title = data.get(&mapping.title)?.as_str()?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        let when = mapping.when.as_ref().and_then(|name| {
            let raw = data.get(name)?.as_str()?;
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
                .ok()
                .or_else(|| {
                    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                        .ok()?
                        .and_hms_opt(9, 0, 0)
                })
        });
        Some((title, when))
    }
}

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

impl WidgetSchema {
    /// Compute the `stat` view over a set of records. Pure and offline: this
    /// is arithmetic over local rows, never a query and never a cloud call.
    pub fn stats(&self, records: &[WidgetRecord]) -> Vec<WidgetStat> {
        let Some(view) = self
            .views
            .iter()
            .find(|view| view.view_type == WidgetViewType::Stat)
        else {
            return Vec::new();
        };
        view.fields
            .iter()
            .filter_map(|name| {
                let field = self.fields.iter().find(|f| &f.name == name)?;
                let op = StatOp::for_field(field.field_type);
                let value = records
                    .iter()
                    .filter_map(|record| record.data.get(name))
                    .filter(|value| !value.is_null())
                    .fold(0.0, |acc, value| match op {
                        StatOp::Sum => acc + value.as_f64().unwrap_or(0.0),
                        StatOp::CountTrue => acc + f64::from(value.as_bool().unwrap_or(false)),
                        StatOp::CountFilled => acc + 1.0,
                    });
                Some(WidgetStat {
                    field: field.name.clone(),
                    label: field.label.clone(),
                    op,
                    op_label: op.label(),
                    value,
                })
            })
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        if self.fields.is_empty() || self.fields.len() > MAX_FIELDS {
            return Err(CoreError::Invalid(format!(
                "组件字段数必须在 1 到 {MAX_FIELDS} 之间"
            )));
        }
        if self.views.is_empty() || self.views.len() > MAX_VIEWS {
            return Err(CoreError::Invalid(format!(
                "组件视图数必须在 1 到 {MAX_VIEWS} 之间"
            )));
        }

        let mut names = HashSet::new();
        for field in &self.fields {
            if !is_field_name(&field.name) {
                return Err(CoreError::Invalid(format!(
                    "字段名 {:?} 必须是小写 ASCII 标识符",
                    field.name
                )));
            }
            if !names.insert(field.name.as_str()) {
                return Err(CoreError::Invalid(format!("字段 {:?} 重复", field.name)));
            }
            if field.label.trim().is_empty() || field.label.trim().len() > 80 {
                return Err(CoreError::Invalid(format!(
                    "字段 {:?} 的标签必须是 1 到 80 个字符",
                    field.name
                )));
            }
            validate_field_options(field)?;
        }

        let mut view_types = HashSet::new();
        let mut has_form = false;
        let mut has_list = false;
        for view in &self.views {
            if !view_types.insert(view.view_type.as_str()) {
                return Err(CoreError::Invalid(format!(
                    "视图类型 {:?} 重复",
                    view.view_type.as_str()
                )));
            }
            has_form |= view.view_type == WidgetViewType::Form;
            has_list |= view.view_type == WidgetViewType::List;
            if view.fields.is_empty() {
                return Err(CoreError::Invalid(format!(
                    "{} 视图必须显式列出字段",
                    view.view_type.as_str()
                )));
            }
            let mut view_fields = HashSet::new();
            for name in &view.fields {
                if !names.contains(name.as_str()) {
                    return Err(CoreError::Invalid(format!(
                        "{} 视图引用了不存在的字段 {:?}",
                        view.view_type.as_str(),
                        name
                    )));
                }
                if !view_fields.insert(name.as_str()) {
                    return Err(CoreError::Invalid(format!(
                        "{} 视图重复引用字段 {:?}",
                        view.view_type.as_str(),
                        name
                    )));
                }
            }
            // Only list and table are ordered collections of rows, so only
            // they can be sorted. Saying so beats silently ignoring a sort_by
            // the model believed it was setting.
            match (view.view_type, &view.sort_by) {
                (WidgetViewType::Form | WidgetViewType::Stat, Some(_)) => {
                    return Err(CoreError::Invalid(format!(
                        "{} 视图不能声明 sort_by",
                        view.view_type.as_str()
                    )));
                }
                (_, Some(sort_by)) if !names.contains(sort_by.as_str()) => {
                    return Err(CoreError::Invalid(format!(
                        "{} 视图按不存在的字段 {:?} 排序",
                        view.view_type.as_str(),
                        sort_by
                    )));
                }
                _ => {}
            }
        }
        if !has_form || !has_list {
            return Err(CoreError::Invalid(
                "组件必须至少提供 form 和 list 视图".into(),
            ));
        }
        Ok(())
    }

    /// Reject invalid record data. We never coerce strings to numbers/times or
    /// discard unknown keys: data has to mean exactly what the schema says.
    /// Note this deliberately does **not** re-run [`Self::validate`]. A schema
    /// merged from two devices can legitimately exceed `MAX_FIELDS` (see MISC
    /// 2026-07-20); enforcing the cap here would make an over-cap widget
    /// reject every record write, i.e. punish the user for having synced.
    /// Caps gate *adding*, never *using*.
    pub fn validate_record(&self, data: &Value) -> Result<()> {
        let object = data
            .as_object()
            .ok_or_else(|| CoreError::Invalid("组件记录 data 必须是对象".into()))?;
        let fields: std::collections::HashMap<&str, &WidgetField> = self
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field))
            .collect();
        for name in object.keys() {
            if !fields.contains_key(name.as_str()) {
                return Err(CoreError::Invalid(format!("记录含未知字段 {:?}", name)));
            }
        }
        for field in &self.fields {
            let Some(value) = object.get(&field.name) else {
                if field.required {
                    return Err(CoreError::Invalid(format!("缺少必填字段 {:?}", field.name)));
                }
                continue;
            };
            if value.is_null() {
                if field.required {
                    return Err(CoreError::Invalid(format!(
                        "必填字段 {:?} 不能为 null",
                        field.name
                    )));
                }
                continue;
            }
            validate_value(field, value)?;
        }
        Ok(())
    }
}

fn strip_json_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some((_, body)) = after_open.split_once('\n') else {
        return trimmed;
    };
    body.strip_suffix("```").map(str::trim).unwrap_or(trimmed)
}

fn validate_field_options(field: &WidgetField) -> Result<()> {
    match field.field_type {
        WidgetFieldType::Enum => {
            if field.options.is_empty() || field.options.len() > 20 {
                return Err(CoreError::Invalid(format!(
                    "枚举字段 {:?} 必须有 1 到 20 个选项",
                    field.name
                )));
            }
            let mut options = HashSet::new();
            for option in &field.options {
                if option.trim().is_empty()
                    || option.trim().len() > 80
                    || !options.insert(option.as_str())
                {
                    return Err(CoreError::Invalid(format!(
                        "枚举字段 {:?} 的选项必须非空且不重复",
                        field.name
                    )));
                }
            }
        }
        _ if !field.options.is_empty() => {
            return Err(CoreError::Invalid(format!(
                "非枚举字段 {:?} 不能声明 options",
                field.name
            )));
        }
        _ => {}
    }
    Ok(())
}

fn is_field_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z' | '_'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
        && name.len() <= 32
}

fn validate_value(field: &WidgetField, value: &Value) -> Result<()> {
    let invalid = || {
        CoreError::Invalid(format!(
            "字段 {:?} 必须是 {} 类型",
            field.name,
            field.field_type.as_str()
        ))
    };
    match field.field_type {
        WidgetFieldType::Text => {
            if value.as_str().is_none() {
                return Err(invalid());
            }
        }
        WidgetFieldType::Number => {
            if !value.is_number() {
                return Err(invalid());
            }
        }
        WidgetFieldType::Bool => {
            if !value.is_boolean() {
                return Err(invalid());
            }
        }
        WidgetFieldType::Enum => {
            let Some(option) = value.as_str() else {
                return Err(invalid());
            };
            if !field.options.iter().any(|candidate| candidate == option) {
                return Err(CoreError::Invalid(format!(
                    "字段 {:?} 的枚举值 {:?} 不在 options 中",
                    field.name, option
                )));
            }
        }
        WidgetFieldType::Date => {
            let Some(date) = value.as_str() else {
                return Err(invalid());
            };
            let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| invalid())?;
            if parsed.format("%Y-%m-%d").to_string() != date {
                return Err(invalid());
            }
        }
        WidgetFieldType::Datetime => {
            let Some(datetime) = value.as_str() else {
                return Err(invalid());
            };
            let parsed =
                NaiveDateTime::parse_from_str(datetime, "%Y-%m-%dT%H:%M").map_err(|_| invalid())?;
            if parsed.format("%Y-%m-%dT%H:%M").to_string() != datetime {
                return Err(invalid());
            }
        }
        WidgetFieldType::Time => {
            let Some(time) = value.as_str() else {
                return Err(invalid());
            };
            crate::routine::parse_time_of_day(time).map_err(|_| invalid())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid() -> WidgetDefinitionDraft {
        WidgetDefinitionDraft {
            name: "课表".into(),
            icon: "calendar".into(),
            fields: vec![
                WidgetField {
                    name: "course".into(),
                    label: "课程".into(),
                    field_type: WidgetFieldType::Text,
                    required: true,
                    options: vec![],
                },
                WidgetField {
                    name: "starts_at".into(),
                    label: "开始时间".into(),
                    field_type: WidgetFieldType::Time,
                    required: true,
                    options: vec![],
                },
            ],
            views: vec![
                WidgetView {
                    view_type: WidgetViewType::Form,
                    fields: vec!["course".into(), "starts_at".into()],
                    sort_by: None,
                },
                WidgetView {
                    view_type: WidgetViewType::List,
                    fields: vec!["course".into(), "starts_at".into()],
                    sort_by: Some("starts_at".into()),
                },
            ],
        }
    }

    #[test]
    fn accepts_the_closed_seven_type_schema_and_time_values() {
        let d = valid();
        d.validate().unwrap();
        d.schema()
            .validate_record(&json!({"course":"数学", "starts_at":"09:00"}))
            .unwrap();
        assert!(d
            .schema()
            .validate_record(&json!({"course":"数学", "starts_at":"9:00"}))
            .is_err());
    }

    #[test]
    fn rejects_unknown_types_and_schema_keys_as_one_definition() {
        let raw = r#"{"name":"x","icon":"doc","fields":[{"name":"x","label":"X","type":"html"}],"views":[{"type":"form","fields":["x"]},{"type":"list","fields":["x"]}]}"#;
        assert!(WidgetDefinitionDraft::parse_generated(raw).is_err());
        let raw = r#"{"name":"x","icon":"doc","fields":[{"name":"x","label":"X","type":"text","script":"no"}],"views":[{"type":"form","fields":["x"]},{"type":"list","fields":["x"]}]}"#;
        assert!(WidgetDefinitionDraft::parse_generated(raw).is_err());
    }

    #[test]
    fn rejects_limits_and_missing_field_references() {
        let mut d = valid();
        d.fields = (0..13)
            .map(|n| WidgetField {
                name: format!("field_{n}"),
                label: format!("字段{n}"),
                field_type: WidgetFieldType::Text,
                required: false,
                options: vec![],
            })
            .collect();
        assert!(d.validate().is_err());

        let mut d = valid();
        d.views[1].fields.push("missing".into());
        assert!(d.validate().is_err());

        let mut d = valid();
        d.views = (0..5)
            .map(|_| WidgetView {
                view_type: WidgetViewType::List,
                fields: vec!["course".into()],
                sort_by: None,
            })
            .collect();
        assert!(d.validate().is_err());
    }

    #[test]
    fn record_validation_rejects_instead_of_coercing_or_dropping() {
        let schema = valid().schema();
        assert!(schema
            .validate_record(&json!({"course":"数学", "starts_at":900}))
            .is_err());
        assert!(schema
            .validate_record(&json!({"course":"数学", "starts_at":"09:00", "extra":true}))
            .is_err());
    }
}

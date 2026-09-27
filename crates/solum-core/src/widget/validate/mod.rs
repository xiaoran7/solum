//! `validate` -- validating a stored record against its schema, which is the check that makes a hand-edited or migrated file safe to render. The largest function in the file lives here because it is one decision tree, and splitting it would put each branch in a different file.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{is_field_name, validate_field_options, validate_value};
use super::outcome::WidgetStat;
use super::*;

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

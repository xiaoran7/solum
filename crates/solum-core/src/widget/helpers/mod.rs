//! `helpers` -- the primitives both validators share: unwrapping the ```json fence a model wraps its output in, the option-list check, the field-name check, and the value check. Split out so neither validator holds the other's primitives — `validate` is already the long one.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) fn strip_json_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some((_, body)) = after_open.split_once('\n') else {
        return trimmed;
    };
    body.strip_suffix("```").map(str::trim).unwrap_or(trimmed)
}

pub(super) fn validate_field_options(field: &WidgetField) -> Result<()> {
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

pub(super) fn is_field_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z' | '_'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
        && name.len() <= 32
}

pub(super) fn validate_value(field: &WidgetField, value: &Value) -> Result<()> {
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

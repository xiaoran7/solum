//! `draft` -- turning a proposed draft into a validated schema, and the refusal path when it cannot be. Separated from the shapes so the shapes stay a table of what things are and this is the only place that decides what is allowed.
//!
//! Part of the `widget` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::strip_json_fence;
use super::icons::ALLOWED_ICONS;
use super::*;

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

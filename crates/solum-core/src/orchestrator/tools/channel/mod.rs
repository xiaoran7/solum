//! `channel` -- the tools that talk outward rather than destroy inward: widget deletion, Soulous event push, and email send. They share a section because the risk they carry is the same — an irreversible message to a third party — and the preview text all three need is defined once, in `previews`.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
/// Cancelling an event cascades to its reminders, so a visible button is not
/// enough authorization. The preview binds the token to the event's current
/// title, time and reminder count; edits between preview and execution make the
/// token stale.
pub(super) struct EventCancelTool;

impl EventCancelTool {
    fn event_id(args: &str) -> Result<i64> {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| CoreError::Invalid(format!("event_cancel 参数不是 JSON：{e}")))?
            .get("event_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| CoreError::Invalid("event_cancel 缺少正整数 event_id".into()))
    }
}

impl Tool for EventCancelTool {
    fn name(&self) -> &str {
        "event_cancel"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn audit_summary(&self, args: &str) -> String {
        match Self::event_id(args) {
            Ok(id) => format!("event_cancel(event#{id})"),
            Err(_) => "event_cancel(参数无效)".to_string(),
        }
    }

    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::event_id(args).and_then(|id| {
            let event = ctx.store.get_event(id)?;
            let reminders = ctx
                .store
                .list_notifications()?
                .into_iter()
                .filter(|notification| notification.event_id == id)
                .count();
            Ok((event, reminders))
        }) {
            Ok((event, reminders)) => format!(
                "将永久取消日程「{}」（{}），并删除其关联的 {} 条提醒。此操作不可恢复。",
                event.title,
                crate::model::fmt_ts_human(&event.start),
                reminders
            ),
            Err(error) => format!("参数或日程无效，无法执行：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let id = Self::event_id(args)?;
        let event = ctx.store.get_event(id)?;
        ctx.store.delete_event(id)?;
        Ok(format!("已取消「{}」并删除其提醒。", event.title))
    }
}

/// Deleting every persona version — the user's whole style history at once.
pub(super) struct PersonaClearTool;

impl Tool for PersonaClearTool {
    fn name(&self) -> &str {
        "persona_clear"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, _args: &str, ctx: &ToolCtx) -> String {
        let n = ctx
            .store
            .list_persona_versions()
            .map(|v| v.len())
            .unwrap_or(0);
        format!("将永久删除全部 {n} 个人格版本与当前活动指针。版本历史不可恢复。")
    }
    fn execute(&self, _args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        ctx.store.clear_persona()?;
        Ok("已删除全部人格版本。".to_string())
    }
}

/// One widget record. Individually small, but it is still an irreversible
/// deletion of user data and the same rule applies.
pub(super) struct WidgetRecordDeleteTool;

impl WidgetRecordDeleteTool {
    fn parse_args(args: &str) -> Result<(i64, i64)> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("widget_record_delete 参数不是 JSON: {e}")))?;
        let widget_id = v["widget_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Invalid("widget_record_delete 缺少 widget_id".into()))?;
        let record_id = v["record_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Invalid("widget_record_delete 缺少 record_id".into()))?;
        Ok((widget_id, record_id))
    }
}

impl Tool for WidgetRecordDeleteTool {
    fn name(&self) -> &str {
        "widget_record_delete"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::parse_args(args) {
            Ok((widget_id, record_id)) => {
                let name = ctx
                    .store
                    .get_widget_definition(widget_id)
                    .map(|d| d.name)
                    .unwrap_or_else(|_| format!("#{widget_id}"));
                format!("将永久删除组件「{name}」的记录 #{record_id}。删除后不可恢复。")
            }
            Err(e) => format!("参数无效，无法执行：{e}"),
        }
    }
    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let (widget_id, record_id) = Self::parse_args(args)?;
        ctx.store.delete_widget_record(widget_id, record_id)?;
        Ok("记录已删除。".to_string())
    }
}

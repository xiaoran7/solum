//! `previews` -- how a preview line is rendered: `truncate_for_preview` and the two tools that need it, each of which shows the human a real line of the outgoing message before the one-time token exists. The truncation lives with the previews rather than with the channel tools so that a new outbound tool has one place to look for how its preview is bounded.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
/// Keep a preview line readable when the underlying content is long.
pub(super) fn truncate_for_preview(s: &str) -> String {
    const MAX: usize = 40;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}

/// F19's one destructive structural operation. Record CRUD is safe, but a
/// definition deletion cascades to its complete local dataset and therefore
/// can only run through the Guard's preview → confirmation → one-time Grant.
pub(super) struct WidgetDeleteTool;

impl WidgetDeleteTool {
    fn widget_id(args: &str) -> Result<i64> {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| CoreError::Invalid(format!("widget_delete 参数不是 JSON：{e}")))?
            .get("widget_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| CoreError::Invalid("widget_delete 缺少正整数 widget_id".into()))
    }
}

impl Tool for WidgetDeleteTool {
    fn name(&self) -> &str {
        "widget_delete"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::widget_id(args).and_then(|id| {
            let definition = ctx.store.get_widget_definition(id)?;
            let records = ctx.store.widget_record_count(id)?;
            Ok((definition.name, records))
        }) {
            Ok((name, records)) => {
                format!("将永久删除组件「{name}」及其全部 {records} 条记录。此操作不可恢复。")
            }
            Err(error) => format!("参数或组件无效，无法执行：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let id = Self::widget_id(args)?;
        let (name, records) = ctx.store.delete_widget_definition(id)?;
        Ok(format!("已永久删除组件「{name}」及其 {records} 条记录。"))
    }
}

/// L2 Phase 8.2: the only outbound Solum → Soulous action initially exposed.
/// It is `Sensitive`, so every execution goes through Guard's visible preview,
/// one-time token, and append-only audit record. The tool has no scheduler or
/// sync caller by design; it cannot silently become background export.
pub(super) struct SoulousPushEventTool;

impl SoulousPushEventTool {
    fn event_id(args: &str) -> Result<i64> {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| CoreError::Invalid(format!("soulous_push_event 参数不是 JSON: {e}")))?
            .get("event_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| CoreError::Invalid("soulous_push_event 缺少正整数 event_id".into()))
    }
}

impl Tool for SoulousPushEventTool {
    fn name(&self) -> &str {
        "soulous_push_event"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Sensitive
    }

    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::event_id(args).and_then(|id| crate::soulous::preview_event_push(ctx.store, id))
        {
            Ok(preview) => preview,
            Err(error) => format!("参数或日程无效，无法推送：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let event_id = Self::event_id(args)?;
        let outcome = crate::soulous::push_event_configured(ctx.store, event_id)?;
        Ok(format!(
            "已向 Soulous 推送日程「{}」{}。",
            outcome.title,
            if outcome.refreshed_tokens {
                "（登录 token 已自动刷新）"
            } else {
                ""
            }
        ))
    }
}

/// F21's only outbound mail action. The exact serialized draft is the Guard
/// token fingerprint, so confirmation cannot be replayed against a changed
/// recipient or body. The audit summary deliberately excludes all message
/// content; the full data remains visible only in the transient preview.
pub(super) struct EmailSendTool;

impl Tool for EmailSendTool {
    fn name(&self) -> &str {
        "email_send"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Sensitive
    }

    fn request_summary(&self, args: &str) -> String {
        match crate::email::EmailSendDraft::parse_json(args) {
            Ok(draft) => format!(
                "邮件发送：账户 {}，收件人 {} 位",
                draft.account_id,
                draft.to.len() + draft.cc.len() + draft.bcc.len()
            ),
            Err(error) => format!("邮件发送（草稿无效：{error}）"),
        }
    }

    fn audit_summary(&self, args: &str) -> String {
        crate::email::EmailSendDraft::parse_json(args)
            .map(|draft| draft.audit_summary())
            .unwrap_or_else(|_| "邮件发送：草稿无效，内容已脱敏".into())
    }

    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        match crate::email::EmailSendDraft::parse_json(args)
            .and_then(|draft| crate::email::preview_configured(&draft))
        {
            Ok(preview) => preview,
            Err(error) => format!("邮件草稿或账户无效，无法发送：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        let draft = crate::email::EmailSendDraft::parse_json(args)?;
        crate::email::send_configured(&draft)
    }
}

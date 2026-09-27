//! `ledger` -- the deletions that target one thing the user can name: memory layers, a single event, the persona, a single widget record. Event cancellation is here rather than with the other channel-facing tools because the cascade to its reminders is what makes its preview bind to the event's current title, time and reminder count.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::previews::truncate_for_preview;
use super::records::DataImportTool;
use super::*;

impl Tool for DataImportTool {
    fn name(&self) -> &str {
        "data_import"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn request_summary(&self, _args: &str) -> String {
        // Constant-sized even for malformed/oversized input. The Guard still
        // fingerprints the complete arguments independently of this label.
        "导入本机备份（正文不记入审计）".into()
    }

    fn audit_summary(&self, args: &str) -> String {
        self.request_summary(args)
    }

    fn audit_error_summary(&self, _error: &CoreError) -> String {
        // Import errors may quote document-controlled table/field names.
        "导入失败（详细原因不记入审计）".into()
    }

    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        match Self::document(args).and_then(|doc| crate::export::plan_import(&doc)) {
            Ok(plan) => {
                let detail = plan
                    .counts
                    .iter()
                    .map(|(tbl, n)| format!("{tbl} {n} 条"))
                    .collect::<Vec<_>>()
                    .join("、");
                format!(
                    "将合并来自设备 {} 于 {} 导出的 {} 条数据（{}）。\
                     不会删除任何本机数据；本机更新过的记录保持本机版本。",
                    plan.origin,
                    plan.exported_at.format("%Y-%m-%d %H:%M"),
                    plan.total(),
                    detail
                )
            }
            Err(error) => format!("无法读取该导出文件：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let doc = Self::document(args)?;
        let (plan, counts) = crate::export::import_document(ctx.store, &doc, ctx.now)?;
        Ok(format!(
            "已从 {} 的备份合并 {} 条数据：写入 {} 条，跳过 {} 条（本机已是更新版本或重复导入）。",
            plan.origin,
            plan.total(),
            counts.applied,
            counts.skipped
        ))
    }
}

/// F12's per-row deletion, behind the Guard.
///
/// This used to be a bare IPC command (`forget`) gated only by a confirmation
/// dialog in the WebView. A dialog is a rendering, not an authorization
/// boundary: the command underneath it can be invoked directly, so anything
/// that got script execution inside the WebView could empty the ledger row by
/// row. §3.3 says destructive operations go through preview → confirmation →
/// one-time token, and there is no reason this one was exempt.
pub(super) struct MemoryForgetTool;

impl MemoryForgetTool {
    fn parse_args(args: &str) -> Result<(MemoryLayer, i64)> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("forget 参数不是 JSON: {e}")))?;
        let layer: MemoryLayer = v["layer"]
            .as_str()
            .ok_or_else(|| CoreError::Invalid("forget 缺少 layer".into()))?
            .parse()?;
        let id = v["id"]
            .as_i64()
            .ok_or_else(|| CoreError::Invalid("forget 缺少 id".into()))?;
        Ok((layer, id))
    }
}

impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    /// The row's own text is the user's private content; keep it out of the
    /// audit trail, which would otherwise become a second copy of everything
    /// they asked to delete.
    fn audit_summary(&self, args: &str) -> String {
        match Self::parse_args(args) {
            Ok((layer, id)) => format!("memory_forget({}#{id})", layer.as_str()),
            Err(_) => "memory_forget(参数无效)".to_string(),
        }
    }
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::parse_args(args) {
            Ok((layer, id)) => {
                let label = layer_label(layer);
                let summary = ctx
                    .store
                    .memory_summary(layer, id)
                    .ok()
                    .flatten()
                    .map(|s| format!("「{}」", truncate_for_preview(&s)))
                    .unwrap_or_else(|| format!("#{id}"));
                let cascade = ctx
                    .store
                    .describe_memory_deletion(layer, id)
                    .unwrap_or_default();
                format!("将永久删除{label}{summary}{cascade}。删除后不可通过对话恢复。")
            }
            Err(e) => format!("参数无效，无法执行：{e}"),
        }
    }
    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let (layer, id) = Self::parse_args(args)?;
        ctx.store.delete_memory(layer, id)?;
        Ok(format!("已永久删除{}#{id}。", layer_label(layer)))
    }
}

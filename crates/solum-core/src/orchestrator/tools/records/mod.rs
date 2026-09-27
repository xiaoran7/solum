//! `records` -- the tools that rewrite stored rows: data import and backup deletion. Both act on a whole set rather than a single row, which is the line between this section and the per-row ones, and both are the kind of bulk operation whose preview is a count the human has to be able to trust.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
/// The first **real** dangerous tool (D3): bulk-delete ledger rows of one
/// layer older than a cutoff. Args are JSON:
/// `{"layer":"behavior|suggestion|wearable","before":"YYYY-MM-DDTHH:MM:SS"}`.
/// The preview reads the store so the human confirms a real row count; the
/// execution is unreachable without the guard's one-time token, and both
/// outcomes land in the append-only audit log.
pub(super) struct LedgerPurgeTool;

impl LedgerPurgeTool {
    fn parse_args(args: &str) -> Result<(MemoryLayer, NaiveDateTime)> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("ledger_purge 参数不是 JSON: {e}")))?;
        let layer = match v["layer"].as_str() {
            Some("behavior") => MemoryLayer::Behavior,
            Some("suggestion") => MemoryLayer::Suggestion,
            Some("wearable") => MemoryLayer::Wearable,
            other => {
                return Err(CoreError::Invalid(format!(
                    "ledger_purge.layer 必须是 behavior|suggestion|wearable，得到 {other:?}"
                )))
            }
        };
        let before = v["before"]
            .as_str()
            .ok_or_else(|| CoreError::Invalid("ledger_purge 缺少 before".into()))
            .and_then(crate::model::parse_ts)?;
        Ok((layer, before))
    }
}

impl Tool for LedgerPurgeTool {
    fn name(&self) -> &str {
        "ledger_purge"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::parse_args(args) {
            Ok((layer, before)) => {
                let count = ctx.store.count_memory_before(layer, before).unwrap_or(0);
                format!(
                    "将从记忆台账永久删除 {count} 条「{}」记录（早于 {}）。删除后不可通过对话恢复。",
                    layer_label(layer),
                    fmt_ts(&before)
                )
            }
            Err(e) => format!("参数无效，无法执行：{e}"),
        }
    }
    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let (layer, before) = Self::parse_args(args)?;
        let n = ctx.store.purge_memory_before(layer, before)?;
        Ok(format!(
            "已永久删除 {n} 条「{}」记录（早于 {}）。",
            layer_label(layer),
            fmt_ts(&before)
        ))
    }
}

/// Restoring a backup writes across every layer at once. It is not
/// destructive by design (nothing is deleted, and LWW keeps newer local rows),
/// but "merge a whole other database into mine" is exactly the kind of
/// broad-blast-radius write the Guard exists for — the user should see how
/// many rows, from which device, and from when, before it happens.
pub(super) struct DataImportTool;

impl DataImportTool {
    /// Ceiling on the raw argument string, checked **before** parsing.
    ///
    /// The shell checks `file.size` before reading, but that is a courtesy in
    /// the UI, not a limit: the IPC command takes a string and can be invoked
    /// directly, and `serde_json::from_str` allocates the whole document tree
    /// before any of the row/field limits downstream get a chance to look at
    /// it. A limit that only exists in the file picker is not a limit.
    const MAX_ARGS_BYTES: usize = 64 * 1024 * 1024;

    pub(super) fn document(args: &str) -> Result<serde_json::Value> {
        if args.len() > Self::MAX_ARGS_BYTES {
            return Err(CoreError::Invalid(format!(
                "导入内容超过 {} MiB 上限，已拒绝解析",
                Self::MAX_ARGS_BYTES / 1024 / 1024
            )));
        }
        let value: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("data_import 参数不是 JSON：{e}")))?;
        value
            .get("document")
            .cloned()
            .ok_or_else(|| CoreError::Invalid("data_import 缺少 document".into()))
    }
}

/// Permanently remove one complete opaque backup version. The exact catalog
/// metadata the user saw is embedded in the Guard fingerprint; execution
/// re-lists the catalog and the server atomically checks the plan ETag again.
pub(super) struct BackupDeleteTool;

impl BackupDeleteTool {
    fn request(args: &str) -> Result<crate::backup_download::BackupDeleteRequest> {
        let request: crate::backup_download::BackupDeleteRequest = serde_json::from_str(args)
            .map_err(|error| {
                CoreError::Invalid(format!("backup_delete 参数不是严格 JSON：{error}"))
            })?;
        crate::backup_download::validate_backup_delete_request(&request)
            .map_err(|error| CoreError::Invalid(error.to_string()))?;
        Ok(request)
    }

    fn size_label(bytes: u64) -> String {
        const MIB: f64 = 1024.0 * 1024.0;
        if bytes >= 1024 * 1024 {
            format!("{:.1} MiB", bytes as f64 / MIB)
        } else {
            format!("{:.1} KiB", bytes as f64 / 1024.0)
        }
    }

    fn committed_label(value: &str) -> String {
        DateTime::parse_from_rfc3339(value)
            .map(|date| {
                date.with_timezone(&Utc)
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string()
            })
            .unwrap_or_else(|_| value.to_string())
    }
}

impl Tool for BackupDeleteTool {
    fn name(&self) -> &str {
        "backup_delete"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn request_summary(&self, _args: &str) -> String {
        "删除一个云端加密备份版本".into()
    }

    fn audit_summary(&self, _args: &str) -> String {
        "删除一个云端加密备份版本（标识已脱敏）".into()
    }

    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        match Self::request(args) {
            Ok(request) => format!(
                "将永久删除 {} 提交的云端加密备份（{}，{} 个密文对象）。删除后不能再用这个封缄点恢复；本机数据和其他备份版本不受影响。",
                Self::committed_label(&request.committed_at),
                Self::size_label(request.total_bytes),
                request.object_count
            ),
            Err(error) => format!("备份版本信息无效，无法删除：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        if crate::privacy::PrivacyConsent::needs_consent() {
            return Err(CoreError::Invalid(
                "请先阅读并同意当前隐私政策，再管理加密云备份".into(),
            ));
        }
        let request = Self::request(args)?;
        let mut account = crate::account::AccountSession::load_full()
            .ok_or_else(|| CoreError::Invalid("登录后才能删除加密云备份".into()))?;
        if account.stable_user_id().is_none() {
            return Err(CoreError::Invalid("当前账号缺少不可变用户标识".into()));
        }
        let transport = crate::backup_download::HttpBackupCloudReader::new();
        crate::backup_download::delete_backup_version(&transport, &mut account, &request)
            .map_err(|error| CoreError::Invalid(error.to_string()))?;
        Ok("已永久删除所选云端加密备份版本。".into())
    }
}

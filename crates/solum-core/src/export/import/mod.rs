//! `import` -- the write side, and deliberately two steps: `plan_import` says what restoring would touch without doing it, `import_document` is the one that acts. The split is what lets a restore be reviewed before it is applied.
//!
//! Part of the `export` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// What restoring a document would do, without doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    pub version: u32,
    pub exported_at: NaiveDateTime,
    pub origin: String,
    /// `(table, row count)`, only non-empty tables.
    pub counts: Vec<(String, usize)>,
}

impl ImportPlan {
    pub fn total(&self) -> usize {
        self.counts.iter().map(|(_, n)| n).sum()
    }
}

fn restore_section(doc: &serde_json::Value) -> Result<&serde_json::Value> {
    let format = doc.get("format").and_then(|v| v.as_str());
    if !matches!(format, Some(FORMAT) | Some(LEGACY_FORMAT)) {
        return Err(CoreError::Invalid(format!("不是 {FORMAT} 文档")));
    }
    let version = doc
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    if version < MIN_RESTORABLE_VERSION {
        return Err(CoreError::Invalid(format!(
            "导出格式 v{version} 不含可还原数据（v{MIN_RESTORABLE_VERSION} 起才有）；\
             该文件仍可人工查阅，但无法自动导入"
        )));
    }
    if version > MAX_PLANNABLE_VERSION {
        return Err(CoreError::Invalid(format!(
            "导出文件版本 v{version} 比本机（v{MAX_PLANNABLE_VERSION}）新，请先升级再导入"
        )));
    }
    doc.get("_restore")
        .ok_or_else(|| CoreError::Invalid("文档缺少 _restore 段".into()))
}

pub fn plan_import(doc: &serde_json::Value) -> Result<ImportPlan> {
    let restore = restore_section(doc)?;
    let version = doc
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let mut counts = Store::count_restore_rows(restore);
    if version == crate::export_v3::PORTABLE_EXPORT_VERSION {
        counts.extend(crate::export_v3::validate_portable_document(doc)?.counts);
    }
    Ok(ImportPlan {
        version,
        exported_at: doc
            .get("exported_at")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| crate::model::parse_ts(s).ok())
            .ok_or_else(|| CoreError::Invalid("文档缺少可解析的 exported_at".into()))?,
        origin: doc
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("import")
            .to_string(),
        counts,
    })
}

/// Merge a document's rows into this store. Idempotent, and never destructive:
/// nothing is deleted, and rows the user has edited more recently than the
/// backup keep their local version (see `Store::import_restore_rows`).
pub fn import_document(
    store: &Store,
    doc: &serde_json::Value,
    now: chrono::NaiveDateTime,
) -> Result<(ImportPlan, crate::store::MergeCounts)> {
    let plan = plan_import(doc)?;
    if plan.version == crate::export_v3::PORTABLE_EXPORT_VERSION {
        return Err(CoreError::Invalid(
            "solum-export v3 需要平台临时库恢复事务；已在写入 Store 前拒绝部分恢复".into(),
        ));
    }
    let restore = restore_section(doc)?;
    let counts = store.import_restore_rows(
        restore,
        plan.exported_at,
        &format!("import:{}", plan.origin),
        now,
    )?;
    Ok((plan, counts))
}

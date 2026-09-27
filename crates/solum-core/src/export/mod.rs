//! 一键全量数据导出（F12 延伸 / §4 承诺兑现）。
//!
//! 定位说"数据完全归你"，§4 明确审计日志用户可导出——这里把承诺落成代码：
//! 本机 SQLite 里用户可见的每一层（原始输入 / 事件 / 提醒 / 行为日志 / 建议 /
//! 语义记忆 / routine / 穿戴 / 人格全部版本 / 规则表 / 主动度 / 审计日志）
//! 聚合成一份可读 JSON。纯只读、纯本地、不上云；导出的用途（备份 / 迁移 /
//! 离机审查）由用户自己决定。
//!
//! 不导出的只有同步内务数据（oplog / 设备游标）——那是传输机制不是记忆，
//! 且换设备重放 oplog 本就不成立。
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use chrono::NaiveDateTime;
use serde_json::json;

use crate::error::{CoreError, Result};
use crate::model::fmt_ts;
use crate::store::Store;
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use build::build_export;
pub use coverage::{
    BackupCoverage, PersistenceCoverage, EXPORT_VERSION, FORMAT, LEGACY_FORMAT,
    MAX_PLANNABLE_VERSION, MIN_RESTORABLE_VERSION, PLATFORM_BACKUP_COVERAGE,
    SQLITE_BACKUP_COVERAGE,
};
pub use import::{import_document, plan_import, ImportPlan};

mod build;
mod coverage;
mod import;
#[cfg(test)]
mod tests;

//! `coverage` -- the format's own constants and the coverage inventory -- what each table's backup actually preserves, checked against `sqlite_schema` rather than trusted, because a table that silently stops persisting is the failure this file exists to prevent.
//!
//! Part of the `export` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

/// 当前导出格式版本。字段增删时 +1，方便未来的导入端按版本兼容。
///
/// v2（2026-07-20）：新增 `_restore` 段。v1 的各层是给人看的，**不带 guid**，
/// 因此无法被还原——重复导入会整份翻倍，事件与其原始输入的关联也接不回来。
/// 备份只有在能还原时才叫备份，故补上一段行级的、带 guid 的线上形状。
pub const EXPORT_VERSION: u32 = 2;

/// 能被 [`import_document`] 还原的最低格式版本。
pub const MIN_RESTORABLE_VERSION: u32 = 2;
/// Newest version this core can fully validate and recovery-plan. v3 requires
/// a platform transaction and is deliberately not accepted by
/// [`import_document`], which only knows how to write Store rows.
pub const MAX_PLANNABLE_VERSION: u32 = crate::export_v3::PORTABLE_EXPORT_VERSION;

/// 导出文档的格式标识。2026-07-20 改名后写出的是 [`FORMAT`]。
pub const FORMAT: &str = "solum-export";
/// 改名前写出的标识。**读取时必须继续接受**——否则用户改名前导出的备份
/// 会在导入端被判为"不是导出文档"而整份拒绝，等于把已有备份作废。
/// 格式本身一个字节都没变，变的只有这个名字。
pub const LEGACY_FORMAT: &str = "pa-export";

/// Audited recovery disposition for every persistent source. This is kept
/// beside the logical export contract so a new storage table cannot silently
/// become either "backed up" or "forgotten" by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupCoverage {
    /// Present in `solum-export` v2's guid-bearing `_restore` section.
    RestorableV2,
    /// User-owned data, but a stable cross-platform restore shape is pending.
    PortablePending,
    /// Only the fact that a capability was configured may move; the target
    /// device must obtain a fresh grant or connection before enabling it.
    RestoreIntent,
    /// One physical source mixes fields with different recovery semantics.
    SplitByField,
    /// Device identity, secrets, caches, or operational transport state.
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistenceCoverage {
    pub source: &'static str,
    pub coverage: BackupCoverage,
}

/// Every application-owned SQLite table created by [`Store`]. A schema change
/// must update this list and choose a recovery disposition; a unit test checks
/// it against `sqlite_schema` rather than trusting this inventory by itself.
pub const SQLITE_BACKUP_COVERAGE: &[PersistenceCoverage] = &[
    PersistenceCoverage {
        source: "meta",
        coverage: BackupCoverage::SplitByField,
    },
    PersistenceCoverage {
        source: "raw_inputs",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "events",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "notifications",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "behavior_log",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "suggestions",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "persona_versions",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "health_samples",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "memory_facts",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "routines",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "soulous_facts",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "notification_captures",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "notification_filter_proposals",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "notification_action_proposals",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "widget_defs",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "widget_fields",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "widget_records",
        coverage: BackupCoverage::RestorableV2,
    },
    PersistenceCoverage {
        source: "widget_schema_rejections",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "audit_log",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "sync_oplog",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "sync_quarantine",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "capture_loss_receipts",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "sync_bad_blobs",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "sync_push_holds",
        coverage: BackupCoverage::Excluded,
    },
];

/// Non-SQLite persistence that future platform adapters must also account for.
/// These identifiers are protocol-facing descriptions, not filesystem paths.
pub const PLATFORM_BACKUP_COVERAGE: &[PersistenceCoverage] = &[
    PersistenceCoverage {
        source: "webview:pa.chat-sessions.v1",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "webview:solum.ui-state.v1/chatDrafts",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "webview:solum.ui-state.v1/navigation-and-scroll",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "indexeddb:pa-local-documents-v1/documents",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "file:notif-spool-and-legacy-inbox",
        coverage: BackupCoverage::PortablePending,
    },
    PersistenceCoverage {
        source: "file:notif-policy.json",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "file:solum-llm.json",
        coverage: BackupCoverage::SplitByField,
    },
    PersistenceCoverage {
        source: "file:solum-email.json",
        coverage: BackupCoverage::SplitByField,
    },
    PersistenceCoverage {
        source: "file:solum-soulous.json",
        coverage: BackupCoverage::SplitByField,
    },
    PersistenceCoverage {
        source: "file:solum-account.json",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "file:solum-sync.json",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "file:solum-privacy-consent.json",
        coverage: BackupCoverage::Excluded,
    },
    PersistenceCoverage {
        source: "native:permissions-connections-and-alarms",
        coverage: BackupCoverage::RestoreIntent,
    },
    PersistenceCoverage {
        source: "native:notification-overflow-and-alert-cursors",
        coverage: BackupCoverage::Excluded,
    },
];

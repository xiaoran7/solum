//! Local SQLite store — the device's authoritative data source (local-first,
//! ARCHITECTURE.md §3.5/§3.8). Holds raw inputs, extracted events, scheduled
//! notifications, the append-only audit log, and user config (rule table,
//! proactivity). Everything here is what the F12 memory ledger exposes for
//! inspection and deletion.
//!
//! Times are stored as fixed-width [`TS_FMT`] strings, which sort correctly
//! lexicographically. Personal data never leaves this file except as encrypted
//! sync blobs (not part of Phase 1).

use std::path::{Path, PathBuf};

use chrono::{NaiveDateTime, NaiveTime};
use rusqlite::{params, Connection, OptionalExtension};

/// One-time adoption of a database left behind by the 2026-07-20 `pa` → `solum`
/// rename. When the current path has no database yet but a pre-rename one sits
/// beside it, move it across (with its `-wal` / `-shm` siblings) instead of
/// silently starting from an empty store.
///
/// Never overwrites: if a database already exists at `current`, the legacy file
/// is left untouched for the user to inspect rather than merged or clobbered.
/// Returns whether anything moved.
///
/// **This only helps where the containing directory survived the rename** — the
/// desktop's cwd. Android's app-data directory is derived from the bundle
/// identifier, which the rename also changed (`dev.pa.app` → `dev.solum.app`),
/// so a pre-rename install's data sits in *another package's* private storage
/// that this process cannot read. That migration is necessarily export →
/// install → import; see docs/PITFALLS.md 2026-07-20.
/// Adoption is **checkpoint-then-move-one-file**, never file-by-file.
///
/// Moving `db`, `-wal` and `-shm` as three separate renames has a failure mode
/// that silently destroys data: if the main file moves and the `-wal` move then
/// fails (or the process dies between them), the next launch sees a database
/// already at the new path, declines to adopt anything, and opens it — without
/// the committed transactions that were still sitting in the abandoned WAL.
/// The loss is invisible and permanent.
///
/// So: fold the WAL into the main file first (`wal_checkpoint(TRUNCATE)`), then
/// there is only one file that matters and only one rename to do. If the
/// checkpoint fails, **nothing is moved** — the legacy database stays whole and
/// the next launch can try again.
pub fn adopt_legacy_db(legacy: &Path, current: &Path) -> std::io::Result<bool> {
    if current.exists() || !legacy.exists() {
        return Ok(false);
    }
    let to_io = |e: rusqlite::Error| std::io::Error::other(format!("checkpoint 旧库失败: {e}"));
    {
        let conn = Connection::open(legacy).map_err(to_io)?;
        // Switching out of WAL checkpoints every outstanding frame back into
        // the main file and removes the WAL — exactly the guarantee we need
        // before treating the main file as the whole database. `migrate()`
        // puts the adopted database back into WAL when it opens it.
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(to_io)?;
    }
    // Only now is the main file self-contained.
    std::fs::rename(legacy, current)?;
    // The sidecars are empty after the checkpoint; failing to remove them is
    // cosmetic, so it must not fail the adoption.
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sqlite_sibling(legacy, suffix));
    }
    Ok(true)
}

/// `foo.sqlite` + `-wal` → `foo.sqlite-wal`. Appends to the whole file name
/// rather than the extension, which is how SQLite names these files.
pub(crate) fn sqlite_sibling(path: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return path.to_path_buf();
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

use crate::classify::RuleTable;
use crate::error::{CoreError, Result};
use crate::guard::AuditEntry;
use crate::journal::{BehaviorEntry, BehaviorKind};
use crate::model::{
    fmt_ts, parse_ts, Channel, Event, MemoryEntry, MemoryLayer, Notification, NotificationStatus,
};
use crate::notification_intelligence::{
    ActionProposalState, CaptureLane, CaptureState, FilterProposalState, NotificationActionKind,
    NotificationActionProposal, NotificationCapture, NotificationCaptureRecord,
    NotificationFilterProposal, NotificationIntelligenceConfig,
};
use crate::proactivity::ProactivityConfig;
use crate::soulous::{SoulousFact, SOURCE as SOULOUS_SOURCE};
use crate::suggest::{Suggestion, SuggestionStatus};
use crate::wearable::{HealthMetric, HealthSample};
use crate::widget::{
    WidgetDefinition, WidgetDefinitionDraft, WidgetField, WidgetRecord, WidgetSchema,
    WidgetSchemaRejection, WidgetView, WidgetViewType,
};

/// One `widget_fields` row as loaded: the field plus its position in each of
/// the four views, indexed by [`WidgetViewType::ALL`] (`None` = not in it).
/// One `widget_fields` row as loaded: the field, plus its position in each of
/// the four view slots (indexed by [`WidgetViewType::ALL`]; `None` = the field
/// is not in that view).
type LoadedWidgetField = (WidgetField, [Option<i64>; 4]);

/// One source of truth for "what a row means on the wire": the sync capture
/// triggers build oplog payloads from these expressions, and the export builds
/// its restorable `_restore` section from the very same ones with the `NEW.`
/// prefix stripped. Keeping one list is what lets an export be replayed through
/// the ordinary merge path instead of needing a second, parallel importer.
///
/// (table, payload json_object expression over NEW.*, syncability condition)
const SYNC_PAYLOADS: &[(&str, &str, &str)] = &[
            // `local_only` rides along so the capture-time cloud-LLM decision
            // reaches every device unchanged (§3.8). Sync itself is
            // unconditional: it is end-to-end encrypted to the user's own
            // relay, which is not the risk surface the switch guards.
            (
                "raw_inputs",
                "json_object('text', NEW.text, 'intent', NEW.intent, \
                 'created_at', NEW.created_at, 'local_only', NEW.local_only, \
                 'source_id', NEW.source_id, 'external_id', NEW.external_id)",
                "1",
            ),
            (
                "events",
                "json_object('title', NEW.title, 'kind', NEW.kind, 'start', NEW.start, \
                 'end', NEW.\"end\", 'location', NEW.location, 'people_json', NEW.people_json, \
                 'raw_input_guid', (SELECT guid FROM raw_inputs WHERE id = NEW.raw_input_id), \
                 'routine_guid', (SELECT guid FROM routines WHERE id = NEW.routine_id), \
                 'created_at', NEW.created_at, 'local_only', NEW.local_only)",
                "1",
            ),
            (
                "notifications",
                "json_object('event_guid', (SELECT guid FROM events WHERE id = NEW.event_id), \
                 'fire_at', NEW.fire_at, 'lead_label', NEW.lead_label, \
                 'channels_json', NEW.channels_json, 'status', NEW.status, \
                 'created_at', NEW.created_at, 'fired_at', NEW.fired_at)",
                "1",
            ),
            (
                "behavior_log",
                "json_object('ts', NEW.ts, 'kind', NEW.kind, 'content', NEW.content, 'source', NEW.source)",
                "1",
            ),
            (
                "suggestions",
                "json_object('created_at', NEW.created_at, 'kind', NEW.kind, 'text', NEW.text, \
                 'dedup_key', NEW.dedup_key, 'source', NEW.source, 'status', NEW.status)",
                "1",
            ),
            (
                "persona_versions",
                "json_object('version', NEW.version, 'created_at', NEW.created_at, 'profile_json', NEW.profile_json)",
                "1",
            ),
            (
                "health_samples",
                "json_object('kind', NEW.kind, 'start', NEW.start, 'end', NEW.\"end\", \
                 'value', NEW.value, 'source', NEW.source, 'created_at', NEW.created_at, \
                 'dedup_key', NEW.dedup_key)",
                "1",
            ),
            (
                "memory_facts",
                "json_object('content', NEW.content, 'source', NEW.source, \
                 'created_at', NEW.created_at, 'last_used_at', NEW.last_used_at)",
                "1",
            ),
            (
                "routines",
                "json_object('title', NEW.title, 'time_of_day', NEW.time_of_day, \
                 'source', NEW.source, 'active', NEW.active, 'created_at', NEW.created_at, \
                 'scheduled_until', NEW.scheduled_until)",
                "1",
            ),
            (
                "soulous_facts",
                "json_object('external_id', NEW.external_id, 'kind', NEW.kind, \
                 'title', NEW.title, 'occurs_at', NEW.occurs_at, 'ends_at', NEW.ends_at, \
                 'payload_json', NEW.payload_json, 'source', NEW.source, \
                 'imported_at', NEW.imported_at)",
                "NEW.source = 'soulous'",
            ),
            (
                "widget_defs",
                "json_object('name', NEW.name, 'icon', NEW.icon, \
                 'list_sort_by', NEW.list_sort_by, 'table_sort_by', NEW.table_sort_by, \n                 'created_at', NEW.created_at)",
                "1",
            ),
            // Every view slot must ride along, and so must `ord`: a field that
            // arrives without them lands on the column defaults (ord 0, NULL
            // membership), which silently drops the table/stat views and
            // scrambles the canonical field order on the receiving device.
            // `_restore` shares this definition, so an omission here loses the
            // same data in backups. Adding a view slot means adding it here.
            (
                "widget_fields",
                "json_object('widget_guid', (SELECT guid FROM widget_defs WHERE id = NEW.widget_id), \
                 'name', NEW.name, 'label', NEW.label, 'field_type', NEW.field_type, \
                 'required', NEW.required, 'options_json', NEW.options_json, \
                 'ord', NEW.ord, 'form_ord', NEW.form_ord, 'list_ord', NEW.list_ord, \
                 'table_ord', NEW.table_ord, 'stat_ord', NEW.stat_ord, \
                 'created_at', NEW.created_at)",
                "1",
            ),
            (
                "widget_records",
                "json_object('widget_guid', (SELECT guid FROM widget_defs WHERE id = NEW.widget_id), \
                 'data_json', NEW.data_json, 'created_at', NEW.created_at)",
                "1",
            ),
];

const SCHEMA_VERSION: i64 = 16;
const NOTIF_CLOUD_SCOPE_SCHEMA_VERSION: i64 = 7;
/// v15 re-emitted `widget_fields` rows whose view slots and canonical order
/// never made it into the sync payload (and therefore never reached a peer).
const WIDGET_VIEW_SLOT_PAYLOAD_SCHEMA_VERSION: i64 = 15;
/// v10 decoupled sync from the cloud-LLM scope: notification rows now sync
/// unconditionally, and `local_only` means "never send to a cloud LLM" only.
const NOTIF_SYNC_DECOUPLE_SCHEMA_VERSION: i64 = 10;
/// A device that never upgrades must not grow `sync_quarantine` without
/// bound. Past this many held ops the oldest are dropped and counted under
/// [`QUARANTINE_DROPPED_KEY`] — losing data is bad, but doing it *visibly* is
/// the point: the count is what a UI can surface.
const MAX_QUARANTINE_OPS: i64 = 5_000;
/// Ceilings on an imported backup. A backup file is arbitrary user-supplied
/// JSON that gets fully parsed *before* the Guard preview can describe it, so
/// an oversized one exhausts memory while the user is still deciding whether
/// to allow it. These are far above any real Solum database.
/// Unopenable blobs are kept so a build with the right key can recover them,
/// but a peer holding a valid token can emit garbage indefinitely — keeping it
/// all would let one misconfigured device fill every other device's disk.
/// Bounded and *visible*, same shape as `sync_quarantine`.
const MAX_BAD_BLOBS: i64 = 200;
const MAX_BAD_BLOB_BYTES: usize = 1024 * 1024;
const BAD_BLOBS_DROPPED_KEY: &str = "sync_bad_blobs_dropped";
const MAX_IMPORT_ROWS: usize = 500_000;
const MAX_IMPORT_ROW_BYTES: usize = 1024 * 1024;
const QUARANTINE_DROPPED_KEY: &str = "sync_quarantine_dropped";
const NOTIF_CLOUD_META_KEY: &str = "notif_cloud";
const NOTIFICATION_INTELLIGENCE_META_KEY: &str = "notification_intelligence";

/// Keyset position for one F12 ledger page: "strictly after this
/// (created_at, id) in (created_at DESC, id ASC) order". `excl_id` compares
/// with `id >` because the ledger sorts same-timestamp rows by ascending id;
/// encoding uses the storage timestamp format so decode is exact.
#[derive(Debug, Clone)]
pub(crate) struct LedgerCursor {
    created_at_str: String,
    excl_id: i64,
}

impl LedgerCursor {
    pub(crate) fn encode(&self) -> String {
        format!("{}|{}", self.created_at_str, self.excl_id)
    }

    pub(crate) fn decode(raw: &str) -> Result<Self> {
        let (created_at_str, excl_id) = raw
            .rsplit_once('|')
            .ok_or_else(|| CoreError::Invalid("台账分页游标格式无效".into()))?;
        if created_at_str.is_empty() || parse_ts(created_at_str).is_err() {
            return Err(CoreError::Invalid("台账分页游标时间无效".into()));
        }
        let excl_id: i64 = excl_id
            .parse()
            .map_err(|_| CoreError::Invalid("台账分页游标序号无效".into()))?;
        Ok(LedgerCursor {
            created_at_str: created_at_str.to_string(),
            excl_id,
        })
    }
}

/// Tables that participate in multi-device sync (§3.8). `audit_log` stays
/// device-local by design (§4), and `meta` is handled key-by-key.
const SYNCED_TABLES: &[&str] = &[
    "raw_inputs",
    "events",
    "notifications",
    "behavior_log",
    "suggestions",
    "persona_versions",
    "health_samples",
    "memory_facts",
    "routines",
    "soulous_facts",
    // F19 second step. `widget_fields` is one row per field precisely so that
    // concurrent additions merge as a union under row-level LWW; do not
    // collapse it back into a JSON column on widget_defs.
    "widget_defs",
    "widget_fields",
    "widget_records",
];

/// Meta keys that sync as LWW documents. `persona_active` values are
/// translated to the version row's guid at capture time (version numbers may
/// differ per device after merges).
const SYNCED_META_KEYS: &str = "'rule_table', 'proactivity', 'persona_active'";

/// What one [`Store::apply_remote_ops`] round did. `quarantined` counts ops
/// this build could not interpret and parked for a later one (§3.8).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MergeCounts {
    pub applied: usize,
    pub skipped: usize,
    pub quarantined: usize,
}

/// A fresh 128-bit random row identity, stable across devices.
pub(crate) fn new_guid() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("os rng");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A read-only view of an audit record (append-only; never deleted or edited).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub ts: String,
    pub tool: String,
    pub risk: String,
    pub summary: String,
    pub decision: String,
    pub token_id: Option<String>,
    pub detail: String,
}

/// A read-only view of a raw input row (for the data export; the ledger's
/// raw_input layer shows the same rows interactively).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RawInputRow {
    pub id: i64,
    pub text: String,
    pub intent: String,
    pub created_at: String,
    pub source_id: String,
    pub external_id: Option<String>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) a store at `path` and run migrations.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        // The desktop shell holds two long-lived connections to one database
        // (`orch` and `sync_store`), so a write started by one can meet a
        // write started by the other. SQLite's default busy timeout is zero:
        // the losing side gets SQLITE_BUSY immediately. That error used to
        // surface mid-`fire_due`, where the ticker's best-effort logging
        // swallowed it — leaving a notification marked fired with no behavior
        // log row, i.e. exactly the silent loss this store is not allowed to
        // do. WAL keeps readers and writers from colliding; the timeout covers
        // writer-vs-writer, which WAL does not. 5s is far above any write this
        // schema performs and far below anything a user would notice.
        conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// An ephemeral in-memory store — used by tests and dry runs.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Fold every committed WAL frame into the main database and leave the
    /// connection in single-file journal mode for profile-generation sealing.
    /// The caller must own the temporary Store exclusively and drop it before
    /// asking the generation publisher to inventory the directory.
    pub fn checkpoint_for_profile_generation(&self) -> Result<()> {
        if !self.conn.is_autocommit() {
            return Err(CoreError::Invalid(
                "cannot seal a profile generation during a Store transaction".into(),
            ));
        }
        let busy: i64 = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        // SQLite returns one row with busy/log/checkpointed integers; only a
        // zero busy flag is safe to publish.
        if busy != 0 {
            return Err(CoreError::Storage(
                "temporary Store WAL checkpoint is busy".into(),
            ));
        }
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(CoreError::Storage(
                "temporary Store could not enter single-file journal mode".into(),
            ));
        }
        Ok(())
    }

    /// Run `f` so that everything it writes commits together or not at all.
    ///
    /// Exists because "delete the event, then delete its reminders" and
    /// "write the event, then write its notification, then advance the
    /// high-water mark" are *one* operation from the user's point of view, and
    /// a crash between the steps leaves states the rest of the code has no way
    /// to describe — an event with no reminder, a routine marked scheduled
    /// that never fires, a rule saved with its old reminders already gone.
    ///
    /// Nests safely: an inner call becomes a savepoint rather than trying (and
    /// failing) to open a second transaction, so store methods that use this
    /// internally can still be composed by a caller that wants a wider unit.
    pub fn with_transaction<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let nested = !self.conn.is_autocommit();
        // Savepoint names are identifiers, not parameters — this one is a
        // compile-time constant, and nesting depth is handled by SQLite's own
        // stack of same-named savepoints (RELEASE pops the innermost).
        let (begin, commit, rollback) = if nested {
            (
                "SAVEPOINT solum_tx",
                "RELEASE solum_tx",
                "ROLLBACK TO solum_tx; RELEASE solum_tx",
            )
        } else {
            ("BEGIN", "COMMIT", "ROLLBACK")
        };
        self.conn.execute_batch(begin)?;
        match f(self) {
            Ok(v) => {
                self.conn.execute_batch(commit)?;
                Ok(v)
            }
            Err(e) => {
                // A rollback failure must not mask the error that caused it.
                let _ = self.conn.execute_batch(rollback);
                Err(e)
            }
        }
    }

    /// Test-only fault injection: install a trigger that aborts every insert
    /// into `table` until disarmed, so multi-step writes can prove their
    /// transaction actually rolls back. Production code never calls this.
    #[cfg(test)]
    pub fn arm_insert_tripwire(&self, table: &str) {
        let name = format!("test_tripwire_{table}");
        self.conn
            .execute_batch(&format!(
                "DROP TRIGGER IF EXISTS {name};
                 CREATE TRIGGER {name} BEFORE INSERT ON {table}
                 BEGIN SELECT RAISE(ABORT, 'injected test failure'); END;"
            ))
            .expect("install test tripwire");
    }

    #[cfg(test)]
    pub fn disarm_insert_tripwire(&self, table: &str) {
        let name = format!("test_tripwire_{table}");
        self.conn
            .execute_batch(&format!("DROP TRIGGER IF EXISTS {name};"))
            .expect("remove test tripwire");
    }


}


mod migrations;
mod events_notifications;
mod journal_insights;
mod widgets_persona;
mod export_import;
mod audit_config_capture;
mod ledger;
pub(crate) mod sync;
pub(crate) use sync::*;

#[cfg(test)]
mod tests;

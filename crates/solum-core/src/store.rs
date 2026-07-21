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
fn sqlite_sibling(path: &Path, suffix: &str) -> PathBuf {
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
                 'created_at', NEW.created_at, 'local_only', NEW.local_only)",
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

const SCHEMA_VERSION: i64 = 15;
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
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) a store at `path` and run migrations.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
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

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS raw_inputs (
                id         INTEGER PRIMARY KEY,
                text       TEXT NOT NULL,
                intent     TEXT NOT NULL,
                created_at TEXT NOT NULL,
                local_only INTEGER NOT NULL DEFAULT 0,
                guid       TEXT
            );

            CREATE TABLE IF NOT EXISTS events (
                id           INTEGER PRIMARY KEY,
                title        TEXT NOT NULL,
                kind         TEXT NOT NULL,
                start        TEXT NOT NULL,
                end          TEXT,
                location     TEXT,
                people_json  TEXT NOT NULL DEFAULT '[]',
                raw_input_id INTEGER,
                routine_id   INTEGER,
                created_at   TEXT NOT NULL,
                local_only   INTEGER NOT NULL DEFAULT 0,
                guid         TEXT
            );

            CREATE TABLE IF NOT EXISTS notifications (
                id            INTEGER PRIMARY KEY,
                event_id      INTEGER NOT NULL,
                fire_at       TEXT NOT NULL,
                lead_label    TEXT NOT NULL,
                channels_json TEXT NOT NULL DEFAULT '[]',
                status        TEXT NOT NULL,
                created_at    TEXT NOT NULL,
                fired_at      TEXT,
                local_only    INTEGER NOT NULL DEFAULT 0,
                guid          TEXT
            );

            -- Behavior journal (F4): status reports, check-in asks, fired
            -- reminders. Part of the F12 ledger, so rows are deletable.
            CREATE TABLE IF NOT EXISTS behavior_log (
                id      INTEGER PRIMARY KEY,
                ts      TEXT NOT NULL,
                kind    TEXT NOT NULL,
                content TEXT NOT NULL,
                source  TEXT,
                guid    TEXT
            );

            -- Suggestion Engine v1 (F10). dedup_key keeps regeneration from
            -- inserting the same suggestion twice.
            CREATE TABLE IF NOT EXISTS suggestions (
                id         INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                kind       TEXT NOT NULL,
                text       TEXT NOT NULL,
                dedup_key  TEXT NOT NULL UNIQUE,
                source     TEXT,
                status     TEXT NOT NULL,
                guid       TEXT
            );

            -- Persona versions (F9 v1 / F15). Append-only history; the active
            -- version is a meta pointer, so rollback keeps every version.
            CREATE TABLE IF NOT EXISTS persona_versions (
                version      INTEGER PRIMARY KEY,
                created_at   TEXT NOT NULL,
                profile_json TEXT NOT NULL,
                guid         TEXT
            );

            -- Wearable health samples (F5, Phase 4). Read-only ingestion from
            -- a platform health store (currently Health Connect); dedup_key
            -- keeps re-polling an overlapping time window from duplicating
            -- rows. Part of the F12 ledger, so rows are deletable.
            CREATE TABLE IF NOT EXISTS health_samples (
                id         INTEGER PRIMARY KEY,
                kind       TEXT NOT NULL,
                start      TEXT NOT NULL,
                end        TEXT NOT NULL,
                value      REAL NOT NULL,
                source     TEXT NOT NULL,
                created_at TEXT NOT NULL,
                dedup_key  TEXT NOT NULL UNIQUE,
                guid       TEXT
            );

            -- Semantic memory facts (§3.10, schema v4). One-sentence facts the
            -- user asked Solum to remember. UNIQUE(content) tolerates the same
            -- fact being written independently on two devices (sync skips the
            -- duplicate, same pattern as suggestions.dedup_key). Part of the
            -- F12 ledger, so rows are deletable — and recall reads this table
            -- directly, so deletion removes the fact from retrieval instantly.
            CREATE TABLE IF NOT EXISTS memory_facts (
                id           INTEGER PRIMARY KEY,
                content      TEXT NOT NULL UNIQUE,
                source       TEXT NOT NULL,
                created_at   TEXT NOT NULL,
                last_used_at TEXT,
                guid         TEXT
            );

            -- Standing routines (F3 完全体, schema v4). Daily occurrences are
            -- materialized as normal events + notifications (see
            -- Orchestrator::materialize_routines), so firing/sync/alarm-mirror
            -- reuse the existing pipeline. Part of the F12 ledger.
            CREATE TABLE IF NOT EXISTS routines (
                id              INTEGER PRIMARY KEY,
                title           TEXT NOT NULL,
                time_of_day     TEXT NOT NULL,
                source          TEXT,
                active          INTEGER NOT NULL DEFAULT 1,
                created_at      TEXT NOT NULL,
                scheduled_until TEXT,
                guid            TEXT
            );

            -- Soulous Phase 8.1: imported facts are a separate read-only
            -- source, not a Solum memory layer. `external_id` is Soulous's own
            -- primary key (or the documented check-in snapshot date); the
            -- stable guid lets Solum devices LWW-merge the same remote record.
            CREATE TABLE IF NOT EXISTS soulous_facts (
                id           INTEGER PRIMARY KEY,
                external_id  TEXT NOT NULL,
                kind         TEXT NOT NULL,
                title        TEXT NOT NULL,
                occurs_at    TEXT,
                ends_at      TEXT,
                payload_json TEXT NOT NULL,
                source       TEXT NOT NULL CHECK(source = 'soulous'),
                imported_at  TEXT NOT NULL,
                guid         TEXT,
                UNIQUE(kind, external_id)
            );

            -- F20: capture outcomes are a local, inspectable processing
            -- journal. The original notification text lives in raw_inputs,
            -- which keeps the existing Phase 9 local_only/sync behavior as
            -- the sole authority for third-party text.
            CREATE TABLE IF NOT EXISTS notification_captures (
                id           INTEGER PRIMARY KEY,
                raw_input_id INTEGER NOT NULL UNIQUE,
                package_name TEXT NOT NULL,
                title        TEXT NOT NULL,
                body         TEXT NOT NULL,
                received_at  TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                local_only   INTEGER NOT NULL DEFAULT 0,
                lane         TEXT NOT NULL,
                state        TEXT NOT NULL,
                reason       TEXT,
                event_id     INTEGER,
                created_at   TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_capture_dedup
                ON notification_captures(package_name, content_hash, received_at);
            CREATE INDEX IF NOT EXISTS idx_capture_queue
                ON notification_captures(state, lane, received_at);

            -- An LLM may propose a local noise rule, but it never writes the
            -- active config directly. This record makes the required human
            -- confirmation visible and auditable in the F12 notification UI.
            CREATE TABLE IF NOT EXISTS notification_filter_proposals (
                id           INTEGER PRIMARY KEY,
                package_name TEXT,
                pattern      TEXT NOT NULL,
                matcher      TEXT NOT NULL,
                reason       TEXT NOT NULL,
                state        TEXT NOT NULL,
                created_at   TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_capture_filter_proposals
                ON notification_filter_proposals(state, created_at);

            -- Existing-record actions are never supplied by the LLM. It only
            -- supplies an intent; Rust resolves one local event and persists
            -- this human-confirmation card. This table stays device-local.
            CREATE TABLE IF NOT EXISTS notification_action_proposals (
                id           INTEGER PRIMARY KEY,
                capture_id   INTEGER NOT NULL,
                kind         TEXT NOT NULL,
                event_id     INTEGER NOT NULL,
                event_title  TEXT NOT NULL,
                -- Snapshot of the target as the user was shown it. A row id is
                -- not an identity (ids get reused), and an event can change
                -- between preview and confirmation, so both are re-checked
                -- before the action runs. Added after the fact, hence the
                -- ALTER-based backfill below.
                event_guid   TEXT NOT NULL DEFAULT '',
                event_start  TEXT NOT NULL DEFAULT '',
                new_start    TEXT,
                reason       TEXT NOT NULL,
                state        TEXT NOT NULL,
                created_at   TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_capture_action_proposals
                ON notification_action_proposals(state, created_at);

            -- F19 second step (schema v13): definitions, their fields and their
            -- records all sync. The schema is stored as one row per field
            -- rather than one JSON column precisely so that concurrent field
            -- additions merge as a set union under the existing row-level LWW
            -- instead of one device's whole schema overwriting the other's
            -- (MISC 2026-07-20 定稿). Do not fold these back into a JSON blob.
            CREATE TABLE IF NOT EXISTS widget_defs (
                id           INTEGER PRIMARY KEY,
                name         TEXT NOT NULL,
                icon         TEXT NOT NULL,
                list_sort_by TEXT,
                created_at   TEXT NOT NULL
            );
            -- form_ord / list_ord NULL means "not in that view"; non-NULL is
            -- the position within it. Membership and order live on the field
            -- itself so neither is a concurrently-edited array. The two views
            -- keep independent orders on purpose — models really do emit a
            -- different field order for form than for list.
            CREATE TABLE IF NOT EXISTS widget_fields (
                id          INTEGER PRIMARY KEY,
                widget_id   INTEGER NOT NULL,
                name        TEXT NOT NULL,
                label       TEXT NOT NULL,
                field_type  TEXT NOT NULL,
                required    INTEGER NOT NULL DEFAULT 0,
                options_json TEXT NOT NULL DEFAULT '[]',
                -- Canonical field order. Needed because created_at is
                -- identical for every field of one definition, which would
                -- leave the order to break on the random guid and so differ
                -- between devices holding the very same rows.
                ord         INTEGER NOT NULL DEFAULT 0,
                form_ord    INTEGER,
                list_ord    INTEGER,
                table_ord   INTEGER,
                stat_ord    INTEGER,
                created_at  TEXT NOT NULL,
                FOREIGN KEY(widget_id) REFERENCES widget_defs(id) ON DELETE CASCADE
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_widget_fields_name
                ON widget_fields(widget_id, name);
            CREATE TABLE IF NOT EXISTS widget_records (
                id          INTEGER PRIMARY KEY,
                widget_id   INTEGER NOT NULL,
                data_json   TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                FOREIGN KEY(widget_id) REFERENCES widget_defs(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_widget_records_widget_created
                ON widget_records(widget_id, created_at, id);

            -- Strict schema rejection is product data: it tells us which
            -- field/view capabilities are actually missing. It is device-local
            -- alongside the proposed definitions and intentionally uneditable.
            CREATE TABLE IF NOT EXISTS widget_schema_rejections (
                id          INTEGER PRIMARY KEY,
                schema_json TEXT NOT NULL,
                reason      TEXT NOT NULL,
                created_at  TEXT NOT NULL
            );

            -- Append-only. No UPDATE/DELETE is ever issued against this table.
            -- Stays device-local (§4): not synced, no guid.
            CREATE TABLE IF NOT EXISTS audit_log (
                id       INTEGER PRIMARY KEY,
                ts       TEXT NOT NULL,
                tool     TEXT NOT NULL,
                risk     TEXT NOT NULL,
                summary  TEXT NOT NULL,
                decision TEXT NOT NULL,
                token_id TEXT,
                detail   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_notif_fire ON notifications(status, fire_at);
            CREATE INDEX IF NOT EXISTS idx_events_start ON events(start);
            CREATE INDEX IF NOT EXISTS idx_behavior_ts ON behavior_log(kind, ts);
            CREATE INDEX IF NOT EXISTS idx_health_kind_start ON health_samples(kind, start);
            CREATE INDEX IF NOT EXISTS idx_soulous_kind_time ON soulous_facts(kind, occurs_at);
            "#,
        )?;

        // Keep the pre-Phase-9 value before writing this migration's version
        // below. Schema v5/v6 treated every captured notification as local
        // only; that legacy privacy scope must remain frozen, not be re-run
        // against captures made after the new setting exists.
        let prior_schema_version = self.schema_version()?;

        // v1 → v2: retrofit guid columns onto existing databases.
        for tbl in SYNCED_TABLES {
            if !self.has_column(tbl, "guid")? {
                self.conn
                    .execute(&format!("ALTER TABLE {tbl} ADD COLUMN guid TEXT"), [])?;
            }
        }

        // v4 → v5: third-party notification content is strictly local-only,
        // including its derived event and notification. Routine occurrences
        // keep a durable parent link so disabling a routine can retract its
        // still-pending materializations.
        for (tbl, col, decl) in [
            ("raw_inputs", "local_only", "INTEGER NOT NULL DEFAULT 0"),
            ("events", "local_only", "INTEGER NOT NULL DEFAULT 0"),
            ("events", "routine_id", "INTEGER"),
            ("notifications", "local_only", "INTEGER NOT NULL DEFAULT 0"),
            // v12 → v13: the list view's sort field is a per-widget scalar, so
            // plain row LWW is the right merge for it.
            ("widget_defs", "list_sort_by", "TEXT"),
            // v13 → v14: table / stat views (Phase 11 third step).
            ("widget_fields", "table_ord", "INTEGER"),
            ("widget_fields", "stat_ord", "INTEGER"),
            ("widget_defs", "table_sort_by", "TEXT"),
            // Bind a pending action card to the event it described, not just to
            // a row number. Existing rows get empty snapshots, which the
            // confirmation path treats as "cannot verify" → refuse and ask for
            // a fresh proposal. Failing closed on the handful of cards in
            // flight during an upgrade is the right trade: the alternative is
            // honouring a card we cannot check.
            (
                "notification_action_proposals",
                "event_guid",
                "TEXT NOT NULL DEFAULT ''",
            ),
            (
                "notification_action_proposals",
                "event_start",
                "TEXT NOT NULL DEFAULT ''",
            ),
        ] {
            if !self.has_column(tbl, col)? {
                self.conn
                    .execute(&format!("ALTER TABLE {tbl} ADD COLUMN {col} {decl}"), [])?;
            }
        }

        if !self.has_column("notification_captures", "local_only")? {
            self.conn.execute(
                "ALTER TABLE notification_captures ADD COLUMN local_only INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        // v12 → v13: explode the single schema_json column into one row per
        // field. Must run before the guid backfill below, so the new rows get
        // guids there and bootstrap into the oplog like any other history.
        self.migrate_widget_schema_to_rows()?;

        // A stable device identity for oplog attribution (not a synced key).
        if self.get_meta("device_id")?.is_none() {
            self.set_meta("device_id", &new_guid()[..8])?;
        }

        self.migrate_sync()?;

        if prior_schema_version < NOTIF_CLOUD_SCOPE_SCHEMA_VERSION {
            self.mark_legacy_captured_data_local_only()?;
        }

        // Backfill guids for pre-v2 rows *after* the capture triggers exist:
        // the backfill UPDATE fires them, which bootstraps the full existing
        // state into the oplog so a new device gets history, not just deltas.
        for tbl in SYNCED_TABLES {
            self.conn.execute(
                &format!("UPDATE {tbl} SET guid = lower(hex(randomblob(16))) WHERE guid IS NULL"),
                [],
            )?;
        }

        if prior_schema_version < NOTIF_SYNC_DECOUPLE_SCHEMA_VERSION {
            self.bootstrap_previously_unsynced_captures()?;
        }

        // After the guid backfill, so rows created by the v13 split already
        // have guids and re-capture as ordinary upserts.
        if prior_schema_version < WIDGET_VIEW_SLOT_PAYLOAD_SCHEMA_VERSION {
            self.rebroadcast_widget_fields_view_slots()?;
        }

        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;

        // Last: every table this version knows now exists, so ops an older
        // build parked because it did not recognise them can finally land.
        self.replay_sync_quarantine()?;
        Ok(())
    }

    fn has_column(&self, tbl: &str, col: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({tbl})"))?;
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names.iter().any(|n| n == col))
    }

    /// Preserve the old always-local privacy boundary while moving a database
    /// to Phase 9. This migration is deliberately one-shot: notifications
    /// captured after the `notif_cloud` setting exists choose their scope at
    /// ingestion time instead.
    fn mark_legacy_captured_data_local_only(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            BEGIN;
            INSERT OR IGNORE INTO meta(key, value) VALUES ('sync_applying', '1');
            UPDATE raw_inputs
               SET local_only = 1
             WHERE text LIKE '[通知·%';
            UPDATE events
               SET local_only = 1
             WHERE raw_input_id IN (SELECT id FROM raw_inputs WHERE local_only = 1);
            UPDATE notifications
               SET local_only = 1
             WHERE event_id IN (SELECT id FROM events WHERE local_only = 1);
            DELETE FROM sync_oplog
             WHERE (tbl = 'raw_inputs' AND guid IN
                    (SELECT guid FROM raw_inputs WHERE local_only = 1))
                OR (tbl = 'events' AND guid IN
                    (SELECT guid FROM events WHERE local_only = 1))
                OR (tbl = 'notifications' AND guid IN
                    (SELECT guid FROM notifications WHERE local_only = 1));
            DELETE FROM meta WHERE key = 'sync_applying';
            COMMIT;
            "#,
        )?;
        Ok(())
    }

    /// v9 → v10: notification rows used to be kept out of the oplog by a
    /// `local_only = 0` trigger condition. Sync is unconditional now, so the
    /// rows skipped back then must enter the oplog once for other devices to
    /// ever receive them. A no-op UPDATE is enough — it fires the AFTER UPDATE
    /// trigger. The `local_only` stamp itself is deliberately left alone: it
    /// still bars these rows from any cloud LLM path (§3.10, PRIVACY.md §2).
    fn bootstrap_previously_unsynced_captures(&self) -> Result<()> {
        for tbl in ["raw_inputs", "events", "notifications"] {
            self.conn.execute(
                &format!(
                    "UPDATE {tbl} SET guid = guid \
                     WHERE guid IS NOT NULL \
                       AND guid NOT IN (SELECT guid FROM sync_oplog WHERE tbl = '{tbl}')"
                ),
                [],
            )?;
        }
        Ok(())
    }

    /// Sync change capture (§3.8): every mutation of a synced row lands in
    /// `sync_oplog` via triggers — including cascade deletes, so no Rust write
    /// path can forget to log. Applying *remote* ops sets the `sync_applying`
    /// meta flag, which every trigger checks, to prevent echo loops. FK
    /// references are translated to guids at capture time (integer ids are
    /// device-local). `hlc` is a millisecond UTC wall-clock stamp used for
    /// last-write-wins merge.
    fn migrate_sync(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sync_oplog (
                id      INTEGER PRIMARY KEY,
                tbl     TEXT NOT NULL,
                guid    TEXT NOT NULL,
                op      TEXT NOT NULL,           -- 'upsert' | 'delete'
                payload TEXT,                    -- row JSON; NULL for delete
                hlc     TEXT NOT NULL,           -- UTC ms timestamp (LWW)
                origin  TEXT NOT NULL            -- device id that made the change
            );
            CREATE INDEX IF NOT EXISTS idx_oplog_guid ON sync_oplog(tbl, guid, hlc);
            CREATE INDEX IF NOT EXISTS idx_oplog_origin ON sync_oplog(origin, id);

            -- Forward compatibility (§3.8): ops this build cannot interpret —
            -- a table a newer peer added, or a payload missing a field we
            -- require — are held here instead of failing the merge. Replayed
            -- by `replay_sync_quarantine` once a migration teaches us the
            -- table. Device-local: never synced, no guid.
            CREATE TABLE IF NOT EXISTS sync_quarantine (
                id      INTEGER PRIMARY KEY,
                tbl     TEXT NOT NULL,
                guid    TEXT NOT NULL,
                op      TEXT NOT NULL,
                payload TEXT,
                hlc     TEXT NOT NULL,
                origin  TEXT NOT NULL,
                reason  TEXT NOT NULL,
                held_at TEXT NOT NULL
            );
            -- The same op can arrive again (peer resends, cursor rewind);
            -- holding it twice would replay it twice.
            CREATE UNIQUE INDEX IF NOT EXISTS idx_quarantine_op
                ON sync_quarantine(tbl, guid, hlc, origin);
            CREATE INDEX IF NOT EXISTS idx_quarantine_tbl ON sync_quarantine(tbl, hlc);

            -- One level up from `sync_quarantine`: a pulled blob we could not
            -- even open (bad base64, wrong key, truncated ciphertext, payload
            -- that is not a SyncBatch). Held here so the pull cursor can move
            -- past it — otherwise one bad blob re-fails forever and every
            -- later blob behind it never arrives. The ciphertext is kept
            -- verbatim so a build with the right key can still recover it.
            -- Idempotency receipts for capture losses (spool overflow markers,
            -- undeliverable payloads). Keyed by the on-disk name of the thing
            -- being counted.
            --
            -- Needed because "record the count" and "delete the files" cannot be
            -- one atomic step across a database and a filesystem. Recording
            -- first and crashing before the delete would re-count the same
            -- markers on the next run, which turns the figure shown to the user
            -- into an over-count — and an over-count is not a lower bound, so
            -- the UI's "at least N" claim would be false. With a receipt, a
            -- re-scan of the same marker adds nothing.
            CREATE TABLE IF NOT EXISTS capture_loss_receipts (
                receipt     TEXT PRIMARY KEY,   -- "<kind>:<on-disk name>"
                recorded_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sync_bad_blobs (
                seq      INTEGER PRIMARY KEY,   -- relay sequence number
                device   TEXT NOT NULL,
                blob_b64 TEXT NOT NULL,
                reason   TEXT NOT NULL,
                held_at  TEXT NOT NULL
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_raw_inputs_guid ON raw_inputs(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_events_guid ON events(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_notifications_guid ON notifications(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_behavior_guid ON behavior_log(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_suggestions_guid ON suggestions(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_persona_guid ON persona_versions(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_health_guid ON health_samples(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_memory_facts_guid ON memory_facts(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_routines_guid ON routines(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_soulous_facts_guid ON soulous_facts(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_widget_defs_guid ON widget_defs(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_widget_fields_guid ON widget_fields(guid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_widget_records_guid ON widget_records(guid);
            "#,
        )?;

        let payloads = SYNC_PAYLOADS;

        const GUARD: &str = "(SELECT COUNT(*) FROM meta WHERE key = 'sync_applying') = 0";
        // The LWW stamp. Called an HLC, but it used to be nothing but the wall
        // clock, which gives up the one property the name promises: a clock
        // that steps backwards (DST, NTP correction, a user fixing a wrong
        // date) makes later writes *lose* to earlier ones, so a correction
        // silently fails to stick and there is nothing in the UI to explain it.
        //
        // Taking the max of the wall clock and the last stamp this device
        // issued makes it monotonic. But `MAX` alone is not enough: it is only
        // *non-decreasing*, and two writes to the same row within one
        // millisecond then get the **identical** `(hlc, origin)` — same clock
        // reading, same device. `apply_remote_ops` requires strictly newer, so
        // a peer applies the first and silently skips the second, and the two
        // devices diverge on that row forever.
        //
        // That is not a rare race: `strftime('now')` twice in one statement
        // returns the same value every time, so any two updates inside a
        // millisecond collide. Hence `+1ms` when the wall clock has not moved
        // past the last stamp — the counter part of a hybrid logical clock,
        // expressed in the timestamp itself so the wire format is unchanged.
        // `julianday` has ~10µs resolution at present-day dates, comfortably
        // finer than the 1ms step.
        const HLC: &str = "strftime('%Y-%m-%dT%H:%M:%f', MAX(              julianday('now'),              julianday(COALESCE((SELECT value FROM meta WHERE key = 'hlc_last'),                                 '0001-01-01T00:00:00.000')) + 0.001 / 86400.0))";
        const ORIGIN: &str = "(SELECT value FROM meta WHERE key = 'device_id')";

        for (tbl, payload, syncable) in payloads {
            let syncable_old = syncable.replace("NEW.", "OLD.");
            self.conn.execute_batch(&format!(
                r#"
                DROP TRIGGER IF EXISTS trg_sync_{tbl}_i;
                DROP TRIGGER IF EXISTS trg_sync_{tbl}_u;
                DROP TRIGGER IF EXISTS trg_sync_{tbl}_d;
                CREATE TRIGGER trg_sync_{tbl}_i AFTER INSERT ON {tbl}
                WHEN NEW.guid IS NOT NULL AND {syncable} AND {GUARD}
                BEGIN
                    INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                    VALUES ('{tbl}', NEW.guid, 'upsert', {payload}, {HLC}, {ORIGIN});
                END;
                CREATE TRIGGER trg_sync_{tbl}_u AFTER UPDATE ON {tbl}
                WHEN NEW.guid IS NOT NULL AND {syncable} AND {GUARD}
                BEGIN
                    INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                    VALUES ('{tbl}', NEW.guid, 'upsert', {payload}, {HLC}, {ORIGIN});
                END;
                CREATE TRIGGER trg_sync_{tbl}_d AFTER DELETE ON {tbl}
                WHEN OLD.guid IS NOT NULL AND ({syncable_old}) AND {GUARD}
                BEGIN
                    INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                    VALUES ('{tbl}', OLD.guid, 'delete', NULL, {HLC}, {ORIGIN});
                END;
                "#
            ))?;
        }

        // Advance the device's logical clock to whatever stamp was just
        // issued. Device-local: `hlc_last` is not in SYNCED_META_KEYS, so this
        // write does not itself produce an op.
        self.conn.execute_batch(
            r#"
            DROP TRIGGER IF EXISTS trg_hlc_advance;
            CREATE TRIGGER trg_hlc_advance AFTER INSERT ON sync_oplog
            BEGIN
                INSERT INTO meta(key, value) VALUES ('hlc_last', NEW.hlc)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value
                  WHERE excluded.value > meta.value;
            END;
            "#,
        )?;

        // meta: only the whitelisted config keys sync (LWW documents). The
        // persona_active pointer is translated version → guid at capture.
        let meta_payload = "json_object('key', NEW.key, 'value', \
             CASE WHEN NEW.key = 'persona_active' \
                  THEN COALESCE((SELECT guid FROM persona_versions WHERE version = CAST(NEW.value AS INTEGER)), NEW.value) \
                  ELSE NEW.value END)";
        self.conn.execute_batch(&format!(
            r#"
            CREATE TRIGGER IF NOT EXISTS trg_sync_meta_i AFTER INSERT ON meta
            WHEN NEW.key IN ({SYNCED_META_KEYS}) AND {GUARD}
            BEGIN
                INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                VALUES ('meta', 'meta:' || NEW.key, 'upsert', {meta_payload}, {HLC}, {ORIGIN});
            END;
            CREATE TRIGGER IF NOT EXISTS trg_sync_meta_u AFTER UPDATE ON meta
            WHEN NEW.key IN ({SYNCED_META_KEYS}) AND {GUARD}
            BEGIN
                INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                VALUES ('meta', 'meta:' || NEW.key, 'upsert', {meta_payload}, {HLC}, {ORIGIN});
            END;
            CREATE TRIGGER IF NOT EXISTS trg_sync_meta_d AFTER DELETE ON meta
            WHEN OLD.key IN ({SYNCED_META_KEYS}) AND {GUARD}
            BEGIN
                INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                VALUES ('meta', 'meta:' || OLD.key, 'delete', NULL, {HLC}, {ORIGIN});
            END;
            "#
        ))?;
        Ok(())
    }

    /// v12 → v13. The first vertical slice stored a whole `WidgetSchema` as
    /// one JSON column; that shape cannot merge concurrent field additions
    /// (MISC 2026-07-20). Rewrite it as rows, then drop the column so nothing
    /// can keep writing the unmergeable representation.
    ///
    /// The whole rewrite is one transaction on purpose. Without it, dying
    /// between the field inserts and the `DROP` (a ROM kill, a flat battery)
    /// commits half the rows while leaving `schema_json` in place — so the
    /// next open re-runs the migration, re-inserts the same fields and hits
    /// `UNIQUE(widget_id, name)`, which makes the database permanently
    /// unopenable rather than merely half-migrated.
    fn migrate_widget_schema_to_rows(&self) -> Result<()> {
        if !self.has_column("widget_defs", "schema_json")? {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        let rows: Vec<(i64, String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, schema_json, created_at FROM widget_defs")?;
            let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (widget_id, schema_json, created_at) in rows {
            // A schema this build cannot parse would silently lose every field
            // it describes. Leave the row's fields empty and keep going rather
            // than aborting the whole migration; the definition stays visible
            // and the rejection log records what happened.
            let Ok(schema) = serde_json::from_str::<crate::widget::WidgetSchema>(&schema_json)
            else {
                self.append_widget_schema_rejection(
                    &schema_json,
                    "v13 迁移无法解析该 schema，其字段未能转成行",
                    parse_ts(&created_at).unwrap_or_default(),
                )?;
                continue;
            };
            let position = |view_type: crate::widget::WidgetViewType, name: &str| {
                schema
                    .views
                    .iter()
                    .find(|view| view.view_type == view_type)
                    .and_then(|view| view.fields.iter().position(|f| f == name))
                    .map(|index| index as i64)
            };
            for (ord, field) in schema.fields.iter().enumerate() {
                self.conn.execute(
                    "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                       options_json, ord, form_ord, list_ord, table_ord, stat_ord, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        widget_id,
                        field.name,
                        field.label,
                        field.field_type.as_str(),
                        field.required as i64,
                        serde_json::to_string(&field.options)?,
                        ord as i64,
                        position(crate::widget::WidgetViewType::Form, &field.name),
                        position(crate::widget::WidgetViewType::List, &field.name),
                        position(crate::widget::WidgetViewType::Table, &field.name),
                        position(crate::widget::WidgetViewType::Stat, &field.name),
                        created_at,
                    ],
                )?;
            }
            let sort_by = schema
                .views
                .iter()
                .find(|view| view.view_type == crate::widget::WidgetViewType::List)
                .and_then(|view| view.sort_by.clone());
            self.conn.execute(
                "UPDATE widget_defs SET list_sort_by = ?1 WHERE id = ?2",
                params![sort_by, widget_id],
            )?;
        }
        self.conn
            .execute("ALTER TABLE widget_defs DROP COLUMN schema_json", [])?;
        tx.commit()?;
        Ok(())
    }

    /// v14 → v15. `widget_fields` rows that synced or were exported before the
    /// payload carried `ord` / `table_ord` / `stat_ord` reached their peers on
    /// the column defaults, losing the table and stat views and the canonical
    /// field order. Touch the rows that still hold information the peers
    /// cannot have, so the fixed trigger re-captures them and LWW repairs the
    /// other devices on the next sync.
    ///
    /// Only rows *away* from the defaults are touched, which is precisely the
    /// set this device is authoritative for: a device that merely received
    /// these rows has `ord = 0` and NULL memberships, so it stays quiet
    /// instead of pushing its own damaged copy back over the good one.
    fn rebroadcast_widget_fields_view_slots(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE widget_fields SET ord = ord \
             WHERE guid IS NOT NULL \
               AND (ord <> 0 OR table_ord IS NOT NULL OR stat_ord IS NOT NULL)",
            [],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    // ---- raw inputs -------------------------------------------------------

    pub fn insert_raw_input(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
    ) -> Result<i64> {
        self.insert_raw_input_with_scope(text, intent, created_at, false)
    }

    /// Store a raw input that must never enter the encrypted sync pipeline.
    /// Used only for text captured from third-party notifications.
    pub fn insert_local_only_raw_input(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
    ) -> Result<i64> {
        self.insert_raw_input_with_scope(text, intent, created_at, true)
    }

    fn insert_raw_input_with_scope(
        &self,
        text: &str,
        intent: &str,
        created_at: NaiveDateTime,
        local_only: bool,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO raw_inputs(text, intent, created_at, local_only, guid)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                text,
                intent,
                fmt_ts(&created_at),
                local_only as i64,
                new_guid()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// All raw inputs, oldest first (for the data export).
    pub fn list_raw_inputs(&self) -> Result<Vec<RawInputRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, text, intent, created_at FROM raw_inputs ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(RawInputRow {
                id: row.get(0)?,
                text: row.get(1)?,
                intent: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- events -----------------------------------------------------------

    pub fn insert_event(&self, ev: &Event, raw_input_id: Option<i64>) -> Result<i64> {
        self.insert_event_with_scope(ev, raw_input_id, false, None)
    }

    /// Insert an event with its sync scope and optional routine provenance.
    /// Local-only events are derived from third-party notification text and are
    /// deliberately excluded from sync triggers.
    pub fn insert_event_with_scope(
        &self,
        ev: &Event,
        raw_input_id: Option<i64>,
        local_only: bool,
        routine_id: Option<i64>,
    ) -> Result<i64> {
        let people = serde_json::to_string(&ev.people)?;
        self.conn.execute(
            "INSERT INTO events(title, kind, start, end, location, people_json, raw_input_id, routine_id, created_at, local_only, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                ev.title,
                ev.kind.as_str(),
                fmt_ts(&ev.start),
                ev.end.map(|d| fmt_ts(&d)),
                ev.location,
                people,
                raw_input_id,
                routine_id,
                fmt_ts(&ev.created_at),
                local_only as i64,
                new_guid(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn row_to_event(row: &rusqlite::Row) -> rusqlite::Result<Event> {
        let kind_s: String = row.get("kind")?;
        let start_s: String = row.get("start")?;
        let end_s: Option<String> = row.get("end")?;
        let people_s: String = row.get("people_json")?;
        let created_s: String = row.get("created_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(Event {
            id: Some(row.get("id")?),
            title: row.get("title")?,
            kind: kind_s.parse().map_err(map)?,
            start: parse_ts(&start_s).map_err(map)?,
            end: match end_s {
                Some(s) => Some(parse_ts(&s).map_err(map)?),
                None => None,
            },
            location: row.get("location")?,
            people: serde_json::from_str(&people_s).unwrap_or_default(),
            raw_input: String::new(),
            created_at: parse_ts(&created_s).map_err(map)?,
        })
    }

    pub fn get_event(&self, id: i64) -> Result<Event> {
        self.conn
            .query_row("SELECT * FROM events WHERE id = ?1", params![id], |r| {
                Self::row_to_event(r)
            })
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("event#{id}")))
    }

    /// Stable identity and privacy scope for an outbound event projection.
    /// `Event` intentionally stays display-domain-only; callers must obtain
    /// these two storage concerns explicitly before any data can leave Solum.
    pub fn event_guid_and_local_only(&self, id: i64) -> Result<(String, bool)> {
        self.conn
            .query_row(
                "SELECT guid, local_only FROM events WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("event#{id}")))
    }

    pub fn list_events(&self) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM events ORDER BY start ASC")?;
        let rows = stmt.query_map([], Self::row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Events starting at or after `now`, soonest first.
    pub fn upcoming_events(&self, now: NaiveDateTime) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM events WHERE start >= ?1 ORDER BY start ASC")?;
        let rows = stmt.query_map(params![fmt_ts(&now)], Self::row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Move an event's start (and end, pre-shifted by the caller). Sync
    /// captures the UPDATE via the existing triggers.
    pub fn update_event_times(
        &self,
        id: i64,
        start: NaiveDateTime,
        end: Option<NaiveDateTime>,
    ) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE events SET start = ?1, end = ?2 WHERE id = ?3",
            params![fmt_ts(&start), end.map(|d| fmt_ts(&d)), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("event#{id}")));
        }
        Ok(())
    }

    /// Drop the still-pending reminders of an event (reschedule re-plans them
    /// from the rule table; fired/dismissed rows stay as history).
    pub fn delete_pending_notifications_for_event(&self, event_id: i64) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM notifications WHERE event_id = ?1 AND status = 'pending'",
            params![event_id],
        )?;
        Ok(n)
    }

    /// Delete an event and its notifications.
    /// Delete an event and its reminders together. Atomic: failing between the
    /// two steps would leave the event with its reminders already gone —
    /// visibly scheduled, silently never firing.
    pub fn delete_event(&self, id: i64) -> Result<()> {
        self.with_transaction(|s| {
            s.conn
                .execute("DELETE FROM notifications WHERE event_id = ?1", params![id])?;
            let n = s
                .conn
                .execute("DELETE FROM events WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(CoreError::NotFound(format!("event#{id}")));
            }
            Ok(())
        })
    }

    // ---- notifications ----------------------------------------------------

    pub fn insert_notification(&self, n: &Notification) -> Result<i64> {
        let channels = serde_json::to_string(&n.channels)?;
        self.conn.execute(
            "INSERT INTO notifications(event_id, fire_at, lead_label, channels_json, status, created_at, fired_at, local_only, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                     COALESCE((SELECT local_only FROM events WHERE id = ?1), 0), ?8)",
            params![
                n.event_id,
                fmt_ts(&n.fire_at),
                n.lead_label,
                channels,
                n.status.as_str(),
                fmt_ts(&n.created_at),
                n.fired_at.map(|d| fmt_ts(&d)),
                new_guid(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn row_to_notification(row: &rusqlite::Row) -> rusqlite::Result<Notification> {
        let fire_s: String = row.get("fire_at")?;
        let channels_s: String = row.get("channels_json")?;
        let status_s: String = row.get("status")?;
        let created_s: String = row.get("created_at")?;
        let fired_s: Option<String> = row.get("fired_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        let channels: Vec<Channel> = serde_json::from_str(&channels_s).unwrap_or_default();
        Ok(Notification {
            id: Some(row.get("id")?),
            event_id: row.get("event_id")?,
            fire_at: parse_ts(&fire_s).map_err(map)?,
            lead_label: row.get("lead_label")?,
            channels,
            status: status_s.parse().map_err(map)?,
            created_at: parse_ts(&created_s).map_err(map)?,
            fired_at: match fired_s {
                Some(s) => Some(parse_ts(&s).map_err(map)?),
                None => None,
            },
        })
    }

    pub fn list_notifications(&self) -> Result<Vec<Notification>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notifications ORDER BY fire_at ASC")?;
        let rows = stmt.query_map([], Self::row_to_notification)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Pending notifications whose fire time has arrived, earliest first.
    pub fn due_notifications(&self, now: NaiveDateTime) -> Result<Vec<Notification>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notifications WHERE status = 'pending' AND fire_at <= ?1 ORDER BY fire_at ASC",
        )?;
        let rows = stmt.query_map(params![fmt_ts(&now)], Self::row_to_notification)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Cancel a still-pending notification so it never fires (without deleting
    /// the event it belongs to).
    pub fn dismiss_notification(&self, id: i64) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET status = ?1 WHERE id = ?2 AND status = 'pending'",
            params![NotificationStatus::Dismissed.as_str(), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!(
                "pending notification#{id} (already fired/dismissed, or missing)"
            )));
        }
        Ok(())
    }

    /// Push a reminder's fire time to `until` and re-arm it (snooze). Works on
    /// a pending reminder (postpone) and on a fired one (ring again later);
    /// a dismissed reminder stays cancelled — un-cancelling is a separate,
    /// deliberate act, not something a snooze should do by accident.
    pub fn snooze_notification(&self, id: i64, until: NaiveDateTime) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET fire_at = ?1, status = 'pending', fired_at = NULL
             WHERE id = ?2 AND status IN ('pending', 'fired')",
            params![fmt_ts(&until), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!(
                "notification#{id}（不存在或已被取消）"
            )));
        }
        Ok(())
    }

    pub fn mark_fired(&self, id: i64, now: NaiveDateTime) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET status = ?1, fired_at = ?2 WHERE id = ?3",
            params![NotificationStatus::Fired.as_str(), fmt_ts(&now), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("notification#{id}")));
        }
        Ok(())
    }

    /// Count raw inputs recorded within `[from, to]` (for the review digest).
    pub fn count_raw_inputs_between(
        &self,
        from: NaiveDateTime,
        to: NaiveDateTime,
    ) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM raw_inputs WHERE created_at >= ?1 AND created_at <= ?2",
            params![fmt_ts(&from), fmt_ts(&to)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    // ---- behavior journal (F4) ---------------------------------------------

    pub fn insert_behavior(&self, e: &BehaviorEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO behavior_log(ts, kind, content, source, guid) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                fmt_ts(&e.ts),
                e.kind.as_str(),
                e.content,
                e.source,
                new_guid()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn row_to_behavior(row: &rusqlite::Row) -> rusqlite::Result<BehaviorEntry> {
        let ts_s: String = row.get("ts")?;
        let kind_s: String = row.get("kind")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(BehaviorEntry {
            id: Some(row.get("id")?),
            ts: parse_ts(&ts_s).map_err(map)?,
            kind: kind_s.parse().map_err(map)?,
            content: row.get("content")?,
            source: row.get("source")?,
        })
    }

    /// The full journal, newest first.
    pub fn list_behavior(&self) -> Result<Vec<BehaviorEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM behavior_log ORDER BY ts DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_behavior)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Journal entries of one kind within `[from, to]`, oldest first (the shape
    /// habit detection wants).
    pub fn list_behavior_between(
        &self,
        kind: BehaviorKind,
        from: NaiveDateTime,
        to: NaiveDateTime,
    ) -> Result<Vec<BehaviorEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM behavior_log WHERE kind = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![kind.as_str(), fmt_ts(&from), fmt_ts(&to)],
            Self::row_to_behavior,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The most recent timestamp of a given kind (e.g. the last check-in ask).
    pub fn last_behavior_ts(&self, kind: BehaviorKind) -> Result<Option<NaiveDateTime>> {
        let s: Option<String> = self
            .conn
            .query_row(
                "SELECT ts FROM behavior_log WHERE kind = ?1 ORDER BY ts DESC, id DESC LIMIT 1",
                params![kind.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        s.map(|s| parse_ts(&s)).transpose()
    }

    pub fn delete_behavior(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM behavior_log WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("behavior#{id}")));
        }
        Ok(())
    }

    // ---- suggestions (F10) --------------------------------------------------

    /// Insert a suggestion unless one with the same `dedup_key` already exists.
    /// Returns the new id, or `None` if it was a duplicate.
    pub fn insert_suggestion_if_new(&self, s: &Suggestion) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO suggestions(created_at, kind, text, dedup_key, source, status, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                fmt_ts(&s.created_at),
                s.kind.as_str(),
                s.text,
                s.dedup_key,
                s.source,
                s.status.as_str(),
                new_guid(),
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    fn row_to_suggestion(row: &rusqlite::Row) -> rusqlite::Result<Suggestion> {
        let created_s: String = row.get("created_at")?;
        let kind_s: String = row.get("kind")?;
        let status_s: String = row.get("status")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(Suggestion {
            id: Some(row.get("id")?),
            created_at: parse_ts(&created_s).map_err(map)?,
            kind: kind_s.parse().map_err(map)?,
            text: row.get("text")?,
            dedup_key: row.get("dedup_key")?,
            source: row.get("source")?,
            status: status_s.parse().map_err(map)?,
        })
    }

    pub fn get_suggestion(&self, id: i64) -> Result<Suggestion> {
        self.conn
            .query_row(
                "SELECT * FROM suggestions WHERE id = ?1",
                params![id],
                Self::row_to_suggestion,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("suggestion#{id}")))
    }

    /// All suggestions, newest first.
    pub fn list_suggestions(&self) -> Result<Vec<Suggestion>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM suggestions ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_suggestion)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Decide a suggestion — `pending` → accepted/dismissed, once.
    ///
    /// The `WHERE status = 'pending'` is the whole point and must stay: without
    /// it a stale card (an old window, a second UI, a replayed request) could
    /// flip an already-`dismissed` suggestion back to `accepted`, and accepting
    /// has side effects — it creates a routine, or pauses one. The user's "no"
    /// has to be final.
    ///
    /// Returns `Ok(false)` when the suggestion exists but was already decided,
    /// so callers can say so instead of silently re-running the side effect.
    pub fn decide_suggestion(&self, id: i64, status: SuggestionStatus) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE suggestions SET status = ?1 WHERE id = ?2 AND status = 'pending'",
            params![status.as_str(), id],
        )?;
        if n > 0 {
            return Ok(true);
        }
        // Distinguish "no such suggestion" from "already decided".
        let exists: i64 = self.conn.query_row(
            "SELECT count(*) FROM suggestions WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(CoreError::NotFound(format!("suggestion#{id}")));
        }
        Ok(false)
    }

    pub fn delete_suggestion(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM suggestions WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("suggestion#{id}")));
        }
        Ok(())
    }

    // ---- wearable health samples (F5, Phase 4) -------------------------------

    /// Insert a sample, or update the one already stored for the same
    /// `dedup_key` (a re-poll of an overlapping platform time window). Returns
    /// the new id, or `None` when an existing row was refreshed instead.
    ///
    /// `dedup_key` deliberately excludes the value: "the same kind of reading,
    /// over the same interval, from the same source" is *one* measurement, and
    /// re-polling must not stack up copies of it. But that made `INSERT OR
    /// IGNORE` wrong — a platform that later corrects a reading re-sends the
    /// same key with a new value, and ignoring it meant Solum kept the first
    /// number forever, contradicting `dedup_key`'s own doc comment. Upsert the
    /// value so a correction lands.
    pub fn insert_health_sample_if_new(&self, s: &HealthSample) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT INTO health_samples(kind, start, end, value, source, created_at, dedup_key, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(dedup_key) DO UPDATE SET value = excluded.value
               WHERE health_samples.value <> excluded.value",
            params![
                s.kind.as_str(),
                fmt_ts(&s.start),
                fmt_ts(&s.end),
                s.value,
                s.source,
                fmt_ts(&s.created_at),
                s.dedup_key(),
                new_guid(),
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    fn row_to_health_sample(row: &rusqlite::Row) -> rusqlite::Result<HealthSample> {
        let kind_s: String = row.get("kind")?;
        let start_s: String = row.get("start")?;
        let end_s: String = row.get("end")?;
        let created_s: String = row.get("created_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(HealthSample {
            id: Some(row.get("id")?),
            kind: kind_s.parse().map_err(map)?,
            start: parse_ts(&start_s).map_err(map)?,
            end: parse_ts(&end_s).map_err(map)?,
            value: row.get("value")?,
            source: row.get("source")?,
            created_at: parse_ts(&created_s).map_err(map)?,
        })
    }

    /// All samples, newest first (the F12 ledger's shape).
    pub fn list_health_samples(&self) -> Result<Vec<HealthSample>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM health_samples ORDER BY start DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_health_sample)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Samples of one kind within `[from, to]`, oldest first.
    pub fn list_health_samples_between(
        &self,
        kind: HealthMetric,
        from: NaiveDateTime,
        to: NaiveDateTime,
    ) -> Result<Vec<HealthSample>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM health_samples WHERE kind = ?1 AND start >= ?2 AND start <= ?3 ORDER BY start ASC, id ASC",
        )?;
        let rows = stmt.query_map(
            params![kind.as_str(), fmt_ts(&from), fmt_ts(&to)],
            Self::row_to_health_sample,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_health_sample(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM health_samples WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("health_sample#{id}")));
        }
        Ok(())
    }

    // ---- Soulous read-only facts (Phase 8.1) --------------------------------

    fn row_to_soulous_fact(row: &rusqlite::Row) -> rusqlite::Result<SoulousFact> {
        let kind_s: String = row.get("kind")?;
        let occurs_s: Option<String> = row.get("occurs_at")?;
        let ends_s: Option<String> = row.get("ends_at")?;
        let imported_s: String = row.get("imported_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(SoulousFact {
            id: Some(row.get("id")?),
            external_id: row.get("external_id")?,
            kind: kind_s.parse().map_err(map)?,
            title: row.get("title")?,
            occurs_at: occurs_s.map(|s| parse_ts(&s)).transpose().map_err(map)?,
            ends_at: ends_s.map(|s| parse_ts(&s)).transpose().map_err(map)?,
            payload_json: row.get("payload_json")?,
            source: row.get("source")?,
            imported_at: parse_ts(&imported_s).map_err(map)?,
        })
    }

    /// The complete cached snapshot, newest data first. These rows are never
    /// added to `memory_ledger` or recall: they are a visible data source,
    /// not something Solum inferred or remembered.
    pub fn list_soulous_facts(&self) -> Result<Vec<SoulousFact>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM soulous_facts ORDER BY occurs_at DESC, imported_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_soulous_fact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Atomically replace the source snapshot only after every remote endpoint
    /// has parsed successfully. Trigger-based sync records row upserts and
    /// removals; a network/parser failure occurs before this transaction.
    pub fn replace_soulous_snapshot(&self, facts: &[SoulousFact]) -> Result<()> {
        if facts.iter().any(|f| f.source != SOULOUS_SOURCE) {
            return Err(CoreError::Invalid(
                "Soulous snapshot contains a non-soulous source marker".into(),
            ));
        }
        let wanted: std::collections::HashSet<String> =
            facts.iter().map(SoulousFact::guid).collect();
        let tx = self.conn.unchecked_transaction()?;
        for fact in facts {
            tx.execute(
                "INSERT INTO soulous_facts(external_id, kind, title, occurs_at, ends_at, payload_json, source, imported_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(guid) DO UPDATE SET external_id=excluded.external_id,
                    kind=excluded.kind, title=excluded.title, occurs_at=excluded.occurs_at,
                    ends_at=excluded.ends_at, payload_json=excluded.payload_json,
                    source=excluded.source, imported_at=excluded.imported_at",
                params![
                    fact.external_id,
                    fact.kind.as_str(),
                    fact.title,
                    fact.occurs_at.map(|d| fmt_ts(&d)),
                    fact.ends_at.map(|d| fmt_ts(&d)),
                    fact.payload_json,
                    fact.source,
                    fmt_ts(&fact.imported_at),
                    fact.guid(),
                ],
            )?;
        }
        let mut stmt = tx.prepare("SELECT guid FROM soulous_facts")?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for guid in existing {
            if !wanted.contains(&guid) {
                tx.execute("DELETE FROM soulous_facts WHERE guid = ?1", params![guid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---- semantic memory facts (§3.10) --------------------------------------

    /// Insert a fact unless the identical content is already remembered.
    /// Returns the new id, or `None` for a duplicate.
    pub fn insert_fact_if_new(&self, f: &crate::memory::MemoryFact) -> Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO memory_facts(content, source, created_at, last_used_at, guid)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                f.content,
                f.source,
                fmt_ts(&f.created_at),
                f.last_used_at.map(|d| fmt_ts(&d)),
                new_guid(),
            ],
        )?;
        Ok((n > 0).then(|| self.conn.last_insert_rowid()))
    }

    fn row_to_fact(row: &rusqlite::Row) -> rusqlite::Result<crate::memory::MemoryFact> {
        let created_s: String = row.get("created_at")?;
        let used_s: Option<String> = row.get("last_used_at")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(crate::memory::MemoryFact {
            id: Some(row.get("id")?),
            content: row.get("content")?,
            source: row.get("source")?,
            created_at: parse_ts(&created_s).map_err(map)?,
            last_used_at: match used_s {
                Some(s) => Some(parse_ts(&s).map_err(map)?),
                None => None,
            },
        })
    }

    /// All facts, newest first.
    pub fn list_facts(&self) -> Result<Vec<crate::memory::MemoryFact>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM memory_facts ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_fact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_fact(&self, id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM memory_facts WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("fact#{id}")));
        }
        Ok(())
    }

    /// Edit a fact's wording in place (F12: 记忆可编辑). Recall reads this
    /// table directly, so the new wording takes effect immediately.
    /// UNIQUE(content) still holds — editing into another fact's exact
    /// wording is rejected rather than silently merged.
    pub fn update_fact(&self, id: i64, content: &str) -> Result<()> {
        let content = content.trim();
        if content.is_empty() {
            return Err(CoreError::Invalid("记忆内容不能为空".into()));
        }
        let n = self
            .conn
            .execute(
                "UPDATE memory_facts SET content = ?1 WHERE id = ?2",
                params![content, id],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    CoreError::Invalid("已有一条相同内容的记忆".into())
                }
                other => other.into(),
            })?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("fact#{id}")));
        }
        Ok(())
    }

    // ---- routines (F3 完全体, D4) --------------------------------------------

    /// Insert a routine unless an *active* one with the same title already
    /// exists (double-accepting a habit suggestion must not double-remind).
    pub fn insert_routine_if_new(&self, r: &crate::routine::Routine) -> Result<Option<i64>> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM routines WHERE title = ?1 AND active = 1",
                params![r.title],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Ok(None);
        }
        self.conn.execute(
            "INSERT INTO routines(title, time_of_day, source, active, created_at, scheduled_until, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                r.title,
                r.time_of_day,
                r.source,
                r.active as i64,
                fmt_ts(&r.created_at),
                r.scheduled_until.map(|d| d.format("%Y-%m-%d").to_string()),
                new_guid(),
            ],
        )?;
        Ok(Some(self.conn.last_insert_rowid()))
    }

    fn row_to_routine(row: &rusqlite::Row) -> rusqlite::Result<crate::routine::Routine> {
        let created_s: String = row.get("created_at")?;
        let until_s: Option<String> = row.get("scheduled_until")?;
        let active: i64 = row.get("active")?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(crate::routine::Routine {
            id: Some(row.get("id")?),
            title: row.get("title")?,
            time_of_day: row.get("time_of_day")?,
            source: row.get("source")?,
            active: active != 0,
            created_at: parse_ts(&created_s).map_err(map)?,
            scheduled_until: until_s
                .and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()),
        })
    }

    /// All routines, newest first.
    pub fn list_routines(&self) -> Result<Vec<crate::routine::Routine>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM routines ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_routine)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_routine_active(&self, id: i64, active: bool) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE routines
             SET active = ?1,
                 scheduled_until = CASE
                     WHEN active = 0 AND ?1 = 1 THEN NULL
                     ELSE scheduled_until
                 END
             WHERE id = ?2",
            params![active as i64, id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("routine#{id}")));
        }
        if !active {
            self.remove_pending_routine_occurrences(id)?;
        }
        Ok(())
    }

    /// Change an existing routine's mutable configuration. Past occurrences
    /// are historical records, while pending ones are projections of this
    /// configuration and must be rebuilt from the new title/time.
    pub fn update_routine(&self, id: i64, title: &str, time_of_day: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            return Err(CoreError::Invalid("固定提醒内容不能为空".into()));
        }
        let time = NaiveTime::parse_from_str(time_of_day, "%H:%M").map_err(|e| {
            CoreError::Invalid(format!("固定提醒时间 {:?} 不合法: {e}", time_of_day))
        })?;
        let time_of_day = time.format("%H:%M").to_string();

        let active: Option<i64> = self
            .conn
            .query_row(
                "SELECT active FROM routines WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let active = active.ok_or_else(|| CoreError::NotFound(format!("routine#{id}")))?;
        if active != 0 {
            let duplicate: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM routines WHERE title = ?1 AND active = 1 AND id != ?2 LIMIT 1",
                    params![title, id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = duplicate {
                return Err(CoreError::Invalid(format!(
                    "已存在启用的同名固定提醒「{title}」（routine#{existing}）"
                )));
            }
        }

        self.conn.execute(
            "UPDATE routines SET title = ?1, time_of_day = ?2, scheduled_until = NULL WHERE id = ?3",
            params![title, time_of_day, id],
        )?;
        self.remove_pending_routine_occurrences(id)?;
        Ok(())
    }

    /// Advance the materialization high-water mark.
    pub fn set_routine_scheduled_until(&self, id: i64, until: chrono::NaiveDate) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE routines SET scheduled_until = ?1 WHERE id = ?2",
            params![until.format("%Y-%m-%d").to_string(), id],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("routine#{id}")));
        }
        Ok(())
    }

    pub fn delete_routine(&self, id: i64) -> Result<()> {
        self.with_transaction(|s| {
            s.remove_pending_routine_occurrences(id)?;
            let n = s
                .conn
                .execute("DELETE FROM routines WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(CoreError::NotFound(format!("routine#{id}")));
            }
            Ok(())
        })
    }

    /// Retract materialized occurrences that have not fully fired yet. The
    /// event carries `routine_id` explicitly, rather than relying on its
    /// display title, so pausing one routine cannot touch a similarly named
    /// user-created event.
    fn remove_pending_routine_occurrences(&self, routine_id: i64) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT e.id FROM events e
             JOIN notifications n ON n.event_id = e.id
             WHERE e.routine_id = ?1 AND n.status = 'pending'",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(params![routine_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for event_id in &ids {
            self.delete_event(*event_id)?;
        }
        Ok(ids.len())
    }

    /// Whether an event with exactly this title and start already exists —
    /// keeps routine materialization idempotent across devices (post-sync).
    pub fn event_exists(&self, title: &str, start: NaiveDateTime) -> Result<bool> {
        let hit: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM events WHERE title = ?1 AND start = ?2 LIMIT 1",
                params![title, fmt_ts(&start)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    // ---- bulk purge (D3, ledger_purge 工具的执行面) ---------------------------

    /// How many events this app has auto-created since `since`.
    ///
    /// Counted from `notification_captures` rather than from a separate tally:
    /// the capture rows *are* the record of what happened, so the count cannot
    /// drift from reality or be reset independently of it.
    pub fn count_auto_events_for_package(
        &self,
        package_name: &str,
        since: NaiveDateTime,
    ) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM notification_captures
             WHERE package_name = ?1 AND state = 'event_created' AND created_at >= ?2",
            params![package_name, fmt_ts(&since)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Per-app auto-created event counts since `since`, most active first —
    /// the "this app created N entries" surface the user reviews.
    pub fn auto_event_counts_by_package(&self, since: NaiveDateTime) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT package_name, count(*) AS n FROM notification_captures
             WHERE state = 'event_created' AND created_at >= ?1
             GROUP BY package_name ORDER BY n DESC, package_name",
        )?;
        let rows = stmt.query_map(params![fmt_ts(&since)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The event this routine already materialized for `start`, if any.
    /// Identified by routine provenance, so an unrelated same-titled event
    /// (typically one that arrived via sync) is never mistaken for it.
    pub fn routine_occurrence_event(
        &self,
        routine_id: i64,
        start: NaiveDateTime,
    ) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM events WHERE routine_id = ?1 AND start = ?2",
                params![routine_id, fmt_ts(&start)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Does this routine occurrence still need materializing?
    ///
    /// True when there is no event for it **or** the event exists but has no
    /// notification at all — the latter being the half-written state a crash
    /// between the two inserts leaves behind. Answering "no" there is what
    /// used to make a missed reminder permanent.
    ///
    /// Deliberately counts notifications in *any* status, not just pending: an
    /// occurrence whose reminder already fired (or was cancelled by the user)
    /// has been materialized, and re-adding one would resurrect a reminder the
    /// user dismissed.
    pub fn routine_occurrence_needs_work(
        &self,
        routine_id: i64,
        start: NaiveDateTime,
    ) -> Result<bool> {
        let Some(ev_id) = self.routine_occurrence_event(routine_id, start)? else {
            return Ok(true);
        };
        let existing: i64 = self.conn.query_row(
            "SELECT count(*) FROM notifications WHERE event_id = ?1",
            params![ev_id],
            |r| r.get(0),
        )?;
        Ok(existing == 0)
    }

    /// How many rows a purge of `layer` before `before` would delete — the
    /// guard's effect preview reads this so the user confirms a real number.
    pub fn count_memory_before(&self, layer: MemoryLayer, before: NaiveDateTime) -> Result<usize> {
        let (table, ts_col) = purge_target(layer)?;
        let n: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {ts_col} < ?1"),
            params![fmt_ts(&before)],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Bulk-delete rows of `layer` older than `before`. Only reachable through
    /// the HITL guard (`ledger_purge`, risk=dangerous). Sync triggers capture
    /// every deleted row, so the purge propagates to other devices.
    pub fn purge_memory_before(&self, layer: MemoryLayer, before: NaiveDateTime) -> Result<usize> {
        let (table, ts_col) = purge_target(layer)?;
        let n = self.conn.execute(
            &format!("DELETE FROM {table} WHERE {ts_col} < ?1"),
            params![fmt_ts(&before)],
        )?;
        Ok(n)
    }

    // ---- persona (F9 v1 / F15) ----------------------------------------------

    /// Persist a new persona version (next version number, made active).
    pub fn insert_persona_version(
        &self,
        draft: &crate::persona::PersonaDraft,
        source: &str,
        note: Option<String>,
        now: NaiveDateTime,
    ) -> Result<crate::persona::PersonaProfile> {
        let next: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM persona_versions",
            [],
            |r| r.get(0),
        )?;
        let profile = crate::persona::PersonaProfile {
            version: next,
            created_at: now,
            source: source.to_string(),
            note,
            draft: draft.clone(),
        };
        self.conn.execute(
            "INSERT INTO persona_versions(version, created_at, profile_json, guid) VALUES (?1, ?2, ?3, ?4)",
            params![next, fmt_ts(&now), profile.to_json()?, new_guid()],
        )?;
        self.set_meta("persona_active", &next.to_string())?;
        Ok(profile)
    }

    pub fn get_persona_version(&self, version: i64) -> Result<crate::persona::PersonaProfile> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT profile_json FROM persona_versions WHERE version = ?1",
                params![version],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(j) => crate::persona::PersonaProfile::from_json(&j),
            None => Err(CoreError::NotFound(format!("persona v{version}"))),
        }
    }

    /// All persona versions, newest first.
    pub fn list_persona_versions(&self) -> Result<Vec<crate::persona::PersonaProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT profile_json FROM persona_versions ORDER BY version DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            out.push(crate::persona::PersonaProfile::from_json(&json?)?);
        }
        Ok(out)
    }

    /// The active persona, or `None` when the user has never set one (or
    /// cleared it).
    pub fn active_persona(&self) -> Result<Option<crate::persona::PersonaProfile>> {
        let Some(v) = self.get_meta("persona_active")? else {
            return Ok(None);
        };
        let Ok(version) = v.parse::<i64>() else {
            return Ok(None);
        };
        // Tolerate a dangling pointer instead of failing. `clear_persona` is
        // now atomic, but a database that already went through the old
        // non-atomic path (or arrives half-merged from sync) must still be able
        // to start: "no persona" is a recoverable state, a startup error is not.
        match self.get_persona_version(version) {
            Ok(profile) => Ok(Some(profile)),
            Err(CoreError::NotFound(_)) => {
                self.conn
                    .execute("DELETE FROM meta WHERE key = 'persona_active'", [])?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Point the active marker at an existing version (F15 rollback — the
    /// history itself is never rewritten).
    pub fn set_active_persona(&self, version: i64) -> Result<crate::persona::PersonaProfile> {
        let profile = self.get_persona_version(version)?;
        self.set_meta("persona_active", &version.to_string())?;
        Ok(profile)
    }

    /// Delete every persona version and the active pointer (the user's
    /// right-to-delete extends to the persona, F12 spirit) — atomically.
    ///
    /// Order and atomicity both matter: dropping the versions first and dying
    /// before clearing `persona_active` leaves the pointer aimed at a version
    /// that no longer exists, and startup loads the active persona — so the
    /// app fails to start over a state the user can't see or repair.
    pub fn clear_persona(&self) -> Result<()> {
        self.with_transaction(|s| {
            s.conn
                .execute("DELETE FROM meta WHERE key = 'persona_active'", [])?;
            s.conn.execute("DELETE FROM persona_versions", [])?;
            Ok(())
        })
    }

    // ---- persistent widgets (F19; schema stored as rows since v13) -------

    /// Rebuild a `WidgetSchema` from its field rows. Views are derived: a
    /// field belongs to a view when that view's ord column is non-NULL, and
    /// the ord orders it. Ties break on guid so every device derives the same
    /// order from the same rows without consulting a clock.
    fn load_widget_schema(
        &self,
        widget_id: i64,
        list_sort_by: Option<String>,
        table_sort_by: Option<String>,
    ) -> Result<WidgetSchema> {
        let mut stmt = self.conn.prepare(
            "SELECT name, label, field_type, required, options_json, form_ord, list_ord,
                    table_ord, stat_ord
             FROM widget_fields WHERE widget_id = ?1 ORDER BY ord, guid",
        )?;
        let rows = stmt.query_map(params![widget_id], |row| {
            let options_json: String = row.get("options_json")?;
            let field_type: String = row.get("field_type")?;
            Ok((
                WidgetField {
                    name: row.get("name")?,
                    label: row.get("label")?,
                    field_type: serde_json::from_value(serde_json::Value::String(field_type))
                        .map_err(|e| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(CoreError::Invalid(
                                format!("字段类型无法识别：{e}"),
                            )))
                        })?,
                    required: row.get::<_, i64>("required")? != 0,
                    options: serde_json::from_str(&options_json).map_err(|e| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(CoreError::Invalid(
                            format!("字段 options 损坏：{e}"),
                        )))
                    })?,
                },
                [
                    row.get::<_, Option<i64>>("form_ord")?,
                    row.get::<_, Option<i64>>("list_ord")?,
                    row.get::<_, Option<i64>>("table_ord")?,
                    row.get::<_, Option<i64>>("stat_ord")?,
                ],
            ))
        })?;
        let loaded: Vec<LoadedWidgetField> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

        let view_fields = |slot: usize| {
            let mut named: Vec<(i64, &str)> = loaded
                .iter()
                .filter_map(|entry| entry.1[slot].map(|ord| (ord, entry.0.name.as_str())))
                .collect();
            named.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
            named
                .into_iter()
                .map(|(_, name)| name.to_string())
                .collect::<Vec<_>>()
        };

        let mut views = Vec::new();
        for (slot, view_type) in WidgetViewType::ALL.iter().enumerate() {
            let fields = view_fields(slot);
            if fields.is_empty() {
                continue;
            }
            views.push(WidgetView {
                view_type: *view_type,
                fields,
                sort_by: match view_type {
                    WidgetViewType::List => list_sort_by.clone(),
                    WidgetViewType::Table => table_sort_by.clone(),
                    _ => None,
                },
            });
        }
        Ok(WidgetSchema {
            fields: loaded.into_iter().map(|entry| entry.0).collect(),
            views,
        })
    }

    pub fn insert_widget_definition(
        &self,
        draft: &WidgetDefinitionDraft,
        now: NaiveDateTime,
    ) -> Result<WidgetDefinition> {
        draft.validate()?;
        // Enforced here rather than in `validate()` because it is a property of
        // the device, not of the schema. Every caller goes through this path,
        // so no future CLI or import route can bypass it.
        let existing = self.count_widget_definitions()?;
        if existing >= crate::widget::MAX_WIDGETS as i64 {
            return Err(CoreError::Invalid(format!(
                "本机组件数已达上限 {}，请先删除不再使用的组件",
                crate::widget::MAX_WIDGETS
            )));
        }
        let schema = draft.schema();
        let sort_for = |view_type: WidgetViewType| {
            schema
                .views
                .iter()
                .find(|view| view.view_type == view_type)
                .and_then(|view| view.sort_by.clone())
        };
        self.conn.execute(
            "INSERT INTO widget_defs(name, icon, list_sort_by, table_sort_by, created_at, guid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                draft.name,
                draft.icon,
                sort_for(WidgetViewType::List),
                sort_for(WidgetViewType::Table),
                fmt_ts(&now),
                new_guid()
            ],
        )?;
        let widget_id = self.conn.last_insert_rowid();
        let position = |view_type: WidgetViewType, name: &str| {
            schema
                .views
                .iter()
                .find(|view| view.view_type == view_type)
                .and_then(|view| view.fields.iter().position(|f| f == name))
                .map(|index| index as i64)
        };
        for (ord, field) in schema.fields.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                   options_json, ord, form_ord, list_ord, table_ord, stat_ord, created_at, guid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    widget_id,
                    field.name,
                    field.label,
                    field.field_type.as_str(),
                    field.required as i64,
                    serde_json::to_string(&field.options)?,
                    ord as i64,
                    position(WidgetViewType::Form, &field.name),
                    position(WidgetViewType::List, &field.name),
                    position(WidgetViewType::Table, &field.name),
                    position(WidgetViewType::Stat, &field.name),
                    fmt_ts(&now),
                    new_guid(),
                ],
            )?;
        }
        self.get_widget_definition(widget_id)
    }

    /// The only schema evolution this design allows (设计稿 ⑧): append a field.
    /// It must be optional — existing records cannot retroactively acquire a
    /// required value — which is exactly what makes records need no migration.
    pub fn add_widget_field(
        &self,
        widget_id: i64,
        field: &WidgetField,
        now: NaiveDateTime,
    ) -> Result<WidgetDefinition> {
        let definition = self.get_widget_definition(widget_id)?;
        if field.required {
            return Err(CoreError::Invalid(
                "新增字段必须可空：已有记录无法追溯填写必填值".into(),
            ));
        }
        field.validate_standalone()?;
        if definition
            .schema
            .fields
            .iter()
            .any(|existing| existing.name == field.name)
        {
            return Err(CoreError::Invalid(format!("字段 {:?} 已存在", field.name)));
        }
        // Checked against the current (possibly merged) field count, so a
        // widget that is already over the cap simply stops accepting new
        // fields rather than losing any.
        if definition.schema.fields.len() >= crate::widget::MAX_FIELDS {
            return Err(CoreError::Invalid(format!(
                "字段数已达上限 {}，无法再加",
                crate::widget::MAX_FIELDS
            )));
        }
        // Only extend views this widget actually has; creating one implicitly
        // would change its shape behind the user's back.
        let next_ord = |view_type: WidgetViewType| -> Option<i64> {
            definition
                .schema
                .views
                .iter()
                .find(|view| view.view_type == view_type)
                .map(|view| view.fields.len() as i64)
        };
        self.conn.execute(
            "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
               options_json, ord, form_ord, list_ord, table_ord, stat_ord, created_at, guid)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                widget_id,
                field.name,
                field.label,
                field.field_type.as_str(),
                serde_json::to_string(&field.options)?,
                definition.schema.fields.len() as i64,
                next_ord(WidgetViewType::Form),
                next_ord(WidgetViewType::List),
                next_ord(WidgetViewType::Table),
                // A new field never joins a stat view: "how many records filled
                // this in" is a meaningless zero for a column every existing
                // row leaves empty.
                None::<i64>,
                fmt_ts(&now),
                new_guid(),
            ],
        )?;
        self.get_widget_definition(widget_id)
    }

    // ---- restorable export / import -------------------------------------

    /// Every syncable row in wire shape: `{guid, ...payload}` per table.
    ///
    /// The human-readable layers of an export are not restorable — they carry
    /// no guid, so re-importing one would duplicate everything instead of
    /// converging, and FK links (an event's raw input) would be unrecoverable.
    /// This section exists so a backup can actually be replayed.
    pub fn export_restore_rows(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let mut out = serde_json::Map::new();
        for (tbl, payload, syncable) in SYNC_PAYLOADS {
            // The very expressions the capture triggers use, read directly off
            // the table instead of off `NEW`.
            let expr = payload.replace("NEW.", "");
            let cond = syncable.replace("NEW.", "");
            let sql = format!("SELECT guid, {expr} FROM {tbl} WHERE guid IS NOT NULL AND ({cond})");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut list = Vec::new();
            for row in rows {
                let (guid, payload_json) = row?;
                let mut value: serde_json::Value = serde_json::from_str(&payload_json)?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("guid".into(), serde_json::Value::String(guid));
                }
                list.push(value);
            }
            out.insert((*tbl).to_string(), serde_json::Value::Array(list));
        }
        // Config that syncs as LWW meta documents rides along too, or a
        // restored device would come back without its rule table.
        let mut metas = Vec::new();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT key, value FROM meta WHERE key IN ({SYNCED_META_KEYS})"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, value) = row?;
            metas.push(
                serde_json::json!({ "guid": format!("meta:{key}"), "key": key, "value": value }),
            );
        }
        out.insert("meta".into(), serde_json::Value::Array(metas));
        Ok(out)
    }

    /// Replay a `_restore` section through the ordinary merge path.
    ///
    /// Stamping every row with the export's own timestamp (not "now") is the
    /// load-bearing choice: LWW then treats a restored row exactly like a row
    /// arriving late from another device, so **restoring an old backup cannot
    /// clobber newer local edits**, and importing the same file twice is a
    /// no-op. A restore is not privileged over what the user has since done.
    pub fn import_restore_rows(
        &self,
        restore: &serde_json::Value,
        exported_at: NaiveDateTime,
        origin: &str,
        now: NaiveDateTime,
    ) -> Result<MergeCounts> {
        let object = restore
            .as_object()
            .ok_or_else(|| CoreError::Invalid("_restore 必须是对象".into()))?;
        // A backup file is untrusted input — it can be edited, or come from
        // somewhere the user did not expect. `exported_at` is only the file's
        // own claim about when it was written, and it is used directly as the
        // LWW stamp: a document claiming the year 3000 beats every local row,
        // now and forever, which makes "import never destroys local data" a
        // false promise.
        //
        // **Refuse** such a document rather than silently repairing it.
        //
        // Clamping the stamp down to "now" was the first instinct and it is
        // not enough: the content would then be old but stamped fresh, which
        // still beats every edit the user made before the import. The file is
        // either an honest backup — in which case its timestamp is in the past
        // and nothing needs fixing — or it is not one, and the honest response
        // is to say so rather than to guess at a repair.
        //
        // The comparison uses **the clock the competing stamps use**: every
        // local write is stamped by the sync trigger's `strftime('now')`, i.e.
        // real UTC. The injected `now` is a local naive time and may even be
        // the debug simulated clock, so the more permissive of the two is the
        // ceiling — this check is about catching forgery, not about policing
        // ordinary timezone and clock skew.
        let db_now: String =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%f', 'now')", [], |r| {
                    r.get(0)
                })?;
        let hlc = exported_at.format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let ceiling = now
            .max(parse_ts(&db_now).unwrap_or(now))
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap_or(now);
        if exported_at > ceiling {
            return Err(CoreError::Invalid(format!(
                "备份自称的导出时间（{}）在未来，已拒绝导入。\
                 这份文件要么被改动过，要么来自时钟错误的设备；\
                 直接合并会让它永久压过你本机后续的所有修改。",
                exported_at.format("%Y-%m-%d %H:%M")
            )));
        }

        let mut total_rows = 0usize;
        let mut ops = Vec::new();
        for (tbl, rows) in object {
            // Reject unknown tables rather than parking them.
            //
            // Quarantine exists for ops from a *peer running a newer build* —
            // data we should keep because a later version will understand it.
            // A table name in an imported file is not that: it is whatever the
            // file said. Letting it through meant a crafted backup could fill
            // the 5 000-op quarantine and push out real cross-device data that
            // was genuinely waiting for an upgrade.
            // The authority is what `export_restore_rows` actually writes:
            // every `SYNC_PAYLOADS` table, plus `meta` (config rows that sync
            // as LWW documents — without them a restored device comes back
            // missing its rule table).
            let known = SYNC_PAYLOADS.iter().any(|(name, _, _)| name == tbl) || tbl == "meta";
            if !known {
                return Err(CoreError::Invalid(format!(
                    "备份里含有本机不认识的表 `{tbl}`，已拒绝导入（备份可能被改动过或来自其他程序）"
                )));
            }
            let rows = rows
                .as_array()
                .ok_or_else(|| CoreError::Invalid(format!("_restore.{tbl} 必须是数组")))?;
            total_rows += rows.len();
            if total_rows > MAX_IMPORT_ROWS {
                return Err(CoreError::Invalid(format!(
                    "备份超过 {MAX_IMPORT_ROWS} 行上限，已拒绝导入"
                )));
            }
            for row in rows {
                let guid = row
                    .get("guid")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| CoreError::Invalid(format!("_restore.{tbl} 有行缺少 guid")))?;
                let payload = row.to_string();
                if payload.len() > MAX_IMPORT_ROW_BYTES {
                    return Err(CoreError::Invalid(format!(
                        "_restore.{tbl} 有单行超过 {MAX_IMPORT_ROW_BYTES} 字节上限，已拒绝导入"
                    )));
                }
                ops.push(crate::sync::SyncOp {
                    tbl: tbl.clone(),
                    guid: guid.to_string(),
                    op: "upsert".into(),
                    payload: Some(payload),
                    hlc: hlc.clone(),
                    origin: origin.to_string(),
                });
            }
        }
        // Parents before children, so an event's raw input already exists when
        // the event's own FK-by-guid lookup runs.
        let order = |tbl: &str| {
            SYNC_PAYLOADS
                .iter()
                .position(|(name, _, _)| *name == tbl)
                .unwrap_or(usize::MAX)
        };
        ops.sort_by_key(|op| order(&op.tbl));
        self.apply_remote_ops(&ops)
    }

    /// How many rows a `_restore` section would offer, per table — the
    /// Guard preview needs this before anything is written.
    pub fn count_restore_rows(restore: &serde_json::Value) -> Vec<(String, usize)> {
        let Some(object) = restore.as_object() else {
            return Vec::new();
        };
        let mut counts: Vec<(String, usize)> = object
            .iter()
            .filter_map(|(tbl, rows)| {
                let len = rows.as_array()?.len();
                (len > 0).then(|| (tbl.clone(), len))
            })
            .collect();
        counts.sort();
        counts
    }

    pub fn count_widget_definitions(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM widget_defs", [], |row| row.get(0))?)
    }

    pub fn get_widget_definition(&self, id: i64) -> Result<WidgetDefinition> {
        // Two steps because assembling the schema needs its own query, which
        // cannot run while the outer statement is still borrowed.
        struct Head {
            name: String,
            icon: String,
            list_sort_by: Option<String>,
            table_sort_by: Option<String>,
            created_at: String,
        }
        let head: Option<Head> = self
            .conn
            .query_row(
                "SELECT name, icon, list_sort_by, table_sort_by, created_at
                 FROM widget_defs WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Head {
                        name: row.get(0)?,
                        icon: row.get(1)?,
                        list_sort_by: row.get(2)?,
                        table_sort_by: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        let Head {
            name,
            icon,
            list_sort_by,
            table_sort_by,
            created_at,
        } = head.ok_or_else(|| CoreError::NotFound(format!("widget#{id}")))?;
        Ok(WidgetDefinition {
            id,
            name,
            icon,
            schema: self.load_widget_schema(id, list_sort_by, table_sort_by)?,
            created_at: parse_ts(&created_at)?,
        })
    }

    pub fn list_widget_definitions(&self) -> Result<Vec<WidgetDefinition>> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM widget_defs ORDER BY created_at DESC, id DESC")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        ids.into_iter()
            .map(|id| self.get_widget_definition(id))
            .collect()
    }

    fn row_to_widget_record(row: &rusqlite::Row) -> rusqlite::Result<WidgetRecord> {
        let data_json: String = row.get("data_json")?;
        let created_at: String = row.get("created_at")?;
        let data = serde_json::from_str(&data_json).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(CoreError::Invalid(format!(
                "widget record JSON 损坏：{e}"
            ))))
        })?;
        let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
        Ok(WidgetRecord {
            id: row.get("id")?,
            widget_id: row.get("widget_id")?,
            data,
            created_at: parse_ts(&created_at).map_err(map)?,
        })
    }

    pub fn list_widget_records(&self, widget_id: i64) -> Result<Vec<WidgetRecord>> {
        // Loading the definition first distinguishes an empty widget from an
        // invalid id, and validates the persisted schema before its records
        // reach a renderer.
        self.get_widget_definition(widget_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT * FROM widget_records WHERE widget_id = ?1 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![widget_id], Self::row_to_widget_record)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every record across every definition, for the export document. Widgets
    /// never sync (§3.12), so this export is the only copy of hand-entered
    /// widget data that can leave the device.
    pub fn list_all_widget_records(&self) -> Result<Vec<WidgetRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM widget_records ORDER BY widget_id, created_at, id")?;
        let rows = stmt.query_map([], Self::row_to_widget_record)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_widget_record(
        &self,
        widget_id: i64,
        data: &serde_json::Value,
        now: NaiveDateTime,
    ) -> Result<WidgetRecord> {
        let definition = self.get_widget_definition(widget_id)?;
        definition.schema.validate_record(data)?;
        let data_json = serde_json::to_string(data)?;
        self.conn.execute(
            "INSERT INTO widget_records(widget_id, data_json, created_at, guid)
             VALUES (?1, ?2, ?3, ?4)",
            params![widget_id, data_json, fmt_ts(&now), new_guid()],
        )?;
        Ok(WidgetRecord {
            id: self.conn.last_insert_rowid(),
            widget_id,
            data: data.clone(),
            created_at: now,
        })
    }

    pub fn update_widget_record(
        &self,
        widget_id: i64,
        id: i64,
        data: &serde_json::Value,
    ) -> Result<WidgetRecord> {
        let definition = self.get_widget_definition(widget_id)?;
        definition.schema.validate_record(data)?;
        let data_json = serde_json::to_string(data)?;
        let changed = self.conn.execute(
            "UPDATE widget_records SET data_json = ?1 WHERE id = ?2 AND widget_id = ?3",
            params![data_json, id, widget_id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("widget_record#{id}")));
        }
        self.conn
            .query_row(
                "SELECT * FROM widget_records WHERE id = ?1 AND widget_id = ?2",
                params![id, widget_id],
                Self::row_to_widget_record,
            )
            .map_err(Into::into)
    }

    pub fn delete_widget_record(&self, widget_id: i64, id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM widget_records WHERE id = ?1 AND widget_id = ?2",
            params![id, widget_id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("widget_record#{id}")));
        }
        Ok(())
    }

    pub fn widget_record_count(&self, widget_id: i64) -> Result<i64> {
        self.get_widget_definition(widget_id)?;
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM widget_records WHERE widget_id = ?1",
                params![widget_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Deletes the definition and everything under it. This is intentionally
    /// only called by the `widget_delete` Dangerous Tool.
    ///
    /// The children are deleted **explicitly** rather than through the FK's
    /// `ON DELETE CASCADE`: SQLite does not fire a table's triggers for rows
    /// removed by a cascade action unless `recursive_triggers` is on, and it
    /// is not. Relying on the cascade here would delete the rows locally
    /// without ever writing their `delete` ops to the oplog, leaving every
    /// other device holding orphan fields and records forever.
    pub fn delete_widget_definition(&self, id: i64) -> Result<(String, i64)> {
        let definition = self.get_widget_definition(id)?;
        let count = self.widget_record_count(id)?;
        self.with_transaction(|s| {
            s.conn.execute(
                "DELETE FROM widget_records WHERE widget_id = ?1",
                params![id],
            )?;
            s.conn.execute(
                "DELETE FROM widget_fields WHERE widget_id = ?1",
                params![id],
            )?;
            let changed = s
                .conn
                .execute("DELETE FROM widget_defs WHERE id = ?1", params![id])?;
            if changed == 0 {
                return Err(CoreError::NotFound(format!("widget#{id}")));
            }
            Ok(())
        })?;
        Ok((definition.name, count))
    }

    pub fn append_widget_schema_rejection(
        &self,
        schema_json: &str,
        reason: &str,
        now: NaiveDateTime,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO widget_schema_rejections(schema_json, reason, created_at) VALUES (?1, ?2, ?3)",
            params![schema_json, reason, fmt_ts(&now)],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_widget_schema_rejections(&self) -> Result<Vec<WidgetSchemaRejection>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM widget_schema_rejections ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], |row| {
            let created_at: String = row.get("created_at")?;
            let map = |e: CoreError| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
            Ok(WidgetSchemaRejection {
                id: row.get("id")?,
                schema_json: row.get("schema_json")?,
                reason: row.get("reason")?,
                created_at: parse_ts(&created_at).map_err(map)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- audit log (append-only) -----------------------------------------

    pub fn append_audit(&self, e: &AuditEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO audit_log(ts, tool, risk, summary, decision, token_id, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                fmt_ts(&e.ts),
                e.tool,
                e.risk.as_str(),
                e.summary,
                e.decision.as_str(),
                e.token_id,
                e.detail,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_audit(&self) -> Result<Vec<AuditRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM audit_log ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(AuditRow {
                id: row.get("id")?,
                ts: row.get("ts")?,
                tool: row.get("tool")?,
                risk: row.get("risk")?,
                summary: row.get("summary")?,
                decision: row.get("decision")?,
                token_id: row.get("token_id")?,
                detail: row.get("detail")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- config -----------------------------------------------------------

    fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn load_rule_table(&self) -> Result<RuleTable> {
        match self.get_meta("rule_table")? {
            Some(json) => RuleTable::from_json(&json),
            None => Ok(RuleTable::default_table()),
        }
    }

    pub fn save_rule_table(&self, t: &RuleTable) -> Result<()> {
        self.set_meta("rule_table", &t.to_json()?)
    }

    pub fn load_proactivity(&self) -> Result<ProactivityConfig> {
        match self.get_meta("proactivity")? {
            Some(json) => ProactivityConfig::from_json(&json),
            None => Ok(ProactivityConfig::defaults()),
        }
    }

    pub fn save_proactivity(&self, c: &ProactivityConfig) -> Result<()> {
        self.set_meta("proactivity", &c.to_json()?)
    }

    /// Whether subsequently captured third-party notifications may leave the
    /// device through the existing sync / recall-context paths. Missing means
    /// enabled: the Phase 9 product default is opt-out.
    pub fn notif_cloud_enabled(&self) -> Result<bool> {
        match self.get_meta(NOTIF_CLOUD_META_KEY)? {
            None => Ok(true),
            Some(value) => match value.as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(CoreError::Invalid(format!(
                    "{} 必须是 true 或 false",
                    NOTIF_CLOUD_META_KEY
                ))),
            },
        }
    }

    /// Persist this device's capture-time notification privacy choice. It is
    /// intentionally not a synced meta key: another device must not silently
    /// alter this device's outbound-data preference.
    pub fn set_notif_cloud_enabled(&self, enabled: bool) -> Result<()> {
        self.set_meta(NOTIF_CLOUD_META_KEY, if enabled { "true" } else { "false" })
    }

    /// F20's package whitelist and confirmed local filtering rules. This is
    /// device-local like the notification-cloud toggle: installed package ids
    /// and capture consent must not be silently changed by another device.
    pub fn notification_intelligence_config(&self) -> Result<NotificationIntelligenceConfig> {
        match self.get_meta(NOTIFICATION_INTELLIGENCE_META_KEY)? {
            Some(json) => serde_json::from_str::<NotificationIntelligenceConfig>(&json)
                .map_err(Into::into)
                .and_then(NotificationIntelligenceConfig::normalized),
            None => Ok(NotificationIntelligenceConfig::default()),
        }
    }

    pub fn save_notification_intelligence_config(
        &self,
        config: &NotificationIntelligenceConfig,
    ) -> Result<()> {
        let config = config.clone().normalized()?;
        self.set_meta(
            NOTIFICATION_INTELLIGENCE_META_KEY,
            &serde_json::to_string(&config)?,
        )
    }

    // ---- F20 notification capture journal ---------------------------------

    pub fn insert_notification_capture(
        &self,
        capture: &NotificationCapture,
        local_only: bool,
        lane: CaptureLane,
        state: CaptureState,
        reason: Option<&str>,
    ) -> Result<NotificationCaptureRecord> {
        let raw_input_id = self.insert_raw_input_with_scope(
            &capture.raw_input(),
            "notification_capture",
            capture.received_at,
            local_only,
        )?;
        let content_hash =
            crate::notification_intelligence::content_hash(&capture.package_name, &capture.text());
        self.conn.execute(
            "INSERT INTO notification_captures(raw_input_id, package_name, title, body, received_at, content_hash, local_only, lane, state, reason, event_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11)",
            params![
                raw_input_id,
                capture.package_name,
                capture.title,
                capture.body,
                fmt_ts(&capture.received_at),
                content_hash,
                local_only as i64,
                lane.as_str(),
                state.as_str(),
                reason,
                fmt_ts(&capture.received_at),
            ],
        )?;
        Ok(NotificationCaptureRecord {
            id: Some(self.conn.last_insert_rowid()),
            raw_input_id,
            package_name: capture.package_name.clone(),
            title: capture.title.clone(),
            body: capture.body.clone(),
            received_at: capture.received_at,
            content_hash,
            local_only,
            lane,
            state,
            reason: reason.map(ToOwned::to_owned),
            event_id: None,
            created_at: capture.received_at,
        })
    }

    fn row_to_notification_capture(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<NotificationCaptureRecord> {
        fn from_core<T>(value: crate::error::Result<T>) -> rusqlite::Result<T> {
            value.map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        }
        let received_at: String = row.get("received_at")?;
        let created_at: String = row.get("created_at")?;
        let lane: String = row.get("lane")?;
        let state: String = row.get("state")?;
        Ok(NotificationCaptureRecord {
            id: Some(row.get("id")?),
            raw_input_id: row.get("raw_input_id")?,
            package_name: row.get("package_name")?,
            title: row.get("title")?,
            body: row.get("body")?,
            received_at: from_core(parse_ts(&received_at))?,
            content_hash: row.get("content_hash")?,
            local_only: row.get::<_, i64>("local_only")? != 0,
            lane: from_core(lane.parse::<CaptureLane>())?,
            state: from_core(state.parse::<CaptureState>())?,
            reason: row.get("reason")?,
            event_id: row.get("event_id")?,
            created_at: from_core(parse_ts(&created_at))?,
        })
    }

    pub fn notification_capture(&self, id: i64) -> Result<NotificationCaptureRecord> {
        self.conn
            .query_row(
                "SELECT * FROM notification_captures WHERE id = ?1",
                params![id],
                Self::row_to_notification_capture,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("notification_capture#{id}")))
    }

    pub fn list_notification_captures(&self) -> Result<Vec<NotificationCaptureRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notification_captures ORDER BY received_at DESC, id DESC")?;
        let rows = stmt.query_map([], Self::row_to_notification_capture)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn queued_notification_captures(
        &self,
        lane: Option<CaptureLane>,
    ) -> Result<Vec<NotificationCaptureRecord>> {
        let sql = if lane.is_some() {
            "SELECT * FROM notification_captures WHERE state = 'queued' AND lane = ?1 ORDER BY received_at ASC, id ASC"
        } else {
            "SELECT * FROM notification_captures WHERE state = 'queued' ORDER BY received_at ASC, id ASC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = match lane {
            Some(lane) => {
                stmt.query_map(params![lane.as_str()], Self::row_to_notification_capture)?
            }
            None => stmt.query_map([], Self::row_to_notification_capture)?,
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn has_recent_notification_duplicate(
        &self,
        package_name: &str,
        content_hash: &str,
        since: NaiveDateTime,
    ) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM notification_captures
                 WHERE package_name = ?1 AND content_hash = ?2 AND received_at >= ?3 LIMIT 1",
                params![package_name, content_hash, fmt_ts(&since)],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn set_notification_capture_state(
        &self,
        id: i64,
        state: CaptureState,
        reason: Option<&str>,
        event_id: Option<i64>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE notification_captures SET state = ?1, reason = ?2, event_id = ?3 WHERE id = ?4",
            params![state.as_str(), reason, event_id, id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!("notification_capture#{id}")));
        }
        Ok(())
    }

    pub fn insert_notification_filter_proposal(
        &self,
        proposal: &NotificationFilterProposal,
    ) -> Result<NotificationFilterProposal> {
        self.conn.execute(
            "INSERT INTO notification_filter_proposals(package_name, pattern, matcher, reason, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                proposal.package_name,
                proposal.pattern,
                match proposal.matcher {
                    crate::classify::NotificationMatchKind::Substring => "substring",
                    crate::classify::NotificationMatchKind::Regex => "regex",
                },
                proposal.reason,
                proposal.state.as_str(),
                fmt_ts(&proposal.created_at),
            ],
        )?;
        let mut out = proposal.clone();
        out.id = Some(self.conn.last_insert_rowid());
        Ok(out)
    }

    fn row_to_notification_filter_proposal(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<NotificationFilterProposal> {
        let matcher: String = row.get("matcher")?;
        let state: String = row.get("state")?;
        let created_at: String = row.get("created_at")?;
        let matcher = match matcher.as_str() {
            "substring" => crate::classify::NotificationMatchKind::Substring,
            "regex" => crate::classify::NotificationMatchKind::Regex,
            _other => {
                return Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "matcher".into(),
                    rusqlite::types::Type::Text,
                ))
            }
        };
        let state = state.parse().map_err(|error: CoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        Ok(NotificationFilterProposal {
            id: Some(row.get("id")?),
            package_name: row.get("package_name")?,
            pattern: row.get("pattern")?,
            matcher,
            reason: row.get("reason")?,
            state,
            created_at: parse_ts(&created_at).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        })
    }

    pub fn list_notification_filter_proposals(&self) -> Result<Vec<NotificationFilterProposal>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notification_filter_proposals ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_notification_filter_proposal)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_notification_filter_proposal_state(
        &self,
        id: i64,
        state: FilterProposalState,
    ) -> Result<NotificationFilterProposal> {
        let changed = self.conn.execute(
            "UPDATE notification_filter_proposals SET state = ?1 WHERE id = ?2",
            params![state.as_str(), id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "notification_filter_proposal#{id}"
            )));
        }
        self.conn
            .query_row(
                "SELECT * FROM notification_filter_proposals WHERE id = ?1",
                params![id],
                Self::row_to_notification_filter_proposal,
            )
            .map_err(Into::into)
    }

    pub fn insert_notification_action_proposal(
        &self,
        proposal: &NotificationActionProposal,
    ) -> Result<NotificationActionProposal> {
        self.conn.execute(
            "INSERT INTO notification_action_proposals(capture_id, kind, event_id, event_title, event_guid, event_start, new_start, reason, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                proposal.capture_id,
                proposal.kind.as_str(),
                proposal.event_id,
                proposal.event_title,
                proposal.event_guid,
                fmt_ts(&proposal.event_start),
                proposal.new_start.map(|value| fmt_ts(&value)),
                proposal.reason,
                proposal.state.as_str(),
                fmt_ts(&proposal.created_at),
            ],
        )?;
        let mut out = proposal.clone();
        out.id = Some(self.conn.last_insert_rowid());
        Ok(out)
    }

    fn row_to_notification_action_proposal(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<NotificationActionProposal> {
        fn from_core<T>(value: crate::error::Result<T>) -> rusqlite::Result<T> {
            value.map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        }
        let kind: String = row.get("kind")?;
        let state: String = row.get("state")?;
        let new_start: Option<String> = row.get("new_start")?;
        let created_at: String = row.get("created_at")?;
        Ok(NotificationActionProposal {
            id: Some(row.get("id")?),
            capture_id: row.get("capture_id")?,
            kind: from_core(kind.parse::<NotificationActionKind>())?,
            event_id: row.get("event_id")?,
            event_title: row.get("event_title")?,
            event_guid: row.get("event_guid")?,
            // Rows written before the snapshot columns existed carry an empty
            // string; `parse_ts` would fail, so map it to the epoch and let the
            // guid check (also empty → mismatch) do the refusing.
            event_start: match row.get::<_, String>("event_start")? {
                s if s.is_empty() => NaiveDateTime::default(),
                s => from_core(parse_ts(&s))?,
            },
            new_start: new_start
                .map(|value| from_core(parse_ts(&value)))
                .transpose()?,
            reason: row.get("reason")?,
            state: from_core(state.parse::<ActionProposalState>())?,
            created_at: from_core(parse_ts(&created_at))?,
        })
    }

    pub fn list_notification_action_proposals(&self) -> Result<Vec<NotificationActionProposal>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM notification_action_proposals ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_notification_action_proposal)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn notification_action_proposal(&self, id: i64) -> Result<NotificationActionProposal> {
        self.conn
            .query_row(
                "SELECT * FROM notification_action_proposals WHERE id = ?1",
                params![id],
                Self::row_to_notification_action_proposal,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("notification_action_proposal#{id}")))
    }

    pub fn set_notification_action_proposal_state(
        &self,
        id: i64,
        state: ActionProposalState,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE notification_action_proposals SET state = ?1 WHERE id = ?2",
            params![state.as_str(), id],
        )?;
        if changed == 0 {
            return Err(CoreError::NotFound(format!(
                "notification_action_proposal#{id}"
            )));
        }
        Ok(())
    }

    fn delete_notification_capture(&self, id: i64) -> Result<()> {
        let raw_input_id: i64 = self
            .conn
            .query_row(
                "SELECT raw_input_id FROM notification_captures WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("notification_capture#{id}")))?;
        self.conn.execute(
            "DELETE FROM notification_action_proposals WHERE capture_id = ?1",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM notification_captures WHERE id = ?1",
            params![id],
        )?;
        self.delete_raw_input_cascade(raw_input_id)
    }

    /// Erase an input and everything derived from it, as one unit.
    ///
    /// This is the user's right-to-delete path: "forget what I said" has to
    /// mean the events, reminders, captures and proposals that were built out
    /// of it too. Half of that is arguably worse than none — a deletion that
    /// reports success while leaving the derived copies behind is a privacy
    /// promise the data doesn't keep.
    fn delete_raw_input_cascade(&self, id: i64) -> Result<()> {
        self.with_transaction(|s| {
            // Cascade: events derived from this input, and their notifications.
            let ids: Vec<i64> = {
                let mut stmt = s
                    .conn
                    .prepare("SELECT id FROM events WHERE raw_input_id = ?1")?;
                let rows = stmt
                    .query_map(params![id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for event_id in ids {
                s.delete_event(event_id)?;
            }
            s.conn.execute(
                "DELETE FROM notification_action_proposals
                 WHERE capture_id IN (SELECT id FROM notification_captures WHERE raw_input_id = ?1)",
                params![id],
            )?;
            s.conn.execute(
                "DELETE FROM notification_captures WHERE raw_input_id = ?1",
                params![id],
            )?;
            let changed = s
                .conn
                .execute("DELETE FROM raw_inputs WHERE id = ?1", params![id])?;
            if changed == 0 {
                return Err(CoreError::NotFound(format!("raw_input#{id}")));
            }
            Ok(())
        })
    }

    // ---- memory ledger (F12) ---------------------------------------------

    /// A unified, newest-first view of everything the agent remembers, across
    /// layers. This is the surface the user inspects and prunes.
    pub fn memory_ledger(&self) -> Result<Vec<MemoryEntry>> {
        let mut entries = Vec::new();

        let mut stmt = self
            .conn
            .prepare("SELECT id, text, intent, created_at FROM raw_inputs")?;
        let rows = stmt.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let text: String = r.get(1)?;
            let intent: String = r.get(2)?;
            let created: String = r.get(3)?;
            Ok((id, text, intent, created))
        })?;
        for row in rows {
            let (id, text, intent, created) = row?;
            entries.push(MemoryEntry {
                layer: MemoryLayer::RawInput,
                id,
                summary: text,
                source: Some(format!("intent={intent}")),
                created_at: parse_ts(&created)?,
            });
        }

        for ev in self.list_events()? {
            let loc = ev
                .location
                .as_ref()
                .map(|l| format!(" @{l}"))
                .unwrap_or_default();
            entries.push(MemoryEntry {
                layer: MemoryLayer::Event,
                id: ev.id.unwrap_or(0),
                summary: format!(
                    "[{}] {} — {}{}",
                    ev.kind.label(),
                    ev.title,
                    crate::model::fmt_ts_human(&ev.start),
                    loc
                ),
                source: None,
                created_at: ev.created_at,
            });
        }

        for n in self.list_notifications()? {
            entries.push(MemoryEntry {
                layer: MemoryLayer::Notification,
                id: n.id.unwrap_or(0),
                summary: format!(
                    "提前 {} 通知 event#{} @ {}（{}）",
                    n.lead_label,
                    n.event_id,
                    crate::model::fmt_ts_human(&n.fire_at),
                    match n.status {
                        NotificationStatus::Pending => "待触发",
                        NotificationStatus::Fired => "已触发",
                        NotificationStatus::Dismissed => "已取消",
                    }
                ),
                source: Some(format!("event#{}", n.event_id)),
                created_at: n.created_at,
            });
        }

        for b in self.list_behavior()? {
            let label = match b.kind {
                BehaviorKind::Status => "状态",
                BehaviorKind::CheckinAsked => "问询",
                BehaviorKind::ReminderFired => "提醒触发",
            };
            entries.push(MemoryEntry {
                layer: MemoryLayer::Behavior,
                id: b.id.unwrap_or(0),
                summary: format!("[{label}] {}", b.content),
                source: b.source,
                created_at: b.ts,
            });
        }

        for s in self.list_suggestions()? {
            entries.push(MemoryEntry {
                layer: MemoryLayer::Suggestion,
                id: s.id.unwrap_or(0),
                summary: format!(
                    "[{}] {}",
                    match s.status {
                        crate::suggest::SuggestionStatus::Pending => "待处理",
                        crate::suggest::SuggestionStatus::Accepted => "已采纳",
                        crate::suggest::SuggestionStatus::Dismissed => "已忽略",
                    },
                    s.text
                ),
                source: s.source,
                created_at: s.created_at,
            });
        }

        for h in self.list_health_samples()? {
            let unit = match h.kind {
                HealthMetric::HeartRate => "bpm",
                HealthMetric::Steps => "步",
                HealthMetric::Sleep => "分钟",
            };
            entries.push(MemoryEntry {
                layer: MemoryLayer::Wearable,
                id: h.id.unwrap_or(0),
                summary: format!(
                    "[{}] {} {unit} @ {}",
                    h.kind.label(),
                    h.value,
                    crate::model::fmt_ts_human(&h.start)
                ),
                source: Some(h.source),
                created_at: h.created_at,
            });
        }

        for f in self.list_facts()? {
            entries.push(MemoryEntry {
                layer: MemoryLayer::Fact,
                id: f.id.unwrap_or(0),
                summary: f.content,
                source: Some(f.source),
                created_at: f.created_at,
            });
        }

        for r in self.list_routines()? {
            entries.push(MemoryEntry {
                layer: MemoryLayer::Routine,
                id: r.id.unwrap_or(0),
                summary: format!(
                    "[{}] 每天 {} 「{}」",
                    if r.active { "启用" } else { "已暂停" },
                    r.time_of_day,
                    r.title
                ),
                source: r.source,
                created_at: r.created_at,
            });
        }

        for capture in self.list_notification_captures()? {
            let lane = match capture.lane {
                CaptureLane::Urgent => "重要·即时",
                CaptureLane::Batch => "普通·批量",
            };
            let state = match capture.state {
                CaptureState::Queued => "待处理",
                CaptureState::EventCreated => "已建事件",
                CaptureState::Filtered => "已过滤",
                CaptureState::Deduplicated => "已判重",
                CaptureState::NeedsReview => "待回看",
                CaptureState::Resolved => "已处理",
            };
            let reason = capture
                .reason
                .as_deref()
                .map(|reason| format!(" · {reason}"))
                .unwrap_or_default();
            entries.push(MemoryEntry {
                layer: MemoryLayer::NotificationCapture,
                id: capture.id.unwrap_or(0),
                summary: format!("[{lane}·{state}] {}", capture.title),
                source: Some(format!("{}{}", capture.package_name, reason)),
                created_at: capture.received_at,
            });
        }

        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        Ok(entries)
    }

    /// Delete a memory entry. Deleting a raw input or event cascades to the
    /// records derived from it — once gone, it cannot be recovered by chatting
    /// (F12 delete semantics).
    /// What deleting this ledger row would actually take with it.
    ///
    /// The Guard's preview is only worth showing if it names the *whole*
    /// blast radius, and for `raw_input` that is not one row: the cascade
    /// removes every event derived from it and every reminder under those.
    /// A preview that said "删除 1 条原始输入" while five reminders quietly
    /// went with it would be exactly the kind of under-stated confirmation
    /// the effect-digest binding exists to prevent.
    pub fn describe_memory_deletion(&self, layer: MemoryLayer, id: i64) -> Result<String> {
        if layer != MemoryLayer::RawInput {
            return Ok(String::new());
        }
        let events: i64 = self.conn.query_row(
            "SELECT count(*) FROM events WHERE raw_input_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let notifs: i64 = self.conn.query_row(
            "SELECT count(*) FROM notifications WHERE event_id IN
             (SELECT id FROM events WHERE raw_input_id = ?1)",
            params![id],
            |r| r.get(0),
        )?;
        let captures: i64 = self.conn.query_row(
            "SELECT count(*) FROM notification_captures WHERE raw_input_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if events == 0 && notifs == 0 && captures == 0 {
            return Ok(String::new());
        }
        let mut parts = Vec::new();
        if events > 0 {
            parts.push(format!("{events} 条日程"));
        }
        if notifs > 0 {
            parts.push(format!("{notifs} 条提醒"));
        }
        if captures > 0 {
            parts.push(format!("{captures} 条通知捕获"));
        }
        Ok(format!("，连同由它派生的 {}", parts.join("、")))
    }

    /// One-line summary of a ledger row, for confirmation previews.
    pub fn memory_summary(&self, layer: MemoryLayer, id: i64) -> Result<Option<String>> {
        let (table, col) = match layer {
            MemoryLayer::RawInput => ("raw_inputs", "text"),
            MemoryLayer::Event => ("events", "title"),
            MemoryLayer::Behavior => ("behavior_log", "content"),
            MemoryLayer::Suggestion => ("suggestions", "text"),
            MemoryLayer::Fact => ("memory_facts", "content"),
            MemoryLayer::Routine => ("routines", "title"),
            MemoryLayer::NotificationCapture => ("notification_captures", "title"),
            // No single human-readable column; the layer label carries enough.
            MemoryLayer::Notification | MemoryLayer::Wearable => return Ok(None),
        };
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {col} FROM {table} WHERE id = ?1"),
                params![id],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    pub fn delete_memory(&self, layer: MemoryLayer, id: i64) -> Result<()> {
        match layer {
            MemoryLayer::Behavior => self.delete_behavior(id)?,
            MemoryLayer::Suggestion => self.delete_suggestion(id)?,
            MemoryLayer::Wearable => self.delete_health_sample(id)?,
            MemoryLayer::Fact => self.delete_fact(id)?,
            MemoryLayer::Routine => self.delete_routine(id)?,
            MemoryLayer::NotificationCapture => self.delete_notification_capture(id)?,
            MemoryLayer::Notification => {
                let n = self
                    .conn
                    .execute("DELETE FROM notifications WHERE id = ?1", params![id])?;
                if n == 0 {
                    return Err(CoreError::NotFound(format!("notification#{id}")));
                }
            }
            MemoryLayer::Event => self.delete_event(id)?,
            MemoryLayer::RawInput => self.delete_raw_input_cascade(id)?,
        }
        Ok(())
    }

    // ---- sync (§3.8) --------------------------------------------------------

    /// This device's stable sync identity.
    pub fn device_id(&self) -> Result<String> {
        self.get_meta("device_id")?
            .ok_or_else(|| CoreError::Invalid("store missing device_id".into()))
    }

    /// Sync bookkeeping (pushed/pulled cursors). Not captured by triggers.
    /// The database's own clock, as the sync triggers see it.
    ///
    /// solum-core does not read the system time (callers inject `now`), but an
    /// audit line has to be stamped with when it *actually* happened, not with
    /// a simulated clock a debug session may have injected. Reading it through
    /// SQLite keeps that distinction explicit.
    pub fn wall_clock(&self) -> Result<NaiveDateTime> {
        let raw: String =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%S', 'now')", [], |r| {
                    r.get(0)
                })?;
        parse_ts(&raw)
    }

    pub fn sync_state(&self, key: &str) -> Result<Option<String>> {
        self.get_meta(key)
    }

    /// Drop a sync-state marker. Used for sticky warnings the user resolves
    /// out-of-band (see `HISTORY_GAP_KEY`).
    pub fn clear_sync_state(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn set_sync_state(&self, key: &str, value: &str) -> Result<()> {
        self.set_meta(key, value)
    }

    /// Locally-originated ops with oplog id greater than `after`, oldest
    /// first. Returns `(oplog_id, op)` so the caller can advance its cursor.
    pub fn local_ops_after(&self, after: i64) -> Result<Vec<(i64, crate::sync::SyncOp)>> {
        self.local_ops_after_limited(after, i64::MAX)
    }

    /// The bounded variant used by the sync driver. Keeping each upload small
    /// prevents a long-lived device from constructing a blob the relay must
    /// reject, while preserving oplog order and retry safety.
    pub fn local_ops_after_limited(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<Vec<(i64, crate::sync::SyncOp)>> {
        let device = self.device_id()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, tbl, guid, op, payload, hlc, origin FROM sync_oplog
             WHERE origin = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![device, after, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                crate::sync::SyncOp {
                    tbl: r.get(1)?,
                    guid: r.get(2)?,
                    op: r.get(3)?,
                    payload: r.get(4)?,
                    hlc: r.get(5)?,
                    origin: r.get(6)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The newest (hlc, origin) recorded for a row, across local and already
    /// merged remote ops — the LWW comparison point.
    fn last_op_stamp(&self, tbl: &str, guid: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT hlc, origin FROM sync_oplog WHERE tbl = ?1 AND guid = ?2
                 ORDER BY hlc DESC, origin DESC LIMIT 1",
                params![tbl, guid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?)
    }

    fn raw_input_is_local_only(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT local_only FROM raw_inputs WHERE id = ?1",
                params![id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            != 0)
    }

    fn event_is_local_only(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT local_only FROM events WHERE id = ?1",
                params![id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            != 0)
    }

    /// Merge a batch of remote ops: last-write-wins per row on (hlc, origin),
    /// idempotent (re-delivery is skipped), echo-free (capture triggers are
    /// suspended via the `sync_applying` meta flag). Applied ops are recorded
    /// in the oplog with their original stamp so future comparisons see them.
    /// Returns (applied, skipped).
    pub fn apply_remote_ops(&self, ops: &[crate::sync::SyncOp]) -> Result<MergeCounts> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('sync_applying', '1')
             ON CONFLICT(key) DO UPDATE SET value = '1'",
            [],
        )?;
        // Ceiling for incoming stamps, computed once per batch.
        //
        // A peer with a badly wrong clock — or a hostile one — can stamp an op
        // years ahead. Under plain LWW that op then outranks every correction
        // this device will ever make to that row: the record is frozen, and
        // nothing surfaces why. Anything beyond a day of skew is not clock
        // drift, so it is quarantined rather than applied — held, visible, and
        // not lost, exactly like an op from a build we cannot understand.
        //
        // Note this also bounds absorption: the local clock only ever advances
        // to stamps that passed this gate (`trg_hlc_advance` fires on the
        // oplog insert below), so a rejected op cannot drag this device's own
        // clock into the future with it.
        let horizon: String = tx.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%f', 'now', '+1 day')",
            [],
            |r| r.get(0),
        )?;

        let mut counts = MergeCounts::default();
        for op in ops {
            if op.hlc > horizon {
                self.hold_in_quarantine(
                    op,
                    "时间戳远超本机时钟（超过 1 天），疑似对端时钟错误或被伪造；已暂存不合并",
                )?;
                counts.quarantined += 1;
                continue;
            }
            // LWW gate: strictly newer (hlc, origin) wins; ties and older lose.
            if let Some((hlc, origin)) = self.last_op_stamp(&op.tbl, &op.guid)? {
                if (op.hlc.as_str(), op.origin.as_str()) <= (hlc.as_str(), origin.as_str()) {
                    counts.skipped += 1;
                    continue;
                }
            }
            // Per-op savepoint: an op we end up quarantining must leave no
            // partial mutation behind, and several arms interleave reads and
            // writes before they can discover the payload is unusable.
            self.conn.execute_batch("SAVEPOINT one_op")?;
            match self.apply_one(op) {
                Ok(true) => {
                    self.conn.execute(
                        "INSERT INTO sync_oplog(tbl, guid, op, payload, hlc, origin)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![op.tbl, op.guid, op.op, op.payload, op.hlc, op.origin],
                    )?;
                    self.conn.execute_batch("RELEASE one_op")?;
                    counts.applied += 1;
                }
                Ok(false) => {
                    self.conn.execute_batch("RELEASE one_op")?;
                    counts.skipped += 1;
                }
                // Data this build cannot interpret: a table a newer peer
                // added, or a payload missing a field we require. Hold it
                // instead of failing the batch — returning Err here used to
                // roll back the whole merge *and* leave the pull cursor
                // unadvanced, so the same blob was re-fetched and re-failed
                // forever while push kept working (device looks alive, never
                // receives again). Storage/IO errors still propagate.
                Err(CoreError::Invalid(reason)) => {
                    self.conn
                        .execute_batch("ROLLBACK TO one_op; RELEASE one_op")?;
                    self.hold_in_quarantine(op, &reason)?;
                    counts.quarantined += 1;
                }
                Err(e) => return Err(e),
            }
        }
        tx.execute("DELETE FROM meta WHERE key = 'sync_applying'", [])?;
        tx.commit()?;
        Ok(counts)
    }

    /// Count capture losses **idempotently** and bump the matching counter, in
    /// one transaction.
    ///
    /// `receipts` are the on-disk names of the things being counted. Names
    /// already seen contribute nothing, so a crash between this commit and the
    /// caller deleting those files cannot inflate the total on the next run.
    /// Returns how many were newly counted.
    pub fn record_capture_loss(&self, counter_key: &str, receipts: &[String]) -> Result<usize> {
        if receipts.is_empty() {
            return Ok(0);
        }
        self.with_transaction(|s| {
            let mut fresh = 0usize;
            for r in receipts {
                let n = s.conn.execute(
                    "INSERT OR IGNORE INTO capture_loss_receipts(receipt, recorded_at)
                     VALUES (?1, strftime('%Y-%m-%dT%H:%M:%f', 'now'))",
                    params![r],
                )?;
                fresh += n;
            }
            if fresh > 0 {
                let current: i64 = s
                    .get_meta(counter_key)?
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                s.set_meta(counter_key, &(current + fresh as i64).to_string())?;
            }
            Ok(fresh)
        })
    }

    /// Drop the receipts naming things that are **confirmed gone from disk**.
    ///
    /// A receipt is only meaningful while the file it names can still be
    /// re-scanned, so the file's disappearance — not its age — is what retires
    /// it. Age-based pruning was wrong: the crash window it guards has no upper
    /// bound. A marker whose deletion keeps failing (read-only volume, a lock, a
    /// permission change) sits on disk indefinitely, so at any cutoff the
    /// receipt expires while the marker survives, the next scan counts it again,
    /// and the total drifts above the truth — which is exactly what the receipt
    /// exists to prevent, and what "at least N" in the UI forbids.
    ///
    /// Callers must therefore delete the file first and pass only the names
    /// whose removal succeeded. Crashing between the two leaks a row, which
    /// costs a few bytes and distorts nothing.
    pub fn release_capture_loss_receipts(&self, receipts: &[String]) -> Result<usize> {
        if receipts.is_empty() {
            return Ok(0);
        }
        self.with_transaction(|s| {
            let mut gone = 0usize;
            for r in receipts {
                gone += s.conn.execute(
                    "DELETE FROM capture_loss_receipts WHERE receipt = ?1",
                    params![r],
                )?;
            }
            Ok(gone)
        })
    }

    /// Park a pulled blob that could not be opened at all, so the pull cursor
    /// can advance past it. Returns nothing useful — the point is that the
    /// ciphertext survives for later recovery and the count is visible.
    pub fn hold_bad_blob(
        &self,
        seq: i64,
        device: &str,
        blob_b64: &str,
        reason: &str,
    ) -> Result<()> {
        // Keeping the ciphertext is the point (a build with the right key can
        // still recover it), but not at unbounded cost: past a size we keep the
        // record and drop the payload, so the *fact* survives even when the
        // bytes do not.
        let kept = if blob_b64.len() > MAX_BAD_BLOB_BYTES {
            ""
        } else {
            blob_b64
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO sync_bad_blobs(seq, device, blob_b64, reason, held_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%f', 'now'))",
            params![seq, device, kept, reason],
        )?;
        let held: i64 = self
            .conn
            .query_row("SELECT count(*) FROM sync_bad_blobs", [], |r| r.get(0))?;
        if held > MAX_BAD_BLOBS {
            let over = held - MAX_BAD_BLOBS;
            self.conn.execute(
                "DELETE FROM sync_bad_blobs WHERE seq IN
                 (SELECT seq FROM sync_bad_blobs ORDER BY seq LIMIT ?1)",
                params![over],
            )?;
            let dropped: i64 = self
                .get_meta(BAD_BLOBS_DROPPED_KEY)?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            self.set_meta(BAD_BLOBS_DROPPED_KEY, &(dropped + over).to_string())?;
        }
        Ok(())
    }

    /// `(still held, dropped for overflow)`. Non-zero dropped means a peer has
    /// been emitting unreadable blobs for a long time — almost always a
    /// mismatched `SOLUM_SYNC_KEY`, not corruption.
    pub fn bad_blob_stats(&self) -> Result<(i64, i64)> {
        let held: i64 = self
            .conn
            .query_row("SELECT count(*) FROM sync_bad_blobs", [], |r| r.get(0))?;
        let dropped = self
            .get_meta(BAD_BLOBS_DROPPED_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok((held, dropped))
    }

    /// How many pulled blobs this device has had to skip. Non-zero means a peer
    /// is sending something we cannot read — usually a mismatched sync key.
    pub fn bad_blob_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM sync_bad_blobs", [], |r| r.get(0))?)
    }

    /// Park an op this build cannot apply. `INSERT OR IGNORE`: the same op can
    /// legitimately arrive twice (peer resend, cursor rewind), and holding it
    /// twice would replay it twice.
    fn hold_in_quarantine(&self, op: &crate::sync::SyncOp, reason: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO sync_quarantine(tbl, guid, op, payload, hlc, origin, reason, held_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%f', 'now'))",
            params![op.tbl, op.guid, op.op, op.payload, op.hlc, op.origin, reason],
        )?;
        let held: i64 = self
            .conn
            .query_row("SELECT count(*) FROM sync_quarantine", [], |r| r.get(0))?;
        if held > MAX_QUARANTINE_OPS {
            let over = held - MAX_QUARANTINE_OPS;
            self.conn.execute(
                "DELETE FROM sync_quarantine WHERE id IN
                 (SELECT id FROM sync_quarantine ORDER BY id LIMIT ?1)",
                params![over],
            )?;
            let dropped: i64 = self
                .get_meta(QUARANTINE_DROPPED_KEY)?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            self.set_meta(QUARANTINE_DROPPED_KEY, &(dropped + over).to_string())?;
        }
        Ok(())
    }

    /// Ops held for a build that did not understand them. Returns
    /// `(still held, dropped for overflow)` — for CLI/UI display.
    pub fn sync_quarantine_stats(&self) -> Result<(i64, i64)> {
        let held = self
            .conn
            .query_row("SELECT count(*) FROM sync_quarantine", [], |r| r.get(0))?;
        let dropped = self
            .get_meta(QUARANTINE_DROPPED_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok((held, dropped))
    }

    fn table_is_known(tbl: &str) -> bool {
        tbl == "meta" || SYNCED_TABLES.contains(&tbl)
    }

    /// Replay ops parked by an older build. Called at the end of [`migrate`]:
    /// a migration that adds a synced table is exactly the moment held ops
    /// become interpretable. Ordered by `(hlc, origin)` so LWW resolves the
    /// way it would have on arrival.
    fn replay_sync_quarantine(&self) -> Result<usize> {
        let held: Vec<(i64, crate::sync::SyncOp)> = self
            .conn
            .prepare(
                "SELECT id, tbl, guid, op, payload, hlc, origin FROM sync_quarantine
                 ORDER BY hlc, origin, id",
            )?
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    crate::sync::SyncOp {
                        tbl: r.get(1)?,
                        guid: r.get(2)?,
                        op: r.get(3)?,
                        payload: r.get(4)?,
                        hlc: r.get(5)?,
                        origin: r.get(6)?,
                    },
                ))
            })?
            .collect::<std::result::Result<_, _>>()?;

        let mut replayed = 0usize;
        for (id, op) in held {
            // Still unknown: a later version may yet learn this table.
            if !Self::table_is_known(&op.tbl) {
                continue;
            }
            replayed += self.apply_remote_ops(std::slice::from_ref(&op))?.applied;
            // The table is known now, so the verdict is final — applied,
            // superseded by LWW, or permanently unapplicable. None of those
            // improve by holding the row any longer.
            self.conn
                .execute("DELETE FROM sync_quarantine WHERE id = ?1", params![id])?;
        }
        Ok(replayed)
    }

    /// Apply a single remote op. Returns false when the op is skipped (orphan
    /// FK, duplicate content, unresolvable pointer) — that is data-dependent,
    /// not an error.
    fn apply_one(&self, op: &crate::sync::SyncOp) -> Result<bool> {
        if op.op == "delete" {
            return self.apply_delete(op);
        }
        let payload: serde_json::Value = match &op.payload {
            Some(p) => serde_json::from_str(p)?,
            None => return Ok(false),
        };
        let s = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let need = |k: &str| {
            s(k).ok_or_else(|| {
                CoreError::Invalid(format!("sync op {}:{} missing {k}", op.tbl, op.guid))
            })
        };
        match op.tbl.as_str() {
            "raw_inputs" => {
                let text = need("text")?;
                // Prefer the originating device's capture-time decision so one
                // record has the same cloud-LLM eligibility everywhere. Older
                // payloads predate the field; fall back to the conservative
                // notification-prefix guess rather than defaulting to allowed.
                let local_only = payload
                    .get("local_only")
                    .and_then(serde_json::Value::as_i64)
                    .map_or_else(|| text.starts_with("[通知·"), |v| v != 0);
                self.conn.execute(
                    "INSERT INTO raw_inputs(text, intent, created_at, local_only, guid) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(guid) DO UPDATE SET text=excluded.text, intent=excluded.intent,
                       created_at=excluded.created_at, local_only=excluded.local_only",
                    params![
                        text,
                        need("intent")?,
                        need("created_at")?,
                        local_only as i64,
                        op.guid
                    ],
                )?;
            }
            "events" => {
                let raw_id: Option<i64> = match s("raw_input_guid") {
                    Some(g) => self.id_by_guid("raw_inputs", &g)?,
                    None => None,
                };
                let routine_id: Option<i64> = match s("routine_guid") {
                    Some(g) => self.id_by_guid("routines", &g)?,
                    None => None,
                };
                // Same rule as raw_inputs: the transmitted stamp wins, and a
                // payload without it falls back to the parent raw input.
                let local_only = match payload
                    .get("local_only")
                    .and_then(serde_json::Value::as_i64)
                {
                    Some(v) => v != 0,
                    None => raw_id
                        .map(|id| self.raw_input_is_local_only(id))
                        .transpose()?
                        .unwrap_or(false),
                };
                self.conn.execute(
                    "INSERT INTO events(title, kind, start, end, location, people_json, raw_input_id, routine_id, created_at, local_only, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(guid) DO UPDATE SET title=excluded.title, kind=excluded.kind,
                       start=excluded.start, end=excluded.end, location=excluded.location,
                       people_json=excluded.people_json, raw_input_id=excluded.raw_input_id,
                       routine_id=excluded.routine_id, created_at=excluded.created_at,
                       local_only=excluded.local_only",
                    params![
                        need("title")?,
                        need("kind")?,
                        need("start")?,
                        s("end"),
                        s("location"),
                        need("people_json")?,
                        raw_id,
                        routine_id,
                        need("created_at")?,
                        local_only as i64,
                        op.guid
                    ],
                )?;
            }
            "notifications" => {
                // Orphan (event deleted or not yet arrived): skip. The event's
                // own op precedes its notifications in origin oplog order, so
                // in-order delivery only orphans genuinely deleted parents.
                let Some(event_id) = s("event_guid")
                    .map(|g| self.id_by_guid("events", &g))
                    .transpose()?
                    .flatten()
                else {
                    return Ok(false);
                };
                let local_only = self.event_is_local_only(event_id)?;
                self.conn.execute(
                    "INSERT INTO notifications(event_id, fire_at, lead_label, channels_json, status, created_at, fired_at, local_only, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(guid) DO UPDATE SET event_id=excluded.event_id, fire_at=excluded.fire_at,
                       lead_label=excluded.lead_label, channels_json=excluded.channels_json,
                       status=excluded.status, created_at=excluded.created_at, fired_at=excluded.fired_at,
                       local_only=excluded.local_only",
                    params![
                        event_id,
                        need("fire_at")?,
                        need("lead_label")?,
                        need("channels_json")?,
                        need("status")?,
                        need("created_at")?,
                        s("fired_at"),
                        local_only as i64,
                        op.guid
                    ],
                )?;
            }
            "behavior_log" => {
                self.conn.execute(
                    "INSERT INTO behavior_log(ts, kind, content, source, guid) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(guid) DO UPDATE SET ts=excluded.ts, kind=excluded.kind,
                       content=excluded.content, source=excluded.source",
                    params![need("ts")?, need("kind")?, need("content")?, s("source"), op.guid],
                )?;
            }
            "suggestions" => {
                // Both devices can generate the same suggestion independently
                // (same dedup_key, different guid) — treat that as a skip.
                let r = self.conn.execute(
                    "INSERT INTO suggestions(created_at, kind, text, dedup_key, source, status, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(guid) DO UPDATE SET created_at=excluded.created_at, kind=excluded.kind,
                       text=excluded.text, dedup_key=excluded.dedup_key, source=excluded.source,
                       status=excluded.status",
                    params![
                        need("created_at")?,
                        need("kind")?,
                        need("text")?,
                        need("dedup_key")?,
                        s("source"),
                        need("status")?,
                        op.guid
                    ],
                );
                if let Err(rusqlite::Error::SqliteFailure(e, _)) = &r {
                    if e.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Ok(false);
                    }
                }
                r?;
            }
            "persona_versions" => {
                // Rows are immutable history: same guid → already have it.
                if self.id_by_guid("persona_versions", &op.guid)?.is_some() {
                    return Ok(false);
                }
                // Version numbers are device-local; renumber on collision and
                // keep the embedded JSON consistent.
                let wanted = payload.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
                let taken: bool = self
                    .conn
                    .query_row(
                        "SELECT 1 FROM persona_versions WHERE version = ?1",
                        params![wanted],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                let version = if wanted > 0 && !taken {
                    wanted
                } else {
                    self.conn.query_row(
                        "SELECT COALESCE(MAX(version), 0) + 1 FROM persona_versions",
                        [],
                        |r| r.get(0),
                    )?
                };
                let mut profile =
                    crate::persona::PersonaProfile::from_json(&need("profile_json")?)?;
                profile.version = version;
                self.conn.execute(
                    "INSERT INTO persona_versions(version, created_at, profile_json, guid) VALUES (?1, ?2, ?3, ?4)",
                    params![version, need("created_at")?, profile.to_json()?, op.guid],
                )?;
            }
            "health_samples" => {
                // Same "independently produced, same content, different
                // guid" tolerance as suggestions: skip on dedup_key clash.
                let r = self.conn.execute(
                    "INSERT INTO health_samples(kind, start, end, value, source, created_at, dedup_key, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(guid) DO UPDATE SET kind=excluded.kind, start=excluded.start,
                       end=excluded.end, value=excluded.value, source=excluded.source,
                       created_at=excluded.created_at, dedup_key=excluded.dedup_key",
                    params![
                        need("kind")?,
                        need("start")?,
                        need("end")?,
                        payload.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        need("source")?,
                        need("created_at")?,
                        need("dedup_key")?,
                        op.guid
                    ],
                );
                if let Err(rusqlite::Error::SqliteFailure(e, _)) = &r {
                    if e.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Ok(false);
                    }
                }
                r?;
            }
            "memory_facts" => {
                // Same tolerance as suggestions: the same fact written
                // independently on two devices (UNIQUE content) is a skip.
                let r = self.conn.execute(
                    "INSERT INTO memory_facts(content, source, created_at, last_used_at, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(guid) DO UPDATE SET content=excluded.content,
                       source=excluded.source, created_at=excluded.created_at,
                       last_used_at=excluded.last_used_at",
                    params![
                        need("content")?,
                        need("source")?,
                        need("created_at")?,
                        s("last_used_at"),
                        op.guid
                    ],
                );
                if let Err(rusqlite::Error::SqliteFailure(e, _)) = &r {
                    if e.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Ok(false);
                    }
                }
                r?;
            }
            "routines" => {
                let active = payload.get("active").and_then(|v| v.as_i64()).unwrap_or(1);
                let title = need("title")?.to_string();
                let time_of_day = need("time_of_day")?.to_string();
                // A remote configuration edit must retract this device's
                // local pending projections before the next ticker rebuilds
                // them. Fired occurrences remain historical records.
                let previous: Option<(String, String, i64)> = self
                    .conn
                    .query_row(
                        "SELECT title, time_of_day, active FROM routines WHERE guid = ?1",
                        params![op.guid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let rebuild_pending = active == 0
                    || previous.is_some_and(|(old_title, old_time, old_active)| {
                        old_active == 0 || old_title != title || old_time != time_of_day
                    });
                self.conn.execute(
                    "INSERT INTO routines(title, time_of_day, source, active, created_at, scheduled_until, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(guid) DO UPDATE SET title=excluded.title,
                       time_of_day=excluded.time_of_day, source=excluded.source,
                       active=excluded.active, created_at=excluded.created_at,
                       scheduled_until=excluded.scheduled_until",
                    params![
                        title,
                        time_of_day,
                        s("source"),
                        active,
                        need("created_at")?,
                        s("scheduled_until"),
                        op.guid
                    ],
                )?;
                if rebuild_pending {
                    if let Some(routine_id) = self.id_by_guid("routines", &op.guid)? {
                        self.remove_pending_routine_occurrences(routine_id)?;
                    }
                }
            }
            "soulous_facts" => {
                let source = need("source")?;
                if source != SOULOUS_SOURCE {
                    return Err(CoreError::Invalid(
                        "refusing a non-soulous row in soulous_facts sync".into(),
                    ));
                }
                self.conn.execute(
                    "INSERT INTO soulous_facts(external_id, kind, title, occurs_at, ends_at, payload_json, source, imported_at, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(guid) DO UPDATE SET external_id=excluded.external_id,
                       kind=excluded.kind, title=excluded.title, occurs_at=excluded.occurs_at,
                       ends_at=excluded.ends_at, payload_json=excluded.payload_json,
                       source=excluded.source, imported_at=excluded.imported_at",
                    params![
                        need("external_id")?,
                        need("kind")?,
                        need("title")?,
                        s("occurs_at"),
                        s("ends_at"),
                        need("payload_json")?,
                        source,
                        need("imported_at")?,
                        op.guid
                    ],
                )?;
            }
            "widget_defs" => {
                self.conn.execute(
                    "INSERT INTO widget_defs(name, icon, list_sort_by, table_sort_by,
                       created_at, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(guid) DO UPDATE SET name=excluded.name, icon=excluded.icon,
                       list_sort_by=excluded.list_sort_by, table_sort_by=excluded.table_sort_by,
                       created_at=excluded.created_at",
                    params![
                        need("name")?,
                        need("icon")?,
                        s("list_sort_by"),
                        s("table_sort_by"),
                        need("created_at")?,
                        op.guid
                    ],
                )?;
            }
            "widget_fields" => {
                // Orphan: the parent definition was deleted, or has not
                // arrived yet. Same rule as notifications — skip rather than
                // invent a parent.
                let Some(widget_id) = self.id_by_guid("widget_defs", &need("widget_guid")?)? else {
                    return Ok(false);
                };
                let name = need("name")?;
                // The one case a grow-only set does not resolve by itself: two
                // devices independently added the same field name to the same
                // widget, so the union contains a duplicate. Keep the smaller
                // guid — deterministic on every device without consulting the
                // clock — and log the loser instead of dropping it silently
                // (the quarantine round's rule: losing data is survivable,
                // losing it quietly is not).
                let existing: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT guid FROM widget_fields WHERE widget_id = ?1 AND name = ?2",
                        params![widget_id, name],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(existing_guid) = existing {
                    if existing_guid != op.guid {
                        if existing_guid <= op.guid {
                            self.append_widget_schema_rejection(
                                op.payload.as_deref().unwrap_or("{}"),
                                &format!(
                                    "同名字段 {name:?} 在另一台设备上并发创建，保留 guid 较小的一份"
                                ),
                                parse_ts(&need("created_at")?).unwrap_or_default(),
                            )?;
                            return Ok(false);
                        }
                        // The arriving row wins; retire the local duplicate so
                        // the unique index does not reject the upsert.
                        self.conn.execute(
                            "DELETE FROM widget_fields WHERE widget_id = ?1 AND name = ?2",
                            params![widget_id, name],
                        )?;
                    }
                }
                self.conn.execute(
                    "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                       options_json, ord, form_ord, list_ord, table_ord, stat_ord,
                       created_at, guid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(guid) DO UPDATE SET widget_id=excluded.widget_id,
                       name=excluded.name, label=excluded.label, field_type=excluded.field_type,
                       required=excluded.required, options_json=excluded.options_json,
                       ord=excluded.ord, form_ord=excluded.form_ord, list_ord=excluded.list_ord,
                       table_ord=excluded.table_ord, stat_ord=excluded.stat_ord,
                       created_at=excluded.created_at",
                    params![
                        widget_id,
                        name,
                        need("label")?,
                        need("field_type")?,
                        payload
                            .get("required")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        need("options_json")?,
                        payload
                            .get("ord")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        payload.get("form_ord").and_then(serde_json::Value::as_i64),
                        payload.get("list_ord").and_then(serde_json::Value::as_i64),
                        payload.get("table_ord").and_then(serde_json::Value::as_i64),
                        payload.get("stat_ord").and_then(serde_json::Value::as_i64),
                        need("created_at")?,
                        op.guid
                    ],
                )?;
            }
            "widget_records" => {
                let Some(widget_id) = self.id_by_guid("widget_defs", &need("widget_guid")?)? else {
                    return Ok(false);
                };
                self.conn.execute(
                    "INSERT INTO widget_records(widget_id, data_json, created_at, guid)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(guid) DO UPDATE SET widget_id=excluded.widget_id,
                       data_json=excluded.data_json, created_at=excluded.created_at",
                    params![widget_id, need("data_json")?, need("created_at")?, op.guid],
                )?;
            }
            "meta" => {
                let key = need("key")?;
                let mut value = need("value")?;
                if key == "persona_active" {
                    // Pointer travels as a version-row guid; resolve locally.
                    let Some(version) = self
                        .conn
                        .query_row(
                            "SELECT version FROM persona_versions WHERE guid = ?1",
                            params![value],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                    else {
                        return Ok(false);
                    };
                    value = version.to_string();
                }
                self.set_meta(&key, &value)?;
            }
            other => {
                return Err(CoreError::Invalid(format!("unknown sync table {other}")));
            }
        }
        Ok(true)
    }

    fn apply_delete(&self, op: &crate::sync::SyncOp) -> Result<bool> {
        if op.tbl == "meta" {
            let key = op.guid.strip_prefix("meta:").unwrap_or(&op.guid);
            self.conn
                .execute("DELETE FROM meta WHERE key = ?1", params![key])?;
            return Ok(true);
        }
        if !SYNCED_TABLES.contains(&op.tbl.as_str()) {
            return Err(CoreError::Invalid(format!("unknown sync table {}", op.tbl)));
        }
        // Deleting an event locally cascades to its notifications; the origin
        // device also emitted their tombstones, but mirroring the cascade here
        // keeps us consistent even if those arrive later.
        if op.tbl == "events" {
            if let Some(id) = self.id_by_guid("events", &op.guid)? {
                self.conn
                    .execute("DELETE FROM notifications WHERE event_id = ?1", params![id])?;
            }
        }
        // Another device may have materialized the same routine occurrence
        // independently, giving it a different event guid. Removing the
        // routine must therefore retract every pending local occurrence too.
        if op.tbl == "routines" {
            if let Some(id) = self.id_by_guid("routines", &op.guid)? {
                self.remove_pending_routine_occurrences(id)?;
            }
        }
        self.conn.execute(
            &format!("DELETE FROM {} WHERE guid = ?1", op.tbl),
            params![op.guid],
        )?;
        Ok(true)
    }

    /// Recall candidates (§3.10). The active privacy setting immediately
    /// excludes notification-derived text when disabled; even when enabled,
    /// legacy `local_only` rows remain excluded so turning the setting on
    /// never backfills historical captures into cloud context.
    pub fn list_recall_events(&self, notif_cloud_enabled: bool) -> Result<Vec<Event>> {
        let sql = if notif_cloud_enabled {
            "SELECT e.* FROM events e
             WHERE e.local_only = 0
             ORDER BY e.start ASC"
        } else {
            "SELECT e.* FROM events e
             LEFT JOIN raw_inputs r ON r.id = e.raw_input_id
             WHERE e.local_only = 0
               AND (r.id IS NULL OR r.text NOT LIKE '[通知·%')
             ORDER BY e.start ASC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], Self::row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn id_by_guid(&self, tbl: &str, guid: &str) -> Result<Option<i64>> {
        let pk = if tbl == "persona_versions" {
            "version"
        } else {
            "id"
        };
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {pk} FROM {tbl} WHERE guid = ?1"),
                params![guid],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// Which layers `ledger_purge` may touch, and their timestamp column. The
/// cascade-heavy layers (raw_input/event/notification) are deliberately *not*
/// purgeable in v1 — batch-deleting schedule data has knock-on effects the
/// preview can't honestly summarize in one number.
fn purge_target(layer: MemoryLayer) -> Result<(&'static str, &'static str)> {
    Ok(match layer {
        MemoryLayer::Behavior => ("behavior_log", "ts"),
        MemoryLayer::Suggestion => ("suggestions", "created_at"),
        MemoryLayer::Wearable => ("health_samples", "start"),
        other => {
            return Err(CoreError::Invalid(format!(
                "层 {} 不支持批量清理（请在台账逐条删除）",
                other.as_str()
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Decision;
    use crate::model::{EventKind, RiskLevel};
    use chrono::NaiveDate;

    /// A private scratch directory; avoids pulling in a dev-dependency just to
    /// exercise three filesystem moves.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solum-adopt-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The P1 this guards: data committed but still living in the WAL must
    /// survive adoption. The old file-by-file rename could move the main
    /// database and leave the WAL behind, and because the next launch then saw
    /// a database already at the new path it never retried — the committed
    /// rows were simply gone, silently and permanently.
    #[test]
    fn adopting_a_pre_rename_database_keeps_data_still_in_the_wal() {
        let dir = scratch("moves");
        let legacy = dir.join("pa.sqlite");
        let current = dir.join("solum.sqlite");

        // A real WAL-mode database with a committed row that has *not* been
        // checkpointed into the main file yet.
        {
            let store = Store::open(legacy.to_str().unwrap()).unwrap();
            store
                .insert_event(
                    &Event::new(
                        "改名前的会议",
                        crate::model::EventKind::Meeting,
                        dt(2026, 7, 9, 15, 0),
                        "src",
                        dt(2026, 7, 7, 10, 0),
                    ),
                    None,
                )
                .unwrap();
            assert!(
                dir.join("pa.sqlite-wal").exists(),
                "precondition: the commit is in the WAL"
            );
        }

        assert!(adopt_legacy_db(&legacy, &current).unwrap());
        assert!(!legacy.exists(), "legacy database is moved, not copied");

        // The adopted database must still contain the row.
        let adopted = Store::open(current.to_str().unwrap()).unwrap();
        let titles: Vec<String> = adopted
            .list_events()
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(titles, vec!["改名前的会议".to_string()]);
        drop(adopted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dangerous case: adopting must never clobber a database the user has
    /// already built up under the new name.
    #[test]
    fn adopting_never_overwrites_an_existing_database() {
        let dir = scratch("keeps");
        let legacy = dir.join("pa.sqlite");
        let current = dir.join("solum.sqlite");
        std::fs::write(&legacy, b"old").unwrap();
        std::fs::write(&current, b"new").unwrap();

        assert!(!adopt_legacy_db(&legacy, &current).unwrap());

        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        assert_eq!(
            std::fs::read(&legacy).unwrap(),
            b"old",
            "the legacy file stays put for the user to inspect"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adopting_is_a_no_op_on_a_clean_install() {
        let dir = scratch("clean");
        assert!(!adopt_legacy_db(&dir.join("pa.sqlite"), &dir.join("solum.sqlite")).unwrap());
        assert!(!dir.join("solum.sqlite").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    /// P2 regression: a peer stamping ops years ahead used to win LWW forever,
    /// freezing that row against every later local correction. Such an op is
    /// now quarantined — held and visible, not applied and not lost.
    #[test]
    fn an_op_stamped_far_in_the_future_is_quarantined_not_applied() {
        let s = Store::open_in_memory().unwrap();
        let counts = s
            .apply_remote_ops(&[crate::sync::SyncOp {
                tbl: "events".into(),
                guid: "guid-future".into(),
                op: "upsert".into(),
                payload: Some(
                    serde_json::json!({
                        "guid": "guid-future",
                        "title": "来自未来的会议",
                        "kind": "meeting",
                        "start": "2026-07-09T15:00:00",
                        "created_at": "2026-07-07T10:00:00",
                    })
                    .to_string(),
                ),
                hlc: "2099-01-01T00:00:00.000".into(),
                origin: "hostile-peer".into(),
            }])
            .unwrap();

        assert_eq!(counts.applied, 0);
        assert_eq!(counts.quarantined, 1);
        assert!(s.list_events().unwrap().is_empty());
        assert_eq!(s.sync_quarantine_stats().unwrap().0, 1, "held, not dropped");

        // …and the hostile stamp must not have dragged this device's own
        // logical clock into the future with it.
        let hlc_last = s.get_meta("hlc_last").unwrap().unwrap_or_default();
        assert!(
            hlc_last.as_str() < "2099",
            "local clock absorbed the forged stamp: {hlc_last}"
        );
    }

    /// Follow-up review P2: `MAX(wall, hlc_last)` is only *non-decreasing*.
    /// Two updates to the same row inside one millisecond got the identical
    /// `(hlc, origin)`, and `apply_remote_ops` requires strictly newer — so a
    /// peer applied the first and silently dropped the second, diverging
    /// forever. Empirically `strftime('now')` twice in one statement always
    /// returns the same value, so this was certain, not rare.
    #[test]
    fn rapid_updates_to_one_row_get_strictly_increasing_stamps() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_event(&sample_event(), None).unwrap();
        // Hammer the same row; all of these land within a millisecond or two.
        for h in 0..20 {
            s.update_event_times(id, dt(2026, 7, 7, h % 24, 0), None)
                .unwrap();
        }
        let mut stamps = s
            .conn
            .prepare("SELECT hlc FROM sync_oplog WHERE tbl = 'events' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(stamps.len() >= 20, "expected one op per update");
        let total = stamps.len();
        stamps.dedup();
        assert_eq!(
            stamps.len(),
            total,
            "duplicate (hlc, origin) means a peer would silently drop an update"
        );
    }

    /// Follow-up review P2: a peer with a mismatched key can emit unreadable
    /// blobs indefinitely. Keeping every one of them would let it fill this
    /// device's disk.
    #[test]
    fn parked_unopenable_blobs_are_bounded_and_the_loss_is_visible() {
        let s = Store::open_in_memory().unwrap();
        for seq in 1..=(MAX_BAD_BLOBS + 25) {
            s.hold_bad_blob(seq, "peer", "AAAA", "wrong key").unwrap();
        }
        let (held, dropped) = s.bad_blob_stats().unwrap();
        assert!(held <= MAX_BAD_BLOBS, "held {held} exceeds the cap");
        assert_eq!(dropped, 25, "the dropped count must be visible, not silent");

        // An oversized payload keeps the record but not the bytes.
        let huge = "A".repeat(MAX_BAD_BLOB_BYTES + 10);
        s.hold_bad_blob(99_999, "peer", &huge, "wrong key").unwrap();
        let kept: String = s
            .conn
            .query_row(
                "SELECT blob_b64 FROM sync_bad_blobs WHERE seq = 99999",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(kept.is_empty(), "oversized ciphertext should not be stored");
    }

    /// P3 regression: `dedup_key` excludes the value on purpose, so a platform
    /// correcting a reading re-sends the same key with a new number. Under
    /// `INSERT OR IGNORE` that correction was dropped and the first number
    /// kept forever — the opposite of what `dedup_key`'s doc comment claims.
    #[test]
    fn a_corrected_health_reading_replaces_the_original() {
        let s = Store::open_in_memory().unwrap();
        let mut sample = crate::wearable::HealthSample::new(
            crate::wearable::HealthMetric::Steps,
            dt(2026, 7, 12, 8, 0),
            dt(2026, 7, 12, 9, 0),
            1000.0,
            "health_connect",
        );
        assert!(s.insert_health_sample_if_new(&sample).unwrap().is_some());

        // Same interval, same source, corrected value.
        sample.value = 1234.0;
        s.insert_health_sample_if_new(&sample).unwrap();

        let stored = s.list_health_samples().unwrap();
        assert_eq!(stored.len(), 1, "a correction must not add a second row");
        assert_eq!(stored[0].value, 1234.0, "the correction should have landed");
    }

    fn sample_event() -> Event {
        let mut ev = Event::new(
            "开会",
            EventKind::Meeting,
            dt(2026, 7, 7, 15, 0),
            "明天下午3点开会",
            dt(2026, 7, 6, 10, 0),
        );
        ev.location = Some("会议室".into());
        ev.people = vec!["张伟".into()];
        ev
    }

    #[test]
    fn migrates_and_versions() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    fn course_draft() -> WidgetDefinitionDraft {
        serde_json::from_value(serde_json::json!({
            "name": "课程记录",
            "icon": "calendar",
            "fields": [
                { "name": "course", "label": "课程", "type": "text", "required": true },
                { "name": "start_time", "label": "开始时刻", "type": "time", "required": true }
            ],
            "views": [
                { "type": "form", "fields": ["course", "start_time"] },
                { "type": "list", "fields": ["start_time", "course"], "sort_by": "start_time" }
            ]
        }))
        .unwrap()
    }

    /// v13 reversed the first slice's "widgets never sync" invariant. The old
    /// assertions here (no guid column, zero triggers) asserted a contract that
    /// no longer holds, so they are replaced rather than kept.
    #[test]
    fn widgets_sync_and_deletion_writes_delete_ops_for_children() {
        let s = Store::open_in_memory().unwrap();
        let definition = s
            .insert_widget_definition(&course_draft(), dt(2026, 7, 19, 10, 0))
            .unwrap();
        s.insert_widget_record(
            definition.id,
            &serde_json::json!({ "course": "数学", "start_time": "09:05" }),
            dt(2026, 7, 19, 10, 1),
        )
        .unwrap();

        for table in ["widget_defs", "widget_fields", "widget_records"] {
            let columns: Vec<String> = s
                .conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert!(
                columns.iter().any(|column| column == "guid"),
                "{table} must carry a guid to sync"
            );
        }
        // The view lists survive the round trip through rows, including the
        // list view's independent field order and its sort field.
        let loaded = s.get_widget_definition(definition.id).unwrap();
        assert_eq!(loaded.schema.fields.len(), 2);
        let list = loaded
            .schema
            .views
            .iter()
            .find(|v| v.view_type == WidgetViewType::List)
            .unwrap();
        assert_eq!(list.fields, vec!["start_time", "course"]);
        assert_eq!(list.sort_by.as_deref(), Some("start_time"));

        // Deleting must emit a delete op per child row. SQLite does not fire
        // triggers for FK cascade deletions, so a cascade-only implementation
        // would leave other devices holding orphans — assert the ops exist.
        let ops_before: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sync_oplog WHERE op = 'delete'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            s.delete_widget_definition(definition.id).unwrap(),
            ("课程记录".into(), 1)
        );
        let deletes: Vec<String> = s
            .conn
            .prepare("SELECT tbl FROM sync_oplog WHERE op = 'delete' ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let deletes = &deletes[ops_before as usize..];
        assert_eq!(deletes.iter().filter(|t| *t == "widget_records").count(), 1);
        assert_eq!(deletes.iter().filter(|t| *t == "widget_fields").count(), 2);
        assert_eq!(deletes.iter().filter(|t| *t == "widget_defs").count(), 1);
    }

    /// A v12 database stored the whole schema in one JSON column. Upgrading
    /// must turn it into rows without losing a field, a view membership, the
    /// per-view order, or the sort field.
    #[test]
    fn v12_schema_json_becomes_rows_without_losing_anything() {
        let s = Store::open_in_memory().unwrap();
        // Rebuild the v12 shape: drop the v13 columns/tables and restore the
        // single JSON column, then let migrate() do the upgrade.
        s.conn
            .execute_batch(
                "DROP TABLE widget_records; DROP TABLE widget_fields; DROP TABLE widget_defs;
                 CREATE TABLE widget_defs (
                   id INTEGER PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL,
                   schema_json TEXT NOT NULL, created_at TEXT NOT NULL);",
            )
            .unwrap();
        let legacy = serde_json::json!({
            "fields": [
                { "name": "course", "label": "课程", "type": "text", "required": true },
                { "name": "start_time", "label": "开始时刻", "type": "time", "required": true },
                { "name": "room", "label": "教室", "type": "text", "required": false }
            ],
            "views": [
                { "type": "form", "fields": ["course", "start_time", "room"] },
                { "type": "list", "fields": ["start_time", "course"], "sort_by": "start_time" }
            ]
        });
        s.conn
            .execute(
                "INSERT INTO widget_defs(name, icon, schema_json, created_at)
                 VALUES ('课程记录', 'calendar', ?1, '2026-07-19T10:00:00')",
                params![legacy.to_string()],
            )
            .unwrap();

        s.migrate().unwrap();

        let loaded = &s.list_widget_definitions().unwrap()[0];
        let names: Vec<&str> = loaded
            .schema
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, vec!["course", "start_time", "room"]);
        assert!(loaded.schema.fields[0].required);
        let form = loaded
            .schema
            .views
            .iter()
            .find(|v| v.view_type == WidgetViewType::Form)
            .unwrap();
        let list = loaded
            .schema
            .views
            .iter()
            .find(|v| v.view_type == WidgetViewType::List)
            .unwrap();
        assert_eq!(form.fields, vec!["course", "start_time", "room"]);
        // The list view kept both its own order and its exclusion of `room`.
        assert_eq!(list.fields, vec!["start_time", "course"]);
        assert_eq!(list.sort_by.as_deref(), Some("start_time"));
        // And the migrated rows are sync-ready, or the upgrade would leave
        // this device silently unable to share its existing widgets.
        let field_guids: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM widget_fields WHERE guid IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(field_guids, 3);
    }

    /// Ship every locally-originated op from `from` into `to`, the way one
    /// sync round would.
    fn push_all(from: &Store, to: &Store) {
        let ops: Vec<crate::sync::SyncOp> = from
            .local_ops_after(0)
            .unwrap()
            .into_iter()
            .map(|(_, op)| op)
            .collect();
        to.apply_remote_ops(&ops).unwrap();
    }

    fn optional_field(name: &str, label: &str) -> WidgetField {
        WidgetField {
            name: name.into(),
            label: label.into(),
            field_type: crate::widget::WidgetFieldType::Text,
            required: false,
            options: vec![],
        }
    }

    /// The whole reason the schema is stored as rows. Two devices each add a
    /// different field to the same widget while apart; after syncing both ways
    /// **both fields must exist on both devices**. Under the previous
    /// single-`schema_json`-column design this is exactly the case row-level
    /// LWW silently destroyed — the later write replaced the whole schema.
    #[test]
    fn concurrent_field_additions_on_two_devices_merge_as_a_union() {
        let phone = Store::open_in_memory().unwrap();
        let desktop = Store::open_in_memory().unwrap();
        phone.set_meta("device_id", "phone").unwrap();
        desktop.set_meta("device_id", "desktop").unwrap();

        let created = phone
            .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
            .unwrap();
        phone
            .insert_widget_record(
                created.id,
                &serde_json::json!({ "course": "数学", "start_time": "09:05" }),
                dt(2026, 7, 20, 9, 1),
            )
            .unwrap();
        push_all(&phone, &desktop);

        let mirrored = desktop.list_widget_definitions().unwrap();
        assert_eq!(mirrored.len(), 1, "definition must reach the other device");
        assert_eq!(
            desktop.list_widget_records(mirrored[0].id).unwrap().len(),
            1
        );

        // Apart: each device adds its own field.
        phone
            .add_widget_field(
                created.id,
                &optional_field("feel", "感受"),
                dt(2026, 7, 20, 10, 0),
            )
            .unwrap();
        desktop
            .add_widget_field(
                mirrored[0].id,
                &optional_field("notes", "备注"),
                dt(2026, 7, 20, 10, 1),
            )
            .unwrap();

        push_all(&phone, &desktop);
        push_all(&desktop, &phone);

        for (label, store, id) in [
            ("phone", &phone, created.id),
            ("desktop", &desktop, mirrored[0].id),
        ] {
            let names: Vec<String> = store
                .get_widget_definition(id)
                .unwrap()
                .schema
                .fields
                .into_iter()
                .map(|f| f.name)
                .collect();
            assert!(
                names.contains(&"feel".to_string()),
                "{label} lost feel: {names:?}"
            );
            assert!(
                names.contains(&"notes".to_string()),
                "{label} lost notes: {names:?}"
            );
            assert_eq!(names.len(), 4, "{label}: {names:?}");
        }

        // The pre-existing record still validates against the grown schema —
        // added fields are optional, so no record migration is needed.
        let definition = phone.get_widget_definition(created.id).unwrap();
        let record = &phone.list_widget_records(created.id).unwrap()[0];
        definition.schema.validate_record(&record.data).unwrap();
    }

    /// Every view slot must survive the trip to another device. The union
    /// merge that [`concurrent_field_additions_on_two_devices_merge_as_a_union`]
    /// checks was already right; what was missing is that the *payload* has to
    /// carry `table_ord` / `stat_ord` at all. Asserting only that field names
    /// arrive is what let the table and stat views vanish across sync while
    /// every test stayed green — so this asserts the rendered view lists.
    #[test]
    fn all_four_view_slots_and_the_canonical_order_survive_sync() {
        let phone = Store::open_in_memory().unwrap();
        let desktop = Store::open_in_memory().unwrap();
        phone.set_meta("device_id", "phone").unwrap();
        desktop.set_meta("device_id", "desktop").unwrap();

        let draft: WidgetDefinitionDraft = serde_json::from_value(serde_json::json!({
            "name": "开销记录",
            "icon": "journal",
            "fields": [
                { "name": "item",   "label": "项目", "type": "text", "required": true },
                { "name": "amount", "label": "金额", "type": "number" },
                { "name": "paid",   "label": "已付", "type": "bool" },
                { "name": "when",   "label": "时间", "type": "datetime" }
            ],
            // Deliberately four different orders, so a slot that silently falls
            // back to a default cannot coincidentally match.
            "views": [
                { "type": "form",  "fields": ["item", "amount", "paid", "when"] },
                { "type": "list",  "fields": ["when", "item", "amount"], "sort_by": "when" },
                { "type": "table", "fields": ["when", "item", "amount", "paid"], "sort_by": "amount" },
                { "type": "stat",  "fields": ["amount", "paid"] }
            ]
        }))
        .unwrap();

        let created = phone
            .insert_widget_definition(&draft, dt(2026, 7, 20, 9, 0))
            .unwrap();
        push_all(&phone, &desktop);

        let mirrored = desktop.list_widget_definitions().unwrap();
        assert_eq!(mirrored.len(), 1);
        let there = desktop.get_widget_definition(mirrored[0].id).unwrap();
        let here = phone.get_widget_definition(created.id).unwrap();

        for view_type in WidgetViewType::ALL {
            let pick = |definition: &crate::widget::WidgetDefinition| {
                definition
                    .schema
                    .views
                    .iter()
                    .find(|v| v.view_type == *view_type)
                    .map(|v| (v.fields.clone(), v.sort_by.clone()))
            };
            assert_eq!(
                pick(&there),
                pick(&here),
                "{} view differs after sync",
                view_type.as_str()
            );
            assert!(
                pick(&there).is_some(),
                "{} view did not survive sync at all",
                view_type.as_str()
            );
        }

        // The canonical field order is a synced property too: leaving `ord` out
        // of the payload lands every arriving field on 0, so the tie breaks on
        // the random guid and the two devices disagree.
        let names = |definition: &crate::widget::WidgetDefinition| {
            definition
                .schema
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&there), names(&here));
        assert_eq!(names(&here), vec!["item", "amount", "paid", "when"]);
    }

    /// A half-applied v13 split used to be unrecoverable: the field inserts
    /// committed, `schema_json` stayed, and the next open re-inserted the same
    /// names straight into `UNIQUE(widget_id, name)` — every subsequent open
    /// failed the same way. The rewrite is one transaction, so a failure must
    /// leave the database exactly as it was.
    #[test]
    fn a_failed_v13_split_rolls_back_instead_of_wedging_the_database() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute_batch(
                "DROP TABLE widget_records; DROP TABLE widget_fields; DROP TABLE widget_defs;
                 CREATE TABLE widget_defs (
                    id INTEGER PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL,
                    schema_json TEXT NOT NULL, created_at TEXT NOT NULL);",
            )
            .unwrap();
        let schema = serde_json::json!({
            "fields": [
                { "name": "course", "label": "课程", "type": "text", "required": true },
                { "name": "start_time", "label": "开始时刻", "type": "time", "required": true }
            ],
            "views": [
                { "type": "form", "fields": ["course", "start_time"] },
                { "type": "list", "fields": ["start_time", "course"] }
            ]
        })
        .to_string();
        s.conn
            .execute(
                "INSERT INTO widget_defs(id, name, icon, schema_json, created_at)
                 VALUES (1, '课程记录', 'calendar', ?1, '2026-07-19T10:00:00')",
                params![schema],
            )
            .unwrap();
        // Recreate the field table and pre-seed the row the split will collide
        // with, which is what a crashed first attempt leaves behind.
        s.migrate().unwrap();
        s.conn
            .execute("ALTER TABLE widget_defs ADD COLUMN schema_json TEXT", [])
            .unwrap();
        s.conn
            .execute(
                "UPDATE widget_defs SET schema_json = ?1 WHERE id = 1",
                params![schema],
            )
            .unwrap();
        // Drop only the *first* field, so the retry inserts one row
        // successfully and then collides on the second. Colliding on the very
        // first insert would leave nothing committed either way and the test
        // would pass with or without the transaction.
        s.conn
            .execute("DELETE FROM widget_fields WHERE name = 'course'", [])
            .unwrap();
        let before: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM widget_fields", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 1);

        assert!(
            s.migrate_widget_schema_to_rows().is_err(),
            "the duplicate names must still be an error"
        );

        // Rolled back: no half-written fields, and the column is still there,
        // so a later build can retry the whole split from a clean state.
        let after: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM widget_fields", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, before, "a failed split must not commit any row");
        assert!(s.has_column("widget_defs", "schema_json").unwrap());
    }

    /// The v15 repair only speaks for rows this device actually knows about.
    /// A device that merely received damaged rows holds the column defaults,
    /// and must stay quiet rather than push its own copy back over the good
    /// one when both devices upgrade.
    #[test]
    fn the_v15_view_slot_repair_only_re_emits_rows_that_carry_information() {
        let s = Store::open_in_memory().unwrap();
        s.insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
            .unwrap();
        // Emulate a device that received these rows before the payload carried
        // the slots: everything sits on the defaults.
        s.conn
            .execute(
                "UPDATE widget_fields SET ord = 0, table_ord = NULL, stat_ord = NULL",
                [],
            )
            .unwrap();
        s.conn.execute("DELETE FROM sync_oplog", []).unwrap();
        s.rebroadcast_widget_fields_view_slots().unwrap();
        let emitted: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sync_oplog WHERE tbl = 'widget_fields'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(emitted, 0, "a receiving device must not rebroadcast");

        // The authoring device does have something to say.
        s.conn
            .execute(
                "UPDATE widget_fields SET ord = 1 WHERE name = 'start_time'",
                [],
            )
            .unwrap();
        s.conn.execute("DELETE FROM sync_oplog", []).unwrap();
        s.rebroadcast_widget_fields_view_slots().unwrap();
        let emitted: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sync_oplog WHERE tbl = 'widget_fields'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(emitted, 1, "the row holding a non-default ord must re-emit");
    }

    /// The one case a grow-only set cannot resolve on its own: the same field
    /// name added on both devices. Resolution must be deterministic and must
    /// not silently discard the loser.
    #[test]
    fn same_named_concurrent_fields_resolve_deterministically_and_are_logged() {
        let phone = Store::open_in_memory().unwrap();
        let desktop = Store::open_in_memory().unwrap();
        phone.set_meta("device_id", "phone").unwrap();
        desktop.set_meta("device_id", "desktop").unwrap();

        let created = phone
            .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
            .unwrap();
        push_all(&phone, &desktop);
        let mirrored_id = desktop.list_widget_definitions().unwrap()[0].id;

        phone
            .add_widget_field(
                created.id,
                &optional_field("notes", "手机备注"),
                dt(2026, 7, 20, 10, 0),
            )
            .unwrap();
        desktop
            .add_widget_field(
                mirrored_id,
                &optional_field("notes", "桌面备注"),
                dt(2026, 7, 20, 10, 1),
            )
            .unwrap();

        push_all(&phone, &desktop);
        push_all(&desktop, &phone);

        // Exactly one `notes` survives on each device, and both devices agree
        // on which one — that is what "deterministic" has to mean here.
        let label_on = |store: &Store, id: i64| {
            store
                .get_widget_definition(id)
                .unwrap()
                .schema
                .fields
                .into_iter()
                .filter(|f| f.name == "notes")
                .map(|f| f.label)
                .collect::<Vec<_>>()
        };
        let phone_labels = label_on(&phone, created.id);
        let desktop_labels = label_on(&desktop, mirrored_id);
        assert_eq!(phone_labels.len(), 1, "{phone_labels:?}");
        assert_eq!(phone_labels, desktop_labels, "devices disagree");

        // The discarded one is visible, not silently gone.
        let rejections = phone.list_widget_schema_rejections().unwrap().len()
            + desktop.list_widget_schema_rejections().unwrap().len();
        assert!(rejections >= 1, "the dropped duplicate must be logged");
    }

    /// Merging may push a widget past MAX_FIELDS. That state is legal — the
    /// user created every one of those fields — but it must stop accepting
    /// new ones rather than truncate.
    #[test]
    fn a_merged_over_cap_schema_still_works_but_refuses_further_fields() {
        let s = Store::open_in_memory().unwrap();
        let definition = s
            .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
            .unwrap();
        // Force the over-cap state the way a merge would: straight into rows.
        for n in 0..(crate::widget::MAX_FIELDS + 2) {
            s.conn
                .execute(
                    "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                       options_json, ord, form_ord, list_ord, created_at, guid)
                     VALUES (?1, ?2, ?3, 'text', 0, '[]', ?4, NULL, NULL, ?5, ?6)",
                    params![
                        definition.id,
                        format!("merged_{n}"),
                        format!("合并字段{n}"),
                        100 + n as i64,
                        fmt_ts(&dt(2026, 7, 20, 11, 0)),
                        new_guid()
                    ],
                )
                .unwrap();
        }
        let loaded = s.get_widget_definition(definition.id).unwrap();
        assert!(loaded.schema.fields.len() > crate::widget::MAX_FIELDS);
        // Still usable: records write and validate normally.
        s.insert_widget_record(
            definition.id,
            &serde_json::json!({ "course": "数学", "start_time": "09:05" }),
            dt(2026, 7, 20, 11, 1),
        )
        .unwrap();
        // But closed to growth.
        let error = s
            .add_widget_field(
                definition.id,
                &optional_field("one_more", "再来一个"),
                dt(2026, 7, 20, 11, 2),
            )
            .unwrap_err();
        assert!(error.to_string().contains("上限"), "{error}");
    }

    /// Added fields must be optional; a required one could never be satisfied
    /// by records that already exist.
    #[test]
    fn added_fields_must_be_optional() {
        let s = Store::open_in_memory().unwrap();
        let definition = s
            .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
            .unwrap();
        let mut field = optional_field("notes", "备注");
        field.required = true;
        let error = s
            .add_widget_field(definition.id, &field, dt(2026, 7, 20, 10, 0))
            .unwrap_err();
        assert!(error.to_string().contains("可空"), "{error}");
        assert!(s
            .add_widget_field(
                definition.id,
                &optional_field("notes", "备注"),
                dt(2026, 7, 20, 10, 0)
            )
            .is_ok());
    }

    /// Park an op the way an older build would have: it named a table that
    /// build did not know.
    fn park(s: &Store, tbl: &str, guid: &str, payload: &str, hlc: &str) {
        s.conn
            .execute(
                "INSERT INTO sync_quarantine(tbl, guid, op, payload, hlc, origin, reason, held_at)
                 VALUES (?1, ?2, 'upsert', ?3, ?4, 'peer', 'unknown sync table', ?4)",
                params![tbl, guid, payload, hlc],
            )
            .unwrap();
    }

    fn event_payload(title: &str) -> String {
        serde_json::json!({
            "title": title,
            "kind": "meeting",
            "start": crate::model::fmt_ts(&dt(2026, 7, 9, 15, 0)),
            "people_json": "[]",
            "created_at": crate::model::fmt_ts(&dt(2026, 7, 7, 10, 0)),
        })
        .to_string()
    }

    /// The whole reason quarantine beats "just skip it": an op held by an
    /// older build must still land after the upgrade that teaches us the
    /// table. Skipping would have advanced the cursor past it forever.
    #[test]
    fn quarantined_ops_replay_once_this_build_knows_the_table() {
        let s = Store::open_in_memory().unwrap();
        park(
            &s,
            "events",
            &"a".repeat(32),
            &event_payload("升级后补上的会议"),
            "2026-07-07T10:00:00.000",
        );
        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));

        // Re-running migrate is what an app restart after an upgrade does.
        s.migrate().unwrap();

        let titles: Vec<String> = s
            .list_events()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["升级后补上的会议".to_string()]);
        assert_eq!(s.sync_quarantine_stats().unwrap(), (0, 0));
    }

    /// A table *this* build still does not know stays parked — it is not
    /// dropped just because a migration ran for other reasons.
    ///
    /// This used to use `widget_defs` as the stand-in for a future table.
    /// That table shipped in v13, so the name has to be one no build knows,
    /// or the test silently starts asserting the opposite of its own name.
    #[test]
    fn a_still_unknown_table_stays_parked_across_migrations() {
        let s = Store::open_in_memory().unwrap();
        park(
            &s,
            "a_table_from_a_future_version",
            &"b".repeat(32),
            r#"{"name":"日程表"}"#,
            "2026-07-07T10:00:00.000",
        );

        s.migrate().unwrap();
        s.migrate().unwrap();

        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
    }

    /// A known table with an unusable payload must not take the batch down
    /// with it, and the savepoint must leave nothing half-written.
    #[test]
    fn a_malformed_known_table_op_is_quarantined_without_partial_writes() {
        let s = Store::open_in_memory().unwrap();
        let good = crate::sync::SyncOp {
            tbl: "events".into(),
            guid: "c".repeat(32),
            op: "upsert".into(),
            payload: Some(event_payload("完好的会议")),
            hlc: "2026-07-07T10:00:00.000".into(),
            origin: "peer".into(),
        };
        // `kind` is required; without it the arm fails while building params.
        let broken = crate::sync::SyncOp {
            tbl: "events".into(),
            guid: "d".repeat(32),
            op: "upsert".into(),
            payload: Some(r#"{"title":"缺字段的会议"}"#.into()),
            hlc: "2026-07-07T10:00:01.000".into(),
            origin: "peer".into(),
        };

        let counts = s.apply_remote_ops(&[good, broken]).unwrap();

        assert_eq!(
            counts,
            MergeCounts {
                applied: 1,
                skipped: 0,
                quarantined: 1
            }
        );
        // The good sibling survived; the broken one wrote nothing.
        let titles: Vec<String> = s
            .list_events()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["完好的会议".to_string()]);
        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
    }

    /// The same op arriving twice must be held once — replaying it twice
    /// would double-apply after an upgrade.
    #[test]
    fn re_holding_the_same_op_does_not_duplicate_it() {
        let s = Store::open_in_memory().unwrap();
        let op = crate::sync::SyncOp {
            tbl: "widget_defs".into(),
            guid: "e".repeat(32),
            op: "upsert".into(),
            payload: Some(r#"{"name":"日程表"}"#.into()),
            hlc: "2026-07-07T10:00:00.000".into(),
            origin: "peer".into(),
        };
        s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
        s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
    }

    /// A device that never upgrades must not grow the hold list without end;
    /// the overflow is dropped *and counted*, never silently.
    #[test]
    fn quarantine_overflow_drops_oldest_and_counts_it() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..(MAX_QUARANTINE_OPS + 3) {
            let op = crate::sync::SyncOp {
                tbl: "widget_defs".into(),
                guid: format!("{i:032}"),
                op: "upsert".into(),
                payload: Some("{}".into()),
                hlc: format!("2026-07-07T10:00:{:02}.000", i % 60),
                origin: "peer".into(),
            };
            s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
        }
        assert_eq!(
            s.sync_quarantine_stats().unwrap(),
            (MAX_QUARANTINE_OPS, 3),
            "held should cap out, and the drops must be visible"
        );
    }

    #[test]
    fn phase9_legacy_capture_migration_runs_once() {
        let s = Store::open_in_memory().unwrap();
        let add_notification = |event_id| {
            s.insert_notification(&Notification {
                id: None,
                event_id,
                fire_at: dt(2026, 7, 7, 14, 30),
                lead_label: "30m".into(),
                channels: vec![Channel::Push],
                status: NotificationStatus::Pending,
                created_at: dt(2026, 7, 6, 10, 0),
                fired_at: None,
            })
            .unwrap()
        };

        // Simulate a v6 database that had notification-source rows before
        // this setting existed. The migration preserves the old local-only
        // boundary and retracts its unsent oplog entries.
        let old_raw = s
            .insert_raw_input(
                "[通知·legacy] 明天下午3点开会",
                "event",
                dt(2026, 7, 6, 10, 0),
            )
            .unwrap();
        let old_event = s
            .insert_event_with_scope(&sample_event(), Some(old_raw), false, None)
            .unwrap();
        let old_notification = add_notification(old_event);
        s.conn
            .execute(
                "UPDATE meta SET value = '6' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        s.migrate().unwrap();
        assert!(s.raw_input_is_local_only(old_raw).unwrap());
        assert!(s.event_is_local_only(old_event).unwrap());
        assert_eq!(
            s.conn
                .query_row(
                    "SELECT local_only FROM notifications WHERE id = ?1",
                    params![old_notification],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        // v10: the legacy rows keep their local_only stamp (no cloud LLM) but
        // are bootstrapped into the oplog, since sync no longer depends on it.
        assert!(
            !s.local_ops_after(0).unwrap().is_empty(),
            "v9→v10 must bootstrap previously unsynced captures into the oplog"
        );

        // A later open is not allowed to re-mark captures made under an
        // enabled setting; their scope is chosen at ingestion time.
        let new_raw = s
            .insert_raw_input("[通知·new] 明天下午4点开会", "event", dt(2026, 7, 6, 10, 0))
            .unwrap();
        let new_event = s
            .insert_event_with_scope(&sample_event(), Some(new_raw), false, None)
            .unwrap();
        let new_notification = add_notification(new_event);
        // A later schema-only upgrade must not re-run the Phase 9 privacy
        // migration over captures whose scope was already chosen at ingest.
        s.conn
            .execute(
                "UPDATE meta SET value = '8' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        s.migrate().unwrap();
        assert!(!s.raw_input_is_local_only(new_raw).unwrap());
        assert!(!s.event_is_local_only(new_event).unwrap());
        assert_eq!(
            s.conn
                .query_row(
                    "SELECT local_only FROM notifications WHERE id = ?1",
                    params![new_notification],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn event_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_event(&sample_event(), None).unwrap();
        let got = s.get_event(id).unwrap();
        assert_eq!(got.title, "开会");
        assert_eq!(got.kind, EventKind::Meeting);
        assert_eq!(got.location.as_deref(), Some("会议室"));
        assert_eq!(got.people, vec!["张伟".to_string()]);
        assert_eq!(got.start, dt(2026, 7, 7, 15, 0));
    }

    #[test]
    fn notification_due_and_fire() {
        let s = Store::open_in_memory().unwrap();
        let ev_id = s.insert_event(&sample_event(), None).unwrap();
        let n = Notification {
            id: None,
            event_id: ev_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        };
        let nid = s.insert_notification(&n).unwrap();
        assert!(s
            .due_notifications(dt(2026, 7, 7, 14, 0))
            .unwrap()
            .is_empty());
        assert_eq!(
            s.due_notifications(dt(2026, 7, 7, 14, 30)).unwrap().len(),
            1
        );
        s.mark_fired(nid, dt(2026, 7, 7, 14, 30)).unwrap();
        assert!(s
            .due_notifications(dt(2026, 7, 7, 15, 0))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn snooze_rearms_pending_and_fired_but_not_dismissed() {
        let s = Store::open_in_memory().unwrap();
        let ev_id = s.insert_event(&sample_event(), None).unwrap();
        let n = Notification {
            id: None,
            event_id: ev_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        };
        let nid = s.insert_notification(&n).unwrap();

        // pending → postpone: not due at the old time, due at the new one.
        s.snooze_notification(nid, dt(2026, 7, 7, 14, 45)).unwrap();
        assert!(s
            .due_notifications(dt(2026, 7, 7, 14, 30))
            .unwrap()
            .is_empty());
        assert_eq!(
            s.due_notifications(dt(2026, 7, 7, 14, 45)).unwrap().len(),
            1
        );

        // fired → re-arm: back to pending with fired_at cleared.
        s.mark_fired(nid, dt(2026, 7, 7, 14, 45)).unwrap();
        s.snooze_notification(nid, dt(2026, 7, 7, 14, 55)).unwrap();
        let got = s
            .list_notifications()
            .unwrap()
            .into_iter()
            .find(|x| x.id == Some(nid))
            .unwrap();
        assert_eq!(got.status, NotificationStatus::Pending);
        assert_eq!(got.fire_at, dt(2026, 7, 7, 14, 55));
        assert!(got.fired_at.is_none());

        // dismissed stays cancelled — snooze must not resurrect it.
        s.dismiss_notification(nid).unwrap();
        assert!(s.snooze_notification(nid, dt(2026, 7, 7, 15, 5)).is_err());
    }

    #[test]
    fn fact_update_edits_in_place_and_rejects_duplicates() {
        let s = Store::open_in_memory().unwrap();
        let now = dt(2026, 7, 6, 10, 0);
        let fact = |content: &str| crate::memory::MemoryFact {
            id: None,
            content: content.into(),
            source: "manual".into(),
            created_at: now,
            last_used_at: None,
        };
        let (f1, f2) = (fact("我对花生过敏"), fact("我不吃辣"));
        let id1 = s.insert_fact_if_new(&f1).unwrap().unwrap();
        s.insert_fact_if_new(&f2).unwrap().unwrap();

        s.update_fact(id1, " 我对花生和腰果都过敏 ").unwrap();
        let facts = s.list_facts().unwrap();
        assert!(facts.iter().any(|f| f.content == "我对花生和腰果都过敏"));

        // Editing into another fact's exact wording is rejected, not merged.
        assert!(s.update_fact(id1, "我不吃辣").is_err());
        // Empty content is rejected.
        assert!(s.update_fact(id1, "   ").is_err());
        // Missing row.
        assert!(s.update_fact(9999, "x").is_err());
    }

    #[test]
    fn audit_is_appendable_and_readable() {
        let s = Store::open_in_memory().unwrap();
        let e = AuditEntry {
            ts: dt(2026, 7, 6, 10, 0),
            tool: "delete_file".into(),
            risk: RiskLevel::Dangerous,
            summary: "delete_file(/x)".into(),
            decision: Decision::Refused,
            token_id: None,
            detail: "no token".into(),
        };
        s.append_audit(&e).unwrap();
        let rows = s.list_audit().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, "refused");
        assert_eq!(rows[0].risk, "dangerous");
    }

    #[test]
    fn config_defaults_and_persist() {
        let s = Store::open_in_memory().unwrap();
        // Defaults when nothing saved.
        assert_eq!(s.load_rule_table().unwrap(), RuleTable::default_table());
        assert!(s.notif_cloud_enabled().unwrap());
        s.set_notif_cloud_enabled(false).unwrap();
        assert!(!s.notif_cloud_enabled().unwrap());
        // This is deliberately device-local; changing it cannot create a
        // synced meta document that flips another device's privacy choice.
        assert!(s.local_ops_after(0).unwrap().is_empty());
        let mut c = ProactivityConfig::defaults();
        c.set(
            crate::proactivity::ProactivityDimension::WeeklyReview,
            crate::proactivity::ProactivityLevel::Butler,
        );
        s.save_proactivity(&c).unwrap();
        assert_eq!(s.load_proactivity().unwrap(), c);
    }

    #[test]
    fn persona_versions_rollback_and_clear() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.active_persona().unwrap().is_none());

        let d1 = crate::persona::PersonaDraft {
            tone: "温和".into(),
            ..Default::default()
        };
        let d2 = crate::persona::PersonaDraft {
            tone: "干练".into(),
            nickname: Some("老板".into()),
            ..Default::default()
        };
        let v1 = s
            .insert_persona_version(&d1, "manual", None, dt(2026, 7, 6, 10, 0))
            .unwrap();
        let v2 = s
            .insert_persona_version(&d2, "manual", Some("改干练".into()), dt(2026, 7, 6, 11, 0))
            .unwrap();
        assert_eq!((v1.version, v2.version), (1, 2));

        // Latest write is active; history is newest-first and complete.
        assert_eq!(s.active_persona().unwrap().unwrap().draft.tone, "干练");
        let versions = s.list_persona_versions().unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 2);

        // Rollback moves the pointer without touching history.
        let back = s.set_active_persona(1).unwrap();
        assert_eq!(back.draft.tone, "温和");
        assert_eq!(s.active_persona().unwrap().unwrap().version, 1);
        assert_eq!(s.list_persona_versions().unwrap().len(), 2);
        assert!(s.set_active_persona(99).is_err());

        // Clear removes everything; the next save restarts at v1.
        s.clear_persona().unwrap();
        assert!(s.active_persona().unwrap().is_none());
        assert!(s.list_persona_versions().unwrap().is_empty());
        let again = s
            .insert_persona_version(&d1, "manual", None, dt(2026, 7, 6, 12, 0))
            .unwrap();
        assert_eq!(again.version, 1);
    }

    #[test]
    fn ledger_lists_and_cascade_deletes() {
        let s = Store::open_in_memory().unwrap();
        let raw = s
            .insert_raw_input("明天下午3点开会", "ingest_event", dt(2026, 7, 6, 10, 0))
            .unwrap();
        let ev_id = s.insert_event(&sample_event(), Some(raw)).unwrap();
        let n = Notification {
            id: None,
            event_id: ev_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        };
        s.insert_notification(&n).unwrap();

        let ledger = s.memory_ledger().unwrap();
        // raw input + event + notification = 3 entries.
        assert_eq!(ledger.len(), 3);

        // Deleting the raw input cascades to event and notification.
        s.delete_memory(MemoryLayer::RawInput, raw).unwrap();
        assert!(s.memory_ledger().unwrap().is_empty());
        assert!(s.list_events().unwrap().is_empty());
        assert!(s.list_notifications().unwrap().is_empty());
    }
}

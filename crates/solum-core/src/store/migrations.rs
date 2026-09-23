use super::*;

impl Store {
    pub(crate) fn migrate(&self) -> Result<()> {
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
                source_id  TEXT NOT NULL DEFAULT 'manual',
                external_id TEXT,
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

            -- F12 ledger paging: keyset reads per layer walk these in
            -- (created_at DESC, id ASC) order without loading whole tables.
            CREATE INDEX IF NOT EXISTS idx_raw_inputs_created ON raw_inputs(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_behavior_created ON behavior_log(ts, id);
            CREATE INDEX IF NOT EXISTS idx_suggestions_created ON suggestions(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_health_created ON health_samples(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_facts_created ON memory_facts(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_routines_created ON routines(created_at, id);
            CREATE INDEX IF NOT EXISTS idx_captures_created ON notification_captures(received_at, id);
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
            ("raw_inputs", "source_id", "TEXT NOT NULL DEFAULT 'manual'"),
            ("raw_inputs", "external_id", "TEXT"),
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

        // v15 → v16: historical notification rows predate explicit source
        // provenance. The capture journal is the authoritative mapping; the
        // text prefix is only a fallback for the older pre-journal shape.
        self.conn.execute_batch(
            "UPDATE raw_inputs
                SET source_id = 'notification',
                    external_id = COALESCE(external_id,
                      (SELECT content_hash FROM notification_captures c
                       WHERE c.raw_input_id = raw_inputs.id LIMIT 1))
              WHERE source_id = 'manual'
                AND (id IN (SELECT raw_input_id FROM notification_captures)
                     OR text LIKE '[通知·%');",
        )?;

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

    pub(crate) fn has_column(&self, tbl: &str, col: &str) -> Result<bool> {
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
    pub(crate) fn mark_legacy_captured_data_local_only(&self) -> Result<()> {
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
    pub(crate) fn bootstrap_previously_unsynced_captures(&self) -> Result<()> {
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
    pub(crate) fn migrate_sync(&self) -> Result<()> {
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

            -- The push-side sibling of `sync_bad_blobs`: a locally-originated
            -- op too large to ever upload (single row over the push-blob cap).
            -- The oplog row itself is kept; this record only marks that the
            -- push cursor moved past it, so the fact stays visible instead of
            -- either wedging every later op behind it or silently skipping it.
            CREATE TABLE IF NOT EXISTS sync_push_holds (
                oplog_id INTEGER PRIMARY KEY,   -- references sync_oplog(id)
                tbl      TEXT NOT NULL,
                guid     TEXT NOT NULL,
                hlc      TEXT NOT NULL,         -- kept so a future replay can
                                                -- re-push in original order
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
    pub(crate) fn migrate_widget_schema_to_rows(&self) -> Result<()> {
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
    pub(crate) fn rebroadcast_widget_fields_view_slots(&self) -> Result<()> {
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
}

use super::*;

impl Store {
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
    pub(crate) fn last_op_stamp(&self, tbl: &str, guid: &str) -> Result<Option<(String, String)>> {
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

    pub(crate) fn raw_input_is_local_only(&self, id: i64) -> Result<bool> {
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

    pub(crate) fn event_is_local_only(&self, id: i64) -> Result<bool> {
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
        self.with_transaction(|store| store.apply_remote_ops_in_transaction(ops))
    }

    /// The merge body without its own BEGIN/COMMIT. Keeping this separate lets
    /// a fresh-profile restore wrap the guid-bearing rows and the portable
    /// local ledgers in one wider transaction.
    pub(crate) fn apply_remote_ops_in_transaction(&self, ops: &[crate::sync::SyncOp]) -> Result<MergeCounts> {
        self.conn.execute(
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
        let horizon: String = self.conn.query_row(
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
        self.conn
            .execute("DELETE FROM meta WHERE key = 'sync_applying'", [])?;
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

    /// Park a locally-originated op that can never be uploaded (single row
    /// over the push-blob cap). The oplog row itself is kept — this record
    /// only marks that the push cursor moved past it, so a later build (larger
    /// cap, compression) can still find the op and push it. `INSERT OR
    /// REPLACE`: a crash between parking and advancing the cursor re-reads
    /// the op next round and re-parks it, which must stay harmless.
    pub fn hold_push_op(
        &self,
        oplog_id: i64,
        op: &crate::sync::SyncOp,
        reason: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sync_push_holds(oplog_id, tbl, guid, hlc, reason, held_at)
             VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%f', 'now'))",
            params![oplog_id, op.tbl, op.guid, op.hlc, reason],
        )?;
        Ok(())
    }

    /// How many local ops this device has had to park as unuploadable.
    /// Non-zero means one row is too big for the relay's request cap — the
    /// data is still in the oplog, but no peer will receive it until a future
    /// build can split or compress it.
    pub fn push_hold_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM sync_push_holds", [], |r| r.get(0))?)
    }

    /// Backlog size for status displays: locally-originated ops not yet
    /// confirmed uploaded (push cursor not past them yet).
    pub fn local_ops_count_after(&self, after: i64) -> Result<i64> {
        let device = self.device_id()?;
        Ok(self.conn.query_row(
            "SELECT count(*) FROM sync_oplog WHERE origin = ?1 AND id > ?2",
            params![device, after],
            |r| r.get(0),
        )?)
    }

    /// Park an op this build cannot apply. `INSERT OR IGNORE`: the same op can
    /// legitimately arrive twice (peer resend, cursor rewind), and holding it
    /// twice would replay it twice.
    pub(crate) fn hold_in_quarantine(&self, op: &crate::sync::SyncOp, reason: &str) -> Result<()> {
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

    pub(crate) fn table_is_known(tbl: &str) -> bool {
        tbl == "meta" || SYNCED_TABLES.contains(&tbl)
    }

    /// Replay ops parked by an older build. Called at the end of [`migrate`]:
    /// a migration that adds a synced table is exactly the moment held ops
    /// become interpretable. Ordered by `(hlc, origin)` so LWW resolves the
    /// way it would have on arrival.
    pub(crate) fn replay_sync_quarantine(&self) -> Result<usize> {
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
    pub(crate) fn apply_one(&self, op: &crate::sync::SyncOp) -> Result<bool> {
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
                let source_id = s("source_id").unwrap_or_else(|| {
                    if text.starts_with("[通知·") {
                        "notification".into()
                    } else {
                        "manual".into()
                    }
                });
                let external_id = s("external_id");
                self.conn.execute(
                    "INSERT INTO raw_inputs(text, intent, created_at, local_only, source_id, external_id, guid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(guid) DO UPDATE SET text=excluded.text, intent=excluded.intent,
                       created_at=excluded.created_at, local_only=excluded.local_only,
                       source_id=excluded.source_id, external_id=excluded.external_id",
                    params![
                        text,
                        need("intent")?,
                        need("created_at")?,
                        local_only as i64,
                        source_id,
                        external_id,
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

    pub(crate) fn apply_delete(&self, op: &crate::sync::SyncOp) -> Result<bool> {
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

    pub(crate) fn id_by_guid(&self, tbl: &str, guid: &str) -> Result<Option<i64>> {
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
pub(crate) fn purge_target(layer: MemoryLayer) -> Result<(&'static str, &'static str)> {
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



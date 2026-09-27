//! `park` -- the three places an op or blob gets parked and the cursor moves on: a blob that could not be opened, a local op too large to ever upload, an op this build cannot apply. Each keeps the ciphertext or the oplog row so a later build can still find it — parking is a deferral, not a deletion, and the insert mode (OR IGNORE vs OR REPLACE) is what makes a re-park harmless.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
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
}

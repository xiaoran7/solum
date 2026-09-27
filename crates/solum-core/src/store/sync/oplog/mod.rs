//! `oplog` -- the local side of the oplog: which local ops are waiting to go out, bounded so one upload stays small enough for the relay to accept, and the per-row (hlc, origin) stamp that last-write-wins compares against. Split from the merge because reading and writing are different questions and the merge is the long one.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
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
}

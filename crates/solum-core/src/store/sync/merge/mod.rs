//! `merge` -- applying a batch of remote ops in one transaction: last-write-wins per row, idempotent so a re-delivery is skipped, and echo-free because the capture triggers are suspended for the duration. One transaction for the whole batch is the point — a half-applied batch is worse than a failed one.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
    /// The merge body without its own BEGIN/COMMIT. Keeping this separate lets
    /// a fresh-profile restore wrap the guid-bearing rows and the portable
    /// local ledgers in one wider transaction.
    pub(crate) fn apply_remote_ops_in_transaction(
        &self,
        ops: &[crate::sync::SyncOp],
    ) -> Result<MergeCounts> {
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
}

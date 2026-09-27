//! `quarantine` -- replaying what an older build parked, and the table whitelist that decides what counts as a synced table at all. Called at the end of `migrate`: a migration that adds a synced table is exactly the moment held ops become interpretable again. Ordered by (hlc, origin) so last-write-wins resolves the way it would have on arrival.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
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
}

//! `applydelete` -- the delete arm, kept next to the dispatch that routes to it so the two are read together — a delete that reaches the wrong table is the one sync bug that cannot be undone by replaying.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
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
}

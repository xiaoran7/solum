//! `recall` -- what the recall feature may offer: candidate events under the current privacy set, and the guid-to-row lookup the rest of the store uses to name a row without knowing its id.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
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

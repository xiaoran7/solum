//! `identity` -- who this device is and what it has already synced: the stable device id, the sync-state markers, and a wall clock read through SQLite rather than the system clock — an audit line has to be stamped with when something actually happened, not with a clock a debug session injected.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

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
}

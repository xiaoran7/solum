//! `captureloss` -- counting capture losses idempotently. The receipt names are the deduplication: a crash between this commit and the caller deleting the files cannot inflate the total on the next run, which is the whole reason this is not a bare counter.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;
impl Store {
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
}

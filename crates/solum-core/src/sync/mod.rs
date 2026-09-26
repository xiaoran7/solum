//! Multi-device sync client (F17, ARCHITECTURE.md §3.8) — local-first.
//!
//! Each device's SQLite store is authoritative. Mutations are captured into
//! `sync_oplog` by triggers (see `store::migrate_sync`); this module pushes
//! locally-originated ops to the relay as an **end-to-end encrypted** blob and
//! pulls + merges other devices' blobs (last-write-wins per row on a
//! `(hlc, origin)` stamp). The relay (`solum-sync-server`) only ever sees
//! ciphertext. The logged-in Solum account selects the relay tenant, while a
//! separate symmetric key in `solum-sync.json` remains device-only and opens
//! the end-to-end encrypted blobs. Legacy direct relay tokens remain readable.
//! The file's path can be overridden with `SOLUM_SYNC_CONFIG`, same as
//! `SOLUM_LLM_CONFIG`; without an explicit override it follows the active
//! account profile and stays in the historical app-data location for guest.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::store::Store;

/// One captured mutation, as it travels between devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOp {
    pub tbl: String,
    pub guid: String,
    /// `"upsert"` or `"delete"`.
    pub op: String,
    /// Row JSON (FKs already translated to guids); `None` for deletes.
    pub payload: Option<String>,
    /// Millisecond UTC wall-clock stamp — the LWW ordering key.
    pub hlc: String,
    /// Device id that originated the change (LWW tie-breaker).
    pub origin: String,
}

/// The plaintext of one pushed blob.
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncBatch {
    pub device: String,
    pub ops: Vec<SyncOp>,
}

/// A blob as returned by the relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulledBlob {
    pub seq: i64,
    pub device: String,
    #[serde(rename = "blob")]
    pub blob_b64: String,
}

/// What one `sync_once` did (for CLI / UI display).
#[derive(Debug, Default, Serialize)]
pub struct SyncOutcome {
    /// Local ops uploaded this round.
    pub pushed: usize,
    /// Remote blobs downloaded this round.
    pub pulled_blobs: usize,
    /// Remote ops merged into the local store.
    pub applied: usize,
    /// Remote ops skipped (older than local state, duplicates, orphans).
    pub skipped: usize,
    /// Remote ops this build could not interpret and parked for a later one
    /// (unknown table, unusable payload). Non-zero means a peer is ahead of
    /// this device — the data is held, not lost.
    pub quarantined: usize,
    /// Blobs that could not be opened at all (bad ciphertext, wrong key) and
    /// were parked so the cursor could advance. Non-zero is a real problem
    /// worth surfacing — usually the devices' sync keys disagree.
    pub bad_blobs: usize,
    /// This device's cursor pointed below the relay's retention floor, so an
    /// unknown number of remote ops are gone for good. Set means **this sync
    /// was not complete**; the device needs a fresh full copy from a peer
    /// (export → import) rather than more incremental rounds.
    pub history_gap: bool,
    /// Local ops that could not be uploaded this round and were left behind:
    /// the relay permanently refused the push (quota, revoked device), or a
    /// single op exceeded the blob cap and was parked. The pull half still
    /// ran — that is the whole point of counting instead of failing. Non-zero
    /// means this device is falling behind on **uploading**; the reason is in
    /// [`Self::push_block`] and, durably, under [`PUSH_BLOCKED_KEY`].
    pub push_skipped: usize,
    /// Why the push half did not finish this round, in user-facing language.
    /// `None` on a clean round. Also recorded durably (cleared by the next
    /// successful push) so a background ticker failure is not the only trace.
    pub push_block: Option<String>,
    /// A *transient* push failure (network, timeout, server 5xx) that ended
    /// the push half early. Not durable — the same blob can succeed next
    /// round, so it must not sit in the sticky status view next to problems
    /// that need a human. The pull half still ran; that is the point.
    pub push_failed: Option<String>,
}

mod account_recovery_envelope;
mod config;
mod crypto;
mod driver;
mod transport;

pub use account_recovery_envelope::*;
pub use config::*;
pub use driver::*;
pub use transport::*;

// `pub(super) use`, not a `pub` glob and not nothing: these two are private to
// the section that defines them, but siblings (`driver`, the envelope, the
// tests) need them. A `pub use` here would be a no-op, and one item leaking
// with the other not would fail the size check on the XChaCha key.
pub(super) use crypto::{decrypt, encrypt};

// `pub(crate)`, not private: the orchestrator's tests borrow this module's
// in-memory relay to exercise end-to-end sync without a network.
#[cfg(test)]
pub(crate) mod tests;

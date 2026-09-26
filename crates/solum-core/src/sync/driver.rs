//! The push/pull driver: capture local ops, encrypt, merge remote blobs.
//!
//! Part of the `sync` module; see `mod.rs` for the module contract.

use super::*;

// ---- driver ---------------------------------------------------------------------

pub(super) const PUSHED_KEY: &str = "sync_pushed_oplog_id";
pub(super) const PULLED_KEY: &str = "sync_pulled_seq";
/// Records that this device's cursor fell below the relay's retention floor.
/// Durable on purpose: the user has to act on it (re-seed from a peer), and a
/// warning that only exists in one round's return value is a warning nobody
/// sees.
pub const HISTORY_GAP_KEY: &str = "sync_history_gap";
/// Records why the push half of a sync round could not finish (relay quota
/// exhausted, device revoked, oversized op parked). Durable for the same
/// reason as [`HISTORY_GAP_KEY`] — background sync only eprintlns, and the
/// next successful push clears it, so it never outlives the problem.
pub const PUSH_BLOCKED_KEY: &str = "sync_push_blocked";
/// Keep enough room below the relay's 8 MiB hard limit for transport framing
/// and future protocol fields. A single oversized local row is parked (see
/// `hold_push_op`) instead of blocking every later operation behind it.
const MAX_PUSH_BLOB_BYTES: usize = 7 * 1024 * 1024;
const MAX_PUSH_OPS: i64 = 256;

/// Conservative serialized-size estimate for the `SyncBatch` envelope minus
/// its ops: `{"device":"...","ops":[...]}` plus one comma per op added by the
/// caller. The estimate only has to under-count never over-count relative to
/// the real ciphertext, which additionally carries a 24-byte nonce and a
/// 16-byte tag.
fn push_batch_overhead_bytes(device: &str) -> usize {
    serde_json::to_string(&SyncBatch {
        device: device.to_string(),
        ops: Vec::new(),
    })
    .map(|json| json.len() + 24 + 16)
    .unwrap_or(MAX_PUSH_BLOB_BYTES)
}

pub(super) fn cursor(store: &Store, key: &str) -> Result<i64> {
    Ok(store
        .sync_state(key)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// One push + pull + merge round. Safe to call repeatedly; every step is
/// idempotent (re-delivered ops lose the LWW comparison and are skipped).
pub fn sync_once(
    store: &Store,
    transport: &dyn SyncTransport,
    cfg: &SyncConfig,
) -> Result<SyncOutcome> {
    let key = cfg.key_bytes()?;
    let device = store.device_id()?;
    let mut outcome = SyncOutcome::default();

    // Push local ops in bounded encrypted blobs. A backlog can grow far past
    // the relay's request cap (especially with health samples), so advance the
    // cursor only after each individual blob is accepted.
    //
    // A permanent refusal must not wedge the device: before the poison-pill
    // handling, a relay 507/403 (or one oversized op) returned Err out of
    // `sync_once` — and since push runs *before* pull, the device not only
    // retried the same doomed upload every round forever, it also never
    // reached the pull half, so it stopped receiving its peers' data too.
    // The pull side solved the same shape by parking bad blobs; here the
    // equivalent is skipping the rest of the push half, counting what was
    // left behind, and recording why durably.
    let mut pushed_cursor = cursor(store, PUSHED_KEY)?;
    let mut push_block: Option<String> = None;
    'push: loop {
        let ops = store.local_ops_after_limited(pushed_cursor, MAX_PUSH_OPS)?;
        if ops.is_empty() {
            break;
        }

        let mut pos = 0usize;
        while pos < ops.len() {
            // Pack by measured sizes: serialize each op exactly once and add
            // up the lengths, instead of re-cloning, re-serializing and
            // re-encrypting the whole candidate batch after every added op.
            // That was quadratic work on precisely the offline-catch-up
            // backlog where a phone has the least CPU to spare. Counting is
            // conservative (one comma per op, full AEAD framing), and the
            // 7 MiB client cap sits 1 MiB below the relay's own limit, so an
            // off-by-a-few estimate cannot turn into a server rejection.
            let mut batch_ops: Vec<&SyncOp> = Vec::new();
            let mut batch_bytes = push_batch_overhead_bytes(&device);
            let mut last_id = pushed_cursor;
            let mut oversize_single: Option<(i64, &SyncOp)> = None;
            while pos < ops.len() {
                let (id, op) = &ops[pos];
                let op_bytes = serde_json::to_string(op)?.len() + 1;
                if batch_bytes + op_bytes > MAX_PUSH_BLOB_BYTES {
                    if batch_ops.is_empty() {
                        // One op alone exceeds the cap: it can never be
                        // uploaded. Failing used to stop every later op behind
                        // it forever. Park it (the oplog row itself is kept,
                        // so a future build with a larger cap or compression
                        // can still find it) and move the cursor past it —
                        // the push-side mirror of `hold_bad_blob`.
                        oversize_single = Some((*id, op));
                    }
                    break;
                }
                batch_bytes += op_bytes;
                batch_ops.push(op);
                last_id = *id;
                pos += 1;
            }

            if let Some((id, op)) = oversize_single {
                let reason = format!(
                    "单条同步变更超过 {} MiB，无法安全上传",
                    MAX_PUSH_BLOB_BYTES / 1024 / 1024
                );
                store.hold_push_op(id, op, &reason)?;
                pushed_cursor = id;
                store.set_sync_state(PUSHED_KEY, &pushed_cursor.to_string())?;
                outcome.push_skipped += 1;
                // The oversized op sits at `pos`; advancing past it is what
                // keeps this loop from retrying it forever.
                pos += 1;
                continue;
            }
            if batch_ops.is_empty() {
                continue;
            }
            let count = batch_ops.len();
            let batch = SyncBatch {
                device: device.clone(),
                ops: batch_ops.into_iter().cloned().collect(),
            };
            let blob = encrypt(&key, serde_json::to_string(&batch)?.as_bytes())?;
            match transport.push(&device, &blob) {
                Ok(_) => {
                    store.set_sync_state(PUSHED_KEY, &last_id.to_string())?;
                    pushed_cursor = last_id;
                    outcome.pushed += count;
                    // A successful upload is the precise proof that the relay
                    // accepts us again — the moment a stale push-block marker
                    // loses its truth. Clearing here (not "any round that
                    // finished") keeps the marker honest across a round that,
                    // say, never reached the server at all.
                    store.clear_sync_state(PUSH_BLOCKED_KEY)?;
                }
                Err(CoreError::SyncPushRejected(reason)) => {
                    // The relay said no and will keep saying no this round.
                    // Count everything left behind (the failed batch plus the
                    // rest of the backlog) so "skipped" is a real number, then
                    // go pull — receiving is exactly what must keep working.
                    outcome.push_skipped += store.local_ops_count_after(pushed_cursor)? as usize;
                    push_block = Some(reason);
                    break 'push;
                }
                // Transient failure (network, timeout, 5xx): the same blob can
                // genuinely succeed next round, so it stays queued and nothing
                // durable is recorded. But it used to abort the whole round —
                // which also cancelled the pull half, so a flaky *upload* path
                // made the device stop *receiving*. Skip the rest of the push
                // half and let the pull half do what it still can.
                Err(e) => {
                    outcome.push_failed = Some(match e {
                        CoreError::Invalid(message) => message,
                        other => other.to_string(),
                    });
                    break 'push;
                }
            }
        }

        if ops.len() < MAX_PUSH_OPS as usize {
            break;
        }
    }

    if let Some(reason) = &push_block {
        store.set_sync_state(PUSH_BLOCKED_KEY, reason)?;
    }
    outcome.push_block = push_block;

    // Pull + merge everyone else's.
    let since = cursor(store, PULLED_KEY)?;
    let blobs = transport.pull(since, &device)?;
    // Retention gap check, before merging anything.
    //
    // `since > 0` excludes a first-ever sync, which legitimately starts below
    // the floor. Beyond that, a cursor under the oldest surviving sequence
    // means the relay swept ops this device never saw. We cannot recover them
    // here, so the one thing that must not happen is reporting success: the
    // flag is recorded durably and surfaced, and the merge still proceeds so
    // the device at least catches up on what *does* remain.
    if let Some(oldest) = transport.oldest_seq() {
        if since > 0 && oldest > since + 1 {
            outcome.history_gap = true;
            store.set_sync_state(HISTORY_GAP_KEY, &format!("cursor={since};oldest={oldest}"))?;
        }
    }
    outcome.pulled_blobs = blobs.len();
    for b in &blobs {
        // Opening a blob is all-or-nothing and entirely outside our control:
        // corrupt base64, a peer with a different SOLUM_SYNC_KEY, a truncated
        // upload. Returning Err here (as this used to) leaves the cursor
        // unadvanced, so the *same* blob is re-fetched and re-fails on every
        // round forever, and every later blob behind it never arrives — one
        // bad byte permanently deafens the device. Park it and move on; the
        // ciphertext is kept so it can still be recovered later.
        let opened = B64
            .decode(&b.blob_b64)
            .map_err(|e| format!("base64 解码失败: {e}"))
            .and_then(|raw| decrypt(&key, &raw).map_err(|e| e.to_string()))
            .and_then(|plain| {
                serde_json::from_slice::<SyncBatch>(&plain)
                    .map_err(|e| format!("批次不可解析: {e}"))
            });
        let batch = match opened {
            Ok(batch) => batch,
            Err(reason) => {
                store.hold_bad_blob(b.seq, &b.device, &b.blob_b64, &reason)?;
                outcome.bad_blobs += 1;
                store.set_sync_state(PULLED_KEY, &b.seq.to_string())?;
                continue;
            }
        };
        let counts = store.apply_remote_ops(&batch.ops)?;
        outcome.applied += counts.applied;
        outcome.skipped += counts.skipped;
        outcome.quarantined += counts.quarantined;
        store.set_sync_state(PULLED_KEY, &b.seq.to_string())?;
    }
    Ok(outcome)
}

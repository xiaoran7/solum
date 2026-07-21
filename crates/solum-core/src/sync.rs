//! Multi-device sync client (F17, ARCHITECTURE.md §3.8) — local-first.
//!
//! Each device's SQLite store is authoritative. Mutations are captured into
//! `sync_oplog` by triggers (see `store::migrate_sync`); this module pushes
//! locally-originated ops to the relay as an **end-to-end encrypted** blob and
//! pulls + merges other devices' blobs (last-write-wins per row on a
//! `(hlc, origin)` stamp). The relay (`solum-sync-server`) only ever sees
//! ciphertext: key management is the pre-shared master key decided 2026-07-07
//! — one symmetric key manually configured on every device via `solum-sync.json`
//! (gitignored, same pattern as `solum-llm.json`) or `SOLUM_SYNC_*` env vars.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
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
}

// ---- config -------------------------------------------------------------------

/// Client-side sync settings. Loaded from env (`SOLUM_SYNC_URL` / `SOLUM_SYNC_TOKEN`
/// / `SOLUM_SYNC_KEY`) first, then `solum-sync.json` next to the executable's cwd —
/// the same lookup pattern as `LlmConfig`. Absent config means sync is off.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncConfig {
    /// Relay base URL, e.g. `http://127.0.0.1:8787`.
    pub url: String,
    /// Bearer token shared with the relay.
    pub token: String,
    /// 64 hex chars — the 32-byte pre-shared master key (§3.8).
    pub key: String,
}

impl SyncConfig {
    pub fn load() -> Option<SyncConfig> {
        if let (Ok(url), Ok(token), Ok(key)) = (
            std::env::var("SOLUM_SYNC_URL"),
            std::env::var("SOLUM_SYNC_TOKEN"),
            std::env::var("SOLUM_SYNC_KEY"),
        ) {
            return Some(SyncConfig { url, token, key });
        }
        let raw =
            std::fs::read_to_string(crate::paths::resolve_with_adoption("solum-sync.json")).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Redacted one-liner for status displays.
    pub fn masked_summary(&self) -> String {
        format!(
            "{} (key ****{})",
            self.url,
            &self.key[self.key.len().saturating_sub(4)..]
        )
    }

    fn key_bytes(&self) -> Result<[u8; 32]> {
        let hex = self.key.trim();
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CoreError::Invalid(
                "SOLUM_SYNC_KEY 必须是 64 位十六进制（32 字节主密钥）".into(),
            ));
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| CoreError::Invalid("SOLUM_SYNC_KEY 不是合法十六进制".into()))?;
        }
        Ok(out)
    }
}

// ---- crypto (end-to-end; the relay never sees plaintext) ------------------------

/// XChaCha20-Poly1305 with a random 24-byte nonce prepended to the ciphertext.
fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; 24];
    getrandom::getrandom(&mut nonce).map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| CoreError::Invalid("加密失败".into()))?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(out)
}

fn decrypt(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 24 {
        return Err(CoreError::Invalid("同步数据太短，不是合法密文".into()));
    }
    let (nonce, ct) = blob.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce), ct)
        .map_err(|_| CoreError::Invalid("解密失败——各设备的 SOLUM_SYNC_KEY 是否一致？".into()))
}

// ---- transport ------------------------------------------------------------------

/// How encrypted blobs reach the relay. Abstracted so tests can run the full
/// merge pipeline without a network.
pub trait SyncTransport {
    /// Upload one blob; returns the relay-assigned sequence number.
    fn push(&self, device: &str, blob: &[u8]) -> Result<i64>;
    /// Blobs with `seq > since` from devices other than `device`.
    fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>>;
    /// Lowest sequence the relay still holds, or `None` if it does not say.
    ///
    /// Relays enforce retention, so history below this is gone. A client whose
    /// cursor sits below it has missed ops it can never fetch — and would
    /// otherwise pull "everything after my cursor", get a later unrelated set,
    /// and call that a successful sync. Must be read from the *same* response
    /// as `pull` to be meaningful.
    fn oldest_seq(&self) -> Option<i64> {
        None
    }
}

/// Connect timeout for relay calls. A relay that cannot be reached should fail
/// in seconds, not hang: sync runs on the ticker, and a hung request used to
/// hold the orchestrator lock (and therefore reminders) with it.
const CONNECT_TIMEOUT_SECS: u64 = 10;
/// Whole-request ceiling, connect + TLS + transfer. Bounded on purpose — a
/// half-open connection to a dead relay otherwise blocks until the OS gives up,
/// which on some platforms is minutes.
const REQUEST_TIMEOUT_SECS: u64 = 60;
/// Largest pull response we will buffer. The relay pages at 500 blobs × up to
/// 8 MiB, which is multiple GiB of JSON if a peer (or the relay itself) is
/// hostile or broken. We are not obliged to hold that in memory.
const MAX_PULL_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// The real relay client (`solum-sync-server` wire protocol).
pub struct HttpTransport {
    url: String,
    token: String,
    agent: ureq::Agent,
    /// Captured from the last `pull` response. Interior mutability because the
    /// trait takes `&self` — and it has to come from the *same* response as the
    /// blobs, or the two could describe different relay states.
    oldest_seq: std::cell::Cell<Option<i64>>,
}

impl HttpTransport {
    /// Fails if the relay URL is not an acceptable endpoint (see
    /// [`crate::net::validate_endpoint`]) — the sync token and every op in the
    /// oplog would otherwise travel in cleartext.
    pub fn new(cfg: &SyncConfig) -> Result<HttpTransport> {
        let url = crate::net::validate_endpoint(&cfg.url, "同步服务器 url")?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build();
        Ok(HttpTransport {
            url,
            token: cfg.token.clone(),
            agent,
            oldest_seq: std::cell::Cell::new(None),
        })
    }
}

impl SyncTransport for HttpTransport {
    fn push(&self, device: &str, blob: &[u8]) -> Result<i64> {
        let resp: serde_json::Value = self
            .agent
            .post(&format!("{}/v1/push", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("X-Device", device)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(blob)
            .map_err(|e| CoreError::Invalid(format!("同步服务器 push 失败: {e}")))?
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("同步服务器响应不可解析: {e}")))?;
        resp.get("seq")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CoreError::Invalid("push 响应缺少 seq".into()))
    }

    fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>> {
        let reader = self
            .agent
            .get(&format!(
                "{}/v1/pull?since={since}&device={device}",
                self.url
            ))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| CoreError::Invalid(format!("同步服务器 pull 失败: {e}")))?
            .into_reader();
        // Read through a cap rather than `into_json()` straight off the socket:
        // `into_json` will happily allocate whatever the peer sends.
        use std::io::Read as _;
        let mut buf = Vec::new();
        reader
            .take(MAX_PULL_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| CoreError::Invalid(format!("读取同步响应失败: {e}")))?;
        if buf.len() > MAX_PULL_RESPONSE_BYTES {
            return Err(CoreError::Invalid(format!(
                "同步响应超过 {} MiB 上限，已拒绝",
                MAX_PULL_RESPONSE_BYTES / 1024 / 1024
            )));
        }
        let body: serde_json::Value = serde_json::from_slice(&buf)
            .map_err(|e| CoreError::Invalid(format!("同步服务器响应不可解析: {e}")))?;
        // Older relays do not send this; `None` then means "cannot tell",
        // which the caller treats as "no gap detected" rather than inventing one.
        self.oldest_seq
            .set(body.get("oldest_seq").and_then(|v| v.as_i64()));
        let blobs = body
            .get("blobs")
            .cloned()
            .ok_or_else(|| CoreError::Invalid("pull 响应缺少 blobs".into()))?;
        Ok(serde_json::from_value(blobs)?)
    }

    fn oldest_seq(&self) -> Option<i64> {
        self.oldest_seq.get()
    }
}

// ---- driver ---------------------------------------------------------------------

const PUSHED_KEY: &str = "sync_pushed_oplog_id";
const PULLED_KEY: &str = "sync_pulled_seq";
/// Records that this device's cursor fell below the relay's retention floor.
/// Durable on purpose: the user has to act on it (re-seed from a peer), and a
/// warning that only exists in one round's return value is a warning nobody
/// sees.
pub const HISTORY_GAP_KEY: &str = "sync_history_gap";
/// Keep enough room below the relay's 8 MiB hard limit for transport framing
/// and future protocol fields. A single oversized local row fails loudly
/// instead of blocking every later operation behind it.
const MAX_PUSH_BLOB_BYTES: usize = 7 * 1024 * 1024;
const MAX_PUSH_OPS: i64 = 256;

fn cursor(store: &Store, key: &str) -> Result<i64> {
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
    let mut pushed_cursor = cursor(store, PUSHED_KEY)?;
    loop {
        let ops = store.local_ops_after_limited(pushed_cursor, MAX_PUSH_OPS)?;
        if ops.is_empty() {
            break;
        }

        let mut pos = 0usize;
        while pos < ops.len() {
            let mut batch_ops = Vec::new();
            let mut last_id = pushed_cursor;
            while pos < ops.len() {
                let (id, op) = &ops[pos];
                batch_ops.push(op.clone());
                let candidate = SyncBatch {
                    device: device.clone(),
                    ops: batch_ops.clone(),
                };
                let blob = encrypt(&key, serde_json::to_string(&candidate)?.as_bytes())?;
                if blob.len() > MAX_PUSH_BLOB_BYTES {
                    batch_ops.pop();
                    if batch_ops.is_empty() {
                        return Err(CoreError::Invalid(format!(
                            "单条同步变更超过 {} MiB，无法安全上传",
                            MAX_PUSH_BLOB_BYTES / 1024 / 1024
                        )));
                    }
                    break;
                }
                last_id = *id;
                pos += 1;
            }

            let count = batch_ops.len();
            let batch = SyncBatch {
                device: device.clone(),
                ops: batch_ops,
            };
            let blob = encrypt(&key, serde_json::to_string(&batch)?.as_bytes())?;
            transport.push(&device, &blob)?;
            store.set_sync_state(PUSHED_KEY, &last_id.to_string())?;
            pushed_cursor = last_id;
            outcome.pushed += count;
        }

        if ops.len() < MAX_PUSH_OPS as usize {
            break;
        }
    }

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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{Channel, Event, EventKind, Notification, NotificationStatus};
    use chrono::NaiveDate;
    use std::sync::Mutex;

    /// In-memory relay with the exact semantics of solum-sync-server.
    #[derive(Default)]
    pub struct MemTransport {
        rows: Mutex<Vec<(i64, String, Vec<u8>)>>,
    }

    impl SyncTransport for MemTransport {
        fn push(&self, device: &str, blob: &[u8]) -> Result<i64> {
            let mut rows = self.rows.lock().unwrap();
            let seq = rows.len() as i64 + 1;
            rows.push((seq, device.to_string(), blob.to_vec()));
            Ok(seq)
        }
        fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|(seq, dev, _)| *seq > since && dev != device)
                .map(|(seq, dev, blob)| PulledBlob {
                    seq: *seq,
                    device: dev.clone(),
                    blob_b64: B64.encode(blob),
                })
                .collect())
        }
    }

    pub fn test_cfg() -> SyncConfig {
        SyncConfig {
            url: "mem://".into(),
            token: "t".into(),
            key: "11".repeat(32),
        }
    }

    fn dt(d: u32, h: u32, mi: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn sample_event(title: &str) -> Event {
        Event::new(title, EventKind::Meeting, dt(9, 15, 0), title, dt(7, 10, 0))
    }

    fn add_event_with_reminder(s: &Store, title: &str) -> (i64, i64) {
        let ev = s.insert_event(&sample_event(title), None).unwrap();
        let nid = s
            .insert_notification(&Notification {
                id: None,
                event_id: ev,
                fire_at: dt(9, 14, 30),
                lead_label: "30m".into(),
                channels: vec![Channel::Push],
                status: NotificationStatus::Pending,
                created_at: dt(7, 10, 0),
                fired_at: None,
            })
            .unwrap();
        (ev, nid)
    }

    /// A relay that hands back one unopenable blob wedged between two good
    /// ones. Reproduces the poison-pill: before the fix, the bad blob failed
    /// the whole round, the cursor never moved, and blob 3 never arrived — on
    /// this round or any future one.
    #[derive(Default)]
    struct PoisonedTransport {
        rows: Mutex<Vec<(i64, String, String)>>,
    }
    impl SyncTransport for PoisonedTransport {
        fn push(&self, _device: &str, _blob: &[u8]) -> Result<i64> {
            Ok(0)
        }
        fn pull(&self, since: i64, _device: &str) -> Result<Vec<PulledBlob>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|(seq, _, _)| *seq > since)
                .map(|(seq, dev, b64)| PulledBlob {
                    seq: *seq,
                    device: dev.clone(),
                    blob_b64: b64.clone(),
                })
                .collect())
        }
    }

    #[test]
    fn one_unopenable_blob_does_not_wedge_the_pull_cursor() {
        let cfg = test_cfg();
        let key = cfg.key_bytes().unwrap();

        // Build the good blobs from a *real* peer store rather than hand-rolled
        // JSON, so the payload shape is whatever the current schema actually
        // emits — otherwise this test could pass by quarantining everything.
        let peer = Store::open_in_memory().unwrap();
        let mut sealed_cursor = 0i64;
        let mut seal = |title: &str| {
            peer.insert_event(&sample_event(title), None).unwrap();
            let ops = peer.local_ops_after_limited(sealed_cursor, 256).unwrap();
            sealed_cursor = ops.last().map(|(id, _)| *id).unwrap_or(sealed_cursor);
            let batch = SyncBatch {
                device: "peer".into(),
                ops: ops.into_iter().map(|(_, op)| op).collect(),
            };
            B64.encode(encrypt(&key, serde_json::to_string(&batch).unwrap().as_bytes()).unwrap())
        };

        let relay = PoisonedTransport::default();
        relay.rows.lock().unwrap().extend([
            (1, "peer".to_string(), seal("第一条")),
            // Valid base64, but not our ciphertext — wrong key / corruption.
            (2, "peer".to_string(), B64.encode(vec![0u8; 64])),
            (3, "peer".to_string(), seal("第三条")),
        ]);

        let store = Store::open_in_memory().unwrap();
        let out = sync_once(&store, &relay, &cfg).unwrap();

        assert_eq!(out.bad_blobs, 1, "the unopenable blob should be parked");
        assert_eq!(store.bad_blob_count().unwrap(), 1);
        // The blob *behind* the bad one is the whole point: it must arrive.
        let titles: Vec<String> = store
            .list_events()
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert!(titles.iter().any(|t| t == "第一条"), "got {titles:?}");
        assert!(titles.iter().any(|t| t == "第三条"), "got {titles:?}");

        // And the cursor really moved: a second round pulls nothing new.
        let again = sync_once(&store, &relay, &cfg).unwrap();
        assert_eq!(again.pulled_blobs, 0);
        assert_eq!(again.bad_blobs, 0);
    }

    /// A relay that has swept history below this device's cursor. Reproduces
    /// the regression the retention sweep introduced: without a gap signal the
    /// device pulls "everything after my cursor", gets a later unrelated set,
    /// and reports a clean sync while having permanently missed ops.
    struct SweptTransport {
        oldest: i64,
    }
    impl SyncTransport for SweptTransport {
        fn push(&self, _device: &str, _blob: &[u8]) -> Result<i64> {
            Ok(0)
        }
        fn pull(&self, _since: i64, _device: &str) -> Result<Vec<PulledBlob>> {
            Ok(Vec::new())
        }
        fn oldest_seq(&self) -> Option<i64> {
            Some(self.oldest)
        }
    }

    #[test]
    fn a_cursor_below_the_relays_retention_floor_is_reported_not_ignored() {
        let cfg = test_cfg();
        let store = Store::open_in_memory().unwrap();
        // This device last synced at seq 5; the relay has since swept
        // everything below 900.
        store.set_sync_state(PULLED_KEY, "5").unwrap();

        let out = sync_once(&store, &SweptTransport { oldest: 900 }, &cfg).unwrap();
        assert!(
            out.history_gap,
            "a swept-away history must not be reported as a clean sync"
        );
        assert!(
            store.sync_state(HISTORY_GAP_KEY).unwrap().is_some(),
            "the gap must be recorded durably, not just returned once"
        );
    }

    #[test]
    fn a_contiguous_cursor_is_not_mistaken_for_a_gap() {
        let cfg = test_cfg();
        let store = Store::open_in_memory().unwrap();
        store.set_sync_state(PULLED_KEY, "899").unwrap();
        // oldest == since + 1 means nothing was missed.
        let out = sync_once(&store, &SweptTransport { oldest: 900 }, &cfg).unwrap();
        assert!(!out.history_gap);

        // …and a first-ever sync (cursor 0) is never a gap.
        let fresh = Store::open_in_memory().unwrap();
        let out = sync_once(&fresh, &SweptTransport { oldest: 900 }, &cfg).unwrap();
        assert!(!out.history_gap);
    }

    #[test]
    fn relay_url_must_not_be_cleartext_http() {
        let bad = SyncConfig {
            url: "http://relay.example.com".into(),
            ..test_cfg()
        };
        assert!(HttpTransport::new(&bad).is_err());
        let ok = SyncConfig {
            url: "https://relay.example.com".into(),
            ..test_cfg()
        };
        assert!(HttpTransport::new(&ok).is_ok());
    }

    #[test]
    fn crypto_round_trip_and_key_validation() {
        let cfg = test_cfg();
        let key = cfg.key_bytes().unwrap();
        let blob = encrypt(&key, "机密内容".as_bytes()).unwrap();
        assert_ne!(&blob[24..], "机密内容".as_bytes());
        assert_eq!(decrypt(&key, &blob).unwrap(), "机密内容".as_bytes());
        // Wrong key → error, not garbage.
        let other = SyncConfig {
            key: "22".repeat(32),
            ..test_cfg()
        }
        .key_bytes()
        .unwrap();
        assert!(decrypt(&other, &blob).is_err());
        assert!(SyncConfig {
            key: "abc".into(),
            ..test_cfg()
        }
        .key_bytes()
        .is_err());
    }

    #[test]
    fn a_large_backlog_is_uploaded_in_multiple_cursor_safe_blobs() {
        let source = Store::open_in_memory().unwrap();
        for i in 0..300 {
            add_event_with_reminder(&source, &format!("批量同步-{i}"));
        }
        let relay = MemTransport::default();
        let cfg = test_cfg();

        let outcome = sync_once(&source, &relay, &cfg).unwrap();
        assert_eq!(outcome.pushed, 600);
        assert!(relay.rows.lock().unwrap().len() > 1);
        assert_eq!(source.sync_state(PUSHED_KEY).unwrap(), Some("600".into()));

        let destination = Store::open_in_memory().unwrap();
        sync_once(&destination, &relay, &cfg).unwrap();
        assert_eq!(destination.list_events().unwrap().len(), 300);
    }

    #[test]
    fn two_devices_converge() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        // A creates an event + reminder; B creates its own.
        add_event_with_reminder(&a, "A的会议");
        add_event_with_reminder(&b, "B的考试");

        // A pushes; B pulls A's data and pushes its own; A pulls B's.
        let oa = sync_once(&a, &relay, &cfg).unwrap();
        assert!(oa.pushed > 0);
        let ob = sync_once(&b, &relay, &cfg).unwrap();
        assert!(ob.applied > 0);
        let oa2 = sync_once(&a, &relay, &cfg).unwrap();
        assert!(oa2.applied > 0);

        let titles = |s: &Store| {
            let mut t: Vec<String> = s
                .list_events()
                .unwrap()
                .iter()
                .map(|e| e.title.clone())
                .collect();
            t.sort();
            t
        };
        assert_eq!(
            titles(&a),
            vec!["A的会议".to_string(), "B的考试".to_string()]
        );
        assert_eq!(titles(&a), titles(&b));
        assert_eq!(a.list_notifications().unwrap().len(), 2);
        assert_eq!(b.list_notifications().unwrap().len(), 2);

        // Idempotency: another round applies nothing new.
        let oa3 = sync_once(&a, &relay, &cfg).unwrap();
        assert_eq!((oa3.pushed, oa3.applied), (0, 0));
    }

    /// A blob as a *newer* build would emit it: one op this build understands,
    /// one naming a table it has never heard of.
    fn blob_from_a_newer_peer(cfg: &SyncConfig) -> Vec<u8> {
        let batch = SyncBatch {
            device: "future".into(),
            ops: vec![
                SyncOp {
                    tbl: "events".into(),
                    guid: "e".repeat(32),
                    op: "upsert".into(),
                    payload: Some(
                        serde_json::json!({
                            "title": "新版设备建的会议",
                            "kind": "meeting",
                            "start": crate::model::fmt_ts(&dt(9, 15, 0)),
                            "people_json": "[]",
                            "created_at": crate::model::fmt_ts(&dt(7, 10, 0)),
                        })
                        .to_string(),
                    ),
                    hlc: "2026-07-07T10:00:00.000".into(),
                    origin: "future".into(),
                },
                SyncOp {
                    tbl: "widget_defs".into(),
                    guid: "w".repeat(32),
                    op: "upsert".into(),
                    payload: Some(r#"{"name":"日程表"}"#.into()),
                    hlc: "2026-07-07T10:00:01.000".into(),
                    origin: "future".into(),
                },
            ],
        };
        encrypt(
            &cfg.key_bytes().unwrap(),
            serde_json::to_string(&batch).unwrap().as_bytes(),
        )
        .unwrap()
    }

    /// Regression (2026-07-19): an op from a table this build does not know
    /// must not wedge the pull cursor. The catch-all arm of `apply_one` used
    /// to return `Err`, which rolled the whole merge transaction back — losing
    /// the legitimate ops sharing the blob — *and* left `sync_pulled_seq`
    /// unadvanced, so every later round re-fetched the same blob and failed
    /// identically. Push runs before pull, so the device kept uploading while
    /// silently never receiving again.
    #[test]
    fn an_op_from_a_newer_peer_does_not_wedge_the_pull_cursor() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let old = Store::open_in_memory().unwrap();
        relay.push("future", &blob_from_a_newer_peer(&cfg)).unwrap();

        let outcome = sync_once(&old, &relay, &cfg).unwrap();

        // The known op still lands — one unknown sibling must not void it.
        let titles: Vec<String> = old
            .list_events()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["新版设备建的会议".to_string()]);
        // The unknown one is held for a later build, not dropped, not fatal.
        assert_eq!(outcome.quarantined, 1);
        // And the cursor moved, so the next round is not a replay of this one.
        assert_eq!(cursor(&old, PULLED_KEY).unwrap(), 1);
        let again = sync_once(&old, &relay, &cfg).unwrap();
        assert_eq!((again.pulled_blobs, again.applied), (0, 0));
    }

    #[test]
    fn health_samples_sync_and_dedup_survive_round_trip() {
        use crate::wearable::{HealthMetric, HealthSample};
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let hr = HealthSample::new(
            HealthMetric::HeartRate,
            dt(12, 9, 0),
            dt(12, 9, 0),
            72.0,
            "health_connect",
        );
        a.insert_health_sample_if_new(&hr).unwrap();

        sync_once(&a, &relay, &cfg).unwrap();
        let ob = sync_once(&b, &relay, &cfg).unwrap();
        assert!(ob.applied > 0);

        let samples = b.list_health_samples().unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].kind, HealthMetric::HeartRate);
        assert_eq!(samples[0].value, 72.0);

        // B re-polling the same instant locally must stay deduped even after
        // the row arrived via sync (dedup_key survives the round trip).
        assert_eq!(b.insert_health_sample_if_new(&hr).unwrap(), None);
    }

    #[test]
    fn soulous_facts_sync_with_stable_guids_lww_and_deletes() {
        use crate::soulous::{SoulousFact, SoulousKind, SOURCE};

        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();
        let fact = |title: &str, imported_at| SoulousFact {
            id: None,
            external_id: "exam-42".into(),
            kind: SoulousKind::Exam,
            title: title.into(),
            occurs_at: Some(dt(20, 9, 0)),
            ends_at: None,
            payload_json: r#"{"id":42,"courseName":"算法"}"#.into(),
            source: SOURCE.into(),
            imported_at,
        };

        a.replace_soulous_snapshot(&[fact("算法", dt(18, 10, 0))])
            .unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert_eq!(b.list_soulous_facts().unwrap()[0].title, "算法");

        // A later snapshot updates the same stable source guid, so B's row
        // converges by the normal trigger/LWW path rather than duplicating it.
        std::thread::sleep(std::time::Duration::from_millis(5));
        a.replace_soulous_snapshot(&[fact("算法（调考）", dt(18, 11, 0))])
            .unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        let after = b.list_soulous_facts().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].title, "算法（调考）");

        std::thread::sleep(std::time::Duration::from_millis(5));
        a.replace_soulous_snapshot(&[]).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert!(b.list_soulous_facts().unwrap().is_empty());
    }

    #[test]
    fn delete_propagates_and_wins() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let (ev, _) = add_event_with_reminder(&a, "要删的会");
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert_eq!(b.list_events().unwrap().len(), 1);

        // A deletes (cascade removes the reminder too) → B converges.
        a.delete_event(ev).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert!(b.list_events().unwrap().is_empty());
        assert!(b.list_notifications().unwrap().is_empty());
        assert!(a.list_events().unwrap().is_empty());
    }

    #[test]
    fn persona_versions_merge_with_renumber_and_active_pointer() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        // Both devices independently create their own v1 (version collision).
        let da = crate::persona::PersonaDraft {
            tone: "干练".into(),
            ..Default::default()
        };
        let db = crate::persona::PersonaDraft {
            tone: "活泼".into(),
            ..Default::default()
        };
        a.insert_persona_version(&da, "manual", None, dt(7, 10, 0))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5)); // hlc 分离
        b.insert_persona_version(&db, "manual", None, dt(7, 11, 0))
            .unwrap();

        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();

        // Both stores hold both versions (renumbered locally, history intact).
        assert_eq!(a.list_persona_versions().unwrap().len(), 2);
        assert_eq!(b.list_persona_versions().unwrap().len(), 2);
        // The active pointer converges on the later write (B's 活泼).
        assert_eq!(a.active_persona().unwrap().unwrap().draft.tone, "活泼");
        assert_eq!(b.active_persona().unwrap().unwrap().draft.tone, "活泼");
    }

    #[test]
    fn lww_update_wins_over_older_state() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let (_, nid) = add_event_with_reminder(&a, "状态之争");
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();

        // B dismisses the reminder later than A created it → dismissal wins on A.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b_nid = b.list_notifications().unwrap()[0].id.unwrap();
        b.dismiss_notification(b_nid).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        let n = a
            .list_notifications()
            .unwrap()
            .into_iter()
            .find(|n| n.id == Some(nid))
            .unwrap();
        assert_eq!(n.status, NotificationStatus::Dismissed);
    }

    #[test]
    fn rule_table_and_proactivity_sync_as_lww_docs() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let mut c = crate::proactivity::ProactivityConfig::defaults();
        c.set(
            crate::proactivity::ProactivityDimension::WeeklyReview,
            crate::proactivity::ProactivityLevel::Butler,
        );
        a.save_proactivity(&c).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert_eq!(b.load_proactivity().unwrap(), c);
    }
}

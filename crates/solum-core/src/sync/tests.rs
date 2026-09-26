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
        account: None,
    }
}

#[test]
fn derive_credentials_is_deterministic() {
    let a = derive_credentials("solum", "514000").unwrap();
    let b = derive_credentials("solum", "514000").unwrap();
    assert_eq!(a, b);
}

#[test]
fn derive_credentials_diverges_on_username_or_password() {
    let base = derive_credentials("solum", "514000").unwrap();
    let other_user = derive_credentials("xiaoran", "514000").unwrap();
    let other_pass = derive_credentials("solum", "999999").unwrap();
    assert_ne!(base, other_user);
    assert_ne!(base, other_pass);
}

#[test]
fn derive_credentials_token_and_key_are_independent_looking() {
    let (token, key) = derive_credentials("solum", "514000").unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(key.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(token, key);
}

#[test]
fn recovery_envelope_binds_password_and_account_uuid() {
    let user = "9d4df1be-9f7b-4a3a-b986-ec920d2df60e";
    let other = "6ea64c69-0531-45cb-b585-500c5f479fd8";
    let master = [0x5au8; 32];
    let envelope = wrap_recovery_key(user, "correct horse", &master).unwrap();
    assert_eq!(
        unwrap_recovery_key(user, "correct horse", &envelope).unwrap(),
        master
    );
    assert!(unwrap_recovery_key(user, "wrong", &envelope).is_err());
    assert!(unwrap_recovery_key(other, "correct horse", &envelope).is_err());
    assert!(!envelope
        .windows(master.len())
        .any(|window| window == master));
}

#[derive(Default)]
struct MemRecoveryTransport {
    envelope: Mutex<Option<Vec<u8>>>,
}

impl RecoveryEnvelopeTransport for MemRecoveryTransport {
    fn get(&self, _session: &crate::account::AccountSession) -> Result<Option<Vec<u8>>> {
        Ok(self.envelope.lock().unwrap().clone())
    }

    fn put_if_absent(
        &self,
        _session: &crate::account::AccountSession,
        proposed: &[u8],
    ) -> Result<Vec<u8>> {
        let mut envelope = self.envelope.lock().unwrap();
        Ok(envelope.get_or_insert_with(|| proposed.to_vec()).clone())
    }
}

fn recovery_session() -> crate::account::AccountSession {
    crate::account::AccountSession {
        server_url: "https://cloud.example".into(),
        user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
        username: "alice".into(),
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        model: crate::account::DEFAULT_CLOUD_MODEL.into(),
    }
}

#[test]
fn account_sync_refuses_sending_access_token_to_another_origin() {
    let session = recovery_session();
    let wrong = SyncConfig {
        url: "https://relay.attacker.example".into(),
        token: String::new(),
        key: "11".repeat(32),
        account: Some(session.clone()),
    };
    assert!(HttpTransport::new(&wrong).is_err());
    let same = SyncConfig {
        url: session.server_url.clone(),
        token: String::new(),
        key: "11".repeat(32),
        account: Some(session),
    };
    assert!(HttpTransport::new(&same).is_ok());
}

#[test]
fn first_device_creates_and_second_device_recovers_same_random_key() {
    let transport = MemRecoveryTransport::default();
    let unique = format!(
        "solum-recovery-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    let dir = std::env::temp_dir().join(unique);
    let first = dir.join("first.json");
    let second = dir.join("second.json");
    let session = recovery_session();

    assert_eq!(
        provision_account_sync_with(&transport, &session, "pw", &first).unwrap(),
        RecoveryProvision::Created
    );
    assert_eq!(
        provision_account_sync_with(&transport, &session, "pw", &second).unwrap(),
        RecoveryProvision::Recovered
    );
    assert_eq!(
        read_local_account_key(&first).unwrap(),
        read_local_account_key(&second).unwrap()
    );
    assert!(provision_account_sync_with(&transport, &session, "wrong", &second).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sync_config_file_parses_all_supported_shapes() {
    let creds: SyncConfigFile = serde_json::from_str(
        r#"{"url":"https://relay.example","username":"solum","password":"514000"}"#,
    )
    .unwrap();
    match creds {
        SyncConfigFile::Credentials {
            url,
            username,
            password,
        } => {
            assert_eq!(url, "https://relay.example");
            assert_eq!(username, "solum");
            assert_eq!(password, "514000");
        }
        SyncConfigFile::Direct { .. } | SyncConfigFile::Account { .. } => {
            panic!("expected Credentials shape")
        }
    }

    let direct: SyncConfigFile = serde_json::from_str(
        &serde_json::json!({"url": "https://relay.example", "token": "t", "key": "11".repeat(32)})
            .to_string(),
    )
    .unwrap();
    match direct {
        SyncConfigFile::Direct { url, token, key } => {
            assert_eq!(url, "https://relay.example");
            assert_eq!(token, "t");
            assert_eq!(key, "11".repeat(32));
        }
        SyncConfigFile::Credentials { .. } | SyncConfigFile::Account { .. } => {
            panic!("expected Direct shape")
        }
    }

    let account: SyncConfigFile = serde_json::from_str(
        &serde_json::json!({"url": "https://relay.example", "key": "22".repeat(32)}).to_string(),
    )
    .unwrap();
    match account {
        SyncConfigFile::Account { url, key } => {
            assert_eq!(url, "https://relay.example");
            assert_eq!(key, "22".repeat(32));
        }
        _ => panic!("expected Account shape"),
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

/// A relay that permanently refuses every push — quota exhausted, device
/// revoked — while pulls keep working. Before the poison-pill handling a
/// refusal aborted the whole round, and since push runs before pull the
/// device stopped *receiving* too: one full answer from the server and the
/// device went permanently deaf and mute. `transient` switches it to a
/// plain network-style failure instead, for the retryable-error path.
struct RefusingTransport {
    refuse: std::sync::atomic::AtomicBool,
    transient: std::sync::atomic::AtomicBool,
    rows: Mutex<Vec<(i64, String, String)>>,
}
impl RefusingTransport {
    fn push_blob(&self, device: &str, blob_b64: String) {
        let mut rows = self.rows.lock().unwrap();
        let seq = rows.len() as i64 + 1;
        rows.push((seq, device.to_string(), blob_b64));
    }
}
impl Default for RefusingTransport {
    fn default() -> Self {
        Self {
            refuse: std::sync::atomic::AtomicBool::new(true),
            transient: std::sync::atomic::AtomicBool::new(false),
            rows: Mutex::new(Vec::new()),
        }
    }
}
impl SyncTransport for RefusingTransport {
    fn push(&self, _device: &str, _blob: &[u8]) -> Result<i64> {
        if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
            if self.transient.load(std::sync::atomic::Ordering::SeqCst) {
                Err(CoreError::Invalid("同步服务器请求失败: 连接超时".into()))
            } else {
                Err(CoreError::SyncPushRejected(
                    "同步服务器存储配额已满（507）".into(),
                ))
            }
        } else {
            Ok(0)
        }
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
fn a_permanent_push_refusal_skips_the_push_half_but_still_pulls() {
    let cfg = test_cfg();
    let key = cfg.key_bytes().unwrap();

    // A peer blob waiting on the relay — the data this device must not
    // lose access to just because its own uploads are refused.
    let peer = Store::open_in_memory().unwrap();
    peer.insert_event(&sample_event("对端的数据"), None)
        .unwrap();
    let peer_ops = peer.local_ops_after(0).unwrap();
    let peer_batch = SyncBatch {
        device: "peer".into(),
        ops: peer_ops.into_iter().map(|(_, op)| op).collect(),
    };
    let relay = RefusingTransport::default();
    relay.push_blob(
        "peer",
        B64.encode(encrypt(&key, serde_json::to_string(&peer_batch).unwrap().as_bytes()).unwrap()),
    );

    let store = Store::open_in_memory().unwrap();
    add_event_with_reminder(&store, "本机推不出去的会议");

    let out = sync_once(&store, &relay, &cfg).unwrap();

    assert!(out.push_block.is_some(), "the refusal must be reported");
    assert_eq!(out.pushed, 0, "nothing can upload against a 507");
    assert!(
        out.push_skipped > 0,
        "the left-behind backlog must be counted"
    );
    // The whole point of skipping instead of failing: the pull half ran.
    assert!(out.applied > 0, "got applied={}", out.applied);
    assert!(store
        .list_events()
        .unwrap()
        .iter()
        .any(|e| e.title == "对端的数据"));
    // Durable, not just this round's return value — background sync only
    // logs, so a marker nobody persists is a marker nobody sees.
    assert!(store.sync_state(PUSH_BLOCKED_KEY).unwrap().is_some());
}

#[test]
fn a_transient_push_failure_still_pulls_and_leaves_no_sticky_marker() {
    let cfg = test_cfg();
    let key = cfg.key_bytes().unwrap();

    let peer = Store::open_in_memory().unwrap();
    peer.insert_event(&sample_event("对端的数据"), None)
        .unwrap();
    let peer_ops = peer.local_ops_after(0).unwrap();
    let peer_batch = SyncBatch {
        device: "peer".into(),
        ops: peer_ops.into_iter().map(|(_, op)| op).collect(),
    };
    let relay = RefusingTransport::default();
    relay
        .transient
        .store(true, std::sync::atomic::Ordering::SeqCst);
    relay.push_blob(
        "peer",
        B64.encode(encrypt(&key, serde_json::to_string(&peer_batch).unwrap().as_bytes()).unwrap()),
    );

    let store = Store::open_in_memory().unwrap();
    add_event_with_reminder(&store, "本机的会议");

    let out = sync_once(&store, &relay, &cfg).unwrap();

    assert!(out.push_failed.is_some(), "the failure must be reported");
    assert!(out.push_block.is_none(), "transient is not a refusal");
    // The queue stays queued for the next round…
    assert!(out.push_skipped == 0 && out.pushed == 0);
    assert_eq!(
        cursor(&store, PUSHED_KEY).unwrap(),
        0,
        "the push cursor must stay queued for the next round"
    );
    // …nothing durable is recorded — a network blip must not sit in the
    // sticky status view next to problems that need a human.
    assert_eq!(store.sync_state(PUSH_BLOCKED_KEY).unwrap(), None);
    // And the pull half still ran.
    assert!(out.applied > 0);
}

#[test]
fn a_backlog_of_only_oversized_ops_terminates_without_pushing_empty_blobs() {
    let cfg = test_cfg();
    let relay = MemTransport::default();
    let store = Store::open_in_memory().unwrap();

    let mut a = sample_event("超大甲");
    a.title = "巨".repeat(2_500_000);
    store.insert_event(&a, None).unwrap();
    let mut b = sample_event("超大乙");
    b.title = "巨".repeat(2_500_000);
    store.insert_event(&b, None).unwrap();

    let out = sync_once(&store, &relay, &cfg).unwrap();

    assert_eq!(out.push_skipped, 2);
    assert_eq!(out.pushed, 0);
    // MemTransport records every push as a row — an empty list means not
    // even one (empty) blob was offered. A real relay 400s on an empty
    // body; only this test's guard keeps that from ever being attempted.
    assert!(relay.rows.lock().unwrap().is_empty());
    // The cursor sits past both parked ops, so nothing re-parks next round.
    assert_eq!(store.sync_state(PUSHED_KEY).unwrap(), Some("2".into()));
    let again = sync_once(&store, &relay, &cfg).unwrap();
    assert_eq!((again.pushed, again.push_skipped), (0, 0));
}

/// Locks the status-code table itself: 401 must stay out of the permanent
/// set (it belongs to the token-refresh path), and every deterministic
/// refusal the servers emit must be in it.
#[test]
fn permanent_push_statuses_are_exactly_the_deterministic_refusals() {
    for code in [400, 403, 413, 429, 507] {
        assert!(permanent_push_status(code), "{code} must be permanent");
    }
    for code in [401, 404, 408, 500, 502, 503] {
        assert!(!permanent_push_status(code), "{code} must stay retryable");
    }
}

#[test]
fn a_later_clean_round_clears_the_push_block_marker() {
    let cfg = test_cfg();
    let relay = RefusingTransport::default();
    let store = Store::open_in_memory().unwrap();
    add_event_with_reminder(&store, "先被拒绝的");

    sync_once(&store, &relay, &cfg).unwrap();
    assert!(store.sync_state(PUSH_BLOCKED_KEY).unwrap().is_some());

    // Quota raised / device re-approved: the very next round must both
    // upload and clear the stale marker — it must not outlive its cause
    // and cry wolf next to the genuinely sticky history-gap warning.
    relay
        .refuse
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let again = sync_once(&store, &relay, &cfg).unwrap();
    assert!(again.pushed > 0);
    assert!(again.push_block.is_none());
    assert_eq!(store.sync_state(PUSH_BLOCKED_KEY).unwrap(), None);
}

#[test]
fn an_oversized_single_op_is_parked_and_the_backlog_still_uploads() {
    let cfg = test_cfg();
    let relay = MemTransport::default();
    let store = Store::open_in_memory().unwrap();

    // One row whose payload alone exceeds the blob cap, with ordinary ops
    // behind it — the shape that used to `return Err` and strand every
    // later op behind it forever.
    let mut huge = sample_event("超大单条");
    huge.title = "巨".repeat(2_500_000); // ~7.5 MB UTF-8 > the 7 MiB cap
    store.insert_event(&huge, None).unwrap();
    add_event_with_reminder(&store, "后面正常的");

    let out = sync_once(&store, &relay, &cfg).unwrap();

    assert_eq!(out.push_skipped, 1, "the oversized op is counted");
    assert_eq!(store.push_hold_count().unwrap(), 1);
    assert!(
        out.push_block.is_none(),
        "parking one oversized op is not a relay-level block"
    );
    assert!(
        out.pushed >= 2,
        "ops behind the parked one must still upload"
    );
    // The parked op stays parked — no re-reading, no re-parking loop.
    let again = sync_once(&store, &relay, &cfg).unwrap();
    assert_eq!((again.pushed, again.push_skipped), (0, 0));
    assert_eq!(store.push_hold_count().unwrap(), 1);
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

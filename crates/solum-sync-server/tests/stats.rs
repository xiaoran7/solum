//! Integration test: spawns the real `solum-sync-server` binary (it has no
//! lib target — the binary *is* the product, see main.rs) and hits it over
//! HTTP, the same way a device or the dashboard would.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

const AUTH_SECRET: &str = "test-auth-secret-that-is-at-least-32-bytes";

struct Server {
    child: Child,
    base: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn spawn(token: &str, db: &std::path::Path) -> Server {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_solum-sync-server"))
        .env("SOLUM_SYNC_SERVER_TOKEN", token)
        .env_remove("SOLUM_AUTH_SECRET")
        .env("SOLUM_SYNC_SERVER_ADDR", &addr)
        .env("SOLUM_SYNC_SERVER_DB", db)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn solum-sync-server");
    let base = format!("http://{addr}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if ureq::get(&format!("{base}/v1/health"))
            .set("Authorization", &format!("Bearer {token}"))
            .call()
            .is_ok()
        {
            break;
        }
        if Instant::now() > deadline {
            panic!("server did not come up in time");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server { child, base }
}

fn account_token(username: &str) -> String {
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let payload = URL_SAFE_NO_PAD
        .encode(serde_json::json!({"sub":username,"exp":expires_at,"nonce":"test"}).to_string());
    let mut mac = Hmac::<Sha256>::new_from_slice(AUTH_SECRET.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    format!(
        "{payload}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn spawn_accounts(db: &std::path::Path, legacy_token: Option<&str>) -> Server {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let alice = account_token("alice");
    let mut command = Command::new(env!("CARGO_BIN_EXE_solum-sync-server"));
    command
        .env("SOLUM_AUTH_SECRET", AUTH_SECRET)
        .env("SOLUM_SYNC_SERVER_ADDR", &addr)
        .env("SOLUM_SYNC_SERVER_DB", db)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(token) = legacy_token {
        command.env("SOLUM_SYNC_SERVER_TOKEN", token);
    } else {
        command.env_remove("SOLUM_SYNC_SERVER_TOKEN");
    }
    let child = command.spawn().expect("spawn account sync server");
    let base = format!("http://{addr}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if ureq::get(&format!("{base}/v1/health"))
            .set("Authorization", &format!("Bearer {alice}"))
            .call()
            .is_ok()
        {
            break;
        }
        if Instant::now() > deadline {
            panic!("account sync server did not come up in time");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Server { child, base }
}

#[test]
fn stats_reports_totals_and_per_device_breakdown() {
    let dir = std::env::temp_dir().join(format!("solum-sync-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("stats.sqlite");
    let token = "test-token-stats";
    let srv = spawn(token, &db);

    ureq::post(&format!("{}/v1/push", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("X-Device", "phone")
        .send_bytes(b"hello-1")
        .expect("push from phone");
    ureq::post(&format!("{}/v1/push", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("X-Device", "phone")
        .send_bytes(b"hello-22")
        .expect("push from phone again");
    ureq::post(&format!("{}/v1/push", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("X-Device", "desktop")
        .send_bytes(b"hi")
        .expect("push from desktop");

    let resp = ureq::get(&format!("{}/v1/stats", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .expect("stats");
    let stats: serde_json::Value = resp.into_json().unwrap();

    assert_eq!(stats["total_blobs"], 3);
    assert_eq!(stats["total_bytes"], 7 + 8 + 2);
    assert_eq!(stats["oldest_seq"], 1);
    assert_eq!(stats["newest_seq"], 3);
    assert_eq!(stats["retention_days"], 30);
    assert_eq!(stats["tenant"], "legacy");
    assert_eq!(stats["auth_mode"], "legacy");

    let devices = stats["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    let phone = devices.iter().find(|d| d["device"] == "phone").unwrap();
    assert_eq!(phone["blob_count"], 2);
    assert_eq!(phone["bytes"], 7 + 8);
    assert_eq!(phone["last_seq"], 2);
    let desktop = devices.iter().find(|d| d["device"] == "desktop").unwrap();
    assert_eq!(desktop["blob_count"], 1);
    assert_eq!(desktop["last_seq"], 3);
}

#[test]
fn stats_requires_auth() {
    let dir = std::env::temp_dir().join(format!("solum-sync-test-noauth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("noauth.sqlite");
    let token = "test-token-noauth";
    let srv = spawn(token, &db);

    let err = ureq::get(&format!("{}/v1/stats", srv.base))
        .call()
        .unwrap_err();
    match err {
        ureq::Error::Status(code, _) => assert_eq!(code, 401),
        other => panic!("expected 401, got {other:?}"),
    }
}

#[test]
fn dashboard_is_served_without_auth() {
    let dir = std::env::temp_dir().join(format!("solum-sync-test-dash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("dash.sqlite");
    let token = "test-token-dash";
    let srv = spawn(token, &db);

    let resp = ureq::get(&srv.base).call().expect("dashboard");
    assert_eq!(resp.status(), 200);
    assert!(resp
        .header("Content-Type")
        .unwrap_or_default()
        .starts_with("text/html"));
    let mut body = String::new();
    resp.into_reader().read_to_string(&mut body).unwrap();
    assert!(body.contains("Solum Sync"));
    assert!(body.contains("/v1/stats"));
}

#[test]
fn generic_alert_round_trip_works() {
    let dir = std::env::temp_dir().join(format!("solum-alert-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("alerts.sqlite");
    let token = "test-token-alerts";
    let srv = spawn(token, &db);

    let alert = serde_json::json!({
        "event_id": "benefit-ai-zjl-13-20260812-operational",
        "source": "benefit-monitor",
        "monitor_id": "ai-zjl-13",
        "name": "Gpt Plus 福利版渠道",
        "status": "operational",
        "latency_ms": 1400,
        "ping_latency_ms": 9,
        "availability_7d": 70.5,
        "checked_at": "2026-08-12T05:00:00Z",
        "detail_url": "https://ai-zjl.cc/monitor"
    });
    let first: serde_json::Value = ureq::post(&format!("{}/v1/alerts", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(alert.clone())
        .expect("publish alert")
        .into_json()
        .unwrap();
    assert_eq!(first["created"], true);
    assert_eq!(first["seq"], 1);

    let duplicate: serde_json::Value = ureq::post(&format!("{}/v1/alerts", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(alert)
        .expect("deduplicate alert")
        .into_json()
        .unwrap();
    assert_eq!(duplicate["created"], false);
    assert_eq!(duplicate["seq"], 1);

    let listed: serde_json::Value = ureq::get(&format!("{}/v1/alerts?since=0&limit=60", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .expect("list alerts")
        .into_json()
        .unwrap();
    let alerts = listed["alerts"].as_array().unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["status"], "operational");
    assert_eq!(alerts[0]["monitor_id"], "ai-zjl-13");
    assert_eq!(alerts[0]["name"], "Gpt Plus 福利版渠道");
    assert_eq!(alerts[0]["latency_ms"], 1400);
}

#[test]
fn alerts_reject_bad_payloads_and_require_auth() {
    let dir = std::env::temp_dir().join(format!("solum-alert-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("alerts-bad.sqlite");
    let token = "test-token-alerts-bad";
    let srv = spawn(token, &db);

    let unauth = ureq::get(&format!("{}/v1/alerts", srv.base))
        .call()
        .unwrap_err();
    assert!(matches!(unauth, ureq::Error::Status(401, _)));

    let bad = ureq::post(&format!("{}/v1/alerts", srv.base))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({
            "event_id": "contains spaces",
            "source": "gptplus",
            "status": "surprise",
            "checked_at": "now"
        }))
        .unwrap_err();
    assert!(matches!(bad, ureq::Error::Status(400, _)));
}

#[test]
fn account_tenants_cannot_read_or_deduplicate_each_other() {
    let dir = std::env::temp_dir().join(format!("solum-tenant-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("tenants.sqlite");
    let srv = spawn_accounts(&db, None);
    let alice = account_token("alice");
    let bob = account_token("bob");

    for (token, device, body) in [
        (&alice, "alice-phone", b"alice-secret".as_slice()),
        (&bob, "bob-phone", b"bob-secret".as_slice()),
    ] {
        ureq::post(&format!("{}/v1/push", srv.base))
            .set("Authorization", &format!("Bearer {token}"))
            .set("X-Device", device)
            .send_bytes(body)
            .unwrap();
    }

    let alert = serde_json::json!({
        "event_id":"same-event-id",
        "source":"benefit-monitor",
        "status":"operational",
        "checked_at":"2026-08-12T05:00:00Z"
    });
    for token in [&alice, &bob] {
        let saved: serde_json::Value = ureq::post(&format!("{}/v1/alerts", srv.base))
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(alert.clone())
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(saved["created"], true);
    }

    let alice_pull: serde_json::Value = ureq::get(&format!(
        "{}/v1/pull?since=0&device=alice-desktop",
        srv.base
    ))
    .set("Authorization", &format!("Bearer {alice}"))
    .call()
    .unwrap()
    .into_json()
    .unwrap();
    let alice_blobs = alice_pull["blobs"].as_array().unwrap();
    assert_eq!(alice_blobs.len(), 1);
    assert_eq!(alice_blobs[0]["device"], "alice-phone");

    let bob_stats: serde_json::Value = ureq::get(&format!("{}/v1/stats", srv.base))
        .set("Authorization", &format!("Bearer {bob}"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(bob_stats["tenant"], "bob");
    assert_eq!(bob_stats["total_blobs"], 1);
    assert_eq!(bob_stats["devices"][0]["device"], "bob-phone");

    let alice_alerts: serde_json::Value = ureq::get(&format!("{}/v1/alerts", srv.base))
        .set("Authorization", &format!("Bearer {alice}"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(alice_alerts["alerts"].as_array().unwrap().len(), 1);
}

#[test]
fn existing_single_tenant_rows_migrate_only_to_legacy() {
    let dir = std::env::temp_dir().join(format!("solum-legacy-migrate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("legacy.sqlite");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        r#"
        CREATE TABLE blobs(seq INTEGER PRIMARY KEY,device TEXT NOT NULL,blob BLOB NOT NULL,received_at TEXT NOT NULL DEFAULT '2026-08-12T00:00:00Z');
        CREATE TABLE alerts(seq INTEGER PRIMARY KEY,event_id TEXT NOT NULL UNIQUE,source TEXT NOT NULL,monitor_id TEXT,name TEXT,status TEXT NOT NULL,latency_ms INTEGER,ping_latency_ms INTEGER,availability_7d REAL,checked_at TEXT NOT NULL,detail_url TEXT,received_at TEXT NOT NULL DEFAULT '2026-08-12T00:00:00Z');
        INSERT INTO blobs(device,blob) VALUES ('old-device',X'0102');
        INSERT INTO alerts(event_id,source,status,checked_at) VALUES ('old-event','benefit-monitor','operational','2026-08-12T00:00:00Z');
        "#,
    )
    .unwrap();
    drop(db);

    let legacy_token = "legacy-migration-token";
    let srv = spawn_accounts(&db_path, Some(legacy_token));
    let legacy_pull: serde_json::Value =
        ureq::get(&format!("{}/v1/pull?since=0&device=new-device", srv.base))
            .set("Authorization", &format!("Bearer {legacy_token}"))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
    assert_eq!(legacy_pull["blobs"].as_array().unwrap().len(), 1);

    let alice = account_token("alice");
    let alice_pull: serde_json::Value =
        ureq::get(&format!("{}/v1/pull?since=0&device=new-device", srv.base))
            .set("Authorization", &format!("Bearer {alice}"))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
    assert!(alice_pull["blobs"].as_array().unwrap().is_empty());
}

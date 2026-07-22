//! Integration test: spawns the real `solum-sync-server` binary (it has no
//! lib target — the binary *is* the product, see main.rs) and hits it over
//! HTTP, the same way a device or the dashboard would.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
    assert!(stats["db_bytes"].as_i64().unwrap() > 0);

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

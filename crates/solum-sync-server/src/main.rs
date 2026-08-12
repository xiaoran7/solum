//! solum-sync-server — the Solum sync relay (ARCHITECTURE.md §3.8).
//!
//! Stores and forwards end-to-end-encrypted oplog blobs between the user's
//! own devices. It never holds a decryption key, never inspects payloads, and
//! keeps no plaintext: a compromised relay yields only ciphertext. Single
//! binary + one SQLite file, made for self-hosting.
//!
//! Wire protocol (all under Bearer-token auth, token is `SOLUM_SYNC_SERVER_TOKEN`):
//! - `POST /v1/push`  (header `X-Device: <id>`, body: raw encrypted blob)
//!   → `{"seq": N}`
//! - `GET  /v1/pull?since=N&device=<id>`
//!   → `{"blobs": [{"seq": N, "device": "...", "blob": "<base64>"}]}`
//! - `GET  /v1/stats` — aggregate counts only (never blob content), for the
//!   ops dashboard below: `{"total_blobs", "total_bytes", "oldest_seq",
//!   "newest_seq", "retention_days", "db_bytes", "devices": [{"device",
//!   "blob_count", "bytes", "last_seq", "last_received_at"}]}`
//!
//! `GET /` serves a static, unauthenticated ops dashboard (`dashboard.html`,
//! embedded in the binary) that itself calls `/v1/health` and `/v1/stats`
//! with a Bearer token entered client-side — the page has no data of its
//! own, so it needs no auth to load.
//!
//! Config via env: `SOLUM_SYNC_SERVER_TOKEN` (required), `SOLUM_SYNC_SERVER_ADDR`
//! (default `127.0.0.1:8787`), `SOLUM_SYNC_SERVER_DB` (default `pa-sync.sqlite`).

use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use base64::Engine;
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection};
use sha2::Sha256;
use std::io::Read;
use std::sync::Mutex;
use tiny_http::{Header, Method, Response, Server};

/// Blobs above this size are rejected — a sane cap for oplog batches.
const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;
/// Ceiling on the *total* ciphertext one pull answers with.
///
/// The row limit alone was not a limit: 500 rows × 8 MiB is ~4 GiB of blobs,
/// which base64 and JSON then inflate past 5 GiB — all of it built in memory,
/// on both ends, by any device holding the token. Pull now stops adding rows
/// once the budget is spent and the client simply comes back for the rest,
/// because `since` already makes pulling resumable.
const MAX_PULL_BYTES: usize = 16 * 1024 * 1024;
/// Rows per pull, on top of the byte budget above.
const MAX_PULL_ROWS: usize = 500;
/// Stored blobs older than this are swept on startup. `0` disables the sweep.
///
/// **Retention silently destroys data unless the client can detect the hole.**
/// A device offline longer than this comes back with a cursor pointing at a
/// sequence that no longer exists, pulls "everything after it" — which is now
/// a later, unrelated set — and reports a clean sync while having permanently
/// missed every op that was swept. That is worse than an unbounded relay file.
///
/// So the sweep is paired with `oldest_seq` in the pull response: the client
/// compares it against its own cursor and refuses to advance silently across a
/// gap (see `sync::sync_once`). Never add retention without that half.
const DEFAULT_RETENTION_DAYS: i64 = 30;
const ALERT_RETENTION_DAYS: i64 = 7;
const MAX_ALERT_BODY_BYTES: usize = 16 * 1024;
const LEGACY_TENANT: &str = "legacy";

/// Ops dashboard, embedded so the deployed artifact stays a single binary —
/// no separate static file to ship alongside it.
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

#[derive(serde::Deserialize)]
struct AlertInput {
    event_id: String,
    source: String,
    monitor_id: Option<String>,
    name: Option<String>,
    status: String,
    latency_ms: Option<i64>,
    ping_latency_ms: Option<i64>,
    availability_7d: Option<f64>,
    checked_at: String,
    detail_url: Option<String>,
}

impl AlertInput {
    fn is_valid(&self) -> bool {
        let event_id_ok = !self.event_id.is_empty()
            && self.event_id.len() <= 128
            && self
                .event_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        let source_ok = self.source == "gptplus" || self.source == "benefit-monitor";
        let monitor_id_ok = self.monitor_id.as_ref().is_none_or(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        });
        let name_ok = self
            .name
            .as_ref()
            .is_none_or(|value| !value.trim().is_empty() && value.chars().count() <= 80);
        let status_ok = matches!(
            self.status.as_str(),
            "operational" | "degraded" | "error" | "unknown" | "test"
        );
        let latency_ok = self.latency_ms.is_none_or(|v| (0..=120_000).contains(&v));
        let ping_ok = self
            .ping_latency_ms
            .is_none_or(|v| (0..=120_000).contains(&v));
        let availability_ok = self
            .availability_7d
            .is_none_or(|v| v.is_finite() && (0.0..=100.0).contains(&v));
        let detail_url_ok = self.detail_url.as_ref().is_none_or(|value| {
            value.is_empty()
                || (value.starts_with("https://")
                    && value.len() <= 2048
                    && !value.contains(['\r', '\n']))
        });
        event_id_ok
            && source_ok
            && monitor_id_ok
            && name_ok
            && status_ok
            && latency_ok
            && ping_ok
            && availability_ok
            && !self.checked_at.is_empty()
            && self.checked_at.len() <= 64
            && detail_url_ok
    }
}

#[derive(Debug)]
struct AuthContext {
    tenant_id: String,
    legacy: bool,
}

fn verify_account_token(token: &str, secret: &str) -> Option<String> {
    if secret.len() < 32 {
        return None;
    }
    let (payload, signature) = token.split_once('.')?;
    if signature.contains('.') {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).ok()?;
    let payload: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    let subject = payload.get("sub")?.as_str()?.trim();
    let expires_at = payload.get("exp")?.as_i64()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    if subject.is_empty() || subject.len() > 256 || expires_at <= now {
        return None;
    }
    Some(subject.to_string())
}

fn authenticate(
    req: &tiny_http::Request,
    legacy_token: Option<&str>,
    auth_secret: Option<&str>,
) -> Option<AuthContext> {
    let bearer = req
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))?
        .value
        .as_str()
        .strip_prefix("Bearer ")?;
    if legacy_token.is_some_and(|token| bearer == token) {
        return Some(AuthContext {
            tenant_id: LEGACY_TENANT.to_string(),
            legacy: true,
        });
    }
    verify_account_token(bearer, auth_secret.unwrap_or_default()).map(|tenant_id| AuthContext {
        tenant_id,
        legacy: false,
    })
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1)"),
        params![column],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

fn migrate_tenants(conn: &Connection) -> rusqlite::Result<()> {
    if !column_exists(conn, "blobs", "tenant_id") {
        conn.execute(
            "ALTER TABLE blobs ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'legacy'",
            [],
        )?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_blobs_tenant_seq ON blobs(tenant_id, seq)",
        [],
    )?;

    if !column_exists(conn, "alerts", "tenant_id") {
        conn.execute_batch(
            r#"
            BEGIN IMMEDIATE;
            ALTER TABLE alerts RENAME TO alerts_single_tenant;
            CREATE TABLE alerts (
                seq             INTEGER PRIMARY KEY,
                tenant_id       TEXT NOT NULL,
                event_id        TEXT NOT NULL,
                source          TEXT NOT NULL,
                monitor_id      TEXT,
                name            TEXT,
                status          TEXT NOT NULL,
                latency_ms      INTEGER,
                ping_latency_ms INTEGER,
                availability_7d REAL,
                checked_at      TEXT NOT NULL,
                detail_url      TEXT,
                received_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                UNIQUE(tenant_id, event_id)
            );
            INSERT INTO alerts(
                seq,tenant_id,event_id,source,monitor_id,name,status,latency_ms,
                ping_latency_ms,availability_7d,checked_at,detail_url,received_at
            )
            SELECT seq,'legacy',event_id,source,monitor_id,name,status,latency_ms,
                   ping_latency_ms,availability_7d,checked_at,detail_url,received_at
            FROM alerts_single_tenant;
            DROP TABLE alerts_single_tenant;
            COMMIT;
            "#,
        )?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_alerts_tenant_source_seq ON alerts(tenant_id, source, seq)",
        [],
    )?;
    Ok(())
}

fn main() {
    let legacy_token = std::env::var("SOLUM_SYNC_SERVER_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let auth_secret = std::env::var("SOLUM_AUTH_SECRET")
        .ok()
        .filter(|value| value.len() >= 32);
    if legacy_token.is_none() && auth_secret.is_none() {
        eprintln!("SOLUM_AUTH_SECRET 与 SOLUM_SYNC_SERVER_TOKEN 均未设置——拒绝无鉴权启动");
        std::process::exit(1);
    }
    let addr = std::env::var("SOLUM_SYNC_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    // 默认库名停留在改名前的 pa-sync.sqlite：它指向的是**已部署中继上的既有
    // 文件**，改默认值会让中继静默从一个空库起步（设备游标全丢）。要改得连同
    // 服务器上的文件一起改，或显式传 SOLUM_SYNC_SERVER_DB。
    let db_path = std::env::var("SOLUM_SYNC_SERVER_DB").unwrap_or_else(|_| "pa-sync.sqlite".into());

    let conn = Connection::open(&db_path).expect("open sync db");
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS blobs (
            seq         INTEGER PRIMARY KEY,
            device      TEXT NOT NULL,
            blob        BLOB NOT NULL,
            received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        );
        CREATE INDEX IF NOT EXISTS idx_blobs_device ON blobs(device, seq);
        CREATE TABLE IF NOT EXISTS alerts (
            seq             INTEGER PRIMARY KEY,
            event_id        TEXT NOT NULL UNIQUE,
            source          TEXT NOT NULL,
            monitor_id      TEXT,
            name            TEXT,
            status          TEXT NOT NULL,
            latency_ms      INTEGER,
            ping_latency_ms INTEGER,
            availability_7d REAL,
            checked_at      TEXT NOT NULL,
            detail_url      TEXT,
            received_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        );
        CREATE INDEX IF NOT EXISTS idx_alerts_source_seq ON alerts(source, seq);
        "#,
    )
    .expect("migrate sync db");
    for (column, migration) in [
        (
            "monitor_id",
            "ALTER TABLE alerts ADD COLUMN monitor_id TEXT",
        ),
        ("name", "ALTER TABLE alerts ADD COLUMN name TEXT"),
        (
            "detail_url",
            "ALTER TABLE alerts ADD COLUMN detail_url TEXT",
        ),
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('alerts') WHERE name = ?1)",
                params![column],
                |row| row.get(0),
            )
            .expect("inspect alert table");
        if !exists {
            conn.execute(migration, []).expect("migrate alert table");
        }
    }
    migrate_tenants(&conn).expect("migrate tenant isolation");
    // Retention sweep. Bounded disk on a long-running self-hosted instance,
    // paired with the `oldest_seq` gap signal so a device that slept through
    // the window is told rather than silently shorted.
    let retention_days: i64 = std::env::var("SOLUM_SYNC_SERVER_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    if retention_days > 0 {
        match conn.execute(
            "DELETE FROM blobs WHERE received_at < strftime('%Y-%m-%dT%H:%M:%fZ','now',?1)",
            params![format!("-{retention_days} days")],
        ) {
            Ok(n) if n > 0 => println!(
                "已清理 {n} 条超过 {retention_days} 天的旧批次                 （离线更久的设备会收到缺口提示，需要重新完整同步）"
            ),
            Ok(_) => {}
            Err(e) => eprintln!("清理旧批次失败（不影响服务）: {e}"),
        }
    } else {
        println!("留存清理已关闭（SOLUM_SYNC_SERVER_RETENTION_DAYS=0），中继文件将持续增长");
    }
    if let Err(e) = conn.execute(
        "DELETE FROM alerts WHERE received_at < strftime('%Y-%m-%dT%H:%M:%fZ','now',?1)",
        params![format!("-{ALERT_RETENTION_DAYS} days")],
    ) {
        eprintln!("alert retention sweep failed (service continues): {e}");
    }
    let conn = Mutex::new(conn);

    let server = Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("无法监听 {addr}: {e}");
        std::process::exit(1);
    });
    println!("solum-sync-server 监听 http://{addr}（db: {db_path}）——只中转密文，不解密");

    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        let method = req.method().clone();

        // Static dashboard has no data of its own — it fetches /v1/* itself
        // with a Bearer token entered client-side — so it needs no auth to
        // load, unlike every /v1/* route below.
        if method == Method::Get && (url == "/" || url == "/index.html") {
            let resp = Response::from_string(DASHBOARD_HTML).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            let _ = req.respond(resp);
            continue;
        }
        if method == Method::Get && url == "/favicon.ico" {
            let _ = req.respond(Response::empty(204));
            continue;
        }
        let Some(auth) = authenticate(&req, legacy_token.as_deref(), auth_secret.as_deref()) else {
            respond(req, 401, r#"{"error":"unauthorized"}"#);
            continue;
        };

        match (method, url.as_str()) {
            (Method::Post, "/v1/alerts") => {
                let mut body = Vec::new();
                if req
                    .as_reader()
                    .take(MAX_ALERT_BODY_BYTES as u64 + 1)
                    .read_to_end(&mut body)
                    .is_err()
                    || body.is_empty()
                    || body.len() > MAX_ALERT_BODY_BYTES
                {
                    respond(req, 400, r#"{"error":"bad alert body"}"#);
                    continue;
                }
                let alert = match serde_json::from_slice::<AlertInput>(&body) {
                    Ok(alert) if alert.is_valid() => alert,
                    _ => {
                        respond(req, 400, r#"{"error":"invalid alert"}"#);
                        continue;
                    }
                };
                let stored = {
                    let c = conn.lock().unwrap();
                    match c.execute(
                        "INSERT OR IGNORE INTO alerts(
                            tenant_id, event_id, source, monitor_id, name, status, latency_ms,
                            ping_latency_ms, availability_7d, checked_at, detail_url
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            auth.tenant_id,
                            alert.event_id,
                            alert.source,
                            alert.monitor_id,
                            alert.name,
                            alert.status,
                            alert.latency_ms,
                            alert.ping_latency_ms,
                            alert.availability_7d,
                            alert.checked_at,
                            alert.detail_url,
                        ],
                    ) {
                        Ok(changed) => c
                            .query_row(
                                "SELECT seq FROM alerts WHERE tenant_id = ?1 AND event_id = ?2",
                                params![auth.tenant_id, alert.event_id],
                                |row| row.get::<_, i64>(0),
                            )
                            .map(|seq| (seq, changed > 0)),
                        Err(e) => Err(e),
                    }
                };
                match stored {
                    Ok((seq, created)) => respond(
                        req,
                        200,
                        &serde_json::json!({"seq": seq, "created": created}).to_string(),
                    ),
                    Err(e) => {
                        eprintln!("alert store failed: {e}");
                        respond(req, 500, r#"{"error":"store failed"}"#);
                    }
                }
            }
            (Method::Get, u) if u.starts_with("/v1/alerts") => {
                let since = query_param(u, "since")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0)
                    .max(0);
                let limit = query_param(u, "limit")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(60)
                    .clamp(1, 100);
                let source = query_param(u, "source");
                if source
                    .as_deref()
                    .is_some_and(|value| value != "gptplus" && value != "benefit-monitor")
                {
                    respond(req, 400, r#"{"error":"invalid source"}"#);
                    continue;
                }
                let rows: Result<Vec<serde_json::Value>, rusqlite::Error> = {
                    let c = conn.lock().unwrap();
                    let sql = match (since == 0, source.is_some()) {
                        (true, true) => "SELECT * FROM (SELECT seq,event_id,source,monitor_id,name,status,latency_ms,ping_latency_ms,availability_7d,checked_at,detail_url,received_at FROM alerts WHERE tenant_id=?1 AND source=?3 ORDER BY seq DESC LIMIT ?4) ORDER BY seq ASC",
                        (true, false) => "SELECT * FROM (SELECT seq,event_id,source,monitor_id,name,status,latency_ms,ping_latency_ms,availability_7d,checked_at,detail_url,received_at FROM alerts WHERE tenant_id=?1 ORDER BY seq DESC LIMIT ?4) ORDER BY seq ASC",
                        (false, true) => "SELECT seq,event_id,source,monitor_id,name,status,latency_ms,ping_latency_ms,availability_7d,checked_at,detail_url,received_at FROM alerts WHERE tenant_id=?1 AND seq>?2 AND source=?3 ORDER BY seq ASC LIMIT ?4",
                        (false, false) => "SELECT seq,event_id,source,monitor_id,name,status,latency_ms,ping_latency_ms,availability_7d,checked_at,detail_url,received_at FROM alerts WHERE tenant_id=?1 AND seq>?2 ORDER BY seq ASC LIMIT ?4",
                    };
                    c.prepare(sql).and_then(|mut stmt| {
                        stmt.query_map(
                            params![auth.tenant_id, since, source.as_deref(), limit],
                            |row| {
                                Ok(serde_json::json!({
                                    "seq": row.get::<_, i64>(0)?,
                                    "event_id": row.get::<_, String>(1)?,
                                    "source": row.get::<_, String>(2)?,
                                    "monitor_id": row.get::<_, Option<String>>(3)?,
                                    "name": row.get::<_, Option<String>>(4)?,
                                    "status": row.get::<_, String>(5)?,
                                    "latency_ms": row.get::<_, Option<i64>>(6)?,
                                    "ping_latency_ms": row.get::<_, Option<i64>>(7)?,
                                    "availability_7d": row.get::<_, Option<f64>>(8)?,
                                    "checked_at": row.get::<_, String>(9)?,
                                    "detail_url": row.get::<_, Option<String>>(10)?,
                                    "received_at": row.get::<_, String>(11)?,
                                }))
                            },
                        )?
                        .collect()
                    })
                };
                match rows {
                    Ok(alerts) => {
                        respond(req, 200, &serde_json::json!({"alerts": alerts}).to_string())
                    }
                    Err(e) => {
                        eprintln!("alert read failed: {e}");
                        respond(req, 500, r#"{"error":"read failed"}"#);
                    }
                }
            }
            (Method::Post, u) if u.starts_with("/v1/push") => {
                let device = header(&req, "X-Device");
                let Some(device) = device.filter(|d| !d.is_empty()) else {
                    respond(req, 400, r#"{"error":"missing X-Device"}"#);
                    continue;
                };
                let mut blob = Vec::new();
                if req
                    .as_reader()
                    .take(MAX_BLOB_BYTES as u64 + 1)
                    .read_to_end(&mut blob)
                    .is_err()
                    || blob.is_empty()
                    || blob.len() > MAX_BLOB_BYTES
                {
                    respond(req, 400, r#"{"error":"bad blob"}"#);
                    continue;
                }
                let seq: i64 = {
                    let c = conn.lock().unwrap();
                    match c.execute(
                        "INSERT INTO blobs(tenant_id, device, blob) VALUES (?1, ?2, ?3)",
                        params![auth.tenant_id, device, blob],
                    ) {
                        Ok(_) => c.last_insert_rowid(),
                        Err(e) => {
                            eprintln!("push 写入失败: {e}");
                            respond(req, 500, r#"{"error":"store failed"}"#);
                            continue;
                        }
                    }
                };
                respond(req, 200, &format!(r#"{{"seq":{seq}}}"#));
            }
            (Method::Get, u) if u.starts_with("/v1/pull") => {
                let since: i64 = query_param(u, "since")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let device = query_param(u, "device").unwrap_or_default();
                let rows: Result<Vec<(i64, String, Vec<u8>)>, rusqlite::Error> = {
                    let c = conn.lock().unwrap();
                    let r = c
                        .prepare(
                            "SELECT seq, device, blob FROM blobs
                             WHERE tenant_id = ?1 AND seq > ?2 AND device != ?3 ORDER BY seq ASC LIMIT ?4",
                        )
                        .and_then(|mut stmt| {
                            stmt.query_map(params![auth.tenant_id, since, device, MAX_PULL_ROWS as i64], |r| {
                                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                            })?
                            .collect()
                        });
                    r
                };
                let oldest: i64 = {
                    let c = conn.lock().unwrap();
                    c.query_row(
                        "SELECT COALESCE(MIN(seq), 0) FROM blobs WHERE tenant_id=?1",
                        params![auth.tenant_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0)
                };
                match rows {
                    Ok(rows) => {
                        // Spend the byte budget in seq order and stop. Always
                        // emit at least one row, so a single large blob can
                        // still make progress instead of wedging the cursor.
                        let mut used = 0usize;
                        let mut blobs: Vec<serde_json::Value> = Vec::new();
                        for (seq, device, blob) in rows {
                            if !blobs.is_empty() && used + blob.len() > MAX_PULL_BYTES {
                                break;
                            }
                            used += blob.len();
                            blobs.push(
                                serde_json::json!({"seq": seq, "device": device, "blob": B64.encode(blob)}),
                            );
                        }
                        // `oldest_seq` is what lets the client notice that its
                        // cursor points into swept history. Without it retention
                        // is silent data loss.
                        respond(
                            req,
                            200,
                            &serde_json::json!({ "blobs": blobs, "oldest_seq": oldest })
                                .to_string(),
                        );
                    }
                    Err(e) => {
                        eprintln!("pull 读取失败: {e}");
                        respond(req, 500, r#"{"error":"read failed"}"#);
                    }
                }
            }
            (Method::Get, u) if u.starts_with("/v1/stats") => {
                type Totals = (i64, i64, i64, i64);
                type Devices = Result<Vec<serde_json::Value>, rusqlite::Error>;
                let (totals, devices): (Totals, Devices) = {
                    let c = conn.lock().unwrap();
                    let totals = c
                        .query_row(
                             "SELECT COUNT(*), COALESCE(SUM(LENGTH(blob)),0),
                                     COALESCE(MIN(seq),0), COALESCE(MAX(seq),0) FROM blobs WHERE tenant_id=?1",
                            params![auth.tenant_id],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                        )
                        .unwrap_or((0, 0, 0, 0));
                    let devices = c
                        .prepare(
                            "SELECT device, COUNT(*), COALESCE(SUM(LENGTH(blob)),0), MAX(seq), MAX(received_at)
                             FROM blobs WHERE tenant_id=?1 GROUP BY device ORDER BY MAX(seq) DESC",
                        )
                        .and_then(|mut stmt| {
                            stmt.query_map(params![auth.tenant_id], |r| {
                                let device: String = r.get(0)?;
                                let count: i64 = r.get(1)?;
                                let bytes: i64 = r.get(2)?;
                                let last_seq: i64 = r.get(3)?;
                                let last_received_at: String = r.get(4)?;
                                Ok(serde_json::json!({
                                    "device": device,
                                    "blob_count": count,
                                    "bytes": bytes,
                                    "last_seq": last_seq,
                                    "last_received_at": last_received_at,
                                }))
                            })?
                            .collect()
                        });
                    (totals, devices)
                };
                match devices {
                    Ok(devices) => {
                        respond(
                            req,
                            200,
                            &serde_json::json!({
                                "total_blobs": totals.0,
                                "total_bytes": totals.1,
                                "oldest_seq": totals.2,
                                "newest_seq": totals.3,
                                "retention_days": retention_days,
                                "tenant": auth.tenant_id,
                                "auth_mode": if auth.legacy { "legacy" } else { "account" },
                                "devices": devices,
                            })
                            .to_string(),
                        );
                    }
                    Err(e) => {
                        eprintln!("stats 读取失败: {e}");
                        respond(req, 500, r#"{"error":"read failed"}"#);
                    }
                }
            }
            (Method::Get, "/v1/health") => respond(
                req,
                200,
                &serde_json::json!({
                    "ok": true,
                    "tenant": auth.tenant_id,
                    "auth_mode": if auth.legacy { "legacy" } else { "account" },
                })
                .to_string(),
            ),
            _ => respond(req, 404, r#"{"error":"not found"}"#),
        }
    }
}

fn header(req: &tiny_http::Request, name: &str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let qs = url.split_once('?')?.1;
    qs.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn respond(req: tiny_http::Request, status: u16, body: &str) {
    let resp = Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    let _ = req.respond(resp);
}

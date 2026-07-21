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
//!
//! Config via env: `SOLUM_SYNC_SERVER_TOKEN` (required), `SOLUM_SYNC_SERVER_ADDR`
//! (default `127.0.0.1:8787`), `SOLUM_SYNC_SERVER_DB` (default `pa-sync.sqlite`).

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, Connection};
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

fn main() {
    let token = match std::env::var("SOLUM_SYNC_SERVER_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!("SOLUM_SYNC_SERVER_TOKEN 未设置——拒绝无鉴权启动");
            std::process::exit(1);
        }
    };
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
        "#,
    )
    .expect("migrate sync db");
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
    let conn = Mutex::new(conn);

    let server = Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("无法监听 {addr}: {e}");
        std::process::exit(1);
    });
    println!("solum-sync-server 监听 http://{addr}（db: {db_path}）——只中转密文，不解密");

    for mut req in server.incoming_requests() {
        let authed = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str() == format!("Bearer {token}"))
            .unwrap_or(false);
        if !authed {
            respond(req, 401, r#"{"error":"unauthorized"}"#);
            continue;
        }

        let url = req.url().to_string();
        let method = req.method().clone();
        match (method, url.as_str()) {
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
                        "INSERT INTO blobs(device, blob) VALUES (?1, ?2)",
                        params![device, blob],
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
                             WHERE seq > ?1 AND device != ?2 ORDER BY seq ASC LIMIT ?3",
                        )
                        .and_then(|mut stmt| {
                            stmt.query_map(params![since, device, MAX_PULL_ROWS as i64], |r| {
                                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                            })?
                            .collect()
                        });
                    r
                };
                let oldest: i64 = {
                    let c = conn.lock().unwrap();
                    c.query_row("SELECT COALESCE(MIN(seq), 0) FROM blobs", [], |r| r.get(0))
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
            (Method::Get, "/v1/health") => respond(req, 200, r#"{"ok":true}"#),
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

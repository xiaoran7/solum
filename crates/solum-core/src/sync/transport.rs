//! How encrypted blobs reach the relay; abstracted so tests need no network.
//!
//! Part of the `sync` module; see `mod.rs` for the module contract.

use super::*;

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
    auth: RelayAuth,
    agent: ureq::Agent,
    /// Captured from the last `pull` response. Interior mutability because the
    /// trait takes `&self` — and it has to come from the *same* response as the
    /// blobs, or the two could describe different relay states.
    oldest_seq: std::cell::Cell<Option<i64>>,
}

enum RelayAuth {
    Legacy(String),
    Account(std::sync::Mutex<crate::account::AccountSession>),
}

enum RelayRequestError {
    Unauthorized,
    /// The server answered and permanently refused this request (quota,
    /// revoked device). Retrying — now or after a token refresh — cannot
    /// change the answer; the caller should stop this half-round.
    Permanent(String),
    Other(String),
}

impl std::fmt::Display for RelayRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("认证失败"),
            Self::Permanent(message) => f.write_str(message),
            Self::Other(message) => f.write_str(message),
        }
    }
}

/// Best-effort read of the relay's machine-readable refusal code from an
/// error response body (`{"error":"storage_quota_exceeded"}`) — used only to
/// make the human message precise; an unreadable body degrades to the empty
/// string, never to a failure.
fn relay_error_code(resp: ureq::Response) -> String {
    resp.into_string()
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|v| v.get("error").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// True if any error in the cause chain is an io error of `kind`. ureq wraps
/// its agent-level deadlines as io errors somewhere down the chain, so this
/// is how a timeout is recognized without matching brittle message strings.
fn chain_has_io_kind(err: &(dyn std::error::Error + 'static), kind: std::io::ErrorKind) -> bool {
    let mut current = Some(err);
    while let Some(e) = current {
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            if io_err.kind() == kind {
                return true;
            }
        }
        current = e.source();
    }
    false
}

/// Turn a ureq transport-layer error into one line of Chinese a user can act
/// on. The relay's own status codes are handled elsewhere; this covers the
/// road there: DNS, TCP connect, the TLS handshake, and timeouts.
///
/// The TLS case is the one that matters (2026-08 真机同步故障): a connection
/// cut *during the handshake* surfaces in ureq 2.12 as `ConnectionFailed` with
/// message "tls connection init failed", whose raw English text ("unexpected
/// end of file" / connection reset) told the user nothing. Live probing
/// (2026-08-21) showed the cut is *server-side* fingerprint filtering —
/// rustls' no-GREASE ClientHello is RST-ed while browser-grade ones pass, from
/// any network path — so the message points there, and never at disabling
/// certificate verification.
pub(crate) fn humanize_transport_error(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("服务器返回 {code}"),
        ureq::Error::Transport(t) => {
            if chain_has_io_kind(t, std::io::ErrorKind::TimedOut) {
                return "同步请求超时".to_string();
            }
            let detail = t.message().unwrap_or("");
            match t.kind() {
                ureq::ErrorKind::Dns => {
                    "无法解析同步服务器地址（DNS 解析失败），请检查网络连接".to_string()
                }
                ureq::ErrorKind::ConnectionFailed if detail.contains("tls") => {
                    "TLS 握手失败：连接在加密握手阶段被对端或中间设备切断，或证书校验未通过。\
                     若浏览器能打开同一地址而这里不行，通常是服务器侧对该域名的 TLS 拦截\
                     （例如云厂商对未备案域名的合规阻断），需要从服务器侧解决"
                        .to_string()
                }
                ureq::ErrorKind::ConnectionFailed => {
                    "无法连接同步服务器，请检查网络后重试".to_string()
                }
                _ => format!("网络错误: {error}"),
            }
        }
    }
}

/// The statuses the transport classifies as permanent push refusals: the same
/// bytes re-sent next round get the same answer, so retrying them is the
/// wedge this classification exists to break. Server codes per
/// postgres-server.js / solum-cloud: 400 invalid request (legacy relay on an
/// unusable blob), 403 `device_revoked`, 413 too large, 429
/// `device_limit_exceeded`, 507 `storage_quota_exceeded`. Everything else —
/// timeouts, resets, 5xx — can plausibly succeed next round and stays
/// retryable.
pub(super) fn permanent_push_status(code: u16) -> bool {
    matches!(code, 400 | 403 | 413 | 429 | 507)
}

/// Human message for those permanent refusals (see `permanent_push_status`).
fn permanent_push_message(code: u16, server_code: &str) -> String {
    match (code, server_code) {
        (507, _) => "同步服务器存储配额已满（507）".to_string(),
        (403, "device_revoked") => "本设备已被同步服务器撤销（403）".to_string(),
        (403, _) => "同步服务器拒绝本设备（403）".to_string(),
        (429, _) => "同步服务器设备数已达上限（429）".to_string(),
        _ => format!("同步服务器拒绝推送（{code}）"),
    }
}

impl HttpTransport {
    /// Fails if the relay URL is not an acceptable endpoint (see
    /// [`crate::net::validate_endpoint`]) — the sync token and every op in the
    /// oplog would otherwise travel in cleartext.
    pub fn new(cfg: &SyncConfig) -> Result<HttpTransport> {
        let url = crate::net::validate_endpoint(&cfg.url, "同步服务器 url")?;
        if let Some(session) = &cfg.account {
            if url != session.server_url {
                return Err(CoreError::Invalid(
                    "账号同步必须使用登录服务器同源地址，已拒绝向其他站点发送账号令牌".into(),
                ));
            }
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build();
        Ok(HttpTransport {
            url,
            auth: match &cfg.account {
                Some(session) => RelayAuth::Account(std::sync::Mutex::new(session.clone())),
                None => RelayAuth::Legacy(cfg.token.clone()),
            },
            agent,
            oldest_seq: std::cell::Cell::new(None),
        })
    }

    fn with_auth<T>(
        &self,
        send: impl Fn(&str) -> std::result::Result<T, RelayRequestError>,
    ) -> Result<T> {
        match &self.auth {
            RelayAuth::Legacy(token) => send(token).map_err(|e| match e {
                RelayRequestError::Unauthorized => CoreError::Invalid("同步服务器认证失败".into()),
                RelayRequestError::Permanent(message) => CoreError::SyncPushRejected(message),
                RelayRequestError::Other(message) => {
                    CoreError::Invalid(format!("同步服务器请求失败: {message}"))
                }
            }),
            RelayAuth::Account(session) => {
                let mut session = session
                    .lock()
                    .map_err(|_| CoreError::Invalid("同步账号状态锁中毒".into()))?;
                match send(&session.access_token) {
                    Ok(value) => Ok(value),
                    Err(RelayRequestError::Unauthorized) => {
                        let rotated = crate::account::refresh_and_save(&session)?;
                        *session = rotated;
                        send(&session.access_token).map_err(|e| {
                            CoreError::Invalid(format!("刷新账号后重试同步请求失败: {e}"))
                        })
                    }
                    Err(RelayRequestError::Permanent(message)) => {
                        // A refusal that has nothing to do with the token must
                        // not spend (and rotate) the refresh token on it.
                        Err(CoreError::SyncPushRejected(message))
                    }
                    Err(RelayRequestError::Other(message)) => {
                        Err(CoreError::Invalid(format!("同步服务器请求失败: {message}")))
                    }
                }
            }
        }
    }
}

impl SyncTransport for HttpTransport {
    fn push(&self, device: &str, blob: &[u8]) -> Result<i64> {
        let resp: serde_json::Value = self
            .with_auth(|token| {
                self.agent
                    .post(&format!("{}/v1/push", self.url))
                    .set("Authorization", &format!("Bearer {token}"))
                    .set("X-Device", device)
                    .set("Content-Type", "application/octet-stream")
                    .send_bytes(blob)
                    .map_err(|error| match error {
                        ureq::Error::Status(401, _) => RelayRequestError::Unauthorized,
                        // Quota / revocation / size answers are permanent: the
                        // same blob re-sent next round gets the same refusal.
                        // Classifying them lets the driver skip the push half
                        // instead of wedging on it forever.
                        ureq::Error::Status(code, resp) if permanent_push_status(code) => {
                            RelayRequestError::Permanent(permanent_push_message(
                                code,
                                &relay_error_code(resp),
                            ))
                        }
                        other => RelayRequestError::Other(humanize_transport_error(&other)),
                    })
            })?
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("同步服务器响应不可解析: {e}")))?;
        resp.get("seq")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CoreError::Invalid("push 响应缺少 seq".into()))
    }

    fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>> {
        let reader = self
            .with_auth(|token| {
                self.agent
                    .get(&format!(
                        "{}/v1/pull?since={since}&device={device}",
                        self.url
                    ))
                    .set("Authorization", &format!("Bearer {token}"))
                    .call()
                    .map_err(|error| match error {
                        ureq::Error::Status(401, _) => RelayRequestError::Unauthorized,
                        other => RelayRequestError::Other(humanize_transport_error(&other)),
                    })
            })?
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

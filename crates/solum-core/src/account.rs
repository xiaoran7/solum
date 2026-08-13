//! Solum 独立账号：鉴权自建 solum-cloud AI 代理（`server/`，与 Solum Harmony 共用同一契约）。
//!
//! 登录后 AI 请求发往 `{server}/v1/ai/chat/completions`，第三方 API Key 只存在于
//! 服务端环境变量；登录动作本身不上传本机数据库。同一短期 access token 也供同步
//! relay 验证租户身份，但同步正文仍由独立的设备端密钥加密。
//!
//! 会话落盘 `solum-account.json`（gitignore）：`server_url` / `user_id` /
//! `username` / `access_token` / `refresh_token`。`user_id` 是服务端生成且不可变的
//! 身份主键；用户名只用于登录和展示，绝不能再充当本地目录或中继租户主键。
//! 旧会话没有 `user_id` 时仍可用于 AI 刷新，但不会被当作已隔离的数据身份。
//! 密码只用于登录请求本身，绝不落盘。

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::extract::Reasoner;

pub const DEFAULT_CLOUD_MODEL: &str = "mimo-v2.5";
pub const CLOUD_MODEL_OPTIONS: &[&str] = &["mimo-v2.5", "mimo-v2.5-pro"];

/// Access tokens live 15 minutes server-side; a 401 mid-conversation is
/// routine, so every authorized call refreshes exactly once and retries.
/// The generous timeout covers streaming replies from thinking-mode models
/// (mirrors the harmony client's 120s read timeout).
const TIMEOUT_SECS: u64 = 120;
static ACCOUNT_REFRESH_LOCK: Mutex<()> = Mutex::new(());

fn default_cloud_model() -> String {
    DEFAULT_CLOUD_MODEL.to_string()
}

/// The persisted login session. `PartialEq` so callers can detect token
/// rotation and persist only when something actually changed (same pattern
/// as [`crate::soulous::SoulousConfig`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSession {
    pub server_url: String,
    /// Immutable server-side user id (UUID). Empty only for a legacy session
    /// created before per-account local storage existed.
    #[serde(default)]
    pub user_id: String,
    pub username: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Which model the proxy should ask upstream for. Not part of the
    /// harmony file shape — `default` keeps old files readable.
    #[serde(default = "default_cloud_model")]
    pub model: String,
}

impl AccountSession {
    pub fn from_json(raw: &str) -> Result<Self> {
        let mut session: AccountSession = serde_json::from_str(raw)?;
        session.normalize()?;
        Ok(session)
    }

    fn normalize(&mut self) -> Result<()> {
        self.server_url = normalize_server_url(&self.server_url)?;
        self.user_id = self.user_id.trim().to_ascii_lowercase();
        if !self.user_id.is_empty() && !is_valid_user_id(&self.user_id) {
            return Err(CoreError::Invalid(
                "solum-account.json 的 user_id 不是有效 UUID".into(),
            ));
        }
        self.username = self.username.trim().to_string();
        self.access_token = self.access_token.trim().to_string();
        self.refresh_token = self.refresh_token.trim().to_string();
        self.model = normalize_cloud_model(&self.model)?;
        if self.username.is_empty() || self.access_token.is_empty() || self.refresh_token.is_empty()
        {
            return Err(CoreError::Invalid(
                "solum-account.json 缺 username/access_token/refresh_token".into(),
            ));
        }
        Ok(())
    }

    /// `SOLUM_ACCOUNT_CONFIG` override (mobile setup points it at app-data),
    /// else `solum-account.json` next to the other local credential files.
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("SOLUM_ACCOUNT_CONFIG") {
            if !p.trim().is_empty() {
                return p.into();
            }
        }
        crate::paths::resolve_with_adoption("solum-account.json")
    }

    /// Missing/unreadable/malformed file → `None` ("not logged in"), never an
    /// error: a broken session file must not take offline paths down.
    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        Self::from_json(&text).ok()
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        crate::fsatomic::write_atomic(&Self::path(), &json)
    }

    pub fn delete_file() {
        let _ = std::fs::remove_file(Self::path());
    }

    /// Displayable status line — never the tokens.
    pub fn masked_summary(&self) -> String {
        format!("{} @ {} · {}", self.username, self.server_url, self.model)
    }

    /// Stable identity used for local profiles and remote tenant selection.
    /// Legacy sessions deliberately return `None`: falling back to username
    /// here would recreate the account-rename/collision bug this field fixes.
    pub fn stable_user_id(&self) -> Option<&str> {
        (!self.user_id.is_empty()).then_some(self.user_id.as_str())
    }
}

pub fn is_valid_user_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, byte)| match i {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
}

/// Same policy as [`crate::net::validate_endpoint`]: HTTPS everywhere, plain
/// HTTP only for loopback (local debugging of the proxy itself).
pub fn normalize_server_url(value: &str) -> Result<String> {
    crate::net::validate_endpoint(value, "账号服务器地址")
}

/// Mirrors the server's `validateModel`: empty → default; otherwise a short
/// identifier limited to the characters real model names use.
pub fn normalize_cloud_model(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(default_cloud_model());
    }
    if trimmed.len() > 100
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
    {
        return Err(CoreError::Invalid(
            "模型名称只能包含字母、数字、点、下划线、冒号、斜杠和连字符".into(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Parse `/v1/auth/login` / `/v1/auth/refresh` responses. Runs the same
/// non-empty validation as the on-disk parser so a malformed server reply can
/// never be persisted as a "looks logged in" broken session.
fn parse_auth_response(
    value: &Value,
    server_url: &str,
    fallback_username: &str,
    model: &str,
) -> Result<AccountSession> {
    let field = |k: &str| value.get(k).and_then(Value::as_str).unwrap_or("");
    let username = value
        .get("user")
        .and_then(|u| u.get("username"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback_username);
    let user_id = value
        .get("user")
        .and_then(|u| u.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut session = AccountSession {
        server_url: server_url.to_string(),
        user_id: user_id.to_string(),
        username: username.to_string(),
        access_token: field("access_token").to_string(),
        refresh_token: field("refresh_token").to_string(),
        model: model.to_string(),
    };
    session
        .normalize()
        .map_err(|_| CoreError::Llm("服务器返回了无法识别的登录结果".into()))?;
    Ok(session)
}

/// HTTP seam so login/refresh/retry logic is testable without a socket
/// (same shape as [`crate::soulous::SoulousHttp`]).
pub trait AccountHttp {
    fn post_json(
        &self,
        url: &str,
        access_token: Option<&str>,
        body: Value,
    ) -> std::result::Result<Value, AccountHttpError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountHttpError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for AccountHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountHttpError::Unauthorized => f.write_str("认证已过期"),
            AccountHttpError::Other(message) => f.write_str(message),
        }
    }
}

pub struct HttpAccountClient {
    agent: ureq::Agent,
}

impl HttpAccountClient {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .build(),
        }
    }
}

impl Default for HttpAccountClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountHttp for HttpAccountClient {
    fn post_json(
        &self,
        url: &str,
        access_token: Option<&str>,
        body: Value,
    ) -> std::result::Result<Value, AccountHttpError> {
        let mut request = self.agent.post(url);
        if let Some(token) = access_token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let response = request.send_json(body).map_err(map_http_error)?;
        response
            .into_json()
            .map_err(|e| AccountHttpError::Other(format!("响应不是 JSON: {e}")))
    }
}

fn map_http_error(error: ureq::Error) -> AccountHttpError {
    match error {
        ureq::Error::Status(401, _) => AccountHttpError::Unauthorized,
        ureq::Error::Status(400, _) => {
            AccountHttpError::Other("请求内容不符合服务器要求".to_string())
        }
        ureq::Error::Status(403, _) => {
            AccountHttpError::Other("服务器未开放注册或当前操作无权限".to_string())
        }
        ureq::Error::Status(409, _) => AccountHttpError::Other("该账号名已经存在".to_string()),
        ureq::Error::Status(429, _) => {
            AccountHttpError::Other("尝试太频繁，请稍后再试".to_string())
        }
        ureq::Error::Status(code, _) => {
            AccountHttpError::Other(format!("服务器暂时不可用（{code}）"))
        }
        other => AccountHttpError::Other(other.to_string()),
    }
}

/// Authenticate without persisting the session. This lets the app acquire its
/// sync/profile switch barrier before making the new identity visible to any
/// background worker.
fn authenticate_via(
    route: &str,
    action: &str,
    server_url: &str,
    username: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let user = username.trim();
    if user.is_empty() || password.is_empty() {
        return Err(CoreError::Invalid("请填写账号和密码".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/{route}"),
            None,
            json!({ "username": user, "password": password }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized => CoreError::Llm("账号或密码不正确".into()),
            other => CoreError::Llm(format!("{action}失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, user, &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm(
            "账号服务器版本过旧：登录结果缺少不可变 user_id，请先升级 solum-cloud".into(),
        ));
    }
    Ok(session)
}

pub fn authenticate(
    server_url: &str,
    username: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    authenticate_via("login", "登录", server_url, username, password, model)
}

/// Create one account in the central Solum service and return its initial
/// session without publishing it locally. Whether registration is open remains
/// a server-side deployment policy.
pub fn register(
    server_url: &str,
    username: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    authenticate_via("register", "注册", server_url, username, password, model)
}

/// Log in and persist the session. The password lives only in this request.
pub fn login(
    server_url: &str,
    username: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    let session = authenticate(server_url, username, password, model)?;
    session.save()?;
    Ok(session)
}

/// Best-effort server-side revocation, then local sign-out. Never blocked by
/// the network: the refresh token expires server-side on its own schedule.
pub fn logout(session: &AccountSession) {
    let client = HttpAccountClient::new();
    let _ = client.post_json(
        &format!("{}/v1/auth/logout", session.server_url),
        Some(&session.access_token),
        json!({ "refresh_token": session.refresh_token }),
    );
    AccountSession::delete_file();
}

/// Rotate the token pair via `/v1/auth/refresh`. The rotated pair is returned
/// (not persisted here) so callers control when to save.
fn refresh_with_client<T: AccountHttp>(
    client: &T,
    session: &AccountSession,
) -> Result<AccountSession> {
    let response = client
        .post_json(
            &format!("{}/v1/auth/refresh", session.server_url),
            None,
            json!({ "refresh_token": session.refresh_token }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized => CoreError::Llm("登录已过期，请重新登录".into()),
            other => CoreError::Llm(format!("刷新登录状态失败: {other}")),
        })?;
    let mut rotated = parse_auth_response(
        &response,
        &session.server_url,
        &session.username,
        &session.model,
    )?;
    match session.stable_user_id() {
        // A legacy process is still running the guest database. Do not make a
        // newly returned UUID visible until an explicit login can cross the
        // app's sync barrier and restart into that account profile.
        None => rotated.user_id.clear(),
        Some(expected) if rotated.stable_user_id().is_none() => {
            rotated.user_id = expected.to_string();
        }
        Some(expected) if rotated.stable_user_id() != Some(expected) => {
            return Err(CoreError::Llm(
                "账号服务器刷新后返回了不同的 user_id，已拒绝切换身份".into(),
            ));
        }
        Some(_) => {}
    }
    Ok(rotated)
}

/// Rotate an account session through the configured identity server.
///
/// This is shared by non-AI clients (sync and alert delivery) so every Solum
/// surface follows the same refresh protocol. The returned session is not
/// persisted; use [`refresh_and_save`] when the caller owns the active local
/// session file.
pub fn refresh(session: &AccountSession) -> Result<AccountSession> {
    refresh_with_client(&HttpAccountClient::new(), session)
}

/// Rotate and atomically persist an account session before it is reused.
/// Persisting before retry prevents a successful refresh token rotation from
/// being lost when the retried relay request fails for an unrelated reason.
pub fn refresh_and_save(session: &AccountSession) -> Result<AccountSession> {
    refresh_and_save_with_client(&HttpAccountClient::new(), session)
}

fn refresh_and_save_with_client<T: AccountHttp>(
    client: &T,
    session: &AccountSession,
) -> Result<AccountSession> {
    let _guard = ACCOUNT_REFRESH_LOCK
        .lock()
        .map_err(|_| CoreError::Llm("账号刷新锁中毒".into()))?;
    // Another client in this process may have rotated the single-use refresh
    // token while this request was in flight. Re-read before spending it.
    let current = AccountSession::load().filter(|loaded| {
        loaded.server_url == session.server_url && loaded.username == session.username
    });
    if let Some(current) = current {
        if current.refresh_token != session.refresh_token {
            return Ok(current);
        }
    }
    let rotated = refresh_with_client(client, session)?;
    rotated.save()?;
    Ok(rotated)
}

/// POST an authorized request; a 401 refreshes the session exactly once and
/// retries. On rotation the new pair is persisted immediately — even if the
/// retry then fails — so a valid replacement token is never lost.
fn post_with_refresh<T: AccountHttp>(
    client: &T,
    session: &mut AccountSession,
    path: &str,
    body: Value,
) -> Result<Value> {
    let url = format!("{}{}", session.server_url, path);
    match client.post_json(&url, Some(&session.access_token), body.clone()) {
        Ok(value) => Ok(value),
        Err(AccountHttpError::Unauthorized) => {
            let rotated = refresh_and_save_with_client(client, session)?;
            *session = rotated;
            client
                .post_json(&url, Some(&session.access_token), body)
                .map_err(|e| CoreError::Llm(format!("刷新后重试失败: {e}")))
        }
        Err(error) => Err(CoreError::Llm(format!("请求失败: {error}"))),
    }
}

/// A [`Reasoner`] backed by the account proxy instead of a direct vendor key.
/// Interior mutability because token rotation happens mid-`&self` call.
pub struct AccountReasoner {
    session: Mutex<AccountSession>,
    agent: ureq::Agent,
}

impl AccountReasoner {
    pub fn new(session: AccountSession) -> Self {
        Self {
            session: Mutex::new(session),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .build(),
        }
    }

    pub fn masked_summary(&self) -> String {
        self.session
            .lock()
            .map(|s| s.masked_summary())
            .unwrap_or_else(|_| "账号状态不可用".to_string())
    }

    fn chat_body(session: &AccountSession, system: &str, user: &str, stream: bool) -> Value {
        json!({
            "model": session.model,
            "stream": stream,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        })
    }

    /// Non-streaming completion via the seam (unit-tested with a fake client).
    pub fn complete_with_client<T: AccountHttp>(
        client: &T,
        session: &mut AccountSession,
        system: &str,
        user: &str,
    ) -> Result<String> {
        let body = Self::chat_body(session, system, user, false);
        let response = post_with_refresh(client, session, "/v1/ai/chat/completions", body)?;
        crate::llm::parse_chat_content(&response)
    }

    /// Send the streaming request once, mapping a 401 to `Unauthorized` so the
    /// caller can refresh and retry (ureq surfaces the status before the SSE
    /// body starts, so auth failures never look like broken streams).
    fn send_streaming(
        &self,
        session: &AccountSession,
        system: &str,
        user: &str,
    ) -> std::result::Result<ureq::Response, AccountHttpError> {
        self.agent
            .post(&format!("{}/v1/ai/chat/completions", session.server_url))
            .set("Authorization", &format!("Bearer {}", session.access_token))
            .set("Accept", "text/event-stream")
            .send_json(Self::chat_body(session, system, user, true))
            .map_err(map_http_error)
    }
}

impl Reasoner for AccountReasoner {
    fn complete(&self, system: &str, user: &str) -> Result<String> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Llm("账号状态锁中毒".into()))?;
        let client = HttpAccountClient::new();
        Self::complete_with_client(&client, &mut session, system, user)
    }

    /// Streaming with the same think-stripping guarantees as
    /// [`crate::llm::LlmReasoner::complete_streaming`], plus the account
    /// layer's refresh-once-on-401 semantics.
    fn complete_streaming(
        &self,
        system: &str,
        user: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<String> {
        use std::io::BufRead;
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Llm("账号状态锁中毒".into()))?;
        let response = match self.send_streaming(&session, system, user) {
            Ok(response) => response,
            Err(AccountHttpError::Unauthorized) => {
                let client = HttpAccountClient::new();
                let rotated = refresh_and_save_with_client(&client, &session)?;
                *session = rotated;
                self.send_streaming(&session, system, user)
                    .map_err(|e| CoreError::Llm(format!("刷新后重试失败: {e}")))?
            }
            Err(error) => return Err(CoreError::Llm(format!("请求失败: {error}"))),
        };
        drop(session);
        let reader = std::io::BufReader::new(response.into_reader());
        let mut full = String::new();
        let mut filter = crate::llm::ThinkFilter::default();
        for line in reader.lines() {
            let line = line.map_err(|e| CoreError::Llm(format!("流式读取中断: {e}")))?;
            match crate::llm::sse_line(&line) {
                crate::llm::SseLine::Done => break,
                crate::llm::SseLine::Ignore => {}
                crate::llm::SseLine::Content(piece) => {
                    full.push_str(&piece);
                    let visible = filter.push(&piece);
                    if !visible.is_empty() {
                        on_token(&visible);
                    }
                }
            }
        }
        let out = crate::llm::strip_think_block(&full).trim().to_string();
        if out.is_empty() {
            return Err(CoreError::Llm(format!("流式响应为空: {full:?}")));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn session_roundtrip_and_harmony_shape_compat() {
        // 鸿蒙版四字段文件（无 model）→ 默认模型补齐。
        let harmony = r#"{
            "server_url": "https://cloud.example.com",
            "username": "tangren",
            "access_token": "a.b",
            "refresh_token": "r"
        }"#;
        let session = AccountSession::from_json(harmony).unwrap();
        assert_eq!(session.model, DEFAULT_CLOUD_MODEL);
        assert_eq!(session.username, "tangren");
        assert_eq!(session.stable_user_id(), None);
        // 本仓写出的形状（含 model）自往返。
        let json = serde_json::to_string(&session).unwrap();
        let back = AccountSession::from_json(&json).unwrap();
        assert_eq!(session, back);
    }

    #[test]
    fn session_rejects_missing_fields_and_bad_urls() {
        let missing = r#"{"server_url":"https://x.example","username":"u","access_token":"","refresh_token":"r"}"#;
        assert!(AccountSession::from_json(missing).is_err());
        let plain_http = r#"{"server_url":"http://10.0.0.2:8787","username":"u","access_token":"a","refresh_token":"r"}"#;
        assert!(AccountSession::from_json(plain_http).is_err());
        let loopback = r#"{"server_url":"http://127.0.0.1:8787","username":"u","access_token":"a","refresh_token":"r"}"#;
        assert!(AccountSession::from_json(loopback).is_ok());
    }

    #[test]
    fn model_validation_mirrors_server() {
        assert_eq!(normalize_cloud_model("").unwrap(), DEFAULT_CLOUD_MODEL);
        assert_eq!(
            normalize_cloud_model(" mimo-v2.5-pro ").unwrap(),
            "mimo-v2.5-pro"
        );
        assert!(normalize_cloud_model("bad model").is_err());
        assert!(normalize_cloud_model(&"x".repeat(101)).is_err());
    }

    #[test]
    fn stable_user_id_requires_a_lowercase_uuid() {
        assert!(is_valid_user_id("9d4df1be-9f7b-4a3a-b986-ec920d2df60e"));
        assert!(!is_valid_user_id("alice"));
        assert!(!is_valid_user_id("9D4DF1BE-9F7B-4A3A-B986-EC920D2DF60E"));
    }

    #[test]
    fn auth_response_missing_tokens_is_an_error_not_a_broken_session() {
        let bad = json!({ "user": { "username": "u" }, "access_token": "a" });
        assert!(parse_auth_response(&bad, "https://x.example", "u", "mimo-v2.5").is_err());
        let good =
            json!({ "user": { "username": "real" }, "access_token": "a", "refresh_token": "r" });
        let session =
            parse_auth_response(&good, "https://x.example", "fallback", "mimo-v2.5").unwrap();
        assert_eq!(session.username, "real");
    }

    /// Scripted fake: first authorized call 401s, refresh rotates the pair,
    /// retry succeeds — and the rotated pair is what the session ends up with.
    struct RotatingFake {
        calls: RefCell<Vec<String>>,
    }

    impl AccountHttp for RotatingFake {
        fn post_json(
            &self,
            url: &str,
            access_token: Option<&str>,
            _body: Value,
        ) -> std::result::Result<Value, AccountHttpError> {
            self.calls.borrow_mut().push(url.to_string());
            if url.ends_with("/v1/auth/refresh") {
                return Ok(json!({
                        "access_token": "access-2",
                        "refresh_token": "refresh-2",
                        "user": {
                            "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                            "username": "u"
                        },
                }));
            }
            match access_token {
                Some("access-2") => Ok(json!({
                    "choices": [ { "message": { "content": "pong" } } ]
                })),
                _ => Err(AccountHttpError::Unauthorized),
            }
        }
    }

    #[test]
    fn expired_access_token_refreshes_once_and_retries() {
        // 会话文件落到临时目录，避免测试写进仓库工作区。
        let dir = std::env::temp_dir().join(format!("solum-account-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var(
            "SOLUM_ACCOUNT_CONFIG",
            dir.join("solum-account.json").to_string_lossy().to_string(),
        );
        let mut session = AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
            username: "u".into(),
            access_token: "access-1".into(),
            refresh_token: "refresh-1".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        };
        let fake = RotatingFake {
            calls: RefCell::new(Vec::new()),
        };
        let reply = AccountReasoner::complete_with_client(&fake, &mut session, "s", "u").unwrap();
        assert_eq!(reply, "pong");
        assert_eq!(session.access_token, "access-2");
        assert_eq!(session.refresh_token, "refresh-2");
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 3, "chat → refresh → chat retry");
        assert!(calls[0].ends_with("/v1/ai/chat/completions"));
        assert!(calls[1].ends_with("/v1/auth/refresh"));
        assert!(calls[2].ends_with("/v1/ai/chat/completions"));
        // 轮换后的会话已持久化（post_with_refresh 立即落盘，重试失败也不丢新令牌）。
        let persisted = AccountSession::load().expect("rotated session persisted");
        assert_eq!(persisted.access_token, "access-2");
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_refresh_does_not_promote_the_running_guest_profile() {
        let client = RotatingFake {
            calls: RefCell::new(Vec::new()),
        };
        let legacy = AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: String::new(),
            username: "u".into(),
            access_token: "access-1".into(),
            refresh_token: "refresh-1".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        };
        let rotated = refresh_with_client(&client, &legacy).unwrap();
        assert_eq!(rotated.stable_user_id(), None);
        assert_eq!(rotated.access_token, "access-2");
    }

    #[test]
    fn refresh_failure_maps_to_relogin_message() {
        struct AlwaysUnauthorized;
        impl AccountHttp for AlwaysUnauthorized {
            fn post_json(
                &self,
                _url: &str,
                _token: Option<&str>,
                _body: Value,
            ) -> std::result::Result<Value, AccountHttpError> {
                Err(AccountHttpError::Unauthorized)
            }
        }
        let mut session = AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
            username: "refresh-failure-test".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        };
        let err =
            AccountReasoner::complete_with_client(&AlwaysUnauthorized, &mut session, "s", "u")
                .unwrap_err();
        assert!(err.to_string().contains("重新登录"), "{err}");
    }
}

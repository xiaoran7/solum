//! Solum 独立账号：鉴权自建 solum-cloud AI 代理（`server/`，与 Solum Harmony 共用同一契约）。
//!
//! 登录后 AI 请求发往 `{server}/v1/ai/chat/completions`，第三方 API Key 只存在于
//! 服务端环境变量；登录动作本身不上传本机数据库。同一短期 access token 也供同步
//! relay 验证租户身份，但同步正文仍由独立的设备端密钥加密。
//!
//! 会话落盘 `solum-account.json`（gitignore）：`server_url` / `user_id` /
//! `username` / `access_token` / `refresh_token`。`user_id` 是服务端生成且不可变的
//! 身份主键；用户名只用于登录和展示，绝不能再充当本地目录或中继租户主键。
//! 旧会话没有 `user_id` 时必须重新登录，不能参与刷新或账号数据访问。
//! 密码只用于登录请求本身，绝不落盘。

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
use base64::Engine;
#[cfg(not(target_os = "android"))]
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(target_os = "android")]
use std::os::fd::AsRawFd;

use crate::error::{CoreError, Result};
use crate::extract::Reasoner;

pub const DEFAULT_CLOUD_MODEL: &str = "gemini-3.8-flash-high";
pub const CLOUD_MODEL_OPTIONS: &[&str] = &["gemini-3.8-flash-high"];

/// Solum Center 当前对外身份（国内直连备案域名）。
pub const SOLUM_CENTER_URL: &str = "https://tannerlab.cn";
/// 已进入废弃流程的旧中心域名；存量会话读到其中任一个时透明改写为新身份
/// （同一后端，token 不绑定 Host），兼容窗口内旧域名仍可用作别名。
const LEGACY_CENTER_URLS: &[&str] = &[
    "https://solumsync.tannerlab.xyz",
    "https://solum.tannerlab.xyz",
    "https://solum.tannerlab.cn",
];

/// 域名合并映射：恰好等于旧中心域名（忽略末尾斜杠）时改写为当前身份，
/// 其余地址原样返回。账号会话与同步密钥配置共用这一条规则。
pub fn migrate_legacy_center_url(value: &str) -> String {
    let trimmed = value.trim_end_matches('/');
    if LEGACY_CENTER_URLS
        .iter()
        .any(|legacy| trimmed.eq_ignore_ascii_case(legacy))
    {
        SOLUM_CENTER_URL.to_string()
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudModelOption {
    pub id: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudModelCatalog {
    pub default_model: String,
    pub models: Vec<CloudModelOption>,
    pub custom_models_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudUsageLatest {
    pub request_id: String,
    pub requested_model: String,
    pub effective_model: String,
    pub status: String,
    pub fallback_occurred: bool,
    pub fallback_reason: Option<String>,
    pub upstream_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudUsage {
    pub plan: String,
    pub custom_models_enabled: bool,
    pub monthly_request_limit: u64,
    pub requests_used: u64,
    pub requests_remaining: u64,
    pub reset_at: String,
    pub success_count: u64,
    pub failure_count: u64,
    pub fallback_count: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub latest_request: Option<CloudUsageLatest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudAccountState {
    pub catalog: CloudModelCatalog,
    pub usage: CloudUsage,
}

/// Access tokens live 15 minutes server-side; a 401 mid-conversation is
/// routine, so every authorized call refreshes exactly once and retries.
/// The generous timeout covers streaming replies from thinking-mode models
/// (mirrors the harmony client's 120s read timeout).
const TIMEOUT_SECS: u64 = 120;
const REVOKE_TIMEOUT_SECS: u64 = 5;
static ACCOUNT_SESSION_LOCK: Mutex<()> = Mutex::new(());

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

    pub(crate) fn normalize(&mut self) -> Result<()> {
        self.server_url = migrate_legacy_center_url(&normalize_server_url(&self.server_url)?);
        self.user_id = self.user_id.trim().to_ascii_lowercase();
        if !self.user_id.is_empty() && !is_valid_user_id(&self.user_id) {
            return Err(CoreError::Invalid(
                "solum-account.json 的 user_id 不是有效 UUID".into(),
            ));
        }
        self.username = self.username.trim().to_string();
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

    /// A session may predate immutable account UUIDs. It can be read for a
    /// controlled migration, but it is not a safe identity boundary for an
    /// account-scoped local profile.
    pub fn load_full() -> Option<Self> {
        Self::load().filter(|session| session.stable_user_id().is_some())
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        crate::fsatomic::write_atomic(&Self::path(), &json)
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

fn session_lock_path(session_path: &Path) -> PathBuf {
    let mut value = session_path.as_os_str().to_os_string();
    value.push(".lock");
    value.into()
}

#[cfg(target_os = "android")]
fn set_session_file_lock(
    file: &std::fs::File,
    command: libc::c_int,
    lock_type: libc::c_short,
) -> std::io::Result<()> {
    // Java FileChannel uses POSIX record locks. Ordinary F_SETLKW locks are
    // process-owned, so Rust and Kotlin threads in the same Android process
    // would not block one another. Linux OFD locks are fd-owned and explicitly
    // conflict with POSIX locks even inside one process, giving both runtimes
    // one real kernel lock on solum-account.json.lock.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = lock_type;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    loop {
        let result = unsafe { libc::fcntl(file.as_raw_fd(), command, &lock) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(target_os = "android")]
fn lock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    set_session_file_lock(file, libc::F_OFD_SETLKW, libc::F_WRLCK as libc::c_short)
}

#[cfg(target_os = "android")]
fn unlock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    set_session_file_lock(file, libc::F_OFD_SETLK, libc::F_UNLCK as libc::c_short)
}

#[cfg(not(target_os = "android"))]
fn lock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    FileExt::lock_exclusive(file)
}

#[cfg(not(target_os = "android"))]
fn unlock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    FileExt::unlock(file)
}

fn with_session_lock<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let _process_guard = ACCOUNT_SESSION_LOCK
        .lock()
        .map_err(|_| CoreError::Llm("账号会话锁中毒".into()))?;
    let session_path = AccountSession::path();
    if let Some(parent) = session_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| CoreError::Storage(error.to_string()))?;
    }
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(session_lock_path(&session_path))
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    lock_session_file(&lock_file).map_err(|error| CoreError::Storage(error.to_string()))?;
    let result = operation();
    let unlock_result =
        unlock_session_file(&lock_file).map_err(|error| CoreError::Storage(error.to_string()));
    match (result, unlock_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

/// Publish a newly authenticated identity without racing token rotation.
pub fn publish_session(session: &AccountSession) -> Result<()> {
    with_session_lock(|| session.save())
}

/// Remove the active identity under the same lock used by refresh writes.
/// Unlike the legacy `delete_file` helper, filesystem failures are visible.
pub fn clear_session() -> Result<()> {
    take_session().map(|_| ())
}

/// Atomically remove and return the latest session. Logout uses the returned
/// refresh token, so a concurrent rotation cannot leave a newer remote token
/// active after local state is gone.
pub fn take_session() -> Result<Option<AccountSession>> {
    with_session_lock(|| {
        let current = AccountSession::load();
        match std::fs::remove_file(AccountSession::path()) {
            Ok(()) => Ok(current),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(CoreError::Storage(error.to_string())),
        }
    })
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

    fn get_json(
        &self,
        _url: &str,
        _access_token: &str,
    ) -> std::result::Result<Value, AccountHttpError> {
        Err(AccountHttpError::Other(
            "当前 HTTP 实现不支持读取账号状态".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountHttpError {
    Unauthorized,
    Retryable(String),
    Other(String),
}

impl std::fmt::Display for AccountHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountHttpError::Unauthorized => f.write_str("认证已过期"),
            AccountHttpError::Retryable(message) => f.write_str(message),
            AccountHttpError::Other(message) => f.write_str(message),
        }
    }
}

pub struct HttpAccountClient {
    agent: ureq::Agent,
}

impl HttpAccountClient {
    pub fn new() -> Self {
        Self::with_timeout(TIMEOUT_SECS)
    }

    fn with_timeout(timeout_secs: u64) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(timeout_secs))
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
            .map_err(|e| AccountHttpError::Retryable(format!("响应不是 JSON: {e}")))
    }

    fn get_json(
        &self,
        url: &str,
        access_token: &str,
    ) -> std::result::Result<Value, AccountHttpError> {
        self.agent
            .get(url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .call()
            .map_err(map_http_error)?
            .into_json()
            .map_err(|e| AccountHttpError::Retryable(format!("响应不是 JSON: {e}")))
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
        ureq::Error::Transport(other) => AccountHttpError::Retryable(other.to_string()),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub enabled: bool,
    pub issuer: String,
    pub client_id: String,
}

/// Generate a cryptographically random PKCE code_verifier (32 bytes -> 43 chars base64url)
/// and S256 code_challenge.
pub fn generate_pkce() -> Result<PkcePair> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| CoreError::Invalid(format!("os rng error: {e}")))?;
    let verifier = B64_URL.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = B64_URL.encode(hasher.finalize());
    Ok(PkcePair {
        code_verifier: verifier,
        code_challenge: challenge,
        code_challenge_method: "S256".to_string(),
    })
}

/// Fetch OAuth config from the server.
pub fn fetch_oauth_config(server_url: &str) -> Result<OAuthConfig> {
    let server = normalize_server_url(server_url)?;
    let client = HttpAccountClient::new();
    let response = client
        .get_json(&format!("{server}/v1/auth/oauth/config"), "")
        .map_err(|e| CoreError::Llm(format!("获取 OAuth 配置失败: {e}")))?;

    let enabled = response
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let issuer = response
        .get("issuer")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let client_id = response
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok(OAuthConfig {
        enabled,
        issuer,
        client_id,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HuaweiOAuthConfig {
    pub enabled: bool,
    pub client_id: String,
    pub authorize_url: String,
    pub redirect_uri: String,
    pub scope: String,
}

/// Fetch Huawei OAuth configuration from server
pub fn fetch_huawei_config(server_url: &str) -> Result<HuaweiOAuthConfig> {
    let server = normalize_server_url(server_url)?;
    let client = HttpAccountClient::new();
    let response = client
        .get_json(&format!("{server}/v1/auth/huawei/config"), "")
        .map_err(|e| CoreError::Llm(format!("获取华为登录配置失败: {e}")))?;

    let enabled = response
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let client_id = response
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let authorize_url = response
        .get("authorize_url")
        .and_then(Value::as_str)
        .unwrap_or("https://oauth-login.cloud.huawei.com/oauth2/v3/authorize")
        .to_string();
    let redirect_uri = response
        .get("redirect_uri")
        .and_then(Value::as_str)
        .unwrap_or("solum://oauth/callback")
        .to_string();
    let scope = response
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("openid profile")
        .to_string();

    Ok(HuaweiOAuthConfig {
        enabled,
        client_id,
        authorize_url,
        redirect_uri,
        scope,
    })
}

/// Authenticate via Huawei Authorization Code
pub fn authenticate_via_huawei(
    server_url: &str,
    code: &str,
    redirect_uri: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    if code.trim().is_empty() {
        return Err(CoreError::Invalid("华为授权码不能为空".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/huawei/exchange"),
            None,
            json!({
                "code": code.trim(),
                "redirect_uri": redirect_uri.trim()
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized => CoreError::Llm("华为授权失败或已过期".into()),
            other => CoreError::Llm(format!("华为登录失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, "", &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm("账号服务器返回缺少 user_id".into()));
    }
    Ok(session)
}

/// Request SMS verification code for a phone number
pub fn request_sms_code(server_url: &str, phone: &str) -> Result<()> {
    let server = normalize_server_url(server_url)?;
    let phone_clean = phone.trim();
    if phone_clean.is_empty() {
        return Err(CoreError::Invalid("请输入手机号".into()));
    }
    let client = HttpAccountClient::new();
    let _ = client
        .post_json(
            &format!("{server}/v1/auth/sms/send"),
            None,
            json!({ "phone": phone_clean }),
        )
        .map_err(|e| match e {
            AccountHttpError::Other(msg) if msg.contains("429") || msg.contains("rate") => {
                CoreError::Llm("短信发送太频繁，请稍后再试".into())
            }
            AccountHttpError::Other(msg) if msg.contains("format") => {
                CoreError::Invalid("手机号格式不正确，请输入大陆11位手机号".into())
            }
            other => CoreError::Llm(format!("发送短信验证码失败: {other}")),
        })?;
    Ok(())
}

/// Register with email and password
pub fn register_email(
    server_url: &str,
    email: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let email_clean = email.trim().to_lowercase();
    if email_clean.is_empty() {
        return Err(CoreError::Invalid("请输入邮箱地址".into()));
    }
    if !email_clean.contains('@') || !email_clean.contains('.') {
        return Err(CoreError::Invalid("邮箱格式不正确".into()));
    }
    if password.len() < 8 {
        return Err(CoreError::Invalid("密码至少需要 8 个字符".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/email/register"),
            None,
            json!({
                "email": email_clean,
                "password": password
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Other(msg)
                if msg.contains("email_already_registered") || msg.contains("409") =>
            {
                CoreError::Llm("该邮箱已被注册，请直接登录".into())
            }
            AccountHttpError::Other(msg) if msg.contains("format") => {
                CoreError::Invalid("邮箱格式不正确".into())
            }
            other => CoreError::Llm(format!("邮箱注册失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, &email_clean, &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm("账号服务器注册结果缺少 user_id".into()));
    }
    Ok(session)
}

/// Login with email and password
pub fn login_email(
    server_url: &str,
    email: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let email_clean = email.trim().to_lowercase();
    if email_clean.is_empty() || password.is_empty() {
        return Err(CoreError::Invalid("请输入邮箱和密码".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/email/login"),
            None,
            json!({
                "email": email_clean,
                "password": password
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized => CoreError::Llm("邮箱或密码不正确".into()),
            other => CoreError::Llm(format!("登录失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, &email_clean, &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm("登录结果缺少 user_id".into()));
    }
    Ok(session)
}

/// Register with phone, SMS code and password
pub fn register_phone(
    server_url: &str,
    phone: &str,
    code: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let phone_clean = phone.trim();
    let code_clean = code.trim();
    if phone_clean.is_empty() {
        return Err(CoreError::Invalid("请输入手机号".into()));
    }
    if code_clean.is_empty() {
        return Err(CoreError::Invalid("请输入短信验证码".into()));
    }
    if password.len() < 8 {
        return Err(CoreError::Invalid("密码至少需要 8 个字符".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/phone/register"),
            None,
            json!({
                "phone": phone_clean,
                "code": code_clean,
                "password": password
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Other(msg) if msg.contains("409") || msg.contains("already") => {
                CoreError::Llm("该手机号已注册，请直接登录".into())
            }
            AccountHttpError::Other(msg) if msg.contains("expired") || msg.contains("invalid") => {
                CoreError::Llm("短信验证码错误或已过期".into())
            }
            other => CoreError::Llm(format!("手机号注册失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, phone_clean, &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm("账号服务器注册结果缺少 user_id".into()));
    }
    Ok(session)
}

/// Login with phone and password
pub fn login_phone_password(
    server_url: &str,
    phone: &str,
    password: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let phone_clean = phone.trim();
    if phone_clean.is_empty() || password.is_empty() {
        return Err(CoreError::Invalid("请输入手机号和密码".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/phone/login"),
            None,
            json!({
                "phone": phone_clean,
                "password": password
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized => CoreError::Llm("手机号或密码不正确".into()),
            other => CoreError::Llm(format!("登录失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, phone_clean, &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm("登录结果缺少 user_id".into()));
    }
    Ok(session)
}

/// Login with phone and SMS code (or quick register)
pub fn login_phone_code(
    server_url: &str,
    phone: &str,
    code: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let phone_clean = phone.trim();
    let code_clean = code.trim();
    if phone_clean.is_empty() || code_clean.is_empty() {
        return Err(CoreError::Invalid("请输入手机号和验证码".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/phone/login"),
            None,
            json!({
                "phone": phone_clean,
                "code": code_clean
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized | AccountHttpError::Other(_) => {
                CoreError::Llm("验证码错误或已失效，请重新获取".into())
            }
            AccountHttpError::Retryable(err) => {
                CoreError::Llm(format!("网络请求失败，请稍后重试: {err}"))
            }
        })?;
    let session = parse_auth_response(&response, &server, phone_clean, &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm("登录结果缺少 user_id".into()));
    }
    Ok(session)
}

/// Delete account (compliance requirement)
pub fn delete_account(server_url: &str, access_token: &str) -> Result<()> {
    let server = normalize_server_url(server_url)?;
    let client = HttpAccountClient::new();
    let _ = client
        .post_json(
            &format!("{server}/v1/account/delete"),
            Some(access_token),
            json!({}),
        )
        .map_err(|e| CoreError::Llm(format!("注销账号失败: {e}")))?;
    Ok(())
}

/// Exchange OAuth authorization code for an AccountSession with Solum Center.
pub fn authenticate_via_oauth(
    server_url: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    if code.trim().is_empty() || code_verifier.trim().is_empty() {
        return Err(CoreError::Invalid("OAuth 授权参数不完整".into()));
    }
    let client = HttpAccountClient::new();
    let response = client
        .post_json(
            &format!("{server}/v1/auth/oauth/exchange"),
            None,
            json!({
                "code": code.trim(),
                "code_verifier": code_verifier.trim(),
                "redirect_uri": redirect_uri.trim()
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Unauthorized => CoreError::Llm("OAuth 授权已过期或无效".into()),
            other => CoreError::Llm(format!("OAuth 登录失败: {other}")),
        })?;
    let session = parse_auth_response(&response, &server, "", &model)?;
    if session.stable_user_id().is_none() {
        return Err(CoreError::Llm(
            "账号服务器版本过旧：OAuth 登录结果缺少不可变 user_id".into(),
        ));
    }
    Ok(session)
}

/// Best-effort revocation without changing whichever local session is active.
/// Login/registration uses this for a newly issued token when local profile
/// preparation fails before that session is published.
pub fn revoke_remote(session: &AccountSession) {
    let client = HttpAccountClient::with_timeout(REVOKE_TIMEOUT_SECS);
    revoke_remote_with_client(&client, session);
}

fn revoke_remote_with_client<T: AccountHttp>(client: &T, session: &AccountSession) {
    let _ = client.post_json(
        &format!("{}/v1/auth/logout", session.server_url),
        None,
        json!({ "refresh_token": session.refresh_token }),
    );
}

fn same_account_identity(left: &AccountSession, right: &AccountSession) -> bool {
    if left.server_url != right.server_url {
        return false;
    }
    match (left.stable_user_id(), right.stable_user_id()) {
        (Some(left_id), Some(right_id)) => left_id == right_id,
        (None, None) => left.username == right.username,
        _ => false,
    }
}

/// Save a model choice onto the latest rotated token pair for this identity.
/// A stale UI request must never overwrite a newer refresh token.
pub fn update_active_model(expected: &AccountSession, model: &str) -> Result<AccountSession> {
    let model = normalize_cloud_model(model)?;
    with_session_lock(|| {
        let mut current = AccountSession::load_full()
            .ok_or_else(|| CoreError::Invalid("当前没有完整账号会话".into()))?;
        if !same_account_identity(&current, expected) {
            return Err(CoreError::Invalid(
                "账号已在其他操作中切换，请重新打开账号设置".into(),
            ));
        }
        current.model = model;
        current.save()?;
        Ok(current)
    })
}

/// Rotate the token pair via `/v1/auth/refresh`. The rotated pair is returned
/// (not persisted here) so callers control when to save.
fn refresh_with_client<T: AccountHttp>(
    client: &T,
    session: &AccountSession,
) -> Result<AccountSession> {
    let expected_user_id = session
        .stable_user_id()
        .ok_or_else(|| CoreError::Llm("旧账号会话缺少不可变 user_id，请重新登录".into()))?;
    let request = || {
        client.post_json(
            &format!("{}/v1/auth/refresh", session.server_url),
            None,
            json!({ "refresh_token": session.refresh_token }),
        )
    };
    let response = match request() {
        Err(AccountHttpError::Retryable(_)) => request(),
        result => result,
    }
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
    if rotated.stable_user_id().is_none() {
        rotated.user_id = expected_user_id.to_string();
    } else if rotated.stable_user_id() != Some(expected_user_id) {
        return Err(CoreError::Llm(
            "账号服务器刷新后返回了不同的 user_id，已拒绝切换身份".into(),
        ));
    }
    Ok(rotated)
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
    let load_current = || {
        let current = AccountSession::load_full()
            .ok_or_else(|| CoreError::Invalid("当前账号会话已退出，请重新登录".into()))?;
        if !same_account_identity(&current, session) {
            return Err(CoreError::Invalid(
                "账号已在其他操作中切换，旧请求不能刷新当前会话".into(),
            ));
        }
        Ok(current)
    };

    // Read the newest token/model under the cross-process lock, then release
    // it before network I/O. Logout must never wait behind a 120-second HTTP
    // timeout merely to remove local credentials.
    let base = with_session_lock(load_current)?;
    let mut rotated = match refresh_with_client(client, &base) {
        Ok(rotated) => rotated,
        Err(refresh_error) => {
            // A different process may have won the single-use rotation while
            // this request was in flight. Adopt only a newly persisted pair;
            // missing/different identity remains a hard stop.
            let current = with_session_lock(load_current)?;
            if current.refresh_token != base.refresh_token {
                return Ok(current);
            }
            return Err(refresh_error);
        }
    };

    let committed = with_session_lock(|| {
        let current = load_current()?;
        if current.refresh_token != base.refresh_token {
            return Ok((current, false));
        }
        // A model-only update may have landed while the refresh request was
        // running. Tokens come from the server; model choice comes from the
        // newest local session.
        rotated.model = current.model;
        rotated.save()?;
        Ok((rotated.clone(), true))
    });
    match committed {
        Ok((session, true)) => Ok(session),
        Ok((session, false)) => {
            revoke_remote_with_client(client, &rotated);
            Ok(session)
        }
        Err(error) => {
            revoke_remote_with_client(client, &rotated);
            Err(error)
        }
    }
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

fn get_with_refresh<T: AccountHttp>(
    client: &T,
    session: &mut AccountSession,
    path: &str,
) -> Result<Value> {
    let url = format!("{}{}", session.server_url, path);
    match client.get_json(&url, &session.access_token) {
        Ok(value) => Ok(value),
        Err(AccountHttpError::Unauthorized) => {
            let rotated = refresh_and_save_with_client(client, session)?;
            *session = rotated;
            client
                .get_json(&url, &session.access_token)
                .map_err(|e| CoreError::Llm(format!("刷新后读取账号状态失败: {e}")))
        }
        Err(error) => Err(CoreError::Llm(format!("读取账号状态失败: {error}"))),
    }
}

pub fn cloud_account_state(session: &mut AccountSession) -> Result<CloudAccountState> {
    let client = HttpAccountClient::new();
    cloud_account_state_with_client(&client, session)
}

fn cloud_account_state_with_client<T: AccountHttp>(
    client: &T,
    session: &mut AccountSession,
) -> Result<CloudAccountState> {
    let catalog = serde_json::from_value::<CloudModelCatalog>(get_with_refresh(
        client,
        session,
        "/v1/ai/models",
    )?)
    .map_err(|_| CoreError::Llm("服务器返回了无法识别的模型目录".into()))?;
    if catalog.models.is_empty()
        || !catalog
            .models
            .iter()
            .any(|model| model.id == catalog.default_model && model.is_default)
    {
        return Err(CoreError::Llm("服务器模型目录缺少默认模型".into()));
    }
    let usage =
        serde_json::from_value::<CloudUsage>(get_with_refresh(client, session, "/v1/ai/usage")?)
            .map_err(|_| CoreError::Llm("服务器返回了无法识别的额度统计".into()))?;
    Ok(CloudAccountState { catalog, usage })
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

    fn tool_chat_body(
        session: &AccountSession,
        system: &str,
        user: &str,
        tools: &[Value],
    ) -> Value {
        json!({
            "model": session.model,
            "stream": false,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "tools": tools,
            "tool_choice": "auto",
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

    fn complete_with_tools(
        &self,
        system: &str,
        user: &str,
        tools: &[Value],
    ) -> Result<crate::extract::ReasonerResponse> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Llm("账号状态锁中毒".into()))?;
        let body = Self::tool_chat_body(&session, system, user, tools);
        let client = HttpAccountClient::new();
        let response = post_with_refresh(&client, &mut session, "/v1/ai/chat/completions", body)?;
        crate::llm::parse_reasoner_response(&response)
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
    use std::cell::{Cell, RefCell};
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    static ACCOUNT_CONFIG_TEST_LOCK: Mutex<()> = Mutex::new(());

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

        let opaque = r#"{
            "server_url":"https://cloud.example.com",
            "username":"u",
            "access_token":"  access  ",
            "refresh_token":"\trefresh\n"
        }"#;
        let opaque = AccountSession::from_json(opaque).unwrap();
        assert_eq!(opaque.access_token, "  access  ");
        assert_eq!(opaque.refresh_token, "\trefresh\n");
    }

    #[test]
    fn sessions_saved_under_retired_centers_follow_the_current_domain() {
        // 两代 `.xyz` 中心域名下的存量会话在加载时都透明迁移到 `.cn`。
        let legacy = r#"{
            "server_url": "LEGACY_CENTER",
            "user_id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
            "username": "tangren",
            "access_token": "a.b",
            "refresh_token": "r"
        }"#;
        for retired in LEGACY_CENTER_URLS {
            let session =
                AccountSession::from_json(&legacy.replace("LEGACY_CENTER", retired)).unwrap();
            assert_eq!(session.server_url, SOLUM_CENTER_URL);
        }
        // 合并只针对旧中心域名，其他地址原样保留。
        let untouched = AccountSession::from_json(
            &legacy.replace("LEGACY_CENTER", "https://solum.example.org"),
        )
        .unwrap();
        assert_eq!(untouched.server_url, "https://solum.example.org");
    }

    #[test]
    fn center_domain_merge_maps_only_retired_hosts() {
        // 恰好等于任一旧中心域名（含末尾斜杠）→ 当前身份；其他地址一律不动。
        for retired in LEGACY_CENTER_URLS {
            assert_eq!(migrate_legacy_center_url(retired), SOLUM_CENTER_URL);
            assert_eq!(
                migrate_legacy_center_url(&format!("{retired}/")),
                SOLUM_CENTER_URL
            );
        }
        assert_eq!(
            migrate_legacy_center_url(SOLUM_CENTER_URL),
            SOLUM_CENTER_URL
        );
        assert_eq!(
            migrate_legacy_center_url("https://solumsync.example.org"),
            "https://solumsync.example.org"
        );
    }

    #[test]
    fn only_uuid_sessions_form_a_full_account_identity() {
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "solum-account-full-session-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("solum-account.json");
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", &path);

        let mut session = AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: String::new(),
            username: "legacy".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        };
        session.save().unwrap();
        assert!(AccountSession::load().is_some());
        assert!(AccountSession::load_full().is_none());

        session.user_id = "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into();
        session.save().unwrap();
        assert_eq!(
            AccountSession::load_full()
                .and_then(|loaded| loaded.stable_user_id().map(str::to_owned))
                .as_deref(),
            Some("9d4df1be-9f7b-4a3a-b986-ec920d2df60e")
        );

        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_never_adopts_a_different_uuid_with_the_same_username() {
        let original = AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
            username: "same-name".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        };
        let mut replacement = original.clone();
        replacement.user_id = "6ea64c69-0531-45cb-b585-500c5f479fd8".into();
        assert!(!same_account_identity(&original, &replacement));
        replacement.user_id = original.user_id.clone();
        assert!(same_account_identity(&original, &replacement));
    }

    #[test]
    fn model_update_preserves_the_latest_rotated_tokens() {
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "solum-account-model-update-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));

        let expected = AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
            username: "same-name".into(),
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        };
        let mut latest = expected.clone();
        latest.access_token = "new-access".into();
        latest.refresh_token = "new-refresh".into();
        latest.save().unwrap();

        let updated = update_active_model(&expected, "mimo-v2.5-pro").unwrap();
        assert_eq!(updated.access_token, "new-access");
        assert_eq!(updated.refresh_token, "new-refresh");
        assert_eq!(updated.model, "mimo-v2.5-pro");
        assert_eq!(AccountSession::load_full().unwrap(), updated);

        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_file_lock_blocks_a_second_writer() {
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "solum-account-file-lock-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
        let lock_path = session_lock_path(&AccountSession::path());
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        lock_session_file(&lock_file).unwrap();

        let session = state_test_session();
        let (sent, received) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            let result = publish_session(&session);
            sent.send(result).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
        unlock_session_file(&lock_file).unwrap();
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("writer did not resume after OS lock release")
            .unwrap();
        writer.join().unwrap();
        drop(lock_file);

        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_refresh_cannot_recreate_a_missing_or_different_session() {
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "solum-account-stale-refresh-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
        let stale = state_test_session();
        let fake = RotatingFake {
            calls: RefCell::new(Vec::new()),
        };

        assert!(refresh_and_save_with_client(&fake, &stale).is_err());
        assert!(
            fake.calls.borrow().is_empty(),
            "missing state must fail before HTTP"
        );

        let mut replacement = stale.clone();
        replacement.user_id = "6ea64c69-0531-45cb-b585-500c5f479fd8".into();
        replacement.access_token = "other-access".into();
        replacement.refresh_token = "other-refresh".into();
        publish_session(&replacement).unwrap();
        assert!(refresh_and_save_with_client(&fake, &stale).is_err());
        assert_eq!(AccountSession::load_full().unwrap(), replacement);
        assert!(
            fake.calls.borrow().is_empty(),
            "another identity must never be contacted"
        );

        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_preserves_a_model_change_that_lands_during_network_io() {
        struct ModelRaceFake {
            expected: AccountSession,
        }
        impl AccountHttp for ModelRaceFake {
            fn post_json(
                &self,
                url: &str,
                _access_token: Option<&str>,
                _body: Value,
            ) -> std::result::Result<Value, AccountHttpError> {
                if url.ends_with("/v1/auth/refresh") {
                    update_active_model(&self.expected, "mimo-v2.5-pro").unwrap();
                    return Ok(json!({
                        "access_token":"access-2", "refresh_token":"refresh-2",
                        "user":{"id":self.expected.user_id.clone(),
                                "username":self.expected.username.clone()}
                    }));
                }
                Ok(json!({"status":"ok"}))
            }
        }

        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "solum-account-model-race-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
        let session = state_test_session();
        publish_session(&session).unwrap();
        let rotated = refresh_and_save_with_client(
            &ModelRaceFake {
                expected: session.clone(),
            },
            &session,
        )
        .unwrap();
        assert_eq!(rotated.refresh_token, "refresh-2");
        assert_eq!(rotated.model, "mimo-v2.5-pro");
        assert_eq!(AccountSession::load_full().unwrap(), rotated);

        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn logout_during_refresh_revokes_the_new_pair_instead_of_resurrecting_it() {
        struct LogoutRaceFake {
            revoke_calls: Cell<usize>,
            user_id: String,
        }
        impl AccountHttp for LogoutRaceFake {
            fn post_json(
                &self,
                url: &str,
                _access_token: Option<&str>,
                _body: Value,
            ) -> std::result::Result<Value, AccountHttpError> {
                if url.ends_with("/v1/auth/refresh") {
                    take_session().unwrap();
                    return Ok(json!({
                        "access_token":"access-2", "refresh_token":"refresh-2",
                        "user":{"id":self.user_id.clone(),"username":"u"}
                    }));
                }
                if url.ends_with("/v1/auth/logout") {
                    self.revoke_calls.set(self.revoke_calls.get() + 1);
                }
                Ok(json!({"status":"ok"}))
            }
        }

        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "solum-account-logout-race-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
        let session = state_test_session();
        publish_session(&session).unwrap();
        let fake = LogoutRaceFake {
            revoke_calls: Cell::new(0),
            user_id: session.user_id.clone(),
        };
        assert!(refresh_and_save_with_client(&fake, &session).is_err());
        assert!(AccountSession::load().is_none());
        assert_eq!(
            fake.revoke_calls.get(),
            1,
            "newly issued pair must be revoked"
        );

        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
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
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
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
        publish_session(&session).unwrap();
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
        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    struct StateFake {
        calls: RefCell<Vec<String>>,
        omit_default: bool,
    }

    impl AccountHttp for StateFake {
        fn post_json(
            &self,
            url: &str,
            _access_token: Option<&str>,
            _body: Value,
        ) -> std::result::Result<Value, AccountHttpError> {
            self.calls.borrow_mut().push(url.to_string());
            Ok(json!({
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "user": {
                    "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                    "username": "u"
                }
            }))
        }

        fn get_json(
            &self,
            url: &str,
            access_token: &str,
        ) -> std::result::Result<Value, AccountHttpError> {
            self.calls.borrow_mut().push(url.to_string());
            if access_token == "access-1" {
                return Err(AccountHttpError::Unauthorized);
            }
            if url.ends_with("/v1/ai/models") {
                let models = if self.omit_default {
                    json!([{ "id": "mimo-v2.5-pro", "is_default": false }])
                } else {
                    json!([
                        { "id": "mimo-v2.5", "is_default": true },
                        { "id": "mimo-v2.5-pro", "is_default": false }
                    ])
                };
                return Ok(json!({
                    "default_model": "mimo-v2.5",
                    "models": models,
                    "custom_models_enabled": true
                }));
            }
            Ok(json!({
                "plan": "standard",
                "custom_models_enabled": true,
                "monthly_request_limit": 1000,
                "requests_used": 7,
                "requests_remaining": 993,
                "reset_at": "2026-09-01T00:00:00.000Z",
                "success_count": 6,
                "failure_count": 1,
                "fallback_count": 1,
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150,
                "latest_request": null
            }))
        }
    }

    fn state_test_session() -> AccountSession {
        AccountSession {
            server_url: "https://cloud.example.com".into(),
            user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
            username: "u".into(),
            access_token: "access-1".into(),
            refresh_token: "refresh-1".into(),
            model: DEFAULT_CLOUD_MODEL.into(),
        }
    }

    #[test]
    fn cloud_state_refreshes_once_then_reads_catalog_and_usage() {
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("solum-account-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
        let fake = StateFake {
            calls: RefCell::new(Vec::new()),
            omit_default: false,
        };
        let mut session = state_test_session();
        publish_session(&session).unwrap();
        let state = cloud_account_state_with_client(&fake, &mut session).unwrap();
        assert_eq!(session.access_token, "access-2");
        assert_eq!(state.catalog.models.len(), 2);
        assert_eq!(state.usage.requests_remaining, 993);
        let calls = fake.calls.borrow();
        assert!(calls[0].ends_with("/v1/ai/models"));
        assert!(calls[1].ends_with("/v1/auth/refresh"));
        assert!(calls[2].ends_with("/v1/ai/models"));
        assert!(calls[3].ends_with("/v1/ai/usage"));
        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cloud_state_rejects_a_catalog_without_its_default_model() {
        let fake = StateFake {
            calls: RefCell::new(Vec::new()),
            omit_default: true,
        };
        let mut session = state_test_session();
        session.access_token = "access-2".into();
        assert!(cloud_account_state_with_client(&fake, &mut session)
            .unwrap_err()
            .to_string()
            .contains("缺少默认模型"));
    }

    #[test]
    fn legacy_refresh_requires_an_explicit_login() {
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
        let error = refresh_with_client(&client, &legacy).unwrap_err();
        assert!(error.to_string().contains("重新登录"));
    }

    #[test]
    fn refresh_retries_one_transient_response_failure() {
        struct TransientRefresh {
            calls: Cell<usize>,
        }
        impl AccountHttp for TransientRefresh {
            fn post_json(
                &self,
                _url: &str,
                _token: Option<&str>,
                _body: Value,
            ) -> std::result::Result<Value, AccountHttpError> {
                let call = self.calls.get() + 1;
                self.calls.set(call);
                if call == 1 {
                    return Err(AccountHttpError::Retryable("connection reset".into()));
                }
                Ok(json!({
                    "access_token": "access-2",
                    "refresh_token": "refresh-2",
                    "user": {
                        "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                        "username": "u"
                    }
                }))
            }
        }
        let client = TransientRefresh {
            calls: Cell::new(0),
        };
        let rotated = refresh_with_client(&client, &state_test_session()).unwrap();
        assert_eq!(client.calls.get(), 2);
        assert_eq!(rotated.refresh_token, "refresh-2");
    }

    #[test]
    fn refresh_failure_maps_to_relogin_message() {
        let _env_guard = ACCOUNT_CONFIG_TEST_LOCK.lock().unwrap();
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
        let dir = std::env::temp_dir().join(format!(
            "solum-account-refresh-failure-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
        publish_session(&session).unwrap();
        let err =
            AccountReasoner::complete_with_client(&AlwaysUnauthorized, &mut session, "s", "u")
                .unwrap_err();
        assert!(err.to_string().contains("重新登录"), "{err}");
        clear_session().unwrap();
        std::env::remove_var("SOLUM_ACCOUNT_CONFIG");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn generate_pkce_creates_valid_s256_challenge() {
        let pkce = generate_pkce().unwrap();
        assert_eq!(pkce.code_challenge_method, "S256");
        assert_eq!(pkce.code_verifier.len(), 43);
        assert_eq!(pkce.code_challenge.len(), 43);
        assert!(!pkce.code_verifier.contains('='));
        assert!(!pkce.code_challenge.contains('='));

        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let expected_challenge = B64_URL.encode(hasher.finalize());
        assert_eq!(pkce.code_challenge, expected_challenge);
    }

    #[test]
    fn parse_auth_response_handles_oauth_payload() {
        let payload = json!({
            "access_token": "oauth-access-token",
            "refresh_token": "oauth-refresh-token",
            "user": {
                "id": "9d4df1be-9f7b-4a3a-b986-ec920d2df60e",
                "username": "tg_user"
            }
        });
        let session =
            parse_auth_response(&payload, "https://tannerlab.cn", "", "qwen3.5-2b").unwrap();
        assert_eq!(session.server_url, SOLUM_CENTER_URL);
        assert_eq!(session.user_id, "9d4df1be-9f7b-4a3a-b986-ec920d2df60e");
        assert_eq!(session.username, "tg_user");
        assert_eq!(session.access_token, "oauth-access-token");
        assert_eq!(session.refresh_token, "oauth-refresh-token");
    }
}

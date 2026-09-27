//! session. Part of the `account` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `account.rs`.

use super::*;

pub const DEFAULT_CLOUD_MODEL: &str = "gemini-3.8-flash-high";
pub const CLOUD_MODEL_OPTIONS: &[&str] = &["gemini-3.8-flash-high"];

/// Solum Center 当前对外身份（国内直连备案域名）。
pub const SOLUM_CENTER_URL: &str = "https://tannerlab.cn";
/// 已进入废弃流程的旧中心域名；存量会话读到其中任一个时透明改写为新身份
/// （同一后端，token 不绑定 Host），兼容窗口内旧域名仍可用作别名。
pub(super) const LEGACY_CENTER_URLS: &[&str] = &[
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
    // 日/周是真正的放行依据（两窗都过才放行），月字段保留给灰度期。
    // #[serde(default)]：老服务端不认识这些键，反序列化时补 0 而不是整条失败。
    #[serde(default)]
    pub daily_request_limit: u64,
    #[serde(default)]
    pub daily_requests_used: u64,
    #[serde(default)]
    pub daily_requests_remaining: u64,
    #[serde(default)]
    pub daily_reset_at: String,
    #[serde(default)]
    pub weekly_request_limit: u64,
    #[serde(default)]
    pub weekly_requests_used: u64,
    #[serde(default)]
    pub weekly_requests_remaining: u64,
    #[serde(default)]
    pub weekly_reset_at: String,
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
pub(super) const TIMEOUT_SECS: u64 = 120;
pub(super) const REVOKE_TIMEOUT_SECS: u64 = 5;
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

    pub(super) fn save(&self) -> Result<()> {
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

pub(super) fn session_lock_path(session_path: &Path) -> PathBuf {
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
pub(super) fn lock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    set_session_file_lock(file, libc::F_OFD_SETLKW, libc::F_WRLCK as libc::c_short)
}

#[cfg(target_os = "android")]
pub(super) fn unlock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    set_session_file_lock(file, libc::F_OFD_SETLK, libc::F_UNLCK as libc::c_short)
}

#[cfg(not(target_os = "android"))]
pub(super) fn lock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    FileExt::lock_exclusive(file)
}

#[cfg(not(target_os = "android"))]
pub(super) fn unlock_session_file(file: &std::fs::File) -> std::io::Result<()> {
    FileExt::unlock(file)
}

pub(super) fn with_session_lock<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
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
pub(super) fn parse_auth_response(
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

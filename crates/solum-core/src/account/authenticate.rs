//! authenticate. Part of the `account` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `account.rs`.

use super::*;

pub(super) fn map_http_error(error: ureq::Error) -> AccountHttpError {
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

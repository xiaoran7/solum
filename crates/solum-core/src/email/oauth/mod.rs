//! `oauth` -- the OAuth2 authorization-code exchange, the XOAUTH2 authenticator, and the access-token fetch.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::random_urlsafe;
use super::*;

#[derive(Debug, Clone)]
pub(super) struct OAuthEndpoints {
    pub(super) authorize: String,
    pub(super) token: String,
    pub(super) scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStart {
    pub authorization_url: String,
    pub state: String,
    pub code_verifier: String,
}

/// Build a PKCE authorization URL. The shell binds the loopback listener and
/// retains the returned verifier/state in memory only.
pub fn oauth_start(account: &EmailAccount, redirect_uri: &str) -> Result<OAuthStart> {
    let EmailAuth::OAuth2 {
        client_id, tenant, ..
    } = &account.auth
    else {
        return Err(CoreError::Invalid("该账户没有选择 OAuth2".into()));
    };
    if client_id.trim().is_empty() {
        return Err(CoreError::Invalid("请先保存 OAuth client id".into()));
    }
    let endpoints = account
        .provider
        .oauth_endpoints(tenant)
        .ok_or_else(|| CoreError::Invalid("此账户类型不支持 OAuth2".into()))?;
    let state = random_urlsafe(24)?;
    let verifier = random_urlsafe(48)?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut serializer = form_urlencoded::Serializer::new(endpoints.authorize);
    serializer.append_pair("response_type", "code");
    serializer.append_pair("client_id", client_id.trim());
    serializer.append_pair("redirect_uri", redirect_uri);
    serializer.append_pair("scope", &endpoints.scopes.join(" "));
    serializer.append_pair("state", &state);
    serializer.append_pair("code_challenge", &challenge);
    serializer.append_pair("code_challenge_method", "S256");
    serializer.append_pair("access_type", "offline");
    serializer.append_pair("prompt", "consent");
    Ok(OAuthStart {
        authorization_url: serializer.finish(),
        state,
        code_verifier: verifier,
    })
}

#[derive(Debug, Deserialize)]
pub(super) struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
}

/// Finish an Authorization Code + PKCE exchange and persist only the refresh
/// token. The caller must compare `state` before reaching this function.
pub fn oauth_finish(
    config: &mut EmailConfig,
    account_id: &str,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Result<()> {
    let account = config.account(account_id)?.clone();
    let EmailAuth::OAuth2 {
        client_id,
        client_secret,
        tenant,
        refresh_token: old_refresh,
    } = account.auth
    else {
        return Err(CoreError::Invalid("该账户没有选择 OAuth2".into()));
    };
    let endpoints = account
        .provider
        .oauth_endpoints(&tenant)
        .ok_or_else(|| CoreError::Invalid("此账户类型不支持 OAuth2".into()))?;
    let mut form = HashMap::new();
    form.insert("grant_type", "authorization_code");
    form.insert("client_id", client_id.as_str());
    form.insert("code", code.trim());
    form.insert("redirect_uri", redirect_uri);
    form.insert("code_verifier", code_verifier);
    if !client_secret.trim().is_empty() {
        form.insert("client_secret", client_secret.trim());
    }
    let token = post_oauth_form(&endpoints.token, &form)?;
    if token.refresh_token.trim().is_empty() && old_refresh.trim().is_empty() {
        return Err(CoreError::Invalid(
            "授权服务没有返回 refresh token；请确认已授予离线访问权限".into(),
        ));
    }
    let _ = token.access_token; // never persist access tokens
    let target = config.account_mut(account_id)?;
    if let EmailAuth::OAuth2 { refresh_token, .. } = &mut target.auth {
        *refresh_token = if token.refresh_token.trim().is_empty() {
            old_refresh
        } else {
            token.refresh_token
        };
    }
    Ok(())
}

pub(super) fn oauth_access_token(account: &EmailAccount) -> Result<String> {
    let EmailAuth::OAuth2 {
        client_id,
        client_secret,
        refresh_token,
        tenant,
    } = &account.auth
    else {
        return Err(CoreError::Invalid("账户没有 OAuth2 凭据".into()));
    };
    let endpoints = account
        .provider
        .oauth_endpoints(tenant)
        .ok_or_else(|| CoreError::Invalid("此账户类型不支持 OAuth2".into()))?;
    let mut form = HashMap::new();
    form.insert("grant_type", "refresh_token");
    form.insert("client_id", client_id.trim());
    form.insert("refresh_token", refresh_token.trim());
    if !client_secret.trim().is_empty() {
        form.insert("client_secret", client_secret.trim());
    }
    Ok(post_oauth_form(&endpoints.token, &form)?.access_token)
}

pub(super) fn post_oauth_form(url: &str, form: &HashMap<&str, &str>) -> Result<OAuthTokenResponse> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in form {
        serializer.append_pair(key, value);
    }
    ureq::post(url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_string(&serializer.finish())
        .map_err(|e| CoreError::Email(format!("OAuth token 请求失败: {e}")))?
        .into_json()
        .map_err(|e| CoreError::Invalid(format!("OAuth token 响应无效: {e}")))
}

pub(super) struct XOAuth2 {
    pub(super) response: Vec<u8>,
}

impl imap::Authenticator for XOAuth2 {
    type Response = Vec<u8>;

    fn process(&self, _challenge: &[u8]) -> Self::Response {
        self.response.clone()
    }
}

// imap 3.x 把连接收敛成 ClientBuilder + 后端无关的 `Connection`（本仓编译期选定
// rustls-tls，见 workspace Cargo.toml 的注释：native-tls 会拖进 openssl-sys，Android
// 上没有系统 OpenSSL）。会话类型因此不再拿具体的 TlsStream 参数化。

//! flows. Part of the `account` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `account.rs`.

use super::*;

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

/// Captcha challenge data from server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptchaChallenge {
    pub captcha_id: String,
    pub image: String,
}

/// Fetch a new SVG captcha challenge from server
pub fn get_captcha(server_url: &str) -> Result<CaptchaChallenge> {
    let server = normalize_server_url(server_url)?;
    let client = HttpAccountClient::new();
    let response = client
        .get_json(&format!("{server}/v1/auth/captcha"), "")
        .map_err(|e| CoreError::Llm(format!("获取验证码失败: {e}")))?;

    let captcha_id = response
        .get("captcha_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Llm("返回数据缺少 captcha_id".into()))?
        .to_string();
    let image = response
        .get("image")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Llm("返回数据缺少 image".into()))?
        .to_string();

    Ok(CaptchaChallenge { captcha_id, image })
}

/// Send email verification code
pub fn send_email_code(
    server_url: &str,
    email: &str,
    captcha_id: &str,
    captcha_answer: &str,
) -> Result<()> {
    let server = normalize_server_url(server_url)?;
    let email_clean = email.trim().to_lowercase();
    if email_clean.is_empty() {
        return Err(CoreError::Invalid("请输入邮箱地址".into()));
    }
    if !email_clean.contains('@') || !email_clean.contains('.') {
        return Err(CoreError::Invalid("邮箱格式不正确".into()));
    }
    if captcha_id.trim().is_empty() || captcha_answer.trim().is_empty() {
        return Err(CoreError::Invalid("请输入图形验证码".into()));
    }

    let client = HttpAccountClient::new();
    client
        .post_json(
            &format!("{server}/v1/auth/email/send-code"),
            None,
            json!({
                "email": email_clean,
                "captcha_id": captcha_id.trim(),
                "captcha_answer": captcha_answer.trim(),
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Other(msg) if msg.contains("invalid_captcha") => {
                CoreError::Invalid("图形验证码错误或已过期，请点击刷新重试".into())
            }
            AccountHttpError::Other(msg)
                if msg.contains("cooldown_active") || msg.contains("429") =>
            {
                CoreError::Llm("发送太频繁，请稍后再试".into())
            }
            AccountHttpError::Other(msg)
                if msg.contains("email_already_registered") || msg.contains("409") =>
            {
                CoreError::Llm("该邮箱已被注册，请直接登录".into())
            }
            AccountHttpError::Other(msg) if msg.contains("format") => {
                CoreError::Invalid("邮箱格式不正确".into())
            }
            other => CoreError::Llm(format!("发送邮件验证码失败: {other}")),
        })?;

    Ok(())
}

/// Register with email, password and verification code
pub fn register_email(
    server_url: &str,
    email: &str,
    password: &str,
    code: &str,
    model: &str,
) -> Result<AccountSession> {
    let server = normalize_server_url(server_url)?;
    let model = normalize_cloud_model(model)?;
    let email_clean = email.trim().to_lowercase();
    let code_clean = code.trim();
    if email_clean.is_empty() {
        return Err(CoreError::Invalid("请输入邮箱地址".into()));
    }
    if !email_clean.contains('@') || !email_clean.contains('.') {
        return Err(CoreError::Invalid("邮箱格式不正确".into()));
    }
    if code_clean.is_empty() {
        return Err(CoreError::Invalid("请输入邮箱验证码".into()));
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
                "password": password,
                "code": code_clean
            }),
        )
        .map_err(|e| match e {
            AccountHttpError::Other(msg)
                if msg.contains("email_already_registered") || msg.contains("409") =>
            {
                CoreError::Llm("该邮箱已被注册，请直接登录".into())
            }
            AccountHttpError::Other(msg) if msg.contains("invalid_verification_code") => {
                CoreError::Invalid("邮箱验证码错误或已失效".into())
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

pub(super) fn same_account_identity(left: &AccountSession, right: &AccountSession) -> bool {
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
pub(super) fn refresh_with_client<T: AccountHttp>(
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

pub(super) fn refresh_and_save_with_client<T: AccountHttp>(
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
pub(super) fn post_with_refresh<T: AccountHttp>(
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

pub(super) fn cloud_account_state_with_client<T: AccountHttp>(
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

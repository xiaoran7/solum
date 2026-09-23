use super::*;

// ---- Solum account (cloud proxy) — ported from the harmony 0.2.0 client ----

/// Everything the account settings block needs — never the tokens.
#[derive(Serialize)]
pub(crate) struct AccountStatus {
    logged_in: bool,
    username: String,
    server_url: String,
    default_server_url: String,
    model: String,
    default_model: String,
    model_options: Vec<String>,
    usage: Option<solum_core::account::CloudUsage>,
    state_error: Option<String>,
    /// Where the session file lives.
    path: String,
}

pub(crate) async fn revoke_pending_session(session: solum_core::account::AccountSession) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::revoke_remote(&session);
    })
    .await;
}

pub(crate) fn schedule_pending_session_revoke(session: solum_core::account::AccountSession) {
    let _task = tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::revoke_remote(&session);
    });
}

#[tauri::command]
pub(crate) async fn account_status_get() -> CmdResult<AccountStatus> {
    let path = solum_core::account::AccountSession::path()
        .to_string_lossy()
        .into_owned();
    match solum_core::account::AccountSession::load_full() {
        Some(mut s) => tauri::async_runtime::spawn_blocking(move || {
            let remote = solum_core::account::cloud_account_state(&mut s);
            let (default_model, model_options, usage, state_error) = match remote {
                Ok(state) => (
                    state.catalog.default_model,
                    state
                        .catalog
                        .models
                        .into_iter()
                        .filter(|item| state.catalog.custom_models_enabled || item.is_default)
                        .map(|item| item.id)
                        .collect(),
                    Some(state.usage),
                    None,
                ),
                Err(error) => (
                    solum_core::account::DEFAULT_CLOUD_MODEL.to_string(),
                    solum_core::account::CLOUD_MODEL_OPTIONS
                        .iter()
                        .map(|item| (*item).to_string())
                        .collect(),
                    None,
                    Some(error.to_string()),
                ),
            };
            Ok(AccountStatus {
                logged_in: true,
                username: s.username,
                server_url: s.server_url,
                default_server_url: solum_core::account::SOLUM_CENTER_URL.to_string(),
                model: s.model,
                default_model,
                model_options,
                usage,
                state_error,
                path,
            })
        })
        .await
        .map_err(|e| format!("账号状态线程失败: {e}"))?,
        None => Ok(AccountStatus {
            logged_in: false,
            username: String::new(),
            server_url: solum_core::account::SOLUM_CENTER_URL.to_string(),
            default_server_url: solum_core::account::SOLUM_CENTER_URL.to_string(),
            model: solum_core::account::DEFAULT_CLOUD_MODEL.to_string(),
            default_model: solum_core::account::DEFAULT_CLOUD_MODEL.to_string(),
            model_options: solum_core::account::CLOUD_MODEL_OPTIONS
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
            usage: None,
            state_error: None,
            path,
        }),
    }
}

/// Switch the active profile without leaving Android on a dead process.
///
/// Tauri's generic `AppHandle::restart()` re-execs the current binary. That
/// works for desktop executables, but Android links this crate into the app
/// process and has no executable path that can be spawned. The generic path
/// therefore exits successfully after the spawn fails. Android delegates to
/// the Activity, which creates a fresh launch task before terminating us.
#[cfg(target_os = "android")]
pub(crate) fn restart_account_app(app: &tauri::AppHandle) -> CmdResult<String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到 Solum 主窗口，账号已保存，请重新打开应用".to_string())?;
    window
        .with_webview(|webview| {
            webview.jni_handle().exec(|env, activity, _webview| {
                if let Err(error) = env.call_method(activity, "restartApp", "()V", &[]) {
                    eprintln!("failed to restart Android activity after account switch: {error}");
                }
            });
        })
        .map_err(|error| format!("无法重新打开 Solum：{error}"))?;
    Ok("账号已切换，正在重新打开 Solum".to_string())
}

#[cfg(not(target_os = "android"))]
pub(crate) fn restart_account_app(app: &tauri::AppHandle) -> CmdResult<String> {
    app.restart()
}

pub(crate) async fn account_auth(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    username: String,
    password: String,
    model: String,
    register: bool,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let session = tauri::async_runtime::spawn_blocking(move || {
        let session = if register {
            solum_core::account::register(&server_url, &username, &password, &model)
        } else {
            solum_core::account::authenticate(&server_url, &username, &password, &model)
        }
        .map_err(|e| e.to_string())?;
        let prepared = (|| {
            let user_id = session
                .stable_user_id()
                .ok_or_else(|| "账号服务器未返回不可变 user_id".to_string())?;
            let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
                .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;
            // Prepare the target account before it becomes globally active.
            // The first device creates a random master key; later devices
            // recover it from the opaque server envelope with this password.
            solum_core::sync::provision_account_sync(&session, &password, &sync_path)
                .map_err(|error| error.to_string())
        })();
        if let Err(error) = prepared {
            // Authentication already issued a refresh token. Do not leave that
            // untracked session alive when the local profile cannot be prepared.
            solum_core::account::revoke_remote(&session);
            let message = if register {
                format!(
                    "账号已创建，但本机加密空间初始化未完成。请切换到登录并用同一账号密码重试：{error}"
                )
            } else {
                format!("身份验证成功，但本机加密空间初始化失败，请重新登录重试：{error}")
            };
            return Err(message);
        }
        Ok::<_, String>(session)
    })
    .await
    .map_err(|e| format!("登录线程失败: {e}"))??;
    // Do not publish the new token until the old profile's sync store is
    // exclusively held. A ticker already in flight either finishes under the
    // old identity or blocks here; it can never pair this new token with the
    // old SQLite database.
    let _sync_switch_guard = match state.sync_store.lock() {
        Ok(guard) => guard,
        Err(error) => {
            let message = error.to_string();
            schedule_pending_session_revoke(session);
            return Err(message);
        }
    };
    if let Err(error) = solum_core::account::publish_session(&session) {
        drop(_sync_switch_guard);
        schedule_pending_session_revoke(session);
        return Err(error.to_string());
    }
    // The authenticated account owns a different SQLite/WebView/config
    // partition. A process restart is the smallest atomic boundary: every
    // connection, cache, ticker and in-memory chat context closes before the
    // new profile opens. Hot-swapping only the reasoner would leave the old
    // user's database live under the new token.
    restart_account_app(&app)
}

/// Log in against a self-hosted solum-cloud (`server/`), prepare the target
/// profile, publish it behind the old profile's sync barrier, then restart.
#[tauri::command]
pub(crate) async fn account_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    username: String,
    password: String,
    model: String,
) -> CmdResult<String> {
    account_auth(state, app, server_url, username, password, model, false).await
}

/// Register once in the central account database, then enter the exact same
/// profile-switch and recovery flow as a normal login.
#[tauri::command]
pub(crate) async fn account_register(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    username: String,
    password: String,
    model: String,
) -> CmdResult<String> {
    account_auth(state, app, server_url, username, password, model, true).await
}

#[tauri::command]
pub(crate) async fn account_oauth_config_get(
    server_url: Option<String>,
) -> CmdResult<solum_core::account::OAuthConfig> {
    let server = server_url.unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::fetch_oauth_config(&server).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("获取 OAuth 配置任务失败: {e}"))?
}

#[tauri::command]
pub(crate) async fn open_external_url(
    window: tauri::WebviewWindow,
    url: String,
) -> CmdResult<bool> {
    #[cfg(target_os = "android")]
    {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let url_clone = url.clone();
        window
            .with_webview(move |webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let result = (|| -> Result<bool, String> {
                        let jurl = env.new_string(&url_clone).map_err(|e| e.to_string())?;
                        let url_obj = jni::objects::JObject::from(jurl);
                        let args = [jni::objects::JValue::Object(&url_obj)];
                        let value = env
                            .call_method(
                                activity,
                                "openExternalUrl",
                                "(Ljava/lang/String;)Z",
                                &args,
                            )
                            .map_err(|e| e.to_string())?;
                        value.z().map_err(|e| e.to_string())
                    })();
                    let _ = sender.send(result);
                });
            })
            .map_err(|e| e.to_string())?;
        tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(3))
                .map_err(|_| "拉起系统浏览器超时".to_string())?
        })
        .await
        .map_err(|e| format!("拉起系统浏览器失败: {e}"))?
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", &url])
                .spawn()
                .map(|_| true)
                .map_err(|e| e.to_string())
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(&url)
                .spawn()
                .map(|_| true)
                .map_err(|e| e.to_string())
        }
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open")
                .arg(&url)
                .spawn()
                .map(|_| true)
                .map_err(|e| e.to_string())
        }
    }
}

#[tauri::command]
pub(crate) fn account_oauth_pkce_generate() -> CmdResult<solum_core::account::PkcePair> {
    solum_core::account::generate_pkce().map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub(crate) struct AccountOAuthLoginArgs {
    server_url: String,
    code: String,
    code_verifier: String,
    redirect_uri: String,
    model: String,
    recovery_code: Option<String>,
}

#[tauri::command]
pub(crate) async fn account_oauth_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountOAuthLoginArgs,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;

    let (session, issued_recovery_code) = tauri::async_runtime::spawn_blocking(move || {
        let mut issued_recovery_code: Option<String> = None;
        let session = solum_core::account::authenticate_via_oauth(
            &args.server_url,
            &args.code,
            &args.code_verifier,
            &args.redirect_uri,
            &args.model,
        )
        .map_err(|e| e.to_string())?;

        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "账号服务器未返回不可变 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;

        // 1. Check if local sync config already exists for this user_id
        if let Some(existing_config) = solum_core::sync::load_account_sync_config(&sync_path) {
            let master_key = existing_config.key_bytes().map_err(|e| e.to_string())?;
            if let Err(error) =
                solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
            {
                solum_core::account::revoke_remote(&session);
                return Err(format!("本机加密空间初始化失败: {error}"));
            }
            return Ok::<_, String>((session, issued_recovery_code));
        }

        // 2. Local config does not exist. Check if server has recovery code envelope.
        let transport = solum_core::recovery_code::HttpRecoveryCodeTransport::new();
        let envelope = transport
            .get_envelope(&session)
            .map_err(|e| e.to_string())?;

        let master_key = if let Some(envelope) = envelope {
            let norm_code = args
                .recovery_code
                .as_deref()
                .map(solum_core::recovery_code::normalize_code)
                .unwrap_or_default();
            if norm_code.is_empty() {
                solum_core::account::revoke_remote(&session);
                return Err(
                    "NEEDS_RECOVERY_CODE:此账号已有云端加密数据，请输入您的 20 字符恢复码以解密"
                        .to_string(),
                );
            }
            solum_core::recovery_code::unwrap_master_key(user_id, &norm_code, &envelope).map_err(
                |e| {
                    solum_core::account::revoke_remote(&session);
                    format!("恢复码解密失败: {e}")
                },
            )?
        } else {
            let mut key_bytes = [0u8; 32];
            getrandom::getrandom(&mut key_bytes).map_err(|e| format!("随机数错误: {e}"))?;
            let issuance = solum_core::recovery_code::issue_recovery_code(user_id, &key_bytes)
                .map_err(|e| e.to_string())?;
            transport
                .put_envelope(&session, &issuance.envelope, &issuance.verify_hash)
                .map_err(|e| format!("保存初始恢复信封失败: {e}"))?;
            // The code is shown once; login restarts the app before the
            // webview could render it, so park it for the next startup.
            issued_recovery_code = Some(issuance.display_code);
            key_bytes
        };

        solum_core::sync::save_account_sync_config_public(
            &sync_path,
            &session.server_url,
            &master_key,
        )
        .map_err(|e| e.to_string())?;

        if let Err(error) =
            solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
        {
            solum_core::account::revoke_remote(&session);
            return Err(format!("加密空间准备失败: {error}"));
        }

        Ok::<_, String>((session, issued_recovery_code))
    })
    .await
    .map_err(|e| format!("OAuth 登录后台任务失败: {e}"))??;

    let _sync_switch_guard = match state.sync_store.lock() {
        Ok(guard) => guard,
        Err(error) => {
            let message = error.to_string();
            schedule_pending_session_revoke(session);
            return Err(message);
        }
    };
    if let Err(error) = solum_core::account::publish_session(&session) {
        drop(_sync_switch_guard);
        schedule_pending_session_revoke(session);
        return Err(error.to_string());
    }
    if let Some(display_code) = issued_recovery_code {
        let pending_path = session.stable_user_id().and_then(|user_id| {
            solum_core::paths::account_profile_file(user_id, "pending-recovery-code.txt")
        });
        if let Some(pending_path) = pending_path {
            if let Err(error) =
                solum_core::recovery_code::save_pending_display(&pending_path, &display_code)
            {
                eprintln!("failed to park recovery code for next-startup display: {error}");
            }
        }
    }
    restart_account_app(&app)
}

#[derive(Deserialize)]
pub(crate) struct AccountHuaweiLoginArgs {
    server_url: Option<String>,
    code: String,
    redirect_uri: Option<String>,
    model: Option<String>,
    recovery_code: Option<String>,
}

#[tauri::command]
pub(crate) async fn account_sms_send(
    server_url: Option<String>,
    phone: String,
) -> CmdResult<String> {
    let server = server_url.unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::request_sms_code(&server, &phone).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("发送验证码线程失败: {e}"))??;
    Ok("验证码已发送".into())
}

#[derive(Serialize)]
pub(crate) struct AccountCaptchaGetResult {
    captcha_id: String,
    image: String,
}

#[tauri::command]
pub(crate) async fn account_captcha_get(
    server_url: Option<String>,
) -> CmdResult<AccountCaptchaGetResult> {
    let server = server_url.unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    let challenge = tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::get_captcha(&server).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("获取图形验证码失败: {e}"))??;

    Ok(AccountCaptchaGetResult {
        captcha_id: challenge.captcha_id,
        image: challenge.image,
    })
}

#[derive(Deserialize)]
pub(crate) struct AccountEmailSendCodeArgs {
    server_url: Option<String>,
    email: String,
    captcha_id: String,
    captcha_answer: String,
}

#[tauri::command]
pub(crate) async fn account_email_send_code(args: AccountEmailSendCodeArgs) -> CmdResult<String> {
    let server = args
        .server_url
        .unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::send_email_code(
            &server,
            &args.email,
            &args.captcha_id,
            &args.captcha_answer,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("发送邮件验证码线程失败: {e}"))??;

    Ok("验证码已发送至您的邮箱".into())
}

#[derive(Deserialize)]
pub(crate) struct AccountEmailRegisterArgs {
    server_url: Option<String>,
    email: String,
    password: String,
    code: String,
    model: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct AccountEmailLoginArgs {
    server_url: Option<String>,
    email: String,
    password: String,
    model: Option<String>,
}

#[tauri::command]
pub(crate) async fn account_email_register(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountEmailRegisterArgs,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let server = args
        .server_url
        .unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    let chosen_model = args
        .model
        .unwrap_or_else(|| "gemini-3.8-flash-high".to_string());

    let session = tauri::async_runtime::spawn_blocking(move || {
        let session = solum_core::account::register_email(
            &server,
            &args.email,
            &args.password,
            &args.code,
            &chosen_model,
        )
        .map_err(|e| e.to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "缺少 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;
        solum_core::sync::provision_account_sync(&session, &args.password, &sync_path)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(session)
    })
    .await
    .map_err(|e| format!("注册线程失败: {e}"))??;

    let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
    solum_core::account::publish_session(&session).map_err(|e| e.to_string())?;
    restart_account_app(&app)
}

#[tauri::command]
pub(crate) async fn account_email_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountEmailLoginArgs,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let server = args
        .server_url
        .unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    let chosen_model = args
        .model
        .unwrap_or_else(|| "gemini-3.8-flash-high".to_string());

    let session = tauri::async_runtime::spawn_blocking(move || {
        let session =
            solum_core::account::login_email(&server, &args.email, &args.password, &chosen_model)
                .map_err(|e| e.to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "缺少 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;
        solum_core::sync::provision_account_sync(&session, &args.password, &sync_path)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(session)
    })
    .await
    .map_err(|e| format!("登录后台任务失败: {e}"))??;

    let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
    solum_core::account::publish_session(&session).map_err(|e| e.to_string())?;
    restart_account_app(&app)
}

#[derive(Deserialize)]
pub(crate) struct AccountPhoneRegisterArgs {
    server_url: Option<String>,
    phone: String,
    code: String,
    password: String,
    model: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct AccountPhoneLoginArgs {
    server_url: Option<String>,
    phone: String,
    code: Option<String>,
    password: Option<String>,
    model: Option<String>,
    recovery_code: Option<String>,
}

#[tauri::command]
pub(crate) async fn account_phone_register(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountPhoneRegisterArgs,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let server = args
        .server_url
        .unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    let chosen_model = args
        .model
        .unwrap_or_else(|| "gemini-3.8-flash-high".to_string());

    let session = tauri::async_runtime::spawn_blocking(move || {
        let session = solum_core::account::register_phone(
            &server,
            &args.phone,
            &args.code,
            &args.password,
            &chosen_model,
        )
        .map_err(|e| e.to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "缺少 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;
        solum_core::sync::provision_account_sync(&session, &args.password, &sync_path)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(session)
    })
    .await
    .map_err(|e| format!("注册线程失败: {e}"))??;

    let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
    solum_core::account::publish_session(&session).map_err(|e| e.to_string())?;
    restart_account_app(&app)
}

#[tauri::command]
pub(crate) async fn account_phone_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountPhoneLoginArgs,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let server = args
        .server_url
        .unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    let chosen_model = args
        .model
        .unwrap_or_else(|| "gemini-3.8-flash-high".to_string());

    let (session, issued_recovery_code) = tauri::async_runtime::spawn_blocking(move || {
        let mut issued_recovery_code: Option<String> = None;
        if let Some(pass) = args.password {
            let session = solum_core::account::login_phone_password(
                &server,
                &args.phone,
                &pass,
                &chosen_model,
            )
            .map_err(|e| e.to_string())?;
            let user_id = session
                .stable_user_id()
                .ok_or_else(|| "缺少 user_id".to_string())?;
            let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
                .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;
            solum_core::sync::provision_account_sync(&session, &pass, &sync_path)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((session, None))
        } else if let Some(sms_code) = args.code {
            let session = solum_core::account::login_phone_code(
                &server,
                &args.phone,
                &sms_code,
                &chosen_model,
            )
            .map_err(|e| e.to_string())?;
            let user_id = session
                .stable_user_id()
                .ok_or_else(|| "缺少 user_id".to_string())?;
            let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
                .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;

            if let Some(existing_config) = solum_core::sync::load_account_sync_config(&sync_path) {
                let master_key = existing_config.key_bytes().map_err(|e| e.to_string())?;
                solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
                    .map_err(|e| format!("本机加密空间初始化失败: {e}"))?;
                return Ok::<_, String>((session, None));
            }

            let transport = solum_core::recovery_code::HttpRecoveryCodeTransport::new();
            let envelope = transport
                .get_envelope(&session)
                .map_err(|e| e.to_string())?;
            let master_key = if let Some(envelope) = envelope {
                let norm_code = args
                    .recovery_code
                    .as_deref()
                    .map(solum_core::recovery_code::normalize_code)
                    .unwrap_or_default();
                if norm_code.is_empty() {
                    solum_core::account::revoke_remote(&session);
                    return Err(
                        "NEEDS_RECOVERY_CODE:此账号已有加密数据，请输入您的 20 字符恢复码以解密"
                            .to_string(),
                    );
                }
                solum_core::recovery_code::unwrap_master_key(user_id, &norm_code, &envelope)
                    .map_err(|e| {
                        solum_core::account::revoke_remote(&session);
                        format!("恢复码解密失败: {e}")
                    })?
            } else {
                let mut key_bytes = [0u8; 32];
                getrandom::getrandom(&mut key_bytes).map_err(|e| format!("随机数错误: {e}"))?;
                let issuance = solum_core::recovery_code::issue_recovery_code(user_id, &key_bytes)
                    .map_err(|e| e.to_string())?;
                transport
                    .put_envelope(&session, &issuance.envelope, &issuance.verify_hash)
                    .map_err(|e| format!("保存初始恢复信封失败: {e}"))?;
                issued_recovery_code = Some(issuance.display_code);
                key_bytes
            };

            solum_core::sync::save_account_sync_config_public(
                &sync_path,
                &session.server_url,
                &master_key,
            )
            .map_err(|e| e.to_string())?;
            solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
                .map_err(|e| format!("加密空间准备失败: {e}"))?;
            Ok::<_, String>((session, issued_recovery_code))
        } else {
            Err("请提供密码或短信验证码".to_string())
        }
    })
    .await
    .map_err(|e| format!("登录后台任务失败: {e}"))??;

    let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
    solum_core::account::publish_session(&session).map_err(|e| e.to_string())?;
    if let Some(display_code) = issued_recovery_code {
        if let Some(user_id) = session.stable_user_id() {
            if let Some(pending_path) =
                solum_core::paths::account_profile_file(user_id, "pending-recovery-code.txt")
            {
                let _ =
                    solum_core::recovery_code::save_pending_display(&pending_path, &display_code);
            }
        }
    }
    restart_account_app(&app)
}

#[tauri::command]
pub(crate) async fn account_huawei_config_get(
    server_url: Option<String>,
) -> CmdResult<solum_core::account::HuaweiOAuthConfig> {
    let server = server_url.unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::fetch_huawei_config(&server).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("获取华为配置任务失败: {e}"))?
}

#[tauri::command]
pub(crate) async fn account_huawei_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountHuaweiLoginArgs,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let server = args
        .server_url
        .unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    let redirect = args
        .redirect_uri
        .unwrap_or_else(|| "solum://oauth/callback".to_string());
    let chosen_model = args
        .model
        .unwrap_or_else(|| "gemini-3.8-flash-high".to_string());

    let (session, issued_recovery_code) = tauri::async_runtime::spawn_blocking(move || {
        let mut issued_recovery_code: Option<String> = None;
        let session = solum_core::account::authenticate_via_huawei(
            &server,
            &args.code,
            &redirect,
            &chosen_model,
        )
        .map_err(|e| e.to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "账号缺少 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;

        if let Some(existing_config) = solum_core::sync::load_account_sync_config(&sync_path) {
            let master_key = existing_config.key_bytes().map_err(|e| e.to_string())?;
            solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
                .map_err(|e| format!("本机加密空间初始化失败: {e}"))?;
            return Ok::<_, String>((session, None));
        }

        let transport = solum_core::recovery_code::HttpRecoveryCodeTransport::new();
        let envelope = transport
            .get_envelope(&session)
            .map_err(|e| e.to_string())?;
        let master_key = if let Some(envelope) = envelope {
            let norm_code = args
                .recovery_code
                .as_deref()
                .map(solum_core::recovery_code::normalize_code)
                .unwrap_or_default();
            if norm_code.is_empty() {
                solum_core::account::revoke_remote(&session);
                return Err(
                    "NEEDS_RECOVERY_CODE:此账号已有加密数据，请输入您的 20 字符恢复码以解密"
                        .to_string(),
                );
            }
            solum_core::recovery_code::unwrap_master_key(user_id, &norm_code, &envelope).map_err(
                |e| {
                    solum_core::account::revoke_remote(&session);
                    format!("恢复码解密失败: {e}")
                },
            )?
        } else {
            let mut key_bytes = [0u8; 32];
            getrandom::getrandom(&mut key_bytes).map_err(|e| format!("随机数错误: {e}"))?;
            let issuance = solum_core::recovery_code::issue_recovery_code(user_id, &key_bytes)
                .map_err(|e| e.to_string())?;
            transport
                .put_envelope(&session, &issuance.envelope, &issuance.verify_hash)
                .map_err(|e| format!("保存初始恢复信封失败: {e}"))?;
            issued_recovery_code = Some(issuance.display_code);
            key_bytes
        };

        solum_core::sync::save_account_sync_config_public(
            &sync_path,
            &session.server_url,
            &master_key,
        )
        .map_err(|e| e.to_string())?;
        solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
            .map_err(|e| format!("加密空间准备失败: {e}"))?;
        Ok::<_, String>((session, issued_recovery_code))
    })
    .await
    .map_err(|e| format!("华为登录任务失败: {e}"))??;

    let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
    solum_core::account::publish_session(&session).map_err(|e| e.to_string())?;
    if let Some(display_code) = issued_recovery_code {
        if let Some(user_id) = session.stable_user_id() {
            if let Some(pending_path) =
                solum_core::paths::account_profile_file(user_id, "pending-recovery-code.txt")
            {
                let _ =
                    solum_core::recovery_code::save_pending_display(&pending_path, &display_code);
            }
        }
    }
    restart_account_app(&app)
}

#[tauri::command]
pub(crate) async fn account_delete(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let session = {
        let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
        solum_core::account::take_session().map_err(|e| e.to_string())?
    };
    if let Some(session) = session {
        let server = session.server_url.clone();
        let token = session.access_token.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = solum_core::account::delete_account(&server, &token);
        })
        .await
        .map_err(|e| format!("注销任务失败: {e}"))?;
    }
    restart_account_app(&app)
}

/// Local sign-out first-class: best-effort server-side revocation, then restart
/// into the offline guest profile. Legacy direct-key files never activate it.
#[tauri::command]
pub(crate) async fn account_logout(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    // Keep the selected profile stable until no sync round can still hold its
    // database, then atomically take the latest rotated token and clear local
    // identity. Remote revocation is bounded and happens only after local
    // sign-out can no longer be undone by an old GUI or CLI request.
    let session = {
        let _sync_switch_guard = state.sync_store.lock().map_err(|e| e.to_string())?;
        solum_core::account::take_session().map_err(|e| e.to_string())?
    };
    if let Some(session) = session {
        revoke_pending_session(session).await;
    }
    restart_account_app(&app)
}

/// Whether a recovery code exists server-side for the signed-in account.
#[tauri::command]
pub(crate) async fn recovery_code_status() -> CmdResult<serde_json::Value> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(session) = solum_core::account::AccountSession::load_full() else {
            return Err("请先登录账号".to_string());
        };
        let transport = solum_core::recovery_code::HttpRecoveryCodeTransport::new();
        let configured = transport
            .get_envelope(&session)
            .map_err(|e| e.to_string())?
            .is_some();
        Ok(serde_json::json!({ "configured": configured }))
    })
    .await
    .map_err(|e| format!("恢复码状态线程失败: {e}"))?
}

/// Generate (or rotate) the recovery code for the signed-in account. The
/// display code is returned exactly once — it is never stored anywhere and
/// never sent to the server.
#[tauri::command]
pub(crate) async fn recovery_code_issue() -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let session = solum_core::account::AccountSession::load_full()
            .ok_or_else(|| "请先登录账号".to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "账号缺少不可变 user_id".to_string())?;
        // Wrap exactly the master key this device uses, so a restore on any
        // device with the code can decrypt the account's backup history.
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法定位本机同步密钥配置".to_string())?;
        let config = solum_core::sync::load_account_sync_config(&sync_path)
            .ok_or_else(|| "本机还没有同步密钥；请先登录以初始化加密空间".to_string())?;
        let master_key = config.key_bytes().map_err(|e| e.to_string())?;

        let issuance = solum_core::recovery_code::issue_recovery_code(user_id, &master_key)
            .map_err(|e| e.to_string())?;
        let transport = solum_core::recovery_code::HttpRecoveryCodeTransport::new();
        transport
            .put_envelope(&session, &issuance.envelope, &issuance.verify_hash)
            .map_err(|e| e.to_string())?;
        // The panel shows the code right now; parking it additionally keeps a
        // one-time next-startup reminder in case the app closes before the
        // user has actually written it down. Re-issuing also invalidates any
        // older parked code by overwriting it.
        if let Some(pending_path) =
            solum_core::paths::account_profile_file(user_id, "pending-recovery-code.txt")
        {
            if let Err(error) = solum_core::recovery_code::save_pending_display(
                &pending_path,
                &issuance.display_code,
            ) {
                eprintln!("failed to park recovery code for next-startup display: {error}");
            }
        }
        Ok(issuance.display_code)
    })
    .await
    .map_err(|e| format!("恢复码生成线程失败: {e}"))?
}

/// Peek the recovery code parked by the last account login or re-issue, if
/// the user has not acknowledged saving it yet. Read-only on purpose: closing
/// the reminder without saving must keep the code parked for next startup.
#[tauri::command]
pub(crate) async fn recovery_code_pending() -> CmdResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(session) = solum_core::account::AccountSession::load_full() else {
            return Ok(None);
        };
        let Some(user_id) = session.stable_user_id() else {
            return Ok(None);
        };
        let Some(path) =
            solum_core::paths::account_profile_file(user_id, "pending-recovery-code.txt")
        else {
            return Ok(None);
        };
        solum_core::recovery_code::load_pending_display(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("恢复码待展示读取线程失败: {e}"))?
}

/// Acknowledge the parked recovery code as saved; deletes the local reminder.
#[tauri::command]
pub(crate) async fn recovery_code_pending_ack() -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(session) = solum_core::account::AccountSession::load_full() else {
            return Ok(());
        };
        let Some(user_id) = session.stable_user_id() else {
            return Ok(());
        };
        let Some(path) =
            solum_core::paths::account_profile_file(user_id, "pending-recovery-code.txt")
        else {
            return Ok(());
        };
        solum_core::recovery_code::clear_pending_display(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("恢复码确认线程失败: {e}"))?
}

/// Sign in with username + recovery code when the password is forgotten.
/// Runs the full profile-preparation flow: fetch the envelope with an
/// authenticated session, unwrap the master key locally, and persist it so
/// the restored key matches what future backups will use.
#[tauri::command]
pub(crate) async fn recovery_code_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    username: String,
    code: String,
) -> CmdResult<String> {
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let normalized_code = solum_core::recovery_code::normalize_code(&code);
    if normalized_code.is_empty() {
        return Err("请输入恢复码".into());
    }
    let session = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::recovery_code::HttpRecoveryCodeTransport::new();
        // 1. Challenge for the PBKDF2 salt; derive + submit only its digest.
        let salt = transport
            .challenge(&server_url, &username)
            .map_err(|e| e.to_string())?;
        let verify_hash = solum_core::recovery_code::compute_verify_hash(&salt, &normalized_code)
            .map_err(|e| e.to_string())?;
        let mut session = transport
            .recover_session(&server_url, &username, &verify_hash)
            .map_err(|e| e.to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "账号服务器未返回不可变 user_id".to_string())?
            .to_string();
        // 2. Fetch the envelope under the new session and unwrap the real
        // master key bound to the immutable user id.
        let envelope = transport
            .get_envelope(&session)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "该账号还没有设置恢复码，无法用恢复码登录".to_string())?;
        let master_key =
            solum_core::recovery_code::unwrap_master_key(&user_id, &normalized_code, &envelope)
                .map_err(|e| e.to_string())?;
        session.model = solum_core::account::DEFAULT_CLOUD_MODEL.to_string();
        // 3. Prepare the target profile with the recovered key before it goes
        // live — same barrier semantics as password login.
        let sync_path = solum_core::paths::account_profile_file(&user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;
        solum_core::sync::save_account_sync_config_public(
            &sync_path,
            &session.server_url,
            &master_key,
        )
        .map_err(|e| e.to_string())?;
        if let Err(error) =
            solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
                .map_err(|error| error.to_string())
        {
            solum_core::account::revoke_remote(&session);
            return Err(format!(
                "身份恢复成功，但本机加密空间初始化失败，请重新尝试：{error}"
            ));
        }
        Ok::<_, String>(session)
    })
    .await
    .map_err(|e| format!("恢复码登录线程失败: {e}"))??;
    let _sync_switch_guard = match state.sync_store.lock() {
        Ok(guard) => guard,
        Err(error) => {
            let message = error.to_string();
            schedule_pending_session_revoke(session);
            return Err(message);
        }
    };
    if let Err(error) = solum_core::account::publish_session(&session) {
        drop(_sync_switch_guard);
        schedule_pending_session_revoke(session);
        return Err(error.to_string());
    }
    restart_account_app(&app)
}

/// Change the account password while signed in. The client rewraps the
/// password envelope under the new password first; the server swaps the
/// credential hash and that envelope in one transaction, so "new password
/// live, envelope still old" cannot happen. Other devices must sign in again.
#[tauri::command]
pub(crate) async fn account_change_password(
    current_password: String,
    new_password: String,
) -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(move || {
        if new_password.chars().count() < 8 {
            return Err("新密码至少需要 8 个字符".to_string());
        }
        if current_password.is_empty() || current_password == new_password {
            return Err("新密码不能与当前密码相同".into());
        }
        let session = solum_core::account::AccountSession::load_full()
            .ok_or_else(|| "请先登录账号".to_string())?;
        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "账号缺少不可变 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法定位本机同步密钥配置".to_string())?;
        let config = solum_core::sync::load_account_sync_config(&sync_path)
            .ok_or_else(|| "本机还没有同步密钥；请先登录以初始化加密空间".to_string())?;
        let master_key = config.key_bytes().map_err(|e| e.to_string())?;
        solum_core::sync::change_account_password(
            &session,
            &current_password,
            &new_password,
            &master_key,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("改密线程失败: {e}"))?
}

/// Change which model the proxy asks upstream for (login stays untouched).
#[tauri::command]
pub(crate) async fn account_model_save(
    state: State<'_, AppState>,
    model: String,
) -> CmdResult<String> {
    let Some(mut session) = solum_core::account::AccountSession::load_full() else {
        return Err("请先登录账号".into());
    };
    let requested =
        solum_core::account::normalize_cloud_model(&model).map_err(|e| e.to_string())?;
    let session = tauri::async_runtime::spawn_blocking(move || {
        let remote = solum_core::account::cloud_account_state(&mut session);
        match remote {
            Ok(account)
                if account.catalog.models.iter().any(|item| {
                    item.id == requested
                        && (account.catalog.custom_models_enabled || item.is_default)
                }) => {}
            Ok(_) => return Err("这个模型不在 Solum 中心当前允许的目录中".to_string()),
            Err(_) if requested == solum_core::account::DEFAULT_CLOUD_MODEL => {}
            Err(error) => {
                return Err(format!(
                    "暂时无法向中心确认这个模型：{error}。默认模型仍可继续使用"
                ));
            }
        }
        solum_core::account::update_active_model(&session, &requested).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("模型设置线程失败: {e}"))??;
    let summary = format!("账号 · {}", session.masked_summary());
    lock!(state).set_reasoner(Box::new(solum_core::account::AccountReasoner::new(session)));
    if let Ok(mut g) = state.llm_summary.lock() {
        *g = Some(summary.clone());
    }
    Ok(summary)
}

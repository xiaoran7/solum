//! `login` -- signing in and registering against a self-hosted solum-cloud, plus the OAuth authorization-code path and the external-browser hand-off.
//!
//! Part of the `account` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

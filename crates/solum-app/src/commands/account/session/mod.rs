//! `session` -- the session itself: the status the settings block renders, scheduling the revoke of a session that has not been adopted yet, and the restart that swaps one live profile for another.
//!
//! Part of the `account` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

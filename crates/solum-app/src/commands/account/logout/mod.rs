//! `logout` -- signing out, deleting the account on the server, and the recovery code: status, issue, the once-only hand-off that survives the restart, and login with a recovery code.
//!
//! Part of the `account` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::session::revoke_pending_session;
use super::*;

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

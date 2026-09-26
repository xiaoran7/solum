//! `huawei` -- the Huawei account path, and deleting the account on the server.
//!
//! Part of the `account` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::sms_email::AccountHuaweiLoginArgs;
use super::*;

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

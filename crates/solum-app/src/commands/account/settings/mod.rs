//! `settings` -- the two account settings that need a live session: changing the password, and choosing which model the proxy asks upstream for.
//!
//! Part of the `account` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

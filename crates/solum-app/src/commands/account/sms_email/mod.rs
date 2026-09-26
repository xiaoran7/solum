//! `sms_email` -- the gates that stand between a request and an account: SMS codes, the captcha, and email-code registration and sign-in.
//!
//! Part of the `account` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

#[derive(Deserialize)]
pub(crate) struct AccountHuaweiLoginArgs {
    pub(super) server_url: Option<String>,
    pub(super) code: String,
    pub(super) redirect_uri: Option<String>,
    pub(super) model: Option<String>,
    pub(super) recovery_code: Option<String>,
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

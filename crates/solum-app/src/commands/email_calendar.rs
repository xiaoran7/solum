use super::*;

// ---- Soulous read-only source settings (Phase 8.1) -------------------------

#[derive(Serialize)]
pub(crate) struct SoulousSettings {
    configured: bool,
    path: String,
    server_url: String,
    access_token_tail: String,
    refresh_token_tail: String,
    timeout_secs: u64,
    push_schedule_events: bool,
    cache: solum_core::soulous::SoulousStatus,
}

#[tauri::command]
pub(crate) fn soulous_config_get(
    state: State<AppState>,
    now: Option<String>,
) -> CmdResult<SoulousSettings> {
    require_full_account()?;
    let now = parse_now(now)?;
    let path = soulous_config_file().to_string_lossy().into_owned();
    let cache = lock!(state).soulous_status(now).map_err(core_err)?;
    match solum_core::soulous::SoulousConfig::load() {
        Some(config) => Ok(SoulousSettings {
            configured: true,
            path,
            server_url: config.server_url,
            access_token_tail: key_tail(&config.access_token),
            refresh_token_tail: key_tail(&config.refresh_token),
            timeout_secs: config.timeout_secs,
            push_schedule_events: config.push_schedule_events,
            cache,
        }),
        None => Ok(SoulousSettings {
            configured: false,
            path,
            server_url: String::new(),
            access_token_tail: String::new(),
            refresh_token_tail: String::new(),
            timeout_secs: 15,
            push_schedule_events: false,
            cache,
        }),
    }
}

#[derive(Deserialize)]
pub(crate) struct SoulousSaveArgs {
    server_url: String,
    /// Leaving either field blank keeps its stored value. First save requires
    /// both values; the UI never receives the complete saved tokens back.
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    timeout_secs: u64,
    /// L2 outbound whitelist, default-off even for an older local config.
    #[serde(default)]
    push_schedule_events: bool,
}

pub(crate) fn resolve_soulous_args(
    args: SoulousSaveArgs,
) -> CmdResult<solum_core::soulous::SoulousConfig> {
    let existing = std::fs::read_to_string(soulous_config_file())
        .ok()
        .and_then(|raw| solum_core::soulous::SoulousConfig::from_json(&raw).ok());
    let mut config = solum_core::soulous::SoulousConfig {
        server_url: args.server_url,
        access_token: if args.access_token.trim().is_empty() {
            existing
                .as_ref()
                .map(|c| c.access_token.clone())
                .ok_or_else(|| "请填写 access token（没有已保存的值可沿用）".to_string())?
        } else {
            args.access_token
        },
        refresh_token: if args.refresh_token.trim().is_empty() {
            existing
                .as_ref()
                .map(|c| c.refresh_token.clone())
                .ok_or_else(|| "请填写 refresh token（没有已保存的值可沿用）".to_string())?
        } else {
            args.refresh_token
        },
        timeout_secs: args.timeout_secs,
        push_schedule_events: args.push_schedule_events,
    };
    config.normalize().map_err(core_err)?;
    Ok(config)
}

#[tauri::command]
pub(crate) fn soulous_config_save(args: SoulousSaveArgs) -> CmdResult<String> {
    require_full_account()?;
    let config = resolve_soulous_args(args)?;
    config.save_to(&soulous_config_file()).map_err(core_err)?;
    Ok(config.masked_summary())
}

#[derive(Serialize)]
pub(crate) struct SoulousPullResp {
    /// False only when solum-soulous.json is absent/incomplete; that is a quiet
    /// offline state rather than an application error.
    configured: bool,
    outcome: Option<solum_core::soulous::PullOutcome>,
    cache: solum_core::soulous::SoulousStatus,
}

/// Real HTTP path, so it must never run on Tauri's main thread. A failure is
/// reported only to this manual action; it cannot delay ingest or reminders.
#[tauri::command]
pub(crate) async fn soulous_pull(
    state: State<'_, AppState>,
    now: Option<String>,
) -> CmdResult<SoulousPullResp> {
    require_full_account()?;
    let now = parse_now(now)?;
    let orch = state.orch.clone();
    let transition = state.account_transition.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
        let outcome = o.pull_soulous(now).map_err(core_err)?;
        let cache = o.soulous_status(now).map_err(core_err)?;
        Ok(SoulousPullResp {
            configured: outcome.is_some(),
            outcome,
            cache,
        })
    })
    .await
    .map_err(|e| format!("Soulous 拉取后台任务失败: {e}"))?
}

// ---- email connector settings + user-initiated mailbox operations (F21) ---

/// Same local-only convention as the LLM/Soulous configuration. The mobile
/// setup below pins this to app-data, where a WebView cwd has no meaning.
pub(crate) fn email_config_file() -> std::path::PathBuf {
    EmailConfig::path()
}

pub(crate) fn read_email_config() -> CmdResult<EmailConfig> {
    let path = email_config_file();
    match std::fs::read_to_string(&path) {
        Ok(raw) => EmailConfig::from_json(&raw).map_err(core_err),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EmailConfig::default()),
        Err(error) => Err(format!("读取邮箱配置 {} 失败: {error}", path.display())),
    }
}

pub(crate) fn email_account_from_config(account_id: &str) -> CmdResult<EmailAccount> {
    require_full_account()?;
    read_email_config()?
        .account(account_id)
        .cloned()
        .map_err(core_err)
}

#[derive(Serialize)]
pub(crate) struct EmailSettings {
    configured: bool,
    path: String,
    accounts: Vec<solum_core::email::EmailAccountSummary>,
}

#[tauri::command]
pub(crate) fn email_config_get() -> CmdResult<EmailSettings> {
    require_full_account()?;
    let config = read_email_config()?;
    Ok(EmailSettings {
        configured: !config.accounts.is_empty(),
        path: email_config_file().to_string_lossy().into_owned(),
        accounts: config.summaries(),
    })
}

#[derive(Deserialize)]
pub(crate) struct EmailAccountSaveArgs {
    id: String,
    label: String,
    provider: EmailProvider,
    address: String,
    auth_kind: String,
    /// Empty means preserve the previously saved value for this account.
    #[serde(default)]
    app_password: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    tenant: String,
    #[serde(default)]
    imap_host: String,
    imap_port: Option<u16>,
    #[serde(default)]
    smtp_host: String,
    smtp_port: Option<u16>,
    smtp_tls: Option<SmtpTls>,
}

pub(crate) fn resolve_email_account(
    args: EmailAccountSaveArgs,
    config: &EmailConfig,
) -> CmdResult<EmailAccount> {
    let prior = config.accounts.iter().find(|account| account.id == args.id);
    let preset = args.provider.preset();
    let fallback = |value: String, preset_value: Option<String>| {
        if value.trim().is_empty() {
            preset_value.unwrap_or_default()
        } else {
            value.trim().to_string()
        }
    };
    let endpoints = EmailEndpoints {
        imap_host: fallback(
            args.imap_host,
            preset.as_ref().map(|item| item.imap_host.clone()),
        ),
        imap_port: args
            .imap_port
            .or_else(|| preset.as_ref().map(|item| item.imap_port))
            .unwrap_or(0),
        smtp_host: fallback(
            args.smtp_host,
            preset.as_ref().map(|item| item.smtp_host.clone()),
        ),
        smtp_port: args
            .smtp_port
            .or_else(|| preset.as_ref().map(|item| item.smtp_port))
            .unwrap_or(0),
        smtp_tls: args
            .smtp_tls
            .or_else(|| preset.as_ref().map(|item| item.smtp_tls.clone()))
            .unwrap_or(SmtpTls::Wrapper),
    };
    let auth = match args.auth_kind.as_str() {
        "app_password" => {
            let secret = if args.app_password.trim().is_empty() {
                prior.and_then(|account| match &account.auth {
                    EmailAuth::AppPassword { secret } => Some(secret.clone()),
                    EmailAuth::OAuth2 { .. } => None,
                })
            } else {
                Some(args.app_password.trim().to_string())
            }
            .ok_or_else(|| "首次保存授权码账户时需要填写邮箱授权码 / 应用专用密码".to_string())?;
            EmailAuth::AppPassword { secret }
        }
        "oauth2" => {
            if !matches!(
                args.provider,
                EmailProvider::Gmail | EmailProvider::Microsoft
            ) {
                return Err("OAuth2 目前仅可用于 Gmail 或 Microsoft 365 / Outlook".into());
            }
            let old = prior.and_then(|account| match &account.auth {
                EmailAuth::OAuth2 {
                    client_id,
                    client_secret,
                    refresh_token,
                    tenant,
                } => Some((client_id, client_secret, refresh_token, tenant)),
                EmailAuth::AppPassword { .. } => None,
            });
            let client_id = if args.client_id.trim().is_empty() {
                old.as_ref().map(|item| item.0.clone()).unwrap_or_default()
            } else {
                args.client_id.trim().to_string()
            };
            if client_id.is_empty() {
                return Err("OAuth2 账户需要 client id".into());
            }
            let client_secret = if args.client_secret.trim().is_empty() {
                old.as_ref().map(|item| item.1.clone()).unwrap_or_default()
            } else {
                args.client_secret.trim().to_string()
            };
            let tenant = if args.tenant.trim().is_empty() {
                old.as_ref().map(|item| item.3.clone()).unwrap_or_default()
            } else {
                args.tenant.trim().to_string()
            };
            EmailAuth::OAuth2 {
                client_id,
                client_secret,
                refresh_token: old.map(|item| item.2.clone()).unwrap_or_default(),
                tenant,
            }
        }
        _ => return Err("认证方式必须是 app_password 或 oauth2".into()),
    };
    let account = EmailAccount {
        id: args.id.trim().to_string(),
        label: args.label.trim().to_string(),
        provider: args.provider,
        address: args.address.trim().to_string(),
        endpoints,
        auth,
    };
    account.validate(false).map_err(core_err)?;
    Ok(account)
}

#[tauri::command]
pub(crate) fn email_config_save(args: EmailAccountSaveArgs) -> CmdResult<EmailSettings> {
    require_full_account()?;
    let mut config = read_email_config()?;
    let account = resolve_email_account(args, &config)?;
    if let Some(slot) = config
        .accounts
        .iter_mut()
        .find(|item| item.id == account.id)
    {
        *slot = account;
    } else {
        config.accounts.push(account);
    }
    config.save_to(&email_config_file()).map_err(core_err)?;
    Ok(EmailSettings {
        configured: true,
        path: email_config_file().to_string_lossy().into_owned(),
        accounts: config.summaries(),
    })
}

#[tauri::command]
pub(crate) fn email_config_remove(account_id: String) -> CmdResult<EmailSettings> {
    require_full_account()?;
    let mut config = read_email_config()?;
    let before = config.accounts.len();
    config.accounts.retain(|account| account.id != account_id);
    if before == config.accounts.len() {
        return Err("找不到要移除的邮箱账户".into());
    }
    config.save_to(&email_config_file()).map_err(core_err)?;
    Ok(EmailSettings {
        configured: !config.accounts.is_empty(),
        path: email_config_file().to_string_lossy().into_owned(),
        accounts: config.summaries(),
    })
}

#[tauri::command]
pub(crate) async fn email_folders(
    account_id: String,
) -> CmdResult<Vec<solum_core::email::EmailFolder>> {
    let account = email_account_from_config(&account_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::email::list_folders(&account).map_err(core_err)
    })
    .await
    .map_err(|e| format!("邮箱文件夹后台任务失败: {e}"))?
}

#[tauri::command]
pub(crate) async fn email_messages(
    account_id: String,
    mailbox: String,
    limit: Option<usize>,
) -> CmdResult<Vec<solum_core::email::EmailSummary>> {
    let account = email_account_from_config(&account_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::email::list_messages(&account, &mailbox, limit.unwrap_or(30)).map_err(core_err)
    })
    .await
    .map_err(|e| format!("邮箱列表后台任务失败: {e}"))?
}

/// Scan only recent INBOX envelope metadata and enqueue reviewable high-value events.
/// Message bodies and attachments are deliberately never fetched by this path.
#[tauri::command]
pub(crate) async fn email_high_value_scan(
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Vec<CaptureDraftView>> {
    let account = email_account_from_config(&account_id)?;
    let id_for_scan = account_id.clone();
    let summaries = tauri::async_runtime::spawn_blocking(move || {
        solum_core::email::list_messages(&account, "INBOX", 100).map_err(core_err)
    })
    .await
    .map_err(|e| format!("邮箱事件扫描后台任务失败: {e}"))??;
    let candidates =
        solum_core::email_events::high_value_candidates(&id_for_scan, "INBOX", &summaries);
    let now = Local::now().timestamp_millis();
    let mut inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    Ok(candidates
        .into_iter()
        .map(|candidate| {
            draft_view(inbox.push_with_external_id(
                solum_core::capture::CaptureSource::Email,
                &candidate.title,
                &candidate.text,
                Some(&candidate.external_id),
                now,
            ))
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn email_search(
    account_id: String,
    mailbox: String,
    query: String,
    limit: Option<usize>,
) -> CmdResult<Vec<solum_core::email::EmailSummary>> {
    let account = email_account_from_config(&account_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::email::search_messages(&account, &mailbox, &query, limit.unwrap_or(30))
            .map_err(core_err)
    })
    .await
    .map_err(|e| format!("邮箱搜索后台任务失败: {e}"))?
}

#[tauri::command]
pub(crate) async fn email_message(
    account_id: String,
    mailbox: String,
    uid: u32,
) -> CmdResult<solum_core::email::EmailMessage> {
    let account = email_account_from_config(&account_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::email::get_message(&account, &mailbox, uid).map_err(core_err)
    })
    .await
    .map_err(|e| format!("邮件正文后台任务失败: {e}"))?
}

pub(crate) fn start_email_oauth_callback(
) -> CmdResult<(String, mpsc::Receiver<CmdResult<EmailOAuthCallback>>)> {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| format!("无法启动本机 OAuth 回调端口: {e}"))?;
    let address = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| "本机 OAuth 回调没有获得 IP 地址".to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", address.port());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // Keep listening until a request that is actually the callback arrives
        // or the window closes. Consuming whichever request showed up first
        // meant a browser prefetch, a favicon fetch, or any local probe could
        // end the authorization session before the user finished signing in —
        // and the user would just see "缺少 code 或 state" with no idea why.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let result = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break Err("邮箱授权等待已超时，请重新开始".to_string());
            }
            match server.recv_timeout(remaining) {
                Ok(Some(request)) => {
                    let is_get = matches!(request.method(), tiny_http::Method::Get);
                    let parsed = url::Url::parse(&format!("http://127.0.0.1{}", request.url()));
                    let is_callback = parsed
                        .as_ref()
                        .map(|url| url.path() == "/callback")
                        .unwrap_or(false);
                    if !is_get || !is_callback {
                        let _ = request.respond(
                            tiny_http::Response::from_string("not found").with_status_code(404),
                        );
                        continue;
                    }
                    let pairs = parsed
                        .ok()
                        .map(|url| url.query_pairs().into_owned().collect::<HashMap<_, _>>())
                        .unwrap_or_default();
                    let callback = match (pairs.get("code"), pairs.get("state"), pairs.get("error"))
                    {
                        (Some(code), Some(state), None) => Ok(EmailOAuthCallback {
                            code: code.clone(),
                            state: state.clone(),
                        }),
                        (_, _, Some(error)) => Err(format!("邮箱授权被拒绝或失败: {error}")),
                        _ => Err("邮箱授权回调缺少 code 或 state".to_string()),
                    };
                    let body = if callback.is_ok() {
                        "<meta charset=\"utf-8\"><h2>邮箱授权已收到</h2><p>请回到息壤完成连接。</p>"
                    } else {
                        "<meta charset=\"utf-8\"><h2>邮箱授权未完成</h2><p>请回到息壤查看错误后重试。</p>"
                    };
                    let _ = request.respond(tiny_http::Response::from_string(body));
                    break callback;
                }
                Ok(None) => break Err("邮箱授权等待已超时，请重新开始".to_string()),
                Err(error) => break Err(format!("邮箱授权回调失败: {error}")),
            }
        };
        let _ = sender.send(result);
    });
    Ok((redirect_uri, receiver))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmailOAuthBegin {
    session_id: String,
    authorization_url: String,
    redirect_uri: String,
}

#[tauri::command]
pub(crate) fn email_oauth_begin(
    state: State<AppState>,
    account_id: String,
) -> CmdResult<EmailOAuthBegin> {
    require_full_account()?;
    let account = email_account_from_config(&account_id)?;
    let (redirect_uri, receiver) = start_email_oauth_callback()?;
    let start = solum_core::email::oauth_start(&account, &redirect_uri).map_err(core_err)?;
    let session_id = format!("mail-{}", start.state);
    let mut sessions = state
        .email_oauth
        .lock()
        .map_err(|_| "邮箱授权状态锁异常".to_string())?;
    sessions.retain(|_, session| session.account_id != account_id);
    sessions.insert(
        session_id.clone(),
        EmailOAuthSession {
            account_id,
            state: start.state,
            code_verifier: start.code_verifier,
            redirect_uri: redirect_uri.clone(),
            receiver,
        },
    );
    Ok(EmailOAuthBegin {
        session_id,
        authorization_url: start.authorization_url,
        redirect_uri,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct EmailOAuthPoll {
    status: String,
}

#[tauri::command]
pub(crate) async fn email_oauth_poll(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<EmailOAuthPoll> {
    require_full_account()?;
    let session = state
        .email_oauth
        .lock()
        .map_err(|_| "邮箱授权状态锁异常".to_string())?
        .remove(&session_id)
        .ok_or_else(|| "邮箱授权会话不存在或已结束".to_string())?;
    match session.receiver.try_recv() {
        Err(mpsc::TryRecvError::Empty) => {
            state
                .email_oauth
                .lock()
                .map_err(|_| "邮箱授权状态锁异常".to_string())?
                .insert(session_id, session);
            Ok(EmailOAuthPoll {
                status: "pending".into(),
            })
        }
        Err(mpsc::TryRecvError::Disconnected) => Err("邮箱授权回调已中断，请重新开始".into()),
        Ok(callback) => {
            let callback = callback?;
            if callback.state != session.state {
                return Err("邮箱授权 state 不匹配，已拒绝保存令牌".into());
            }
            tauri::async_runtime::spawn_blocking(move || {
                let mut config = read_email_config()?;
                solum_core::email::oauth_finish(
                    &mut config,
                    &session.account_id,
                    &session.redirect_uri,
                    &callback.code,
                    &session.code_verifier,
                )
                .map_err(core_err)?;
                config.save_to(&email_config_file()).map_err(core_err)?;
                Ok(EmailOAuthPoll {
                    status: "complete".into(),
                })
            })
            .await
            .map_err(|e| format!("邮箱授权收尾后台任务失败: {e}"))?
        }
    }
}

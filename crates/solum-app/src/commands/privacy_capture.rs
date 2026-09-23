use super::*;
use chrono::TimeZone;
#[cfg(mobile)]
use tauri_plugin_notification::NotificationExt;

// ---- 首启隐私门 + 应用内隐私政策（自 Solum Harmony 回移）----------------------

#[derive(Serialize)]
pub(crate) struct PrivacyConsentStatus {
    needs_consent: bool,
    accepted_at: Option<String>,
    /// 同意记录落在哪——用户要能自己找到、自己删。
    path: String,
    document: solum_core::privacy::PolicyDocument,
}

/// 启动时问一次：这台设备对**当前版本**的政策同意过没有。
///
/// 只有 GUI 壳层过这道门；`solum-cli` 不受影响（见 `solum_core::privacy` 模块文档）。
#[tauri::command]
pub(crate) fn privacy_consent_status() -> CmdResult<PrivacyConsentStatus> {
    let existing = solum_core::privacy::PrivacyConsent::load();
    Ok(PrivacyConsentStatus {
        needs_consent: !solum_core::privacy::has_current_consent(
            existing.as_ref().map(|c| c.version),
        ),
        accepted_at: existing.map(|c| c.accepted_at),
        path: solum_core::privacy::PrivacyConsent::path()
            .to_string_lossy()
            .into_owned(),
        document: solum_core::privacy::policy_document(),
    })
}

/// 记下同意。写盘失败必须报错**而不是**放行——放行等于产生一条没有记录的同意。
#[tauri::command]
pub(crate) fn privacy_consent_accept() -> CmdResult<String> {
    let c = solum_core::privacy::PrivacyConsent::accept(Local::now()).map_err(|e| e.to_string())?;
    Ok(c.accepted_at)
}

/// Refusing the first-run policy must terminate the native app, including on
/// Android where closing a WebView window is not a reliable process-exit path.
#[cfg(not(mobile))]
#[tauri::command]
pub(crate) fn privacy_consent_decline(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg(mobile)]
#[tauri::command]
pub(crate) fn privacy_consent_decline(_app: tauri::AppHandle) {
    // On Android a WebView window is not the application task. Exit the native
    // process after the user explicitly declines so the button always has the
    // promised outcome.
    std::process::exit(0);
}

/// Request OS notification delivery only after the user has accepted the
/// in-app privacy policy. Notification-listener access remains separate.
#[tauri::command]
pub(crate) fn notification_permission_request(app: tauri::AppHandle) -> CmdResult<bool> {
    #[cfg(mobile)]
    {
        use tauri_plugin_notification::PermissionState;
        if !matches!(
            app.notification().permission_state(),
            Ok(PermissionState::Granted)
        ) {
            app.notification()
                .request_permission()
                .map_err(|e| e.to_string())?;
        }
        app.notification()
            .permission_state()
            .map(|state| matches!(state, PermissionState::Granted))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(mobile))]
    {
        let _ = app;
        Ok(true)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReminderDeliveryHealth {
    platform_supported: bool,
    receiver_registered: bool,
    boot_receiver_registered: bool,
    exact: bool,
    notifications_enabled: bool,
    debug_process_death_available: bool,
}

/// Surface the startup-time Android receiver tripwire in Settings. Desktop
/// returns `platform_supported = false` and keeps the section hidden.
#[tauri::command]
pub(crate) fn reminder_delivery_health(app: tauri::AppHandle) -> CmdResult<ReminderDeliveryHealth> {
    let health = app.alarm().health().map_err(|error| error.to_string())?;
    Ok(ReminderDeliveryHealth {
        platform_supported: health.platform_supported,
        receiver_registered: health.receiver_registered,
        boot_receiver_registered: health.boot_receiver_registered,
        exact: health.exact,
        notifications_enabled: health.notifications_enabled,
        debug_process_death_available: cfg!(all(target_os = "android", debug_assertions)),
    })
}

/// Fault injection for the exact scenario `force-stop` cannot test: the app
/// process dies while AlarmManager retains an already armed PendingIntent.
#[tauri::command]
pub(crate) async fn debug_simulate_process_death(window: tauri::WebviewWindow) -> CmdResult<bool> {
    if !cfg!(debug_assertions) {
        return Err("进程回收模拟只在 Debug 构建中可用".into());
    }
    #[cfg(target_os = "android")]
    {
        android_activity_bool(window, "debugScheduleProcessDeath").await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Err("进程回收模拟仅在 Android Debug 构建中可用".into())
    }
}

// ---- 多入口采集（capture 领域层，自 Solum Harmony 回移）------------------------

#[derive(Serialize)]
pub(crate) struct CaptureDraftView {
    #[serde(flatten)]
    draft: solum_core::capture::CaptureDraft,
    source_label: &'static str,
    clues: solum_core::capture::CaptureClues,
    clue_summary: String,
}

pub(crate) fn draft_view(draft: solum_core::capture::CaptureDraft) -> CaptureDraftView {
    let clues = solum_core::capture::extract_capture_clues(&draft.text);
    CaptureDraftView {
        source_label: solum_core::capture::capture_source_label(draft.source),
        clue_summary: clues.summary(),
        clues,
        draft,
    }
}

#[derive(Serialize)]
pub(crate) struct CaptureConnectorView {
    #[serde(flatten)]
    connector: solum_core::capture::CaptureConnector,
    authorized: Option<bool>,
    pending_count: usize,
    accepted_count: i64,
    last_success: Option<String>,
    last_error: Option<String>,
}

/// 这台设备上的采集入口清单及其真实状态。运行态错误只投影错误类别，
/// 不把来源正文带进设置页。
#[tauri::command]
pub(crate) fn capture_entry_points(
    app: tauri::AppHandle,
    state: State<AppState>,
) -> CmdResult<Vec<CaptureConnectorView>> {
    use solum_core::capture::CaptureSource;

    let connectors = solum_core::capture::capture_connectors(cfg!(target_os = "android"));
    let pending = {
        let mut inbox = state
            .capture_inbox
            .lock()
            .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
        inbox.prune_expired(Local::now().timestamp_millis());
        inbox.snapshot()
    };
    let store = state
        .sync_store
        .lock()
        .map_err(|_| "信息源状态异常（锁中毒）".to_string())?;
    let mut out = Vec::with_capacity(connectors.len());
    for connector in connectors {
        let (accepted_count, last_success) = store
            .raw_input_source_activity(connector.source.as_str())
            .map_err(core_err)?;
        let pending_count = pending
            .iter()
            .filter(|draft| {
                draft.source == connector.source || draft.also_seen_from.contains(&connector.source)
            })
            .count();
        let (authorized, last_error) = match connector.source {
            CaptureSource::Manual => (Some(true), None),
            CaptureSource::Share => (Some(cfg!(target_os = "android")), None),
            CaptureSource::Notification => match app.notif_access().is_enabled() {
                Ok(value) => (Some(value), None),
                Err(_) => (Some(false), Some("notification_status_unavailable".into())),
            },
            CaptureSource::Health => {
                let hc = app.health_connect();
                match hc.is_available() {
                    Ok(false) => (Some(false), None),
                    Ok(true) => match hc.has_permissions() {
                        Ok(value) => (Some(value), None),
                        Err(_) => (Some(false), Some("health_permission_unavailable".into())),
                    },
                    Err(_) => (Some(false), Some("health_status_unavailable".into())),
                }
            }
            CaptureSource::Email => match read_email_config() {
                Ok(config) => (Some(!config.accounts.is_empty()), None),
                Err(_) => (Some(false), Some("email_config_unavailable".into())),
            },
            CaptureSource::Soulous => (
                Some(solum_core::soulous::SoulousConfig::load().is_some()),
                None,
            ),
            // Calendar permission is checked by its Android bridge below the
            // registry; location is deliberately not requested in F22.
            CaptureSource::Calendar | CaptureSource::Location => (None, None),
        };
        out.push(CaptureConnectorView {
            connector,
            authorized,
            pending_count,
            accepted_count,
            last_success,
            last_error,
        });
    }
    Ok(out)
}

#[tauri::command]
pub(crate) fn capture_inbox_list(state: State<AppState>) -> CmdResult<Vec<CaptureDraftView>> {
    let mut inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    inbox.prune_expired(Local::now().timestamp_millis());
    Ok(inbox.snapshot().into_iter().map(draft_view).collect())
}

/// 收下一条外部输入。**只进待确认区，不写数据库**——落库要用户在界面上确认后
/// 走既有的 `ingest` 管道，这条边界是本模块存在的理由。
#[tauri::command]
pub(crate) fn capture_inbox_add(
    state: State<AppState>,
    source: String,
    title: String,
    text: String,
    external_id: Option<String>,
) -> CmdResult<CaptureDraftView> {
    if text.trim().is_empty() {
        return Err("采集内容为空".into());
    }
    let source = solum_core::capture::CaptureSource::parse(&source)
        .ok_or_else(|| format!("未知采集入口：{source}"))?;
    let mut inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    Ok(draft_view(inbox.push_with_external_id(
        source,
        &title,
        &text,
        external_id.as_deref(),
        Local::now().timestamp_millis(),
    )))
}

/// 丢弃一条待确认草稿。误点分享目标时这一步必须是干净的：内存里删掉即可，
/// 本来就没有磁盘副本。
#[tauri::command]
pub(crate) fn capture_inbox_discard(state: State<AppState>, id: String) -> CmdResult<bool> {
    let mut inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    Ok(inbox.remove(&id))
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
pub(crate) struct AndroidShareItem {
    source: String,
    title: String,
    text: String,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
pub(crate) struct AndroidShareDrain {
    items: Vec<AndroidShareItem>,
    notices: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct AndroidShareDrainView {
    drafts: Vec<CaptureDraftView>,
    notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidOAuthCallbackItem {
    pub code: String,
    pub state: String,
    pub error: String,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
pub(crate) struct AndroidOAuthDrain {
    #[serde(default)]
    items: Vec<AndroidOAuthCallbackItem>,
}

#[tauri::command]
pub(crate) async fn account_oauth_android_drain(
    #[allow(unused_variables)] window: tauri::WebviewWindow,
) -> CmdResult<Vec<AndroidOAuthCallbackItem>> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(Vec::new())
    }
    #[cfg(target_os = "android")]
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        window
            .with_webview(move |webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let result = (|| -> Result<String, String> {
                        let value = env
                            .call_method(
                                activity,
                                "drainOAuthInboxJson",
                                "()Ljava/lang/String;",
                                &[],
                            )
                            .map_err(|error| error.to_string())?;
                        let object = value.l().map_err(|error| error.to_string())?;
                        let java_string = jni::objects::JString::from(object);
                        env.get_string(&java_string)
                            .map(|value| value.into())
                            .map_err(|error| error.to_string())
                    })();
                    let _ = sender.send(result);
                });
            })
            .map_err(|error| error.to_string())?;
        let raw = tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| "读取 OAuth 回调超时".to_string())?
        })
        .await
        .map_err(|error| format!("读取 OAuth 回调失败：{error}"))??;
        let drained: AndroidOAuthDrain =
            serde_json::from_str(&raw).map_err(|error| format!("OAuth 回调格式无效：{error}"))?;
        Ok(drained.items)
    }
}

/// Drain Android's bounded, RAM-only ACTION_SEND queue into the ordinary
/// candidate inbox. This is invoked on launch/focus rather than by a battery-
/// consuming poll loop. Desktop returns an empty list.
#[tauri::command]
pub(crate) async fn capture_android_share_drain(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<AndroidShareDrainView> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        let _ = state;
        Ok(AndroidShareDrainView {
            drafts: Vec::new(),
            notices: Vec::new(),
        })
    }
    #[cfg(target_os = "android")]
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        window
            .with_webview(move |webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let result = (|| -> Result<String, String> {
                        let value = env
                            .call_method(
                                activity,
                                "drainShareInboxJson",
                                "()Ljava/lang/String;",
                                &[],
                            )
                            .map_err(|error| error.to_string())?;
                        let object = value.l().map_err(|error| error.to_string())?;
                        let java_string = jni::objects::JString::from(object);
                        env.get_string(&java_string)
                            .map(|value| value.into())
                            .map_err(|error| error.to_string())
                    })();
                    let _ = sender.send(result);
                });
            })
            .map_err(|error| error.to_string())?;
        let raw = tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| "读取系统分享超时，请重新打开息壤".to_string())?
        })
        .await
        .map_err(|error| format!("读取系统分享失败：{error}"))??;
        let drained: AndroidShareDrain =
            serde_json::from_str(&raw).map_err(|error| format!("系统分享内容格式无效：{error}"))?;
        let mut inbox = state
            .capture_inbox
            .lock()
            .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
        let drafts = drained
            .items
            .into_iter()
            .filter(|item| !item.text.trim().is_empty())
            .map(|item| {
                let source = solum_core::capture::CaptureSource::parse(&item.source)
                    .unwrap_or(solum_core::capture::CaptureSource::Share);
                draft_view(inbox.push(
                    source,
                    &item.title,
                    &item.text,
                    Local::now().timestamp_millis(),
                ))
            })
            .collect();
        Ok(AndroidShareDrainView {
            drafts,
            notices: drained.notices,
        })
    }
}

#[tauri::command]
pub(crate) async fn capture_android_voice_start(window: tauri::WebviewWindow) -> CmdResult<bool> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(false)
    }
    #[cfg(target_os = "android")]
    android_activity_bool(window, "startVoiceCapture").await
}

#[tauri::command]
pub(crate) async fn capture_android_ocr_pick(window: tauri::WebviewWindow) -> CmdResult<bool> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(false)
    }
    #[cfg(target_os = "android")]
    android_activity_bool(window, "startOcrPicker").await
}

#[cfg(target_os = "android")]
pub(crate) async fn android_activity_bool(
    window: tauri::WebviewWindow,
    method: &'static str,
) -> CmdResult<bool> {
    let (sender, receiver) = mpsc::sync_channel(1);
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                let result = env
                    .call_method(activity, method, "()Z", &[])
                    .and_then(|value| value.z())
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            });
        })
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "等待 Android 系统界面超时".to_string())?
    })
    .await
    .map_err(|error| format!("Android 系统调用失败：{error}"))?
}

#[tauri::command]
pub(crate) async fn calendar_access_status(window: tauri::WebviewWindow) -> CmdResult<bool> {
    #[cfg(target_os = "android")]
    {
        android_activity_bool(window, "hasCalendarReadPermission").await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(false)
    }
}

#[tauri::command]
pub(crate) async fn calendar_access_request(window: tauri::WebviewWindow) -> CmdResult<bool> {
    #[cfg(target_os = "android")]
    {
        android_activity_bool(window, "requestCalendarReadPermission").await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Err("系统日历读取仅在 Android 可用".into())
    }
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
pub(crate) struct AndroidCalendarItem {
    external_id: String,
    title: String,
    begin_ms: i64,
    end_ms: i64,
    location: String,
    calendar: String,
    all_day: bool,
}

/// Read the next bounded window into the in-memory candidate queue. Reading
/// never creates a Solum event; adoption still takes the ordinary two-step
/// confirmation path.
#[tauri::command]
pub(crate) async fn calendar_candidates_import(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    days_ahead: Option<i64>,
) -> CmdResult<Vec<CaptureDraftView>> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        let _ = state;
        let _ = days_ahead;
        Err("系统日历读取仅在 Android 可用".into())
    }
    #[cfg(target_os = "android")]
    {
        let days = days_ahead.unwrap_or(30).clamp(1, 90);
        let start_ms = Local::now().timestamp_millis();
        let end_ms = start_ms + days * 24 * 60 * 60 * 1000;
        let (sender, receiver) = mpsc::sync_channel(1);
        window
            .with_webview(move |webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let args = [
                        jni::objects::JValue::Long(start_ms),
                        jni::objects::JValue::Long(end_ms),
                        jni::objects::JValue::Int(100),
                    ];
                    let result = (|| -> Result<String, String> {
                        let value = env
                            .call_method(
                                activity,
                                "calendarCandidatesJson",
                                "(JJI)Ljava/lang/String;",
                                &args,
                            )
                            .map_err(|error| error.to_string())?;
                        let object = value.l().map_err(|error| error.to_string())?;
                        let java_string = jni::objects::JString::from(object);
                        env.get_string(&java_string)
                            .map(|value| value.into())
                            .map_err(|error| error.to_string())
                    })();
                    let _ = sender.send(result);
                });
            })
            .map_err(|error| error.to_string())?;
        let raw = tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(3))
                .map_err(|_| "读取系统日历超时".to_string())?
        })
        .await
        .map_err(|error| format!("读取系统日历失败：{error}"))??;
        let items: Vec<AndroidCalendarItem> =
            serde_json::from_str(&raw).map_err(|error| format!("系统日历内容格式无效：{error}"))?;
        let mut inbox = state
            .capture_inbox
            .lock()
            .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
        let now_ms = Local::now().timestamp_millis();
        Ok(items
            .into_iter()
            .map(|item| {
                let time = if item.all_day {
                    Utc.timestamp_millis_opt(item.begin_ms)
                        .single()
                        .map(|value| format!("{}（全天）", value.format("%Y-%m-%d")))
                        .unwrap_or_else(|| "日期未知（全天）".into())
                } else {
                    let start = Local
                        .timestamp_millis_opt(item.begin_ms)
                        .single()
                        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "时间未知".into());
                    let end = Local
                        .timestamp_millis_opt(item.end_ms)
                        .single()
                        .map(|value| value.format("%H:%M").to_string())
                        .unwrap_or_default();
                    if end.is_empty() {
                        start
                    } else {
                        format!("{start} - {end}")
                    }
                };
                let location = if item.location.trim().is_empty() {
                    String::new()
                } else {
                    format!("\n地点：{}", item.location.trim())
                };
                let source_calendar = if item.calendar.trim().is_empty() {
                    String::new()
                } else {
                    format!("\n日历：{}", item.calendar.trim())
                };
                let text = format!(
                    "{}\n时间：{}{}{}",
                    item.title.trim(),
                    time,
                    location,
                    source_calendar
                );
                draft_view(inbox.push_candidate(
                    solum_core::capture::CaptureSource::Calendar,
                    &item.title,
                    &text,
                    Some(&item.external_id),
                    item.begin_ms,
                    now_ms,
                ))
            })
            .collect())
    }
}

/// Hand one existing Solum event to Android's own calendar editor. The native
/// provider is never written directly and WRITE_CALENDAR is never requested.
#[tauri::command]
pub(crate) async fn calendar_open_insert(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    event_id: i64,
) -> CmdResult<bool> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        let _ = state;
        let _ = event_id;
        Err("系统日历写入仅在 Android 可用".into())
    }
    #[cfg(target_os = "android")]
    {
        let event = lock!(state).event(event_id).map_err(core_err)?;
        let start_ms = Local
            .from_local_datetime(&event.start)
            .earliest()
            .ok_or_else(|| "日程开始时间无法映射到本地时区".to_string())?
            .timestamp_millis();
        let end_ms = event
            .end
            .and_then(|value| Local.from_local_datetime(&value).earliest())
            .map(|value| value.timestamp_millis())
            .unwrap_or(0);
        let title = event.title;
        let location = event.location.unwrap_or_default();
        let description = "由 Solum 提议；保存前请在此系统界面核对。".to_string();
        let (sender, receiver) = mpsc::sync_channel(1);
        window
            .with_webview(move |webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let result = (|| -> Result<bool, String> {
                        let jtitle = env.new_string(title).map_err(|error| error.to_string())?;
                        let jlocation = env
                            .new_string(location)
                            .map_err(|error| error.to_string())?;
                        let jdescription = env
                            .new_string(description)
                            .map_err(|error| error.to_string())?;
                        let title_obj = jni::objects::JObject::from(jtitle);
                        let location_obj = jni::objects::JObject::from(jlocation);
                        let description_obj = jni::objects::JObject::from(jdescription);
                        let args = [
                            jni::objects::JValue::Object(&title_obj),
                            jni::objects::JValue::Long(start_ms),
                            jni::objects::JValue::Long(end_ms),
                            jni::objects::JValue::Object(&location_obj),
                            jni::objects::JValue::Object(&description_obj),
                        ];
                        env.call_method(
                            activity,
                            "openCalendarInsert",
                            "(Ljava/lang/String;JJLjava/lang/String;Ljava/lang/String;)Z",
                            &args,
                        )
                        .and_then(|value| value.z())
                        .map_err(|error| error.to_string())
                    })();
                    let _ = sender.send(result);
                });
            })
            .map_err(|error| error.to_string())?;
        tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| "打开系统日历超时".to_string())?
        })
        .await
        .map_err(|error| format!("打开系统日历失败：{error}"))?
    }
}

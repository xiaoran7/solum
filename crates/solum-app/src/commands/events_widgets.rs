use super::*;

// ---- the closed loop ---------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct IngestResp {
    intent: String,
    message: String,
    event: Option<Event>,
    notifications: Vec<Notification>,
    /// F18 generative UI envelope (already validated in solum-core). The
    /// frontend renders it inline in the agent bubble; absent = text only.
    ui: Option<solum_core::genui::UiEnvelope>,
    /// F19 definition preview. This is separate from F18's ephemeral envelope
    /// and must be explicitly confirmed before any widget table is written.
    widget_preview: Option<solum_core::orchestrator::WidgetPreview>,
}

/// One streamed chat token (§3.6 第 7 条). Emitted on `solum-chat-delta` as a
/// plain-prose reply arrives; `stream_id` correlates a burst of deltas with the
/// `ingest` call that produced them (the frontend passes it in).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatDelta {
    stream_id: String,
    delta: String,
}

/// `async` + `spawn_blocking`: ingest can block tens of seconds on the cloud
/// reasoner — running it on the main thread stalled the message pump until
/// Windows flagged the window 未响应 (2026-07-18 走查发现 1).
///
/// When `stream_id` is present a chat-intent reply streams its visible prose
/// through `solum-chat-delta` events while the call runs; the command still
/// returns the full `IngestResp` (message + optional envelope) for the
/// frontend to reconcile. Absent `stream_id` = no streaming (backward
/// compatible; other callers and non-chat intents are unaffected).
#[tauri::command]
pub(crate) async fn ingest(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    text: String,
    now: Option<String>,
    stream_id: Option<String>,
    capture_id: Option<String>,
) -> CmdResult<IngestResp> {
    let now = parse_now(now)?;
    if text.trim().is_empty() {
        return Err("请输入内容".into());
    }
    let orch = state.orch.clone();
    let transition = state.account_transition.clone();
    let capture = match capture_id.as_deref() {
        Some(id) => Some(
            state
                .capture_inbox
                .lock()
                .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?
                .get(id)
                .cloned()
                .ok_or_else(|| "这条待确认内容已不存在，请重新分享".to_string())?,
        ),
        None => None,
    };
    let capture_for_task = capture.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        let mut o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
        if let Some(draft) = capture_for_task {
            return o
                .ingest_from_source(
                    &text,
                    now,
                    draft.source.as_str(),
                    draft.external_id.as_deref(),
                )
                .map_err(core_err);
        }
        match stream_id {
            Some(sid) => {
                let mut on_delta = |d: &str| {
                    let _ = app.emit(
                        "solum-chat-delta",
                        ChatDelta {
                            stream_id: sid.clone(),
                            delta: d.to_string(),
                        },
                    );
                };
                o.ingest_streaming(&text, now, &mut on_delta)
                    .map_err(core_err)
            }
            None => o.ingest(&text, now).map_err(core_err),
        }
    })
    .await
    .map_err(|e| format!("后台任务失败：{e}"))??;
    if let Some(draft) = capture {
        state
            .capture_inbox
            .lock()
            .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?
            .remove(&draft.id);
    }
    Ok(IngestResp {
        intent: intent_str(out.intent).to_string(),
        message: out.message,
        event: out.event,
        notifications: out.notifications,
        ui: out.ui,
        widget_preview: out.widget_preview,
    })
}

// ---- persistent widgets (F19) ---------------------------------------------

/// Step 2 of component creation: only a server-held, rendered preview id may
/// be confirmed. There is intentionally no IPC command that inserts an
/// arbitrary definition directly.
#[tauri::command]
pub(crate) fn widget_confirm_preview(
    state: State<AppState>,
    preview_id: String,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetDefinition> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .confirm_widget_preview(&preview_id, now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn widget_discard_preview(state: State<AppState>, preview_id: String) -> CmdResult<()> {
    require_full_account()?;
    lock!(state)
        .discard_widget_preview(&preview_id)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn widget_defs(
    state: State<AppState>,
) -> CmdResult<Vec<solum_core::widget::WidgetDefinition>> {
    require_full_account()?;
    lock!(state).widget_definitions().map_err(core_err)
}

#[tauri::command]
pub(crate) fn widget_records(
    state: State<AppState>,
    widget_id: i64,
) -> CmdResult<Vec<solum_core::widget::WidgetRecord>> {
    require_full_account()?;
    lock!(state).widget_records(widget_id).map_err(core_err)
}

/// Schema evolution, add-only (设计稿 ⑧). The core rejects a required field;
/// the shell does not pre-filter, so the one rule lives in one place.
#[tauri::command]
pub(crate) fn widget_add_field(
    state: State<AppState>,
    widget_id: i64,
    field: solum_core::widget::WidgetField,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetDefinition> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .add_widget_field(widget_id, &field, now)
        .map_err(core_err)
}

/// 设计稿 ⑦: both event bridges are snapshot copies, never live links.
#[tauri::command]
pub(crate) fn widget_import_events(
    state: State<AppState>,
    widget_id: i64,
    limit: usize,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetImportOutcome> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .import_events_into_widget(widget_id, limit, now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn widget_promote_record(
    state: State<AppState>,
    widget_id: i64,
    record_id: i64,
    now: Option<String>,
) -> CmdResult<solum_core::model::Event> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .promote_widget_record(widget_id, record_id, now)
        .map_err(core_err)
}

/// Record CRUD is safe and local-first: no cloud route or Guard is involved.
#[tauri::command]
pub(crate) fn widget_record_create(
    state: State<AppState>,
    widget_id: i64,
    data: serde_json::Value,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetRecord> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .add_widget_record(widget_id, data, now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn widget_record_update(
    state: State<AppState>,
    widget_id: i64,
    record_id: i64,
    data: serde_json::Value,
) -> CmdResult<solum_core::widget::WidgetRecord> {
    require_full_account()?;
    lock!(state)
        .update_widget_record(widget_id, record_id, data)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn agenda(state: State<AppState>, now: Option<String>) -> CmdResult<Vec<Event>> {
    let now = parse_now(now)?;
    lock!(state).agenda(now).map_err(core_err)
}

#[tauri::command]
pub(crate) fn all_events(state: State<AppState>) -> CmdResult<Vec<Event>> {
    lock!(state).all_events().map_err(core_err)
}

// ---- notifications -----------------------------------------------------------

#[tauri::command]
pub(crate) fn all_notifications(state: State<AppState>) -> CmdResult<Vec<Notification>> {
    lock!(state).all_notifications().map_err(core_err)
}

#[tauri::command]
pub(crate) fn due(state: State<AppState>, now: Option<String>) -> CmdResult<Vec<Notification>> {
    let now = parse_now(now)?;
    // 与 CLI 的 due 一致：先按注入时钟物化 routine 当日/次日发生（否则模拟
    // 时钟下 routine 永远不会长出提醒——ticker 只认系统时钟）。
    let mut o = lock!(state);
    o.materialize_routines(now).map_err(core_err)?;
    o.due(now).map_err(core_err)
}

#[tauri::command]
pub(crate) fn fire_due(
    state: State<AppState>,
    now: Option<String>,
) -> CmdResult<Vec<Notification>> {
    let now = parse_now(now)?;
    let mut o = lock!(state);
    o.materialize_routines(now).map_err(core_err)?;
    o.fire_due(now).map_err(core_err)
}

#[tauri::command]
pub(crate) fn dismiss(state: State<AppState>, id: i64) -> CmdResult<()> {
    lock!(state).dismiss(id).map_err(core_err)
}

/// Apply a reschedule picked from a GenUI option (or any UI surface): move
/// the event to `start` and re-plan its reminders. Returns a human summary.
#[tauri::command]
pub(crate) fn event_reschedule(
    state: State<AppState>,
    id: i64,
    start: String,
    now: Option<String>,
) -> CmdResult<String> {
    let now = parse_now(now)?;
    let new_start = solum_core::model::parse_ts(&start).map_err(core_err)?;
    let (ev, stored) = lock!(state)
        .reschedule_event(id, new_start, now)
        .map_err(core_err)?;
    Ok(format!(
        "已把「{}」改到 {}（提醒已重排，共 {} 条）",
        ev.title,
        solum_core::model::fmt_ts(&ev.start),
        stored.len()
    ))
}

/// Snooze a reminder: ring again `minutes` from `now`. Returns the new fire
/// time (formatted). The Android alarm mirror converges on the next ticker
/// pass like any other pending-set change.
#[tauri::command]
pub(crate) fn snooze(
    state: State<AppState>,
    id: i64,
    minutes: i64,
    now: Option<String>,
) -> CmdResult<String> {
    let now = parse_now(now)?;
    let until = lock!(state).snooze(id, minutes, now).map_err(core_err)?;
    Ok(solum_core::model::fmt_ts(&until))
}

// ---- memory ledger (F12) -------------------------------------------------------

#[tauri::command]
pub(crate) fn ledger(state: State<AppState>) -> CmdResult<Vec<MemoryEntry>> {
    lock!(state).ledger().map_err(core_err)
}

/// One paged ledger slice for the F12 UI: a single layer, keyset-paginated.
/// Returns (entries, next_cursor); a null cursor means the layer is done.
#[tauri::command]
pub(crate) fn ledger_page(
    state: State<AppState>,
    layer: String,
    limit: usize,
    cursor: Option<String>,
) -> CmdResult<(Vec<MemoryEntry>, Option<String>)> {
    let layer: solum_core::model::MemoryLayer = layer.parse().map_err(core_err)?;
    lock!(state)
        .ledger_page(layer, limit, cursor.as_deref())
        .map_err(core_err)
}

/// F12 可编辑：改写一条语义记忆的措辞（recall 立即生效）。
#[tauri::command]
pub(crate) fn fact_update(state: State<AppState>, id: i64, content: String) -> CmdResult<()> {
    lock!(state).update_fact(id, &content).map_err(core_err)
}

/// §4 数据完全归你：把本机全部数据导出为一份 JSON 文件，写在数据库同目录，
/// 返回完整路径。纯只读、纯本地、不上云。
#[tauri::command]
pub(crate) fn export_data(state: State<AppState>, now: Option<String>) -> CmdResult<String> {
    let now = parse_now(now)?;
    let json = lock!(state).export_json(now).map_err(core_err)?;
    write_export_json(&state.db_path, &json, "solum-export")
}

/// Build a v3 main logical payload from a WebView/IndexedDB snapshot plus the
/// Rust-owned spool and non-secret configuration. This command is deliberately
/// not wired to the current v2 export/import UI: v3 restore stays fail-closed
/// until the platform-wide temporary-store transaction exists.
#[tauri::command]
pub(crate) fn export_portable_data(
    state: State<AppState>,
    app: tauri::AppHandle,
    snapshot: portable_snapshot::WebViewPortableSnapshot,
    now: Option<String>,
) -> CmdResult<String> {
    let now = parse_now(now)?;
    let json = collect_portable_json(&state, &app, snapshot, now)?;
    write_export_json(&state.db_path, &json, "solum-export-v3")
}

pub(crate) fn collect_portable_json(
    state: &AppState,
    app: &tauri::AppHandle,
    snapshot: portable_snapshot::WebViewPortableSnapshot,
    now: NaiveDateTime,
) -> CmdResult<String> {
    let expected_profile = snapshot.profile_id.clone();
    portable_snapshot::validate_profile(&expected_profile, &active_local_profile_id())?;

    let health = app.health_connect();
    let health_sources = if health.is_available().map_err(|e| e.to_string())?
        && health.has_permissions().map_err(|e| e.to_string())?
    {
        vec!["android-health-connect".to_string()]
    } else {
        Vec::new()
    };
    let background_reminders = app.alarm().is_available().map_err(|e| e.to_string())?;
    let dir = std::path::Path::new(&state.db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // Holding the Orchestrator mutex prevents the resident ticker from moving
    // a notification between spool and SQLite while both halves are captured.
    let json = {
        let orch = state
            .orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        portable_snapshot::validate_profile(&expected_profile, &active_local_profile_id())?;
        let platform =
            portable_snapshot::collect(snapshot, &dir, health_sources, background_reminders)?;
        orch.export_portable_json(now, platform).map_err(core_err)?
    };
    portable_snapshot::validate_profile(&expected_profile, &active_local_profile_id())?;
    Ok(json)
}

pub(crate) fn current_backup_account(
    expected_profile: &str,
    expected: &solum_core::account::AccountSession,
) -> CmdResult<solum_core::account::AccountSession> {
    let current = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "备份已取消：当前账号会话不可用".to_string())?;
    if active_local_profile_id() != expected_profile
        || current.stable_user_id() != Some(expected_profile)
        || current.user_id != expected.user_id
        || current.server_url != expected.server_url
    {
        return Err("备份已取消：账号或本地 profile 已切换".into());
    }
    Ok(current)
}

pub(crate) fn require_current_privacy_consent() -> CmdResult<()> {
    if solum_core::privacy::PrivacyConsent::needs_consent() {
        return Err("请先阅读并同意当前隐私政策，再使用加密云备份".into());
    }
    Ok(())
}

pub(crate) fn backup_master_key(
    account: &solum_core::account::AccountSession,
) -> CmdResult<[u8; 32]> {
    let config = solum_core::sync::SyncConfig::load()
        .ok_or_else(|| "尚未配置端到端主密钥，无法创建加密备份".to_string())?;
    let bound = config
        .account
        .as_ref()
        .ok_or_else(|| "旧式同步密钥未绑定不可变账号，无法用于云备份".to_string())?;
    if bound.user_id != account.user_id
        || bound.server_url != account.server_url
        || config.url.trim_end_matches('/') != account.server_url
    {
        return Err("端到端密钥与当前账号或服务端不匹配".into());
    }
    config.e2ee_master_key().map_err(core_err)
}

pub(crate) fn require_backup_delete_is_not_in_restore(
    state: &AppState,
    args: &str,
) -> CmdResult<()> {
    let request: solum_core::backup_download::BackupDeleteRequest =
        serde_json::from_str(args).map_err(|_| "备份删除参数无效".to_string())?;
    solum_core::backup_download::validate_backup_delete_request(&request)
        .map_err(|_| "备份删除参数无效".to_string())?;
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能删除加密云备份".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?;
    if active_local_profile_id() != profile_id {
        return Err("备份删除已取消：账号或本地 profile 不一致".into());
    }
    if state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())?
        .as_ref()
        .is_some_and(|restore| restore.backup_id == request.backup_id)
    {
        return Err("这个封缄点正在用于恢复，请先取消或完成恢复".into());
    }
    if solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        profile_id,
        &account.server_url,
    )
    .map_err(|error| error.to_string())?
    .is_some_and(|pending| pending.status().backup_id == request.backup_id)
    {
        return Err("这个封缄点已有待继续的接管事务，不能删除".into());
    }
    Ok(())
}

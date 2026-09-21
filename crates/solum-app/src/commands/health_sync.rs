use super::*;

// ---- notification-listener access (F1 companion) ---------------------------------

/// Whether Solum's notification-capture listener is enabled in OS settings.
/// Always `true` on desktop (no such permission exists there) so the
/// frontend only ever shows the nag banner where it's actually actionable.
#[tauri::command]
pub(crate) fn notif_access_status(app: tauri::AppHandle) -> CmdResult<bool> {
    require_full_account()?;
    app.notif_access().is_enabled().map_err(|e| e.to_string())
}

/// Jump to the notification-listener settings page (Android: Settings →
/// Notifications → Notification access). No runtime-request dialog exists
/// for this permission — the user must flip the toggle themselves.
#[tauri::command]
pub(crate) fn notif_access_open_settings(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
    app.notif_access()
        .open_settings()
        .map_err(|e| e.to_string())
}

// ---- wearable health samples (F5, Phase 4) -----------------------------------------

#[derive(Serialize)]
pub(crate) struct HealthStatusDto {
    /// Whether the Health Connect SDK/app exists on this device at all
    /// (always `false` on desktop — it's an Android-only platform service).
    available: bool,
    granted: bool,
}

#[tauri::command]
pub(crate) fn health_status(app: tauri::AppHandle) -> CmdResult<HealthStatusDto> {
    require_full_account()?;
    let hc = app.health_connect();
    let available = hc.is_available().map_err(|e| e.to_string())?;
    let granted = if available {
        hc.has_permissions().map_err(|e| e.to_string())?
    } else {
        false
    };
    Ok(HealthStatusDto { available, granted })
}

/// Launch Health Connect's own grant screen; blocks until the user returns.
#[tauri::command]
pub(crate) fn health_request(app: tauri::AppHandle) -> CmdResult<bool> {
    require_full_account()?;
    app.health_connect()
        .request_permissions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn health_samples(state: State<AppState>) -> CmdResult<Vec<HealthSample>> {
    require_full_account()?;
    lock!(state).health_samples().map_err(core_err)
}

#[tauri::command]
pub(crate) fn health_trends(
    state: State<AppState>,
) -> CmdResult<Vec<solum_core::wearable::HealthTrendSummary>> {
    require_full_account()?;
    let samples = lock!(state).health_samples().map_err(core_err)?;
    Ok(solum_core::wearable::health_trend_summaries(
        &samples,
        Local::now().naive_local(),
    ))
}

/// ISO-8601 instant (Health Connect's `Instant.toString()`, always UTC) to
/// the local wall-clock `NaiveDateTime` the store expects.
pub(crate) fn parse_instant_local(s: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Local).naive_local())
}

pub(crate) fn convert_raw_sample(r: solum_health_connect::RawSample) -> Option<HealthSample> {
    let kind: HealthMetric = r.kind.parse().ok()?;
    let start = parse_instant_local(&r.start)?;
    let end = parse_instant_local(&r.end)?;
    Some(HealthSample::new(
        kind,
        start,
        end,
        r.value,
        "health_connect",
    ))
}

// ---- sync (F17 §3.8) ---------------------------------------------------------------

/// The JSON file the settings UI reads/writes. Matches `SyncConfig::load`'s
/// fallback: explicit `SOLUM_SYNC_CONFIG` first, then the active account profile
/// (or the historical app-data path for guest).
pub(crate) fn sync_config_file() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SOLUM_SYNC_CONFIG") {
        return p.into();
    }
    solum_core::paths::resolve_profile_with_adoption("solum-sync.json")
}

/// Everything the settings form needs — never the password, and not the
/// derived token/key either (those are recomputed from username+password at
/// load time, the UI has no business holding them).
#[derive(Serialize)]
pub(crate) struct SyncConfigSettings {
    configured: bool,
    /// "credentials" (username+password, current recommended shape) |
    /// "raw" (legacy `{url,token,key}` file — still works, not shown as
    /// editable fields here since there's no username/password to show) |
    /// "none".
    format: &'static str,
    path: String,
    url: String,
    username: String,
    account_logged_in: bool,
    device_id: String,
}

#[tauri::command]
pub(crate) fn sync_config_get(state: State<AppState>) -> CmdResult<SyncConfigSettings> {
    require_full_account()?;
    let path = sync_config_file();
    let path_str = path.to_string_lossy().into_owned();
    let device_id = lock!(state).sync_device_id().map_err(core_err)?;
    let raw = std::fs::read_to_string(&path).ok();
    let parsed = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let account = solum_core::account::AccountSession::load_full();
    let account_username = account
        .as_ref()
        .map(|session| session.username.clone())
        .unwrap_or_default();
    let account_server_url = account
        .as_ref()
        .map(|session| session.server_url.clone())
        .unwrap_or_default();
    let account_logged_in = account.is_some();
    let Some(v) = parsed else {
        return Ok(SyncConfigSettings {
            configured: false,
            format: "none",
            path: path_str,
            url: account_server_url,
            username: account_username,
            account_logged_in,
            device_id,
        });
    };
    let url = v
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    if v.get("key").is_some() && v.get("token").is_none() && v.get("username").is_none() {
        Ok(SyncConfigSettings {
            configured: true,
            format: "account",
            path: path_str,
            url,
            username: account_username,
            account_logged_in,
            device_id,
        })
    } else if let Some(username) = v.get("username").and_then(|x| x.as_str()) {
        Ok(SyncConfigSettings {
            configured: true,
            format: "credentials",
            path: path_str,
            url,
            username: username.to_string(),
            account_logged_in,
            device_id,
        })
    } else if v.get("token").is_some() && v.get("key").is_some() {
        Ok(SyncConfigSettings {
            configured: true,
            format: "raw",
            path: path_str,
            url,
            username: String::new(),
            account_logged_in,
            device_id,
        })
    } else {
        Ok(SyncConfigSettings {
            configured: false,
            format: "none",
            path: path_str,
            url: account_server_url,
            username: account_username,
            account_logged_in,
            device_id,
        })
    }
}

#[derive(Deserialize)]
pub(crate) struct SyncSaveArgs {
    url: String,
    /// Account mode no longer accepts a separate password. Kept in the IPC
    /// shape so older frontends fail closed instead of deserializing badly.
    #[serde(default)]
    password: String,
}

/// Validate the auto-managed `{url,key}` account sync configuration and return
/// a status line for a toast. Account tokens remain in `solum-account.json`;
/// neither the login password nor a separate sync password is written here.
#[tauri::command]
pub(crate) fn sync_config_save(
    state: State<AppState>,
    app: tauri::AppHandle,
    cfg: SyncSaveArgs,
) -> CmdResult<String> {
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "请先在“云端接入”登录 Solum 账号".to_string())?;
    // Account access tokens are origin-bound by policy. A configurable relay
    // URL would turn a typo or edited config into credential exfiltration.
    let requested_url = cfg.url.trim().trim_end_matches('/');
    if !requested_url.is_empty() && requested_url != account.server_url {
        return Err("账号同步与登录服务必须使用同一个地址".into());
    }
    let url = account.server_url.clone();
    let password = cfg.password.trim().to_string();
    if !password.is_empty() {
        return Err("同步主密钥已由账号登录自动恢复，不再接受单独的同步密码".into());
    }
    let key = std::fs::read_to_string(sync_config_file())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(|v| v.get("token").is_none() && v.get("username").is_none())
        .and_then(|v| v.get("key").and_then(|p| p.as_str()).map(String::from))
        .unwrap_or_default();
    if key.is_empty() {
        return Err("账号同步密钥尚未建立，请退出后重新登录以自动恢复".into());
    }

    let json = serde_json::to_string_pretty(&serde_json::json!({
        "url": url,
        "key": key,
    }))
    .map_err(|e| e.to_string())?;
    let path = sync_config_file();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    solum_core::fsatomic::write_atomic(&path, &json).map_err(|e| e.to_string())?;

    // A configured sync relay also carries small, fixed-shape self-use alert
    // events. Start the Android foreground pipeline immediately so the user
    // does not need to restart Solum after binding this device.
    app.notif_access()
        .start_pipeline()
        .map_err(|e| e.to_string())?;

    let device_id = lock!(state).sync_device_id().map_err(core_err)?;
    Ok(format!(
        "已关联账号 {}（本机设备标识：{device_id}）",
        account.username
    ))
}

#[derive(Serialize)]
pub(crate) struct SyncStatusDto {
    configured: bool,
    summary: Option<String>,
    device_id: String,
    /// Set once this device's cursor fell below the relay's retention floor.
    /// **Sticky**: it describes data that is gone, so it stays visible until
    /// the user re-seeds from a peer — a warning that scrolls past in one toast
    /// is a warning nobody acts on.
    history_gap: Option<String>,
    /// Unopenable blobs currently parked, and how many were dropped for
    /// overflow. Dropped > 0 means recovery material was discarded.
    bad_blobs_held: i64,
    bad_blobs_dropped: i64,
    /// Why the push half of the last round could not finish (relay quota,
    /// revoked device). Sticky until a clean push clears it — same reasoning
    /// as `history_gap`: background failures only log, so the status view is
    /// where "my changes are not reaching my other devices" must be visible.
    push_block: Option<String>,
    /// Local ops parked as too large to ever upload; data stays in the local
    /// oplog but no peer will receive it.
    push_holds: i64,
}

#[tauri::command]
pub(crate) fn sync_status(state: State<AppState>) -> CmdResult<SyncStatusDto> {
    require_full_account()?;
    let account_logged_in = solum_core::account::AccountSession::load_full().is_some();
    let cfg = account_logged_in
        .then(solum_core::sync::SyncConfig::load)
        .flatten();
    let store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let (held, dropped) = store.bad_blob_stats().map_err(core_err)?;
    let history_gap = store
        .sync_state(solum_core::sync::HISTORY_GAP_KEY)
        .map_err(core_err)?;
    let push_block = store
        .sync_state(solum_core::sync::PUSH_BLOCKED_KEY)
        .map_err(core_err)?;
    let push_holds = store.push_hold_count().map_err(core_err)?;
    drop(store);
    Ok(SyncStatusDto {
        configured: cfg.is_some(),
        summary: cfg.map(|c| c.masked_summary()),
        device_id: lock!(state).sync_device_id().map_err(core_err)?,
        history_gap,
        bad_blobs_held: held,
        bad_blobs_dropped: dropped,
        push_block,
        push_holds,
    })
}

/// Clear the sticky history-gap marker after the user has re-seeded from a
/// peer. Deliberately explicit: nothing clears it automatically, because
/// nothing else can know the data was recovered.
#[tauri::command]
pub(crate) fn sync_gap_acknowledge(state: State<AppState>) -> CmdResult<()> {
    require_full_account()?;
    let store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    store
        .clear_sync_state(solum_core::sync::HISTORY_GAP_KEY)
        .map_err(core_err)
}

/// Run one sync round **without holding the orchestrator lock**.
///
/// The network call happens against `sync_store`, a connection nobody else
/// uses; only the cache reload afterwards touches `orch`, and only when the
/// merge actually changed something. See the `sync_store` field comment.
pub(crate) fn sync_round(state: &AppState) -> CmdResult<solum_core::sync::SyncOutcome> {
    if solum_core::account::AccountSession::load_full().is_none() {
        return Err("多设备同步需要先注册或登录 Solum 账号".into());
    }
    let Some(cfg) = solum_core::sync::SyncConfig::load() else {
        return Err("同步未配置，或当前未登录对应的 Solum 账号".into());
    };
    let transport = solum_core::sync::HttpTransport::new(&cfg).map_err(core_err)?;
    let outcome = {
        let store = state
            .sync_store
            .lock()
            .map_err(|_| "sync store lock poisoned".to_string())?;
        if state.account_transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
        solum_core::sync::sync_once(&store, &transport, &cfg).map_err(core_err)?
    };
    if outcome.applied > 0 {
        state
            .orch
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?
            .reload_caches()
            .map_err(core_err)?;
    }
    Ok(outcome)
}

/// One manual sync round. Returns the outcome counts for a toast.
#[tauri::command]
pub(crate) fn sync_now(state: State<AppState>) -> CmdResult<solum_core::sync::SyncOutcome> {
    sync_round(&state)
}


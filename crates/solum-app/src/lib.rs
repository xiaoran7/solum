#![allow(linker_messages)] // MSVC prints normal import-library progress to stdout.

//! solum-app — the Tauri 2 desktop shell for the Solum Phase 1 core.
//!
//! This is a thin adapter: every command locks the shared [`Orchestrator`] and
//! forwards to `solum-core`, exactly like `solum-cli` does. No business logic lives
//! here — the UI can only do what the core exposes, and the HITL guard's
//! compile-time guarantees carry over untouched (the frontend never sees a
//! `Grant`; confirming in the dialog calls `confirm` + `run_tool` on the Rust
//! side).
//!
//! The clock is injectable end-to-end: every command accepts an optional `now`
//! string (from the UI's "模拟时钟"); absent means the real system clock.
//!
//! Phase 2 turns the shell into a resident host: a background ticker (system
//! clock only — the simulated clock stays a UI demo device) fires due
//! reminders, asks status check-ins, and auto-generates suggestions, surfacing
//! each through OS notifications and window events.
//!
//! Phase 3 makes the same crate the mobile shell: the crate is a library so
//! Tauri's Android runner can link it (`mobile_entry_point`), and the desktop
//! binary (`main.rs`) is a thin wrapper around [`run`].

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{mpsc, Arc, Mutex};

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use solum_alarm::AlarmExt;
use solum_health_connect::HealthConnectExt;
use solum_notif_access::NotifAccessExt;
use tauri::{Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;

use solum_core::classify::{ImportanceRule, LeadTime};
use solum_core::email::{
    EmailAccount, EmailAuth, EmailConfig, EmailEndpoints, EmailProvider, SmtpTls,
};
use solum_core::extract::Intent;
use solum_core::journal::BehaviorEntry;
use solum_core::llm::{ChatTurn, MAX_HISTORY_TURNS};
use solum_core::model::{Channel, Event, EventKind, MemoryEntry, Notification};
use solum_core::notification_intelligence::{
    CaptureLane, CaptureState, NotificationCapture, NotificationIntelligenceConfig,
};
use solum_core::persona::{PersonaDraft, PersonaProfile};
use solum_core::proactivity::{ProactivityDimension, ProactivityLevel};
use solum_core::suggest::{Suggestion, SuggestionStatus};
use solum_core::wearable::{HealthMetric, HealthSample};
use solum_core::Orchestrator;

struct AppState {
    /// `Arc` so cloud-bound commands can move a handle into
    /// `spawn_blocking` — cloud HTTP on the main thread froze the whole
    /// window (Windows「未响应」, 2026-07-18 走查发现 1).
    orch: Arc<Mutex<Orchestrator>>,
    db_path: String,
    /// Masked provider summary when the cloud reasoner is configured.
    /// Mutable: the settings UI can save/apply a new endpoint at runtime.
    llm_summary: Mutex<Option<String>>,
    /// F5: the start of the next Health Connect poll window (epoch ms).
    /// Narrows after each successful poll so the ticker doesn't re-read the
    /// same history every 5 minutes.
    ///
    /// **Mirrored into the database** (`HEALTH_SINCE_KEY`), because in-memory
    /// alone it was not the "pure efficiency detail" it was described as: on
    /// every restart it reset to now-6h and re-read six hours of history.
    /// Interval-aggregated metrics like step counts are not idempotent under
    /// that — the same steps arrive again inside a differently-bounded window,
    /// which is a different `dedup_key`, so they are stored *again* and the
    /// day's total inflates. Sync then spreads the inflated figure.
    health_since_ms: Mutex<i64>,
    /// Signature of the alarm set last pushed to the OS (Android AlarmManager
    /// mirror, see `resync_alarms`). 0 = nothing pushed yet.
    alarm_sig: Mutex<u64>,
    /// Date whose focus brief has already been offered to this running app.
    /// A brief is a transient UI projection, so this small delivery gate does
    /// not belong in the durable store or sync layer.
    last_daily_brief_date: Mutex<Option<NaiveDate>>,
    /// OAuth callback codes and PKCE verifiers are ephemeral. They never
    /// enter SQLite or the durable mail config; a completed/expired session is
    /// removed immediately after polling.
    email_oauth: Mutex<HashMap<String, EmailOAuthSession>>,
    /// A **second** SQLite connection used only by sync.
    ///
    /// Sync is network-bound and can block for as long as its request timeout;
    /// running it through `orch` meant a slow or half-dead relay held the one
    /// lock that reminders, the ticker, and every UI command also need — the
    /// app froze for reasons that had nothing to do with the user. SQLite in
    /// WAL mode is happy with concurrent connections, so sync gets its own and
    /// touches `orch` only for the brief cache reload afterwards.
    sync_store: Mutex<solum_core::store::Store>,
    /// 多入口采集的待确认队列（进程内，不落盘——见 `solum_core::capture`）。
    /// 放 `AppState` 而不是全局静态，是因为它的生命周期本来就该跟这个窗口一致。
    capture_inbox: Mutex<solum_core::capture::CaptureInbox>,
}

struct EmailOAuthCallback {
    code: String,
    state: String,
}

struct EmailOAuthSession {
    account_id: String,
    state: String,
    code_verifier: String,
    redirect_uri: String,
    receiver: mpsc::Receiver<CmdResult<EmailOAuthCallback>>,
}

type CmdResult<T> = Result<T, String>;

/// Durable cursor for the Health Connect poll window (epoch ms). Device-local
/// like the sync cursors it sits beside — a poll position is not memory.
const HEALTH_SINCE_KEY: &str = "health_poll_since_ms";

fn parse_now(now: Option<String>) -> CmdResult<NaiveDateTime> {
    let Some(s) = now.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        // Zero out sub-minute noise like solum-cli does: otherwise derived event
        // times inherit nanosecond tails and the UI shows "12:53:47.425371200".
        use chrono::Timelike;
        let now = Local::now().naive_local();
        return Ok(now
            .with_second(0)
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(now));
    };
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(dt);
        }
    }
    Err(format!("无法解析模拟时钟：{s:?}"))
}

fn intent_str(intent: Intent) -> &'static str {
    match intent {
        Intent::Chat => "chat",
        Intent::CreateWidget => "create_widget",
        Intent::IngestEvent => "ingest_event",
        Intent::StatusAnswer => "status_answer",
        Intent::MemoryWrite => "memory_write",
        Intent::RescheduleEvent => "reschedule_event",
        Intent::CancelEvent => "cancel_event",
        Intent::DangerousCommand => "dangerous_command",
    }
}

macro_rules! lock {
    ($state:expr) => {
        $state
            .orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?
    };
}

fn core_err(e: solum_core::CoreError) -> String {
    e.to_string()
}

fn daily_brief_is_due(last_emitted: Option<NaiveDate>, today: NaiveDate) -> bool {
    last_emitted != Some(today)
}

/// The Android listener reads only this tiny local projection before appending
/// an inbox line. Missing/unreadable means an empty whitelist (capture none),
/// which is the safe default even during a concurrent settings write.
fn write_notification_capture_policy(
    db_path: &str,
    config: &NotificationIntelligenceConfig,
) -> CmdResult<()> {
    let parent = std::path::Path::new(db_path)
        .parent()
        .ok_or_else(|| "数据库路径没有父目录，无法写通知白名单".to_string())?;
    let path = parent.join("notif-policy.json");
    let projection = serde_json::json!({ "allowed_packages": config.allowed_packages });
    // Atomic replacement, not truncate-then-write: the native listener reads
    // this file on every notification, and a torn read means it either keeps
    // capturing an app the user just revoked, or stops capturing entirely.
    let text = serde_json::to_string(&projection).map_err(|e| e.to_string())?;
    solum_core::fsatomic::write_atomic(&path, &text)
        .map_err(|e| format!("写入通知白名单失败（{}）: {e}", path.display()))
}

fn sync_notification_pipeline(
    app: &tauri::AppHandle,
    config: &NotificationIntelligenceConfig,
) -> CmdResult<()> {
    if config.allowed_packages.is_empty() {
        app.notif_access()
            .stop_pipeline()
            .map_err(|e| e.to_string())
    } else {
        app.notif_access()
            .start_pipeline()
            .map_err(|e| e.to_string())
    }
}

// ---- meta ------------------------------------------------------------------

#[derive(Serialize)]
struct AppInfo {
    db_path: String,
    system_now: String,
    /// e.g. "https://… · mimo-v2.5 · key:…zju9"; `None` = offline mode.
    llm: Option<String>,
}

#[tauri::command]
fn app_info(state: State<AppState>) -> CmdResult<AppInfo> {
    Ok(AppInfo {
        db_path: state.db_path.clone(),
        system_now: Local::now()
            .naive_local()
            .format("%Y-%m-%dT%H:%M")
            .to_string(),
        llm: state.llm_summary.lock().map(|g| g.clone()).unwrap_or(None),
    })
}

// ---- cloud LLM settings (§3.6; provider survey in docs/LLM-PROVIDERS.md) ----

/// The JSON file the settings UI reads/writes. Matches `LlmConfig::load`'s
/// fallback: `SOLUM_LLM_CONFIG` if set (mobile setup points it at app-data),
/// else `./solum-llm.json` (desktop cwd).
fn llm_config_file() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SOLUM_LLM_CONFIG") {
        return p.into();
    }
    solum_core::paths::resolve_with_adoption("solum-llm.json")
}

/// Soulous uses the same local-only config convention as the LLM. On mobile
/// setup points `SOLUM_SOULOUS_CONFIG` next to the database, so the WebView's
/// meaningless cwd never decides where dual tokens are stored.
fn soulous_config_file() -> std::path::PathBuf {
    solum_core::soulous::SoulousConfig::path()
}

/// When both env credentials are set they win over the file at next launch —
/// the UI surfaces this so a save doesn't look mysteriously ignored.
fn llm_env_active() -> bool {
    let set = |k: &str| {
        std::env::var(k)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    };
    set("SOLUM_LLM_BASE_URL") && set("SOLUM_LLM_API_KEY")
}

fn key_tail(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        tail
    } else {
        format!("…{tail}")
    }
}

/// Everything the settings form needs — never the key itself.
#[derive(Serialize)]
struct LlmSettings {
    configured: bool,
    /// "env" | "file"; `None` when unconfigured.
    source: Option<&'static str>,
    /// Where saves land.
    path: String,
    base_url: String,
    model: String,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    timeout_secs: u64,
    /// Last 4 chars of the stored key, e.g. "…zju9".
    key_tail: String,
}

#[tauri::command]
fn llm_config_get() -> CmdResult<LlmSettings> {
    let path = llm_config_file().to_string_lossy().into_owned();
    match solum_core::llm::LlmConfig::load() {
        Some(c) => Ok(LlmSettings {
            configured: true,
            source: Some(if llm_env_active() { "env" } else { "file" }),
            path,
            base_url: c.base_url,
            model: c.model,
            temperature: c.temperature,
            max_tokens: c.max_tokens,
            timeout_secs: c.timeout_secs,
            key_tail: key_tail(&c.api_key),
        }),
        None => Ok(LlmSettings {
            configured: false,
            source: None,
            path,
            base_url: String::new(),
            model: String::new(),
            temperature: Some(0.3),
            max_tokens: None,
            timeout_secs: 30,
            key_tail: String::new(),
        }),
    }
}

#[derive(Deserialize)]
struct LlmSaveArgs {
    base_url: String,
    /// Empty → reuse the key already stored in the config file, so editing
    /// the model doesn't force re-entering the secret.
    #[serde(default)]
    api_key: String,
    model: String,
    /// `null` = don't send the field (OpenAI gpt-5 family rejects non-default).
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    timeout_secs: u64,
}

fn resolve_llm_args(a: LlmSaveArgs) -> CmdResult<solum_core::llm::LlmConfig> {
    let base_url = solum_core::net::validate_endpoint(&a.base_url, "LLM base_url")
        .map_err(|e| e.to_string())?;
    let model = a.model.trim().to_string();
    if model.is_empty() {
        return Err("请填写模型名".into());
    }
    let api_key = a.api_key.trim().to_string();
    let api_key = if api_key.is_empty() {
        std::fs::read_to_string(llm_config_file())
            .ok()
            .and_then(|t| solum_core::llm::LlmConfig::from_json(&t).ok())
            .map(|c| c.api_key)
            .ok_or_else(|| "请填写 API Key（没有已保存的密钥可沿用）".to_string())?
    } else {
        api_key
    };
    Ok(solum_core::llm::LlmConfig {
        base_url,
        api_key,
        model,
        temperature: a.temperature,
        max_tokens: a.max_tokens,
        timeout_secs: a.timeout_secs.clamp(5, 600),
    })
}

/// Persist to the config file and hot-swap the running reasoner. Returns the
/// masked summary for the status footer.
#[tauri::command]
fn llm_config_save(state: State<AppState>, cfg: LlmSaveArgs) -> CmdResult<String> {
    let c = resolve_llm_args(cfg)?;
    let json = serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?;
    let path = llm_config_file();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    solum_core::fsatomic::write_atomic(&path, &json).map_err(|e| e.to_string())?;
    let summary = c.masked_summary();
    // 账号登录时账号代理优先（对齐鸿蒙 0.2.0 的模式）：直连配置照常保存，
    // 但不抢走正在运行的账号 reasoner——退出登录后自动回落到这份配置。
    if solum_core::account::AccountSession::load().is_some() {
        return Ok(format!(
            "{summary}（已保存；当前账号登录优先，退出登录后生效）"
        ));
    }
    lock!(state).set_reasoner(Box::new(solum_core::llm::LlmReasoner::new(c)));
    if let Ok(mut g) = state.llm_summary.lock() {
        *g = Some(summary.clone());
    }
    Ok(summary)
}

#[derive(Serialize)]
struct LlmTestResult {
    latency_ms: u64,
    reply: String,
}

/// One real round-trip with the (possibly unsaved) form values. Async so the
/// up-to-`timeout_secs` blocking HTTP call never freezes the UI thread.
#[tauri::command]
async fn llm_config_test(cfg: LlmSaveArgs) -> CmdResult<LlmTestResult> {
    let c = resolve_llm_args(cfg)?;
    tauri::async_runtime::spawn_blocking(move || {
        use solum_core::extract::Reasoner;
        let r = solum_core::llm::LlmReasoner::new(c);
        let t0 = std::time::Instant::now();
        let reply = r
            .complete("你是连通性测试。不管用户说什么，只回复一个词：pong", "ping")
            .map_err(|e| e.to_string())?;
        Ok(LlmTestResult {
            latency_ms: t0.elapsed().as_millis() as u64,
            reply,
        })
    })
    .await
    .map_err(|e| format!("测试线程失败: {e}"))?
}

// ---- Solum account (cloud proxy) — ported from the harmony 0.2.0 client ----

/// Everything the account settings block needs — never the tokens.
#[derive(Serialize)]
struct AccountStatus {
    logged_in: bool,
    username: String,
    server_url: String,
    model: String,
    model_options: Vec<&'static str>,
    /// Where the session file lives.
    path: String,
}

#[tauri::command]
fn account_status_get() -> CmdResult<AccountStatus> {
    let path = solum_core::account::AccountSession::path()
        .to_string_lossy()
        .into_owned();
    match solum_core::account::AccountSession::load() {
        Some(s) => Ok(AccountStatus {
            logged_in: true,
            username: s.username,
            server_url: s.server_url,
            model: s.model,
            model_options: solum_core::account::CLOUD_MODEL_OPTIONS.to_vec(),
            path,
        }),
        None => Ok(AccountStatus {
            logged_in: false,
            username: String::new(),
            server_url: String::new(),
            model: solum_core::account::DEFAULT_CLOUD_MODEL.to_string(),
            model_options: solum_core::account::CLOUD_MODEL_OPTIONS.to_vec(),
            path,
        }),
    }
}

/// Log in against a self-hosted solum-cloud (`server/`) and hot-swap the
/// running reasoner to the account proxy. The password exists only inside
/// this call. Async so the network round-trip never freezes the UI thread.
#[tauri::command]
async fn account_login(
    state: State<'_, AppState>,
    server_url: String,
    username: String,
    password: String,
    model: String,
) -> CmdResult<String> {
    let session = tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::login(&server_url, &username, &password, &model)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("登录线程失败: {e}"))??;
    let summary = format!("账号 · {}", session.masked_summary());
    lock!(state).set_reasoner(Box::new(solum_core::account::AccountReasoner::new(session)));
    if let Ok(mut g) = state.llm_summary.lock() {
        *g = Some(summary.clone());
    }
    Ok(summary)
}

/// Local sign-out first-class: best-effort server-side revocation, then the
/// reasoner falls back to the direct-key config (if any) or fully offline.
#[tauri::command]
async fn account_logout(state: State<'_, AppState>) -> CmdResult<String> {
    if let Some(session) = solum_core::account::AccountSession::load() {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            solum_core::account::logout(&session);
        })
        .await;
    } else {
        solum_core::account::AccountSession::delete_file();
    }
    match solum_core::llm::LlmConfig::load() {
        Some(cfg) => {
            let summary = cfg.masked_summary();
            lock!(state).set_reasoner(Box::new(solum_core::llm::LlmReasoner::new(cfg)));
            if let Ok(mut g) = state.llm_summary.lock() {
                *g = Some(summary.clone());
            }
            Ok(format!("已退出登录；云端改走直连配置（{summary}）"))
        }
        None => {
            lock!(state).clear_reasoner();
            if let Ok(mut g) = state.llm_summary.lock() {
                *g = None;
            }
            Ok("已退出登录；云端对话已关闭，本机功能不受影响".to_string())
        }
    }
}

/// Change which model the proxy asks upstream for (login stays untouched).
#[tauri::command]
fn account_model_save(state: State<AppState>, model: String) -> CmdResult<String> {
    let Some(mut session) = solum_core::account::AccountSession::load() else {
        return Err("请先登录账号".into());
    };
    session.model =
        solum_core::account::normalize_cloud_model(&model).map_err(|e| e.to_string())?;
    session.save().map_err(|e| e.to_string())?;
    let summary = format!("账号 · {}", session.masked_summary());
    lock!(state).set_reasoner(Box::new(solum_core::account::AccountReasoner::new(session)));
    if let Ok(mut g) = state.llm_summary.lock() {
        *g = Some(summary.clone());
    }
    Ok(summary)
}

// ---- 首启隐私门 + 应用内隐私政策（自 PA-harmony 回移）----------------------

#[derive(Serialize)]
struct PrivacyConsentStatus {
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
fn privacy_consent_status() -> CmdResult<PrivacyConsentStatus> {
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
fn privacy_consent_accept() -> CmdResult<String> {
    let c = solum_core::privacy::PrivacyConsent::accept(Local::now()).map_err(|e| e.to_string())?;
    Ok(c.accepted_at)
}

// ---- 多入口采集（capture 领域层，自 PA-harmony 回移）------------------------

#[derive(Serialize)]
struct CaptureDraftView {
    #[serde(flatten)]
    draft: solum_core::capture::CaptureDraft,
    source_label: &'static str,
    clues: solum_core::capture::CaptureClues,
    clue_summary: String,
}

fn draft_view(draft: solum_core::capture::CaptureDraft) -> CaptureDraftView {
    let clues = solum_core::capture::extract_capture_clues(&draft.text);
    CaptureDraftView {
        source_label: solum_core::capture::capture_source_label(draft.source),
        clue_summary: clues.summary(),
        clues,
        draft,
    }
}

/// 这台设备上的采集入口清单及其真实状态。
#[tauri::command]
fn capture_entry_points() -> CmdResult<Vec<solum_core::capture::CaptureConnector>> {
    Ok(solum_core::capture::capture_connectors(cfg!(
        target_os = "android"
    )))
}

#[tauri::command]
fn capture_inbox_list(state: State<AppState>) -> CmdResult<Vec<CaptureDraftView>> {
    let inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    Ok(inbox.snapshot().into_iter().map(draft_view).collect())
}

/// 收下一条外部输入。**只进待确认区，不写数据库**——落库要用户在界面上确认后
/// 走既有的 `ingest` 管道，这条边界是本模块存在的理由。
#[tauri::command]
fn capture_inbox_add(
    state: State<AppState>,
    source: String,
    title: String,
    text: String,
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
    Ok(draft_view(inbox.push(
        source,
        &title,
        &text,
        Local::now().timestamp_millis(),
    )))
}

/// 丢弃一条待确认草稿。误点分享目标时这一步必须是干净的：内存里删掉即可，
/// 本来就没有磁盘副本。
#[tauri::command]
fn capture_inbox_discard(state: State<AppState>, id: String) -> CmdResult<bool> {
    let mut inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    Ok(inbox.remove(&id))
}

// ---- Soulous read-only source settings (Phase 8.1) -------------------------

#[derive(Serialize)]
struct SoulousSettings {
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
fn soulous_config_get(state: State<AppState>, now: Option<String>) -> CmdResult<SoulousSettings> {
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
struct SoulousSaveArgs {
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

fn resolve_soulous_args(args: SoulousSaveArgs) -> CmdResult<solum_core::soulous::SoulousConfig> {
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
fn soulous_config_save(args: SoulousSaveArgs) -> CmdResult<String> {
    let config = resolve_soulous_args(args)?;
    config.save_to(&soulous_config_file()).map_err(core_err)?;
    Ok(config.masked_summary())
}

#[derive(Serialize)]
struct SoulousPullResp {
    /// False only when solum-soulous.json is absent/incomplete; that is a quiet
    /// offline state rather than an application error.
    configured: bool,
    outcome: Option<solum_core::soulous::PullOutcome>,
    cache: solum_core::soulous::SoulousStatus,
}

/// Real HTTP path, so it must never run on Tauri's main thread. A failure is
/// reported only to this manual action; it cannot delay ingest or reminders.
#[tauri::command]
async fn soulous_pull(
    state: State<'_, AppState>,
    now: Option<String>,
) -> CmdResult<SoulousPullResp> {
    let now = parse_now(now)?;
    let orch = state.orch.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
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
fn email_config_file() -> std::path::PathBuf {
    EmailConfig::path()
}

fn read_email_config() -> CmdResult<EmailConfig> {
    let path = email_config_file();
    match std::fs::read_to_string(&path) {
        Ok(raw) => EmailConfig::from_json(&raw).map_err(core_err),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EmailConfig::default()),
        Err(error) => Err(format!("读取邮箱配置 {} 失败: {error}", path.display())),
    }
}

fn email_account_from_config(account_id: &str) -> CmdResult<EmailAccount> {
    read_email_config()?
        .account(account_id)
        .cloned()
        .map_err(core_err)
}

#[derive(Serialize)]
struct EmailSettings {
    configured: bool,
    path: String,
    accounts: Vec<solum_core::email::EmailAccountSummary>,
}

#[tauri::command]
fn email_config_get() -> CmdResult<EmailSettings> {
    let config = read_email_config()?;
    Ok(EmailSettings {
        configured: !config.accounts.is_empty(),
        path: email_config_file().to_string_lossy().into_owned(),
        accounts: config.summaries(),
    })
}

#[derive(Deserialize)]
struct EmailAccountSaveArgs {
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

fn resolve_email_account(
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
fn email_config_save(args: EmailAccountSaveArgs) -> CmdResult<EmailSettings> {
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
fn email_config_remove(account_id: String) -> CmdResult<EmailSettings> {
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
async fn email_folders(account_id: String) -> CmdResult<Vec<solum_core::email::EmailFolder>> {
    let account = email_account_from_config(&account_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::email::list_folders(&account).map_err(core_err)
    })
    .await
    .map_err(|e| format!("邮箱文件夹后台任务失败: {e}"))?
}

#[tauri::command]
async fn email_messages(
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

#[tauri::command]
async fn email_search(
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
async fn email_message(
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

fn start_email_oauth_callback() -> CmdResult<(String, mpsc::Receiver<CmdResult<EmailOAuthCallback>>)>
{
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
struct EmailOAuthBegin {
    session_id: String,
    authorization_url: String,
    redirect_uri: String,
}

#[tauri::command]
fn email_oauth_begin(state: State<AppState>, account_id: String) -> CmdResult<EmailOAuthBegin> {
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
struct EmailOAuthPoll {
    status: String,
}

#[tauri::command]
async fn email_oauth_poll(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<EmailOAuthPoll> {
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

// ---- the closed loop ---------------------------------------------------------

#[derive(Serialize)]
struct IngestResp {
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
struct ChatDelta {
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
async fn ingest(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    text: String,
    now: Option<String>,
    stream_id: Option<String>,
) -> CmdResult<IngestResp> {
    let now = parse_now(now)?;
    if text.trim().is_empty() {
        return Err("请输入内容".into());
    }
    let orch = state.orch.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        let mut o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
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
fn widget_confirm_preview(
    state: State<AppState>,
    preview_id: String,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetDefinition> {
    let now = parse_now(now)?;
    lock!(state)
        .confirm_widget_preview(&preview_id, now)
        .map_err(core_err)
}

#[tauri::command]
fn widget_discard_preview(state: State<AppState>, preview_id: String) -> CmdResult<()> {
    lock!(state)
        .discard_widget_preview(&preview_id)
        .map_err(core_err)
}

#[tauri::command]
fn widget_defs(state: State<AppState>) -> CmdResult<Vec<solum_core::widget::WidgetDefinition>> {
    lock!(state).widget_definitions().map_err(core_err)
}

#[tauri::command]
fn widget_records(
    state: State<AppState>,
    widget_id: i64,
) -> CmdResult<Vec<solum_core::widget::WidgetRecord>> {
    lock!(state).widget_records(widget_id).map_err(core_err)
}

/// Schema evolution, add-only (设计稿 ⑧). The core rejects a required field;
/// the shell does not pre-filter, so the one rule lives in one place.
#[tauri::command]
fn widget_add_field(
    state: State<AppState>,
    widget_id: i64,
    field: solum_core::widget::WidgetField,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetDefinition> {
    let now = parse_now(now)?;
    lock!(state)
        .add_widget_field(widget_id, &field, now)
        .map_err(core_err)
}

/// 设计稿 ⑦: both event bridges are snapshot copies, never live links.
#[tauri::command]
fn widget_import_events(
    state: State<AppState>,
    widget_id: i64,
    limit: usize,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetImportOutcome> {
    let now = parse_now(now)?;
    lock!(state)
        .import_events_into_widget(widget_id, limit, now)
        .map_err(core_err)
}

#[tauri::command]
fn widget_promote_record(
    state: State<AppState>,
    widget_id: i64,
    record_id: i64,
    now: Option<String>,
) -> CmdResult<solum_core::model::Event> {
    let now = parse_now(now)?;
    lock!(state)
        .promote_widget_record(widget_id, record_id, now)
        .map_err(core_err)
}

/// Record CRUD is safe and local-first: no cloud route or Guard is involved.
#[tauri::command]
fn widget_record_create(
    state: State<AppState>,
    widget_id: i64,
    data: serde_json::Value,
    now: Option<String>,
) -> CmdResult<solum_core::widget::WidgetRecord> {
    let now = parse_now(now)?;
    lock!(state)
        .add_widget_record(widget_id, data, now)
        .map_err(core_err)
}

#[tauri::command]
fn widget_record_update(
    state: State<AppState>,
    widget_id: i64,
    record_id: i64,
    data: serde_json::Value,
) -> CmdResult<solum_core::widget::WidgetRecord> {
    lock!(state)
        .update_widget_record(widget_id, record_id, data)
        .map_err(core_err)
}

#[tauri::command]
fn agenda(state: State<AppState>, now: Option<String>) -> CmdResult<Vec<Event>> {
    let now = parse_now(now)?;
    lock!(state).agenda(now).map_err(core_err)
}

#[tauri::command]
fn all_events(state: State<AppState>) -> CmdResult<Vec<Event>> {
    lock!(state).all_events().map_err(core_err)
}

// ---- notifications -----------------------------------------------------------

#[tauri::command]
fn all_notifications(state: State<AppState>) -> CmdResult<Vec<Notification>> {
    lock!(state).all_notifications().map_err(core_err)
}

#[tauri::command]
fn due(state: State<AppState>, now: Option<String>) -> CmdResult<Vec<Notification>> {
    let now = parse_now(now)?;
    // 与 CLI 的 due 一致：先按注入时钟物化 routine 当日/次日发生（否则模拟
    // 时钟下 routine 永远不会长出提醒——ticker 只认系统时钟）。
    let mut o = lock!(state);
    o.materialize_routines(now).map_err(core_err)?;
    o.due(now).map_err(core_err)
}

#[tauri::command]
fn fire_due(state: State<AppState>, now: Option<String>) -> CmdResult<Vec<Notification>> {
    let now = parse_now(now)?;
    let mut o = lock!(state);
    o.materialize_routines(now).map_err(core_err)?;
    o.fire_due(now).map_err(core_err)
}

#[tauri::command]
fn dismiss(state: State<AppState>, id: i64) -> CmdResult<()> {
    lock!(state).dismiss(id).map_err(core_err)
}

/// Apply a reschedule picked from a GenUI option (or any UI surface): move
/// the event to `start` and re-plan its reminders. Returns a human summary.
#[tauri::command]
fn event_reschedule(
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

/// The cancel confirmation tap: delete the event and its reminders.
#[tauri::command]
fn event_cancel(state: State<AppState>, id: i64) -> CmdResult<String> {
    let ev = lock!(state).cancel_event(id).map_err(core_err)?;
    Ok(format!(
        "已取消「{}」并删除其提醒（事件不可恢复）",
        ev.title
    ))
}

/// Snooze a reminder: ring again `minutes` from `now`. Returns the new fire
/// time (formatted). The Android alarm mirror converges on the next ticker
/// pass like any other pending-set change.
#[tauri::command]
fn snooze(state: State<AppState>, id: i64, minutes: i64, now: Option<String>) -> CmdResult<String> {
    let now = parse_now(now)?;
    let until = lock!(state).snooze(id, minutes, now).map_err(core_err)?;
    Ok(solum_core::model::fmt_ts(&until))
}

// ---- memory ledger (F12) -------------------------------------------------------

#[tauri::command]
fn ledger(state: State<AppState>) -> CmdResult<Vec<MemoryEntry>> {
    lock!(state).ledger().map_err(core_err)
}

/// F12 可编辑：改写一条语义记忆的措辞（recall 立即生效）。
#[tauri::command]
fn fact_update(state: State<AppState>, id: i64, content: String) -> CmdResult<()> {
    lock!(state).update_fact(id, &content).map_err(core_err)
}

/// §4 数据完全归你：把本机全部数据导出为一份 JSON 文件，写在数据库同目录，
/// 返回完整路径。纯只读、纯本地、不上云。
#[tauri::command]
fn export_data(state: State<AppState>, now: Option<String>) -> CmdResult<String> {
    let now = parse_now(now)?;
    let json = lock!(state).export_json(now).map_err(core_err)?;
    let dir = std::path::Path::new(&state.db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // 文件名一律用真实墙钟：模拟时钟只影响导出内容里的数据视角，不该让
    // 磁盘上出现"未来"时间戳的文件（2026-07-17 走查小项）。
    // Second-resolution names collide: two exports in the same second (a
    // double-click, or a manual one landing on a scheduled one) used to have
    // the later silently overwrite the earlier. A backup that can be
    // clobbered by another backup is not a backup — take the first free name.
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let mut file = dir.join(format!("solum-export-{stamp}.json"));
    for n in 2..100 {
        if !file.exists() {
            break;
        }
        file = dir.join(format!("solum-export-{stamp}-{n}.json"));
    }
    // Atomic write so an interrupted export leaves no truncated file that
    // looks like a usable backup.
    solum_core::fsatomic::write_atomic(&file, &json).map_err(|e| e.to_string())?;
    Ok(file.display().to_string())
}

// ---- rules & proactivity -------------------------------------------------------

#[derive(Serialize)]
struct RuleDto {
    kind: String,
    leads: Vec<String>,
    channels: Vec<String>,
}

#[tauri::command]
fn rules(state: State<AppState>) -> CmdResult<Vec<RuleDto>> {
    let o = lock!(state);
    let table = o.rule_table();
    Ok(solum_core::model::EventKind::all()
        .iter()
        .map(|&kind| {
            let r = table.rule(kind);
            RuleDto {
                kind: kind.as_str().to_string(),
                leads: r.lead_times.iter().map(|l| l.label.clone()).collect(),
                channels: r.channels.iter().map(|c| c.as_str().to_string()).collect(),
            }
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleEdit {
    kind: String,
    leads: Vec<String>,
    channels: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleSaveResult {
    replanned_events: usize,
}

/// The rule editor deliberately accepts the compact labels users see (30m,
/// 1h, 3d) and validates them in the core type before anything reaches the
/// persisted rule table. Saving immediately re-plans only future, pending
/// reminders; historical delivery records stay an honest audit trail.
#[tauri::command]
fn rules_save(
    state: State<AppState>,
    rule: RuleEdit,
    now: Option<String>,
) -> CmdResult<RuleSaveResult> {
    let kind = EventKind::from_str(rule.kind.trim()).map_err(core_err)?;
    if rule.leads.is_empty() || rule.leads.len() > 4 {
        return Err("每类事件需要保留 1–4 个提前提醒".into());
    }
    let mut seen_leads = HashSet::new();
    let mut leads = Vec::with_capacity(rule.leads.len());
    for raw in rule.leads {
        let lead = LeadTime::parse(&raw).map_err(core_err)?;
        if lead.minutes > 365 * 24 * 60 {
            return Err("提前量不能超过 365 天".into());
        }
        if !seen_leads.insert(lead.minutes) {
            return Err("同一提前量不能重复".into());
        }
        leads.push(lead);
    }
    let mut seen_channels = HashSet::new();
    let mut channels = Vec::with_capacity(rule.channels.len());
    for raw in rule.channels {
        let channel = match raw.as_str() {
            "push" => Channel::Push,
            "banner" => Channel::Banner,
            _ => return Err("通知渠道只支持 push 或 banner".into()),
        };
        if seen_channels.insert(channel.as_str()) {
            channels.push(channel);
        }
    }
    if channels.is_empty() {
        return Err("至少选择一种通知渠道".into());
    }
    let replanned_events = lock!(state)
        .set_importance_rule(
            ImportanceRule {
                kind,
                lead_times: leads,
                channels,
            },
            parse_now(now)?,
        )
        .map_err(core_err)?;
    Ok(RuleSaveResult { replanned_events })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatTurnInput {
    user: String,
    assistant: String,
}

/// The browser/WebView owns durable, local-only conversation sessions. Before
/// a new cloud reply it restores the selected session's short context here;
/// the core clamps it again so an IPC caller cannot expand the cloud context.
#[tauri::command]
fn chat_context_set(state: State<AppState>, turns: Vec<ChatTurnInput>) -> CmdResult<()> {
    if turns.len() > MAX_HISTORY_TURNS {
        return Err(format!("会话上下文最多保留 {MAX_HISTORY_TURNS} 轮"));
    }
    let mut out = Vec::with_capacity(turns.len());
    for turn in turns {
        let user = turn.user.trim().to_string();
        let assistant = turn.assistant.trim().to_string();
        if user.is_empty() || assistant.is_empty() {
            return Err("会话上下文不能包含空消息".into());
        }
        if user.chars().count() > 4_000 || assistant.chars().count() > 4_000 {
            return Err("单条会话上下文不能超过 4000 个字符".into());
        }
        out.push(ChatTurn { user, assistant });
    }
    lock!(state).replace_chat_history(out);
    Ok(())
}

#[derive(Serialize)]
struct ProactivityDto {
    dimension: String,
    level: String,
}

#[tauri::command]
fn proactivity_get(state: State<AppState>) -> CmdResult<Vec<ProactivityDto>> {
    let o = lock!(state);
    let p = o.proactivity();
    Ok(ProactivityDimension::all()
        .iter()
        .map(|&dim| ProactivityDto {
            dimension: dim.as_str().to_string(),
            level: p.level(dim).as_str().to_string(),
        })
        .collect())
}

#[tauri::command]
fn proactivity_set(state: State<AppState>, dimension: String, level: String) -> CmdResult<()> {
    let dim: ProactivityDimension = dimension.parse().map_err(core_err)?;
    let lvl: ProactivityLevel = level.parse().map_err(core_err)?;
    lock!(state).set_proactivity(dim, lvl).map_err(core_err)
}

// ---- notification privacy (Phase 9) -----------------------------------------

#[tauri::command]
fn notif_cloud_get(state: State<AppState>) -> CmdResult<bool> {
    lock!(state).notif_cloud_enabled().map_err(core_err)
}

#[tauri::command]
fn notif_cloud_set(state: State<AppState>, enabled: bool) -> CmdResult<()> {
    lock!(state)
        .set_notif_cloud_enabled(enabled)
        .map_err(core_err)
}

// ---- notification intelligence (F20) --------------------------------------

#[derive(Serialize)]
struct NotificationIntelligenceStatus {
    config: NotificationIntelligenceConfig,
    priority_rules: Vec<solum_core::classify::NotificationPriorityRule>,
    captures: Vec<solum_core::notification_intelligence::NotificationCaptureRecord>,
    filter_proposals: Vec<solum_core::notification_intelligence::NotificationFilterProposal>,
    action_proposals: Vec<NotificationActionProposalView>,
    pipeline: solum_notif_access::PipelineStatus,
    /// Notifications dropped by the native listener because the spool was full.
    intake_overflow: i64,
    /// Notifications this side could not hand to the store after repeated
    /// attempts; their payloads are parked in `notif-spool/failed/`, not gone.
    intake_undeliverable: i64,
}

/// User-facing projection of one locally installed Android app. Package names
/// remain an implementation detail of the native listener policy and never
/// have to be typed or displayed in the settings UI.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationAppDto {
    name: String,
    package_name: String,
}

#[tauri::command]
fn notif_intelligence_apps(app: tauri::AppHandle) -> CmdResult<Vec<NotificationAppDto>> {
    let apps = app
        .notif_access()
        .installed_apps()
        .map_err(|e| e.to_string())?;
    Ok(apps
        .into_iter()
        .map(|entry| NotificationAppDto {
            name: entry.name,
            package_name: entry.package_name,
        })
        .collect())
}

/// A read-only projection for F12. The proposal stores only the locally
/// resolved id; the current event snapshot is fetched at render time so the
/// confirmation card can honestly show what it would affect without a schema
/// change or making a deleted event break the entire notification panel.
#[derive(Serialize)]
struct NotificationActionProposalView {
    #[serde(flatten)]
    proposal: solum_core::notification_intelligence::NotificationActionProposal,
    event: Option<NotificationActionEventView>,
}

#[derive(Serialize)]
struct NotificationActionEventView {
    title: String,
    start: NaiveDateTime,
}

#[tauri::command]
fn notif_intelligence_status(
    state: State<AppState>,
    app: tauri::AppHandle,
) -> CmdResult<NotificationIntelligenceStatus> {
    let o = lock!(state);
    Ok(NotificationIntelligenceStatus {
        config: o.notification_intelligence_config().map_err(core_err)?,
        priority_rules: o.rule_table().notification_priority_rules().to_vec(),
        captures: o.notification_captures().map_err(core_err)?,
        filter_proposals: o.notification_filter_proposals().map_err(core_err)?,
        action_proposals: o
            .notification_action_proposals()
            .map_err(core_err)?
            .into_iter()
            .map(|proposal| NotificationActionProposalView {
                event: o
                    .event(proposal.event_id)
                    .ok()
                    .map(|event| NotificationActionEventView {
                        title: event.title,
                        start: event.start,
                    }),
                proposal,
            })
            .collect(),
        pipeline: app
            .notif_access()
            .pipeline_status()
            .map_err(|e| e.to_string())?,
        intake_overflow: o.capture_overflow_count().map_err(core_err)?,
        intake_undeliverable: parked_payload_count(&state.db_path),
    })
}

/// The user has seen the intake-loss notice; stop showing it.
#[tauri::command]
fn notif_intelligence_acknowledge_losses(state: State<AppState>) -> CmdResult<()> {
    lock!(state).acknowledge_capture_losses().map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_set_app(
    state: State<AppState>,
    app: tauri::AppHandle,
    package_name: String,
    enabled: bool,
) -> CmdResult<NotificationIntelligenceConfig> {
    // Native policy file first, database second.
    //
    // These are two stores of the same decision, and the listener reads only
    // the file. Committing the database first meant that if the file write
    // failed, the user had revoked an app in the UI while the listener kept
    // reading its notifications and writing them to the inbox — the core would
    // discard them later, but the sensitive text had already been read and put
    // on disk. Writing the file first is safe in *both* directions: a failure
    // now changes nothing anywhere, and if the database commit then fails, the
    // core's whitelist is the stricter of the two and drops the capture.
    let target = {
        let o = lock!(state);
        let mut config = o.notification_intelligence_config().map_err(core_err)?;
        let package = package_name.trim().to_string();
        config.allowed_packages.retain(|p| p != &package);
        if enabled {
            config.allowed_packages.push(package);
        }
        config.allowed_packages.sort();
        config
    };
    write_notification_capture_policy(&state.db_path, &target)?;

    let config = {
        let mut o = lock!(state);
        o.set_notification_app_enabled(&package_name, enabled)
            .map_err(core_err)?;
        o.notification_intelligence_config().map_err(core_err)?
    };
    // Re-write from the committed config so the file always reflects what the
    // database actually holds (validation may have normalized the package).
    write_notification_capture_policy(&state.db_path, &config)?;
    sync_notification_pipeline(&app, &config)?;
    Ok(config)
}

/// The **second** grant: this app may create calendar entries by itself.
/// Deliberately a separate command from `notif_intelligence_set_app`, because
/// it is a separate decision — see `auto_event_packages`.
#[tauri::command]
fn notif_intelligence_set_app_auto_event(
    state: State<AppState>,
    package_name: String,
    enabled: bool,
) -> CmdResult<NotificationIntelligenceConfig> {
    let mut o = lock!(state);
    o.set_notification_app_auto_event(&package_name, enabled)
        .map_err(core_err)?;
    o.notification_intelligence_config().map_err(core_err)
}

/// Per-app counts of events auto-created over the trailing week. The
/// after-the-fact discovery surface the user reviews; it authorizes nothing.
#[tauri::command]
fn notif_intelligence_auto_event_counts(
    state: State<AppState>,
    now: Option<String>,
) -> CmdResult<Vec<(String, i64)>> {
    let now = parse_now(now)?;
    lock!(state).auto_event_counts(now).map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_set_batch_interval(
    state: State<AppState>,
    minutes: u16,
) -> CmdResult<NotificationIntelligenceConfig> {
    let mut config = lock!(state)
        .notification_intelligence_config()
        .map_err(core_err)?;
    config.batch_interval_minutes = minutes;
    lock!(state)
        .set_notification_intelligence_config(config)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_add_priority_rule(
    state: State<AppState>,
    pattern: String,
    package_name: Option<String>,
    matcher: String,
) -> CmdResult<solum_core::classify::NotificationPriorityRule> {
    let matcher = match matcher.as_str() {
        "substring" => solum_core::classify::NotificationMatchKind::Substring,
        "regex" => solum_core::classify::NotificationMatchKind::Regex,
        _ => return Err("匹配方式只支持 substring 或 regex".into()),
    };
    lock!(state)
        .add_notification_priority_rule(pattern, package_name, matcher)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_remove_priority_rule(state: State<AppState>, id: String) -> CmdResult<()> {
    lock!(state)
        .remove_notification_priority_rule(&id)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_set_filter_proposal(
    state: State<AppState>,
    app: tauri::AppHandle,
    id: i64,
    accepted: bool,
) -> CmdResult<()> {
    let config = {
        let o = lock!(state);
        o.set_notification_filter_proposal(id, accepted)
            .map_err(core_err)?;
        o.notification_intelligence_config().map_err(core_err)?
    };
    write_notification_capture_policy(&state.db_path, &config)?;
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn notif_intelligence_set_action_proposal(
    state: State<AppState>,
    id: i64,
    accepted: bool,
    now: Option<String>,
) -> CmdResult<String> {
    lock!(state)
        .resolve_notification_action_proposal(id, accepted, parse_now(now)?)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_remove_filter_rule(
    state: State<AppState>,
    app: tauri::AppHandle,
    id: String,
) -> CmdResult<()> {
    let config = {
        let o = lock!(state);
        o.remove_notification_filter_rule(&id).map_err(core_err)?;
        o.notification_intelligence_config().map_err(core_err)?
    };
    write_notification_capture_policy(&state.db_path, &config)?;
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn notif_intelligence_restore_capture(state: State<AppState>, id: i64) -> CmdResult<()> {
    lock!(state)
        .restore_notification_capture(id)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_promote_capture(
    state: State<AppState>,
    id: i64,
    now: Option<String>,
) -> CmdResult<String> {
    let now = parse_now(now)?;
    lock!(state)
        .promote_notification_capture(id, now)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_process_now(state: State<AppState>, now: Option<String>) -> CmdResult<usize> {
    let now = parse_now(now)?;
    lock!(state)
        .process_notification_batch(now)
        .map_err(core_err)
}

#[tauri::command]
fn notif_pipeline_status(app: tauri::AppHandle) -> CmdResult<solum_notif_access::PipelineStatus> {
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn notif_pipeline_request_battery_optimization(app: tauri::AppHandle) -> CmdResult<()> {
    app.notif_access()
        .request_ignore_battery_optimizations()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn notif_pipeline_open_battery_settings(app: tauri::AppHandle) -> CmdResult<()> {
    app.notif_access()
        .open_battery_settings()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn notif_pipeline_open_background_settings(app: tauri::AppHandle) -> CmdResult<()> {
    app.notif_access()
        .open_app_background_settings()
        .map_err(|e| e.to_string())
}

// ---- HITL guard ---------------------------------------------------------------

#[derive(Serialize)]
struct ToolDto {
    name: String,
    risk: String,
}

#[tauri::command]
fn tools(state: State<AppState>) -> CmdResult<Vec<ToolDto>> {
    let o = lock!(state);
    Ok(o.tool_names()
        .into_iter()
        .map(|name| {
            let risk = o
                .tool_risk(&name)
                .map(|r| r.as_str().to_string())
                .unwrap_or_else(|| "?".into());
            ToolDto { name, risk }
        })
        .collect())
}

#[derive(Serialize)]
struct GuardRunResp {
    ok: bool,
    output: String,
}

/// Attempt a tool with **no** confirmation. For non-safe tools this is refused
/// by the guard and audited — the UI uses it to demonstrate the F7 hard stop.
#[tauri::command]
fn guard_run(
    state: State<AppState>,
    tool: String,
    args: String,
    now: Option<String>,
) -> CmdResult<GuardRunResp> {
    let now = parse_now(now)?;
    match lock!(state).run_tool(&tool, &args, None, now) {
        Ok(output) => Ok(GuardRunResp { ok: true, output }),
        Err(e) => Ok(GuardRunResp {
            ok: false,
            output: e.to_string(),
        }),
    }
}

#[derive(Serialize)]
struct PendingDto {
    pending_id: String,
    tool: String,
    args: String,
    risk: String,
    summary: String,
    preview: String,
}

/// Step 1 of the HITL flow: describe the action, get a pending confirmation.
/// Nothing executes; no token exists yet.
#[tauri::command]
fn guard_request(
    state: State<AppState>,
    tool: String,
    args: String,
    now: Option<String>,
) -> CmdResult<PendingDto> {
    let now = parse_now(now)?;
    let pending = lock!(state)
        .request_confirmation(&tool, &args, now)
        .map_err(core_err)?;
    Ok(PendingDto {
        pending_id: pending.id.clone(),
        tool,
        args,
        risk: pending.request.risk.as_str().to_string(),
        summary: pending.request.summary.clone(),
        preview: pending.request.effect_preview.clone(),
    })
}

/// Steps 2+3, driven by the human clicking 确认 in the dialog: mint the
/// one-time token and immediately spend it on exactly this action.
#[tauri::command]
async fn guard_confirm(
    state: State<'_, AppState>,
    pending_id: String,
    tool: String,
    args: String,
    now: Option<String>,
) -> CmdResult<GuardRunResp> {
    let now = parse_now(now)?;
    let orch = state.orch.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        let token = o.confirm(&pending_id, now).map_err(core_err)?;
        match o.run_tool(&tool, &args, Some(token), now) {
            Ok(output) => Ok(GuardRunResp { ok: true, output }),
            Err(e) => Ok(GuardRunResp {
                ok: false,
                output: e.to_string(),
            }),
        }
    })
    .await
    .map_err(|e| format!("护栏后台执行失败：{e}"))?
}

#[derive(Serialize)]
struct AuditDto {
    id: i64,
    ts: String,
    tool: String,
    risk: String,
    summary: String,
    decision: String,
    detail: String,
}

#[tauri::command]
fn audit(state: State<AppState>) -> CmdResult<Vec<AuditDto>> {
    let rows = lock!(state).audit_log().map_err(core_err)?;
    Ok(rows
        .into_iter()
        .map(|r| AuditDto {
            id: r.id,
            ts: r.ts,
            tool: r.tool,
            risk: r.risk,
            summary: r.summary,
            decision: r.decision,
            detail: r.detail,
        })
        .collect())
}

// ---- behavior journal & check-ins (F3/F4) -----------------------------------------

#[tauri::command]
fn behavior_log(state: State<AppState>) -> CmdResult<Vec<BehaviorEntry>> {
    lock!(state).behavior_log().map_err(core_err)
}

#[derive(Serialize, Clone)]
struct CheckinResp {
    question: String,
    /// F18: tap-to-answer quick options (offline template).
    ui: solum_core::genui::UiEnvelope,
}

fn checkin_resp(question: String) -> CheckinResp {
    let ui = solum_core::genui::checkin_prompt(&question);
    CheckinResp { question, ui }
}

/// Manual check-in probe against the UI clock (the resident ticker does the
/// same automatically on the system clock).
#[tauri::command]
fn checkin_now(state: State<AppState>, now: Option<String>) -> CmdResult<Option<CheckinResp>> {
    let now = parse_now(now)?;
    Ok(lock!(state)
        .checkin_if_due(now)
        .map_err(core_err)?
        .map(checkin_resp))
}

// ---- suggestions (F10) --------------------------------------------------------------

#[tauri::command]
fn suggestions(state: State<AppState>) -> CmdResult<Vec<Suggestion>> {
    lock!(state).suggestions().map_err(core_err)
}

#[tauri::command]
fn suggest_generate(
    state: State<AppState>,
    days: i64,
    now: Option<String>,
) -> CmdResult<Vec<Suggestion>> {
    let now = parse_now(now)?;
    lock!(state)
        .generate_suggestions(now, days.clamp(1, 30))
        .map_err(core_err)
}

/// Returns an optional follow-up message: accepting a habit suggestion
/// auto-creates its routine (D4), accepting a pause suggestion deactivates
/// one — the UI toasts whatever comes back.
#[tauri::command]
fn suggest_set(
    state: State<AppState>,
    id: i64,
    status: String,
    now: Option<String>,
) -> CmdResult<Option<String>> {
    // 注入时钟（AGENTS.md 红线）：采纳习惯建议会创建 routine，created_at 驱动
    // 7 天暂停刹车——用真实时钟会让模拟时钟下的演示/验证行为漂移。
    let now = parse_now(now)?;
    let status: SuggestionStatus = status.parse().map_err(core_err)?;
    lock!(state)
        .set_suggestion_status(id, status, now)
        .map_err(core_err)
}

// ---- routines (F3 完全体, D4) --------------------------------------------------

#[tauri::command]
fn routines(state: State<AppState>) -> CmdResult<Vec<solum_core::routine::Routine>> {
    lock!(state).routines().map_err(core_err)
}

#[tauri::command]
fn routine_set_active(
    state: State<AppState>,
    id: i64,
    active: bool,
    now: Option<String>,
) -> CmdResult<()> {
    let now = parse_now(now)?;
    lock!(state)
        .set_routine_active(id, active, now)
        .map_err(core_err)
}

#[tauri::command]
fn routine_update(
    state: State<AppState>,
    id: i64,
    title: String,
    time_of_day: String,
    now: Option<String>,
) -> CmdResult<()> {
    let now = parse_now(now)?;
    lock!(state)
        .update_routine(id, &title, &time_of_day, now)
        .map_err(core_err)
}

/// D4「一键已完成」：确认某条 routine 今天完成了（落行为日志，按日去重）。
#[tauri::command]
fn routine_done(state: State<AppState>, id: i64, now: Option<String>) -> CmdResult<String> {
    let now = parse_now(now)?;
    lock!(state).confirm_routine(id, now).map_err(core_err)
}

// ---- offline data review (D2) ---------------------------------------------------

#[tauri::command]
fn stats(state: State<AppState>, now: Option<String>) -> CmdResult<String> {
    let now = parse_now(now)?;
    Ok(lock!(state).stats(now).map_err(core_err)?.render())
}

// ---- self-review (F14) ----------------------------------------------------------

#[derive(Serialize)]
struct ReviewResp {
    text: String,
    /// Whether the text was rewritten in the persona voice by the cloud.
    styled: bool,
}

/// `async` for the same reason as [`ingest`]: the persona rewrite is a cloud
/// call and must not stall the UI thread.
#[tauri::command]
async fn review(
    state: State<'_, AppState>,
    days: i64,
    now: Option<String>,
    styled: Option<bool>,
) -> CmdResult<ReviewResp> {
    let now = parse_now(now)?;
    let from = now - Duration::days(days.clamp(1, 365));
    let styled = styled.unwrap_or(false);
    let orch = state.orch.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if styled {
            let (text, styled) = o.review_text(from, now).map_err(core_err)?;
            Ok(ReviewResp { text, styled })
        } else {
            let digest = o.review(from, now).map_err(core_err)?;
            Ok(ReviewResp {
                text: digest.render(),
                styled: false,
            })
        }
    })
    .await
    .map_err(|e| format!("后台任务失败：{e}"))?
}

/// Build today's read-only focus brief for an on-demand UI refresh.
#[tauri::command]
fn daily_brief(state: State<AppState>, now: Option<String>) -> CmdResult<solum_core::brief::Brief> {
    let now = parse_now(now)?;
    lock!(state).daily_brief(now).map_err(core_err)
}

// ---- persona (F9 v1 / F15) ---------------------------------------------------

#[derive(Serialize)]
struct PersonaDto {
    active: Option<PersonaProfile>,
    versions: Vec<PersonaProfile>,
}

#[tauri::command]
fn persona_get(state: State<AppState>) -> CmdResult<PersonaDto> {
    let o = lock!(state);
    Ok(PersonaDto {
        active: o.persona().cloned(),
        versions: o.persona_versions().map_err(core_err)?,
    })
}

#[tauri::command]
fn persona_set(
    state: State<AppState>,
    nickname: Option<String>,
    tone: Option<String>,
    catchphrases: Vec<String>,
    style_notes: Option<String>,
    note: Option<String>,
    now: Option<String>,
) -> CmdResult<PersonaProfile> {
    let now = parse_now(now)?;
    let draft = PersonaDraft {
        nickname,
        tone: tone.unwrap_or_default(),
        catchphrases,
        style_notes,
    };
    lock!(state).set_persona(draft, note, now).map_err(core_err)
}

#[derive(Serialize)]
struct ImportPreviewResp {
    report: solum_core::persona_import::ImportReport,
    /// F18: the draft as an in-place edit form (submit → persona_import_save).
    ui: solum_core::genui::UiEnvelope,
}

/// F9 §3.4: run the strictly-local chat-log extraction. Pure preview — the
/// raw log stays in the frontend, nothing is stored or sent to the cloud.
#[tauri::command]
fn persona_import_preview(
    state: State<AppState>,
    raw: String,
    me: String,
) -> CmdResult<ImportPreviewResp> {
    let report = lock!(state)
        .preview_persona_import(&raw, &me)
        .map_err(core_err)?;
    let ui = solum_core::genui::persona_draft_form(&report.suggested, Some("从聊天记录导入"));
    Ok(ImportPreviewResp { report, ui })
}

/// Save the user-confirmed (possibly edited) import draft as a new persona
/// version with `source = "import"`.
#[tauri::command]
fn persona_import_save(
    state: State<AppState>,
    nickname: Option<String>,
    tone: Option<String>,
    catchphrases: Vec<String>,
    style_notes: Option<String>,
    note: Option<String>,
    now: Option<String>,
) -> CmdResult<PersonaProfile> {
    let now = parse_now(now)?;
    let draft = PersonaDraft {
        nickname,
        tone: tone.unwrap_or_default(),
        catchphrases,
        style_notes,
    };
    lock!(state)
        .import_persona(draft, note, now)
        .map_err(core_err)
}

#[tauri::command]
fn persona_rollback(state: State<AppState>, version: i64) -> CmdResult<PersonaProfile> {
    lock!(state).rollback_persona(version).map_err(core_err)
}

// ---- notification-listener access (F1 companion) ---------------------------------

/// Whether Solum's notification-capture listener is enabled in OS settings.
/// Always `true` on desktop (no such permission exists there) so the
/// frontend only ever shows the nag banner where it's actually actionable.
#[tauri::command]
fn notif_access_status(app: tauri::AppHandle) -> CmdResult<bool> {
    app.notif_access().is_enabled().map_err(|e| e.to_string())
}

/// Jump to the notification-listener settings page (Android: Settings →
/// Notifications → Notification access). No runtime-request dialog exists
/// for this permission — the user must flip the toggle themselves.
#[tauri::command]
fn notif_access_open_settings(app: tauri::AppHandle) -> CmdResult<()> {
    app.notif_access()
        .open_settings()
        .map_err(|e| e.to_string())
}

// ---- wearable health samples (F5, Phase 4) -----------------------------------------

#[derive(Serialize)]
struct HealthStatusDto {
    /// Whether the Health Connect SDK/app exists on this device at all
    /// (always `false` on desktop — it's an Android-only platform service).
    available: bool,
    granted: bool,
}

#[tauri::command]
fn health_status(app: tauri::AppHandle) -> CmdResult<HealthStatusDto> {
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
fn health_request(app: tauri::AppHandle) -> CmdResult<bool> {
    app.health_connect()
        .request_permissions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn health_samples(state: State<AppState>) -> CmdResult<Vec<HealthSample>> {
    lock!(state).health_samples().map_err(core_err)
}

/// ISO-8601 instant (Health Connect's `Instant.toString()`, always UTC) to
/// the local wall-clock `NaiveDateTime` the store expects.
fn parse_instant_local(s: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Local).naive_local())
}

fn convert_raw_sample(r: solum_health_connect::RawSample) -> Option<HealthSample> {
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
/// fallback: `SOLUM_SYNC_CONFIG` if set (mobile setup points it at app-data),
/// else `./solum-sync.json` (desktop cwd, adopted into app-data).
fn sync_config_file() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SOLUM_SYNC_CONFIG") {
        return p.into();
    }
    solum_core::paths::resolve_with_adoption("solum-sync.json")
}

/// Everything the settings form needs — never the password, and not the
/// derived token/key either (those are recomputed from username+password at
/// load time, the UI has no business holding them).
#[derive(Serialize)]
struct SyncConfigSettings {
    configured: bool,
    /// "credentials" (username+password, current recommended shape) |
    /// "raw" (legacy `{url,token,key}` file — still works, not shown as
    /// editable fields here since there's no username/password to show) |
    /// "none".
    format: &'static str,
    path: String,
    url: String,
    username: String,
    device_id: String,
}

#[tauri::command]
fn sync_config_get(state: State<AppState>) -> CmdResult<SyncConfigSettings> {
    let path = sync_config_file();
    let path_str = path.to_string_lossy().into_owned();
    let device_id = lock!(state).sync_device_id().map_err(core_err)?;
    let raw = std::fs::read_to_string(&path).ok();
    let parsed = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let Some(v) = parsed else {
        return Ok(SyncConfigSettings {
            configured: false,
            format: "none",
            path: path_str,
            url: String::new(),
            username: String::new(),
            device_id,
        });
    };
    let url = v
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    if let Some(username) = v.get("username").and_then(|x| x.as_str()) {
        Ok(SyncConfigSettings {
            configured: true,
            format: "credentials",
            path: path_str,
            url,
            username: username.to_string(),
            device_id,
        })
    } else if v.get("token").is_some() && v.get("key").is_some() {
        Ok(SyncConfigSettings {
            configured: true,
            format: "raw",
            path: path_str,
            url,
            username: String::new(),
            device_id,
        })
    } else {
        Ok(SyncConfigSettings {
            configured: false,
            format: "none",
            path: path_str,
            url: String::new(),
            username: String::new(),
            device_id,
        })
    }
}

#[derive(Deserialize)]
struct SyncSaveArgs {
    url: String,
    username: String,
    /// Empty → reuse the password already in the stored file (if that file
    /// is itself the credentials shape) — same convenience as `LlmSaveArgs`'s
    /// `api_key`, so editing the URL doesn't force retyping the password.
    #[serde(default)]
    password: String,
}

/// Persist `{url,username,password}` and return a status line for a toast.
/// The relay is untouched by this — it still only ever compares whatever
/// static token it was configured with; this just has to derive the *same*
/// token/key on every device via `derive_credentials`, which happens at
/// `SyncConfig::load()` time, not here.
#[tauri::command]
fn sync_config_save(state: State<AppState>, cfg: SyncSaveArgs) -> CmdResult<String> {
    let url = solum_core::net::validate_endpoint(&cfg.url, "同步服务器 url").map_err(core_err)?;
    let username = cfg.username.trim().to_string();
    if username.is_empty() {
        return Err("请填写用户名".into());
    }
    let password = cfg.password.trim().to_string();
    let password = if password.is_empty() {
        // Only a file already in the credentials shape has a password to
        // reuse — a legacy raw `{token,key}` file has none, so this stays
        // empty and the check below asks the user to type one.
        std::fs::read_to_string(sync_config_file())
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("password").and_then(|p| p.as_str()).map(String::from))
            .unwrap_or_default()
    } else {
        password
    };
    if password.is_empty() {
        return Err("请填写密码（没有已保存的密码可沿用）".into());
    }

    let json = serde_json::to_string_pretty(&serde_json::json!({
        "url": url,
        "username": username,
        "password": password,
    }))
    .map_err(|e| e.to_string())?;
    let path = sync_config_file();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    solum_core::fsatomic::write_atomic(&path, &json).map_err(|e| e.to_string())?;

    let device_id = lock!(state).sync_device_id().map_err(core_err)?;
    Ok(format!("已保存（本机设备标识：{device_id}）"))
}

#[derive(Serialize)]
struct SyncStatusDto {
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
}

#[tauri::command]
fn sync_status(state: State<AppState>) -> CmdResult<SyncStatusDto> {
    let cfg = solum_core::sync::SyncConfig::load();
    let store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let (held, dropped) = store.bad_blob_stats().map_err(core_err)?;
    let history_gap = store
        .sync_state(solum_core::sync::HISTORY_GAP_KEY)
        .map_err(core_err)?;
    drop(store);
    Ok(SyncStatusDto {
        configured: cfg.is_some(),
        summary: cfg.map(|c| c.masked_summary()),
        device_id: lock!(state).sync_device_id().map_err(core_err)?,
        history_gap,
        bad_blobs_held: held,
        bad_blobs_dropped: dropped,
    })
}

/// Clear the sticky history-gap marker after the user has re-seeded from a
/// peer. Deliberately explicit: nothing clears it automatically, because
/// nothing else can know the data was recovered.
#[tauri::command]
fn sync_gap_acknowledge(state: State<AppState>) -> CmdResult<()> {
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
fn sync_round(state: &AppState) -> CmdResult<solum_core::sync::SyncOutcome> {
    let Some(cfg) = solum_core::sync::SyncConfig::load() else {
        return Err("同步未配置：设置 SOLUM_SYNC_URL/TOKEN/KEY 或 solum-sync.json".into());
    };
    let transport = solum_core::sync::HttpTransport::new(&cfg).map_err(core_err)?;
    let outcome = {
        let store = state
            .sync_store
            .lock()
            .map_err(|_| "sync store lock poisoned".to_string())?;
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
fn sync_now(state: State<AppState>) -> CmdResult<solum_core::sync::SyncOutcome> {
    sync_round(&state)
}

// ---- resident ticker --------------------------------------------------------------

fn system_now() -> NaiveDateTime {
    let now = Local::now().naive_local();
    now.with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(now)
}

#[cfg_attr(target_os = "android", allow(dead_code))]
fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    // OS notification failure must never break the loop (F16 spirit): the
    // in-window surfaces still show everything.
    let _ = app.notification().builder().title(title).body(body).show();
}

/// F20: drain the notification-capture spool written by the Android listener.
/// The listener has already applied the app whitelist; core repeats it as the
/// authority, records every accepted outcome for F12, and immediately handles
/// only the urgent lane. Ordinary items stay queued for the batch cadence.
///
/// **One notification, one file.** The listener writes `<stem>.tmp`, fsyncs it,
/// and renames it to `<stem>.json`; this side reads whole `.json` files and
/// deletes each once the core has accepted it.
///
/// The previous design had both processes sharing one appended JSONL, with this
/// side renaming it aside to "claim" it. That cannot be made safe: an append
/// already in flight keeps writing to the old inode — which we then delete —
/// and a partially-flushed append leaves a half line. Both lose notifications
/// with no trace. A spool sidesteps the coordination problem rather than trying
/// to win it: nothing is ever written and read through the same path.
fn drain_capture_inbox(
    orch: &mut solum_core::Orchestrator,
    db_path: &str,
    now: NaiveDateTime,
) -> Vec<String> {
    let Some(dir) = std::path::Path::new(db_path).parent() else {
        return Vec::new();
    };
    let mut captured = Vec::new();

    drain_overflow_counter(orch, dir);

    // One-time carry-over from the shared-JSONL era, so an upgrade does not
    // strand whatever was pending at the moment the app was replaced.
    // Same rule as the spool: only delete once the core accepted every line.
    for legacy in ["notif-inbox.jsonl", "notif-inbox.processing.jsonl"] {
        let path = dir.join(legacy);
        if path.is_file() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if content.trim().is_empty() {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            let batch = ingest_inbox_lines(orch, &content, now);
            captured.extend(batch.outcomes);
            if batch.failed == 0 {
                let _ = std::fs::remove_file(&path);
            }
            // Otherwise leave it: the next tick retries. Duplicates are
            // absorbed by the core's content hash; a deletion is not.
        }
    }

    let spool = dir.join("notif-spool");
    let Ok(entries) = std::fs::read_dir(&spool) else {
        return captured;
    };
    // Oldest first, so capture order matches arrival order (the file stem
    // starts with the notification's post time).
    let mut files: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();

    for path in files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.trim().is_empty() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let batch = ingest_inbox_lines(orch, &content, now);
        captured.extend(batch.outcomes);

        if batch.failed == 0 {
            // Delete only after the core has actually taken every line. Dying
            // before this costs a duplicate next tick, which content-hash dedup
            // absorbs — the deliberate direction to fail in.
            let _ = std::fs::remove_file(&path);
            continue;
        }

        // Retry, but bounded. Leaving a permanently-failing file in place
        // forever is the poison-pill shape: it would be retried every tick and,
        // because the loop is ordered, keep the whole queue company. Park it
        // after a few attempts — kept, counted, and out of the way.
        let attempt = spool_attempt(&path);
        if attempt < MAX_SPOOL_ATTEMPTS {
            let next =
                path.with_file_name(format!("{}.try{}.json", spool_base(&path), attempt + 1));
            let _ = std::fs::rename(&path, next);
        } else {
            let parked = spool.join("failed");
            let _ = std::fs::create_dir_all(&parked);
            let name = path.file_name().unwrap_or_default().to_os_string();
            // No counter to bump: the parked file *is* the record. The status
            // surface counts what is in `failed/` at the time it is asked, so
            // there is no window between parking and counting to crash in.
            let _ = std::fs::rename(&path, parked.join(&name));
        }
    }
    captured
}

/// Fold the listener's "spool was full, dropped it" tally into durable state.
///
/// Each drop is its own immutable marker file. There is deliberately no shared
/// mutable counter: the writer lives in another process, and every version of
/// a shared counter raced it —
///
///  - *read total → write total+1* races this side's read directly;
///  - *append one byte, claim by rename* still loses a write whose descriptor
///    was opened **before** the rename but whose `write` lands **after** this
///    side read the length and unlinked the file. That byte goes to an
///    orphaned inode: not in the claimed file, not in the new live file.
///
/// Markers have no shared mutable state to race over. `collect` then
/// `record_and_remove` deletes **exactly the paths it counted**, so a marker
/// created at any point during the drain is simply picked up next pass — it is
/// never counted twice and never dropped.
fn drain_overflow_counter(orch: &mut solum_core::Orchestrator, dir: &std::path::Path) {
    migrate_legacy_overflow_counter(orch, dir);
    let markers = collect_overflow_markers(dir);
    record_and_remove_overflow_markers(orch, &markers);
}

/// Marker paths currently present, oldest first.
fn collect_overflow_markers(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir.join("notif-overflow")) else {
        return Vec::new();
    };
    let mut out: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mark"))
        .collect();
    out.sort();
    out
}

/// Count these markers (once each, by name) and then delete **exactly the
/// files that were counted** — no more.
///
/// Two rules, both load-bearing:
///  - deleting by re-listing the directory would erase a marker created between
///    the census and the delete, without ever counting it;
///  - a marker whose name cannot be counted must not be deleted either. The
///    names are ASCII by construction, but pairing path with name rather than
///    building two lists means the code cannot drift into deleting something it
///    did not record — which is the exact failure this whole design exists to
///    prevent.
fn record_and_remove_overflow_markers(
    orch: &mut solum_core::Orchestrator,
    markers: &[std::path::PathBuf],
) {
    let counted: Vec<(&std::path::PathBuf, String)> = markers
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| (p, n.to_string()))
        })
        .collect();
    if counted.is_empty() {
        return;
    }
    let names: Vec<String> = counted.iter().map(|(_, n)| n.clone()).collect();
    if orch.note_capture_overflow(&names).is_err() {
        // Recording failed — leave every marker in place and retry next tick.
        return;
    }
    // Crashing here is harmless: the receipts written above mean a re-scan of
    // these same markers adds nothing to the total.
    //
    // A receipt is retired only by the disappearance of the file it names, and
    // only for the files that actually went away. A marker whose deletion fails
    // keeps its receipt for as long as it stays on disk, however long that is.
    let mut deleted: Vec<String> = Vec::with_capacity(counted.len());
    for (path, name) in counted {
        if remove_counted_file(path) {
            deleted.push(name);
        }
    }
    let _ = orch.release_capture_overflow(&deleted);
}

/// Delete a file that has been counted; report whether it is now absent.
///
/// Written as a statement rather than a predicate because it deletes: pairing
/// the removal with its own outcome is what keeps a receipt from being retired
/// for a file that is still sitting there.
///
/// "Already not there" counts as absent — what matters is that no later scan
/// can find it, not who removed it. Any other error is a failure, and the
/// caller keeps the receipt that stops the survivor from being counted again.
fn remove_counted_file(path: &std::path::Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Carry over counts written by the previous (shared-counter) implementation so
/// an upgrade does not silently lose them. Both the live file and an
/// interrupted claim held one byte per drop.
fn migrate_legacy_overflow_counter(orch: &mut solum_core::Orchestrator, dir: &std::path::Path) {
    for legacy in ["notif-spool.overflow", "notif-spool.overflow.taking"] {
        let path = dir.join(legacy);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let n = meta.len() as usize;
        if n == 0 {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        // Same crash window as the markers, so the same guard: one receipt per
        // counted drop, named after the legacy file and the position within it.
        // The new code never recreates these files, so the names cannot collide
        // with a later, different tally.
        let names: Vec<String> = (0..n).map(|i| format!("{legacy}#{i}")).collect();
        if orch.note_capture_overflow(&names).is_ok() && remove_counted_file(&path) {
            // Same rule as the markers: the receipts are retired by the file
            // being gone, never by age. While it survives a failed delete it
            // keeps being re-counted as the same tally and adds nothing.
            let _ = orch.release_capture_overflow(&names);
        }
    }
}

/// How many undeliverable payloads are parked right now.
///
/// A live census rather than a counter. These files are retained indefinitely,
/// so "how many are there" is answerable by looking — and an answer derived
/// from the thing itself cannot drift from it, which a separately-maintained
/// tally provably did (missed on a crash between parking and counting;
/// double-counted once the idempotency receipt aged out).
fn parked_payload_count(db_path: &str) -> i64 {
    let Some(dir) = std::path::Path::new(db_path).parent() else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(dir.join("notif-spool").join("failed")) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count() as i64
}

/// Attempts already spent on a spool file, encoded in its name
/// (`<stem>.try2.json`). Filesystem-encoded rather than in-memory so the count
/// survives the restart that a transient failure often prompts.
fn spool_attempt(path: &std::path::Path) -> u32 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.rsplit_once(".try"))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(1)
}

/// The stem with any `.tryN` suffix removed.
fn spool_base(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("capture");
    match stem.rsplit_once(".try") {
        Some((base, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            base.to_string()
        }
        _ => stem.to_string(),
    }
}

/// How many times a spool file is retried before being parked in `failed/`.
const MAX_SPOOL_ATTEMPTS: u32 = 3;

/// Hand one batch of inbox lines to the core, returning the per-capture
/// outcome strings. Split out so a leftover claim file and a freshly claimed
/// inbox go through exactly the same path.
fn ingest_inbox_lines(
    orch: &mut solum_core::Orchestrator,
    content: &str,
    now: NaiveDateTime,
) -> IngestBatch {
    let mut batch = IngestBatch::default();
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            // Unparseable line: a decision, not a failure. Retrying cannot help.
            continue;
        };
        let title = v["title"].as_str().unwrap_or("").trim();
        let text = v["text"].as_str().unwrap_or("").trim();
        let pkg = v["pkg"].as_str().unwrap_or("?");
        if title.is_empty() && text.is_empty() {
            continue;
        }
        let received_at = v["ts"]
            .as_i64()
            .and_then(|millis| Local.timestamp_millis_opt(millis).single())
            .map(|time| time.naive_local())
            .unwrap_or(now);
        // `Ok(None)` is the core *deciding* not to keep this one (package not
        // whitelisted, empty text) — that is a completed outcome. `Err` is the
        // core failing to record something it should have (database locked,
        // disk full), and the two must not be conflated: the old code matched
        // only `Ok(Some(..))`, so an error silently produced no outcome and the
        // caller then deleted the file anyway. That is a lost notification, and
        // the comment above the delete claimed the opposite.
        let outcome = match orch.capture_notification(NotificationCapture {
            package_name: pkg.to_string(),
            title: title.to_string(),
            body: text.to_string(),
            received_at,
        }) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[notif] 捕获落库失败，保留待重试: {e}");
                batch.failed += 1;
                continue;
            }
        };
        if let Some(record) = outcome {
            let mut state = record.state;
            if record.lane == CaptureLane::Urgent && record.state == CaptureState::Queued {
                state = orch
                    .process_urgent_notification(record.id.unwrap_or(0), now)
                    .map(|updated| updated.state)
                    .unwrap_or(CaptureState::NeedsReview);
            }
            let outcome = match state {
                CaptureState::Queued => "已入普通批量队列",
                CaptureState::EventCreated => "已创建日程",
                CaptureState::Filtered => "已按规则过滤（可在台账回看）",
                CaptureState::Deduplicated => "已判重（可在台账回看）",
                CaptureState::NeedsReview => "待你回看",
                CaptureState::Resolved => "已处理（可在台账回看）",
            };
            batch
                .outcomes
                .push(format!("通知·{}：{}", record.package_name, outcome));
        }
    }
    batch
}

/// Result of handing one spool file's lines to the core.
///
/// `failed` is what decides whether the file may be deleted: a line the core
/// could not record has to stay on disk, or the notification is gone.
#[derive(Default)]
struct IngestBatch {
    outcomes: Vec<String>,
    failed: usize,
}

/// The resident heartbeat: every minute, on the **system clock**, deliver due
/// reminders, ask a check-in when due, and auto-generate suggestions. The
/// UI's simulated clock never drives this — it remains a demo device.
fn ticker(app: tauri::AppHandle) {
    // Small initial delay so the window is up before the first possible ask.
    std::thread::sleep(std::time::Duration::from_secs(5));
    // Auto-sync cadence: every 5th minute tick, when sync is configured.
    let mut tick_count: u64 = 0;
    loop {
        let now = system_now();
        let state = app.state::<AppState>();
        let should_emit_daily_brief = state
            .last_daily_brief_date
            .lock()
            .map(|last| daily_brief_is_due(*last, now.date()))
            .unwrap_or(false);
        let (fired_msgs, checkin, fresh, captured, notification_batch, daily_brief) =
            match state.orch.lock() {
                Ok(mut o) => {
                    let captured = drain_capture_inbox(&mut o, &state.db_path, now);
                    let notification_batch = o
                        .notification_intelligence_config()
                        .ok()
                        .filter(|config| {
                            now.minute()
                                .is_multiple_of(u32::from(config.batch_interval_minutes))
                        })
                        .and_then(|_| o.process_notification_batch(now).ok())
                        .unwrap_or(0);
                    // D4: materialize upcoming routine occurrences (today +
                    // tomorrow) before delivery — they then ride the normal
                    // reminder pipeline, including the Android alarm mirror below.
                    let _ = o.materialize_routines(now);
                    let fired = o.fire_due(now).unwrap_or_default();
                    let fired_msgs: Vec<String> = fired
                        .iter()
                        .map(|n| {
                            let title = o
                                .event(n.event_id)
                                .map(|e| e.title)
                                .unwrap_or_else(|_| format!("event#{}", n.event_id));
                            format!("「{title}」（提前{}）", n.lead_label)
                        })
                        .collect();
                    let checkin = o.checkin_if_due(now).ok().flatten();
                    let fresh = o.auto_generate_suggestions(now).unwrap_or_default();
                    let daily_brief = should_emit_daily_brief.then(|| o.daily_brief(now));
                    (
                        fired_msgs,
                        checkin,
                        fresh,
                        captured,
                        notification_batch,
                        daily_brief,
                    )
                }
                Err(_) => (Vec::new(), None, Vec::new(), Vec::new(), 0, None),
            };

        // OS notifications are reserved for event reminders (user decision
        // 2026-07-14): check-ins / suggestions / captures are informational
        // and stay on in-window surfaces only. On Android even the reminder
        // toast is owned by the AlarmManager receiver (solum-alarm) — the
        // ticker posting too would double-fire — so `notify` runs for
        // reminders on non-Android platforms only.
        if !captured.is_empty() {
            let _ = app.emit("solum-captured", &captured);
        }
        if notification_batch > 0 {
            let _ = app.emit("solum-notification-processed", notification_batch);
        }

        #[cfg(not(target_os = "android"))]
        for m in &fired_msgs {
            notify(&app, "Solum 提醒", m);
        }
        if !fired_msgs.is_empty() {
            let _ = app.emit("solum-fired", &fired_msgs);
        }
        if let Some(q) = &checkin {
            // F18: the payload carries the quick-answer envelope alongside the
            // question, so the chat surface renders tap-to-answer options.
            let _ = app.emit("solum-checkin", checkin_resp(q.clone()));
        }
        if !fresh.is_empty() {
            let texts: Vec<String> = fresh.iter().map(|s| s.text.clone()).collect();
            // F18: accept/dismiss right in the conversation flow.
            let ui = solum_core::genui::suggestions_prompt(&fresh);
            let _ = app.emit(
                "solum-suggestions",
                serde_json::json!({ "texts": texts, "ui": ui }),
            );
        }
        if let Some(Ok(brief)) = daily_brief {
            if let Ok(mut last) = state.last_daily_brief_date.lock() {
                *last = Some(now.date());
            }
            if let Some(ui) = solum_core::genui::daily_brief_prompt(&brief) {
                let _ = app.emit("solum-daily-brief", ui);
            }
        }
        let _ = app.emit("solum-tick", now.format("%Y-%m-%dT%H:%M:%S").to_string());

        // F2/F16: mirror the pending reminder set into OS-level alarms so
        // Android still delivers when the app process is dead. Cheap
        // signature check every tick; crosses the plugin bridge only when
        // the set changed (covers ingest / dismiss / forget / sync / fired
        // within ≤60s, including the very first tick after launch).
        resync_alarms(&app);

        // F5 wearable poll: same ~5-minute cadence as sync, not every tick —
        // this is a real cross-process call (Health Connect), not a local
        // file read. Desktop's plugin stub is always unavailable, so this is
        // a cheap no-op there (see solum-health-connect::desktop).
        if tick_count.is_multiple_of(5) {
            let hc = app.health_connect();
            if matches!(hc.is_available(), Ok(true)) && matches!(hc.has_permissions(), Ok(true)) {
                let since_ms = state.health_since_ms.lock().map(|g| *g).unwrap_or(0);
                // Advance the cursor only after the platform read *and* local
                // persistence succeed. Moving it on an IPC failure would
                // permanently skip that health-data window on the next poll.
                let poll_started_ms = Local::now().timestamp_millis();
                match hc.read_recent(since_ms) {
                    Ok(raw) => {
                        let samples: Vec<HealthSample> =
                            raw.into_iter().filter_map(convert_raw_sample).collect();
                        let stored = match state.orch.lock() {
                            Ok(mut o) => o.record_health_samples(samples, now),
                            Err(_) => {
                                Err(solum_core::CoreError::Invalid("state lock poisoned".into()))
                            }
                        };
                        match stored {
                            Ok(stored) => {
                                if stored > 0 {
                                    let _ = app.emit("solum-health", stored);
                                }
                                if let Ok(mut g) = state.health_since_ms.lock() {
                                    *g = poll_started_ms;
                                }
                                // Durable too, so a restart resumes here
                                // instead of re-reading (and re-counting) the
                                // last six hours.
                                if let Ok(store) = state.sync_store.lock() {
                                    let _ = store.set_sync_state(
                                        HEALTH_SINCE_KEY,
                                        &poll_started_ms.to_string(),
                                    );
                                }
                            }
                            Err(e) => eprintln!("Health Connect 样本落库失败: {e}"),
                        }
                    }
                    Err(e) => eprintln!("Health Connect 读取失败，将在下次轮询重试: {e}"),
                }
            }
        }

        // Background sync (F17): quiet best-effort — failures only log, the
        // local-first store keeps working offline (F16), next tick retries.
        if tick_count.is_multiple_of(5) {
            match sync_round(&state) {
                Ok(r) => {
                    if r.applied > 0 {
                        let _ = app.emit("solum-synced", r.applied);
                    }
                    if r.history_gap {
                        eprintln!(
                            "[sync] 同步游标落在中继留存窗口之外，本轮不是完整同步；                             需从另一台设备导出并导入以重新对齐"
                        );
                        let _ = app.emit("solum-sync-gap", true);
                    }
                    if r.bad_blobs > 0 {
                        eprintln!(
                            "[sync] {} 个批次无法解密并已暂存（各设备 SOLUM_SYNC_KEY 是否一致？）",
                            r.bad_blobs
                        );
                    }
                }
                Err(e) => eprintln!("[sync] 后台同步失败（下轮重试）: {e}"),
            }
        }
        tick_count += 1;

        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

/// Mirror the pending reminder set into OS-level alarms (solum-alarm; Android
/// only — the desktop stub reports unavailable and this returns immediately).
/// The OS alarm registry is treated as a disposable projection of the
/// notifications table: we always push the whole set and let the Kotlin side
/// cancel + re-arm, keyed by notification row id.
fn resync_alarms(app: &tauri::AppHandle) {
    use chrono::TimeZone;
    use std::hash::{Hash, Hasher};

    let alarm = app.alarm();
    if !matches!(alarm.is_available(), Ok(true)) {
        return;
    }
    let state = app.state::<AppState>();
    let specs: Vec<solum_alarm::AlarmSpec> = match state.orch.lock() {
        Ok(o) => {
            let Ok(all) = o.all_notifications() else {
                return;
            };
            let mut v: Vec<solum_alarm::AlarmSpec> = all
                .into_iter()
                .filter(|n| n.status == solum_core::model::NotificationStatus::Pending)
                .filter_map(|n| {
                    let id = n.id?;
                    // Wall-clock → epoch instant; a nonexistent local time
                    // (DST gap) simply skips this round and self-heals when
                    // the clock moves on.
                    let at_ms = Local
                        .from_local_datetime(&n.fire_at)
                        .earliest()?
                        .timestamp_millis();
                    let title = o
                        .event(n.event_id)
                        .map(|e| e.title)
                        .unwrap_or_else(|_| format!("event#{}", n.event_id));
                    Some(solum_alarm::AlarmSpec {
                        id,
                        at_ms,
                        title: "Solum 提醒".into(),
                        body: format!("「{title}」（提前{}）", n.lead_label),
                    })
                })
                .collect();
            v.sort_by_key(|s| s.at_ms);
            v.truncate(64); // enough lookahead; the set refreshes as reminders fire
            v
        }
        Err(_) => return,
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in &specs {
        // Title and body belong in the signature, not just id + time: an event
        // renamed without moving produced an identical signature, so the OS
        // alarm was never re-armed and went on announcing the old name.
        (s.id, s.at_ms, &s.title, &s.body).hash(&mut h);
    }
    let sig = h.finish().max(1); // 0 stays the "never pushed" sentinel
    if state.alarm_sig.lock().map(|g| *g == sig).unwrap_or(true) {
        return;
    }
    match alarm.sync(specs) {
        Ok(_exact) => {
            if let Ok(mut g) = state.alarm_sig.lock() {
                *g = sig;
            }
        }
        Err(e) => eprintln!("[alarm] 系统闹钟同步失败（下轮重试）: {e}"),
    }
}

// ---- entry point ------------------------------------------------------------------

/// Where the SQLite store lives. `SOLUM_DB` always wins; the desktop default stays
/// "solum.sqlite in the cwd" (shared with solum-cli, see MISC.md), while mobile has
/// no meaningful cwd and uses the platform app-data directory instead.
///
/// Both branches first adopt a `pa.sqlite` left by the 2026-07-20 rename, so an
/// existing store keeps its data rather than starting empty. On Android this is
/// effectively a no-op — the identifier changed too, so the old install's data
/// is in another package's private storage (see `adopt_legacy_db`).
fn resolve_db_path(#[cfg_attr(desktop, allow(unused_variables))] app: &tauri::App) -> String {
    if let Ok(p) = std::env::var("SOLUM_DB") {
        return p;
    }
    #[cfg(mobile)]
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let current = dir.join("solum.sqlite");
        report_legacy_adoption(&dir.join("pa.sqlite"), &current);
        return current.to_string_lossy().into_owned();
    }
    // Desktop: app-data, not the working directory.
    //
    // Two adoptions can apply, in order: the pre-rename `pa.sqlite` (2026-07-20)
    // and the pre-app-data `./solum.sqlite` (2026-07-21). Run the rename
    // adoption first *in the old location*, so a user who skipped a release
    // still ends up with one store rather than two half-populated ones.
    //
    // `solum-cli` calls the same resolver, so the "desktop and CLI share one
    // database" property the cwd default existed for is preserved — it just no
    // longer depends on the launch directory.
    report_legacy_adoption(
        std::path::Path::new("pa.sqlite"),
        std::path::Path::new("solum.sqlite"),
    );
    solum_core::paths::resolve_with_adoption("solum.sqlite")
        .to_string_lossy()
        .into_owned()
}

/// Adoption failing must not stop the app from starting — a fresh store is a
/// worse outcome than a loud log, but a dead shell is worse than both. Log and
/// carry on; the untouched legacy file is still on disk either way.
fn report_legacy_adoption(legacy: &std::path::Path, current: &std::path::Path) {
    match solum_core::store::adopt_legacy_db(legacy, current) {
        Ok(true) => eprintln!("[db] 已接管改名前的数据库 {legacy:?} → {current:?}"),
        Ok(false) => {}
        Err(e) => eprintln!("[db] 改名前数据库接管失败（将使用 {current:?}）: {e}"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(solum_notif_access::init())
        .plugin(solum_health_connect::init())
        .plugin(solum_alarm::init())
        .setup(|app| {
            let db_path = resolve_db_path(app);
            // Mobile has no meaningful cwd, so the default "./solum-llm.json"
            // lookup can never hit: point the config path at the app-data dir
            // (next to the SQLite file) instead. Push the gitignored file there
            // once (debug builds: `adb push` + `run-as cp`). Desktop behavior
            // is unchanged; an explicit SOLUM_LLM_CONFIG always wins.
            #[cfg(mobile)]
            if std::env::var("SOLUM_LLM_CONFIG").is_err() {
                if let Some(dir) = std::path::Path::new(&db_path).parent() {
                    std::env::set_var("SOLUM_LLM_CONFIG", dir.join("solum-llm.json"));
                }
            }
            #[cfg(mobile)]
            if std::env::var("SOLUM_SOULOUS_CONFIG").is_err() {
                if let Some(dir) = std::path::Path::new(&db_path).parent() {
                    std::env::set_var("SOLUM_SOULOUS_CONFIG", dir.join("solum-soulous.json"));
                }
            }
            #[cfg(mobile)]
            if std::env::var("SOLUM_EMAIL_CONFIG").is_err() {
                if let Some(dir) = std::path::Path::new(&db_path).parent() {
                    std::env::set_var("SOLUM_EMAIL_CONFIG", dir.join("solum-email.json"));
                }
            }
            // Same gap as the three above until this fix (2026-07-22): sync's
            // config had no mobile-aware path, only `SOLUM_SYNC_URL/TOKEN/KEY`
            // or a cwd-relative `solum-sync.json` — neither resolvable on
            // Android, so multi-device sync could not be configured there at
            // all. `adb push`/`run-as cp` a `solum-sync.json` into this same
            // app-data dir to bind a phone, same as the LLM config above.
            #[cfg(mobile)]
            if std::env::var("SOLUM_SYNC_CONFIG").is_err() {
                if let Some(dir) = std::path::Path::new(&db_path).parent() {
                    std::env::set_var("SOLUM_SYNC_CONFIG", dir.join("solum-sync.json"));
                }
            }
            // Account session file follows the same mobile-aware convention as
            // the other credential files above.
            #[cfg(mobile)]
            if std::env::var("SOLUM_ACCOUNT_CONFIG").is_err() {
                if let Some(dir) = std::path::Path::new(&db_path).parent() {
                    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
                }
            }
            let mut orch = Orchestrator::open(&db_path)?;
            // Cloud reasoner is optional by design (F16): no config → stay offline.
            // Account proxy (harmony-0.2.0 model) outranks the direct-key config
            // while a session exists; logging out falls back automatically.
            let llm_summary = if let Some(session) = solum_core::account::AccountSession::load() {
                let summary = format!("账号 · {}", session.masked_summary());
                orch.set_reasoner(Box::new(solum_core::account::AccountReasoner::new(session)));
                Some(summary)
            } else {
                solum_core::llm::LlmConfig::load().map(|cfg| {
                    let summary = cfg.masked_summary();
                    orch.set_reasoner(Box::new(solum_core::llm::LlmReasoner::new(cfg)));
                    summary
                })
            };
            app.manage(AppState {
                orch: Arc::new(Mutex::new(orch)),
                db_path: db_path.clone(),
                llm_summary: Mutex::new(llm_summary),
                // First poll looks back 6h so a freshly-granted permission
                // doesn't have to wait for new data to show anything.
                // Resume from the persisted cursor; only a device that has
                // never polled looks back 6h, so a freshly-granted permission
                // still shows something immediately.
                health_since_ms: Mutex::new(
                    solum_core::store::Store::open(&db_path)
                        .ok()
                        .and_then(|s| s.sync_state(HEALTH_SINCE_KEY).ok().flatten())
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or_else(|| (Local::now() - Duration::hours(6)).timestamp_millis()),
                ),
                alarm_sig: Mutex::new(0),
                last_daily_brief_date: Mutex::new(None),
                email_oauth: Mutex::new(HashMap::new()),
                sync_store: Mutex::new(
                    solum_core::store::Store::open(&db_path)
                        .expect("sync store opens on the already-migrated db"),
                ),
                capture_inbox: Mutex::new(solum_core::capture::CaptureInbox::new()),
            });
            // The native listener consults this projection before it writes an
            // inbox line. A missing/corrupt file intentionally means capture
            // none, matching the core's default-empty whitelist.
            if let Ok(config) = app
                .state::<AppState>()
                .orch
                .lock()
                .map_err(|_| "state lock poisoned".to_string())
                .and_then(|o| {
                    o.notification_intelligence_config()
                        .map_err(|e| e.to_string())
                })
            {
                if let Err(error) = write_notification_capture_policy(&db_path, &config) {
                    eprintln!("[notification-intelligence] {error}");
                }
                #[cfg(mobile)]
                if let Err(error) = sync_notification_pipeline(&app.handle(), &config) {
                    // F16: a foreground-service/ROM failure may delay batch
                    // work, never the local database or alarm delivery path.
                    eprintln!("[notification-intelligence] foreground service: {error}");
                }
            }
            // Android 13+ requires a runtime POST_NOTIFICATIONS grant; ask once
            // at startup so the ticker's notifications can get through. Denial
            // is fine — in-window surfaces still show everything (F16 spirit).
            #[cfg(mobile)]
            {
                use tauri_plugin_notification::PermissionState;
                if !matches!(
                    app.notification().permission_state(),
                    Ok(PermissionState::Granted)
                ) {
                    let _ = app.notification().request_permission();
                }
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || ticker(handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            ingest,
            widget_confirm_preview,
            widget_discard_preview,
            widget_defs,
            widget_records,
            widget_add_field,
            widget_import_events,
            widget_promote_record,
            widget_record_create,
            widget_record_update,
            agenda,
            all_events,
            all_notifications,
            due,
            fire_due,
            dismiss,
            ledger,
            rules,
            rules_save,
            chat_context_set,
            proactivity_get,
            proactivity_set,
            notif_cloud_get,
            notif_cloud_set,
            notif_intelligence_status,
            notif_intelligence_apps,
            notif_intelligence_set_app,
            notif_intelligence_set_app_auto_event,
            notif_intelligence_acknowledge_losses,
            notif_intelligence_auto_event_counts,
            notif_intelligence_set_batch_interval,
            notif_intelligence_add_priority_rule,
            notif_intelligence_remove_priority_rule,
            notif_intelligence_set_filter_proposal,
            notif_intelligence_set_action_proposal,
            notif_intelligence_remove_filter_rule,
            notif_intelligence_restore_capture,
            notif_intelligence_promote_capture,
            notif_intelligence_process_now,
            notif_pipeline_status,
            notif_pipeline_request_battery_optimization,
            notif_pipeline_open_battery_settings,
            notif_pipeline_open_background_settings,
            tools,
            guard_run,
            guard_request,
            guard_confirm,
            audit,
            review,
            daily_brief,
            behavior_log,
            checkin_now,
            suggestions,
            suggest_generate,
            suggest_set,
            routines,
            routine_set_active,
            routine_update,
            routine_done,
            stats,
            persona_get,
            persona_set,
            persona_import_preview,
            persona_import_save,
            sync_status,
            sync_gap_acknowledge,
            sync_now,
            sync_config_get,
            sync_config_save,
            notif_access_status,
            notif_access_open_settings,
            persona_rollback,
            health_status,
            health_request,
            health_samples,
            llm_config_get,
            llm_config_save,
            llm_config_test,
            account_status_get,
            account_login,
            account_logout,
            account_model_save,
            privacy_consent_status,
            privacy_consent_accept,
            capture_entry_points,
            capture_inbox_list,
            capture_inbox_add,
            capture_inbox_discard,
            soulous_config_get,
            soulous_config_save,
            soulous_pull,
            email_config_get,
            email_config_save,
            email_config_remove,
            email_folders,
            email_messages,
            email_search,
            email_message,
            email_oauth_begin,
            email_oauth_poll,
            snooze,
            fact_update,
            export_data,
            event_reschedule,
            event_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Solum");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(base: &str, key: &str, model: &str) -> LlmSaveArgs {
        LlmSaveArgs {
            base_url: base.into(),
            api_key: key.into(),
            model: model.into(),
            temperature: Some(0.3),
            max_tokens: None,
            timeout_secs: 30,
        }
    }

    #[test]
    fn resolve_validates_and_normalizes() {
        // Trailing slash trimmed; values pass through.
        let c = resolve_llm_args(args("https://x/v1/", "k", "m")).unwrap();
        assert_eq!(c.base_url, "https://x/v1");
        assert_eq!(c.model, "m");
        // Bad scheme / empty model are actionable errors.
        assert!(resolve_llm_args(args("x/v1", "k", "m")).is_err());
        assert!(resolve_llm_args(args("https://x/v1", "k", "  ")).is_err());
        // Timeout clamps into [5, 600].
        let mut a = args("https://x/v1", "k", "m");
        a.timeout_secs = 1;
        assert_eq!(resolve_llm_args(a).unwrap().timeout_secs, 5);
    }

    /// Scratch dir + a real orchestrator, so the spool tests exercise the
    /// actual filesystem behaviour rather than a model of it.
    fn spool_fixture(name: &str) -> (std::path::PathBuf, solum_core::Orchestrator) {
        let dir = std::env::temp_dir().join(format!("solum-spool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notif-spool")).unwrap();
        let db = dir.join("solum.sqlite");
        let orch = solum_core::Orchestrator::open(db.to_str().unwrap()).unwrap();
        (dir, orch)
    }

    fn write_spool(dir: &std::path::Path, stem: &str, pkg: &str, title: &str) {
        let line = serde_json::json!({ "ts": 1_784_000_000_000i64, "pkg": pkg, "title": title, "text": "" });
        std::fs::write(
            dir.join("notif-spool").join(format!("{stem}.json")),
            format!(
                "{line}
"
            ),
        )
        .unwrap();
    }

    /// Full queue: the listener's byte-per-drop tally becomes durable state and
    /// The race the reviewer found: a drop recorded by the listener *while* the
    /// drainer is working must not be swallowed. Claiming by rename means the
    /// Crash recovery: dying between "claim the counter" and "record it" must
    fn overflow_dir(dir: &std::path::Path) -> std::path::PathBuf {
        let d = dir.join("notif-overflow");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn drop_marker(dir: &std::path::Path, stem: &str) {
        std::fs::write(overflow_dir(dir).join(format!("{stem}.mark")), [1u8]).unwrap();
    }

    fn markers_left(dir: &std::path::Path) -> usize {
        collect_overflow_markers(dir).len()
    }

    /// Full queue: each marker becomes durable state exactly once.
    #[test]
    fn a_full_queue_tally_is_recorded_once_and_cleared() {
        let (dir, mut orch) = spool_fixture("overflow");
        let db = dir.join("solum.sqlite");
        for i in 0..5 {
            drop_marker(&dir, &format!("100{i}-aaaa"));
        }

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 5);
        assert_eq!(markers_left(&dir), 0);

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 5, "no re-count");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The interleaving the reviewer identified**, constructed for real: a
    /// drop recorded by the listener *after* this side has taken its census but
    /// *before* it deletes must survive. The previous test only drained, wrote,
    /// and drained again — which never exercised the window at all.
    ///
    /// With a shared counter file this is exactly where a byte was lost: the
    /// writer's descriptor was opened pre-rename and its write landed
    /// post-unlink. With markers, `record_and_remove` touches only the paths it
    /// counted, so the newcomer is untouched and counted next pass.
    #[test]
    fn a_marker_created_between_the_census_and_the_delete_is_not_lost() {
        let (dir, mut orch) = spool_fixture("overflow-interleave");
        let db = dir.join("solum.sqlite");
        drop_marker(&dir, "1000-aaaa");
        drop_marker(&dir, "1001-bbbb");

        // Step 1: this side takes its census (2 markers).
        let census = collect_overflow_markers(&dir);
        assert_eq!(census.len(), 2);

        // Step 2: the listener drops another one *right now* — after the
        // census, before the delete. This is the window that was losing data.
        drop_marker(&dir, "1002-cccc");

        // Step 3: record + delete, which must touch only the censused paths.
        record_and_remove_overflow_markers(&mut orch, &census);
        assert_eq!(orch.capture_overflow_count().unwrap(), 2);
        assert_eq!(
            markers_left(&dir),
            1,
            "the marker that arrived mid-drain must still be on disk"
        );

        // Step 4: the next pass counts it — total 3, nothing lost or doubled.
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 3);
        assert_eq!(markers_left(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Crash between "counted" and "deleted" must not inflate the total.
    ///
    /// This previously asserted `>= 1`, i.e. it *accepted* double counting —
    /// while the code comment claimed markers were never counted twice and the
    /// UI presented the figure as "at least N". A number that can overshoot is
    /// not a lower bound, so all three could not be true at once. The receipt
    /// makes the strict assertion below hold.
    #[test]
    fn a_crash_between_counting_and_deleting_does_not_double_count() {
        let (dir, mut orch) = spool_fixture("overflow-crash");
        let db = dir.join("solum.sqlite");
        drop_marker(&dir, "1000-aaaa");
        drop_marker(&dir, "1001-bbbb");

        // Count them, then "crash": the files are deliberately left behind.
        let census = collect_overflow_markers(&dir);
        let names: Vec<String> = census
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(orch.note_capture_overflow(&names).unwrap(), 2);
        assert_eq!(orch.capture_overflow_count().unwrap(), 2);
        assert_eq!(markers_left(&dir), 2, "simulated crash before deletion");

        // Next run re-scans the same markers.
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(
            orch.capture_overflow_count().unwrap(),
            2,
            "re-scanning already-counted markers must add nothing"
        );
        assert_eq!(markers_left(&dir), 0, "and they are cleaned up");

        // A genuinely new drop still counts.
        drop_marker(&dir, "1002-cccc");
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A receipt must not expire while the marker it guards is still on disk.
    ///
    /// The drain used to prune receipts older than 30 days on every pass. The
    /// crash window that receipts guard has no upper bound: a marker whose
    /// deletion keeps failing outlives any cutoff, so the receipt aged out, the
    /// next scan counted the same marker again, and the total climbed above the
    /// truth — reintroducing the very over-count the receipt was added to stop.
    #[test]
    fn time_passing_does_not_retire_a_receipt_whose_marker_survives() {
        let (dir, mut orch) = spool_fixture("overflow-aged");
        let db = dir.join("solum.sqlite");

        // A marker whose deletion fails *every* time. Made undeletable by
        // being a directory: `remove_file` refuses one on every platform, and
        // the census filters on extension, so it is censused and counted like
        // any other. A real device hits this via a read-only volume, a lock, or
        // a permission change — the point is only that it persists.
        std::fs::create_dir_all(overflow_dir(&dir).join("1000-stuck.mark")).unwrap();

        // Two ticks, both long after any retention cutoff — a device that has
        // simply been running a while.
        let much_later = system_now() + Duration::days(400);
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), much_later);
        assert_eq!(orch.capture_overflow_count().unwrap(), 1);
        assert_eq!(markers_left(&dir), 1, "deletion failed; it persists");

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), much_later);
        assert_eq!(
            orch.capture_overflow_count().unwrap(),
            1,
            "the receipt must outlive any cutoff for as long as its marker does"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Receipts are retired one file at a time, by that file being gone.
    #[test]
    fn releasing_one_receipt_leaves_the_others_guarding() {
        let (dir, orch) = spool_fixture("overflow-release");
        let names = vec!["1000-aaaa.mark".to_string(), "1001-bbbb.mark".to_string()];
        assert_eq!(orch.note_capture_overflow(&names).unwrap(), 2);

        // Only the first file was actually deleted.
        assert_eq!(orch.release_capture_overflow(&names[..1]).unwrap(), 1);

        // Re-counting both: the deleted one is a genuinely new drop if its name
        // ever recurs; the survivor is still suppressed.
        assert_eq!(
            orch.note_capture_overflow(&names).unwrap(),
            1,
            "only the released name may be counted again"
        );
        assert_eq!(orch.capture_overflow_count().unwrap(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The legacy migration had the identical window; it is receipt-guarded too.
    #[test]
    fn a_crash_during_legacy_migration_does_not_double_count() {
        let (dir, mut orch) = spool_fixture("overflow-legacy-crash");
        let db = dir.join("solum.sqlite");
        let legacy = dir.join("notif-spool.overflow");
        std::fs::write(&legacy, [1u8; 4]).unwrap();

        // The crash is *between* counting and deleting, so the count lands and
        // the file is still there next boot. Writing the file back after a
        // successful delete would not reproduce that — under this design a
        // recreated file is a genuinely new tally, and counting it is correct.
        let names: Vec<String> = (0..4)
            .map(|i| format!("notif-spool.overflow#{i}"))
            .collect();
        assert_eq!(orch.note_capture_overflow(&names).unwrap(), 4);
        assert!(legacy.is_file(), "simulated crash before deletion");

        // Next boot re-reads the same 4 bytes; the receipts absorb them. Time
        // passing must not change that, so run the retry far in the future.
        let much_later = system_now() + Duration::days(400);
        drain_capture_inbox(&mut orch, db.to_str().unwrap(), much_later);
        assert_eq!(
            orch.capture_overflow_count().unwrap(),
            4,
            "the same legacy tally must not be counted twice"
        );
        assert!(!legacy.exists(), "and the retry clears the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Undeliverable payloads are a **live census** of `failed/`, not a tally.
    /// Counting the directory means the figure cannot drift from the files it
    /// describes — no window between parking and counting, and nothing to
    /// double-count when a receipt would have aged out.
    #[test]
    fn undeliverable_payloads_are_counted_from_the_directory() {
        let (dir, _orch) = spool_fixture("undeliverable");
        let db = dir.join("solum.sqlite");
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 0);

        let parked = dir.join("notif-spool").join("failed");
        std::fs::create_dir_all(&parked).unwrap();
        std::fs::write(parked.join("a.json"), "{}").unwrap();
        std::fs::write(parked.join("b.json"), "{}").unwrap();
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 2);

        // Idempotent by construction: asking twice cannot inflate it.
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 2);
        // Non-payload files are not counted.
        std::fs::write(parked.join("notes.txt"), "x").unwrap();
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Counts written by the previous shared-counter implementation are carried
    /// over rather than dropped on upgrade.
    #[test]
    fn legacy_shared_counter_files_are_migrated() {
        let (dir, mut orch) = spool_fixture("overflow-legacy");
        let db = dir.join("solum.sqlite");
        std::fs::write(dir.join("notif-spool.overflow"), [1u8; 3]).unwrap();
        std::fs::write(dir.join("notif-spool.overflow.taking"), [1u8; 2]).unwrap();

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(orch.capture_overflow_count().unwrap(), 5);
        assert!(!dir.join("notif-spool.overflow").exists());
        assert!(!dir.join("notif-spool.overflow.taking").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Happy path: an accepted spool file is consumed and removed.
    #[test]
    fn an_accepted_spool_file_is_consumed_and_deleted() {
        let (dir, mut orch) = spool_fixture("accept");
        let db = dir.join("solum.sqlite");
        orch.set_notification_app_enabled("com.a", true).unwrap();
        write_spool(&dir, "1784000000000-aaaa", "com.a", "明天下午3点开会");

        let out = drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        assert_eq!(out.len(), 1, "the capture should be reported");
        let left: Vec<_> = std::fs::read_dir(dir.join("notif-spool"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert!(left.is_empty(), "accepted file must be gone, got {left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A capture the core *declines* (package not whitelisted) is a completed
    /// outcome, not a failure — the file is consumed rather than retried
    /// forever.
    #[test]
    fn a_declined_capture_does_not_wedge_the_spool() {
        let (dir, mut orch) = spool_fixture("decline");
        let db = dir.join("solum.sqlite");
        // No whitelist entry → core returns Ok(None).
        write_spool(
            &dir,
            "1784000000000-bbbb",
            "com.not.allowed",
            "明天下午3点开会",
        );

        drain_capture_inbox(&mut orch, db.to_str().unwrap(), system_now());
        let left: Vec<_> = std::fs::read_dir(dir.join("notif-spool"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert!(
            left.is_empty(),
            "a declined capture is decided, not failed: {left:?}"
        );
        assert_eq!(parked_payload_count(db.to_str().unwrap()), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The spool-file lifecycle rule, pinned: a file is deleted only when the
    /// core accepted every line. The reviewer caught the opposite — errors were
    /// swallowed and the file deleted regardless, losing the notification while
    /// the comment claimed it could not happen.
    #[test]
    fn a_spool_file_survives_when_the_core_could_not_record_it() {
        // The decision under test is `batch.failed == 0`, so exercise it
        // directly rather than trying to fake a database failure.
        let accepted = IngestBatch {
            outcomes: vec!["ok".into()],
            failed: 0,
        };
        let partly_failed = IngestBatch {
            outcomes: vec!["ok".into()],
            failed: 1,
        };
        assert_eq!(accepted.failed, 0, "fully accepted → safe to delete");
        assert!(
            partly_failed.failed > 0,
            "any failure must keep the file for the next tick"
        );
        // A rejection (`Ok(None)`) is not a failure: nothing to retry.
        let all_rejected = IngestBatch::default();
        assert_eq!(
            all_rejected.failed, 0,
            "core deciding not to keep a capture is a completed outcome"
        );
    }

    /// Retry bookkeeping lives in the filename so it survives the restart a
    /// transient failure usually prompts.
    #[test]
    fn spool_retry_counter_round_trips_through_the_filename() {
        use std::path::Path;
        let fresh = Path::new("1700000000-abcd1234.json");
        assert_eq!(spool_attempt(fresh), 1);
        assert_eq!(spool_base(fresh), "1700000000-abcd1234");

        let retried = Path::new("1700000000-abcd1234.try2.json");
        assert_eq!(spool_attempt(retried), 2);
        assert_eq!(
            spool_base(retried),
            "1700000000-abcd1234",
            "the base must not accumulate .try suffixes across retries"
        );

        // A stem that merely contains ".try" but not a number is not a counter.
        let odd = Path::new("weird.tryX.json");
        assert_eq!(spool_attempt(odd), 1);
        assert_eq!(spool_base(odd), "weird.tryX");
    }

    #[test]
    fn empty_key_reuses_stored_file_key() {
        let dir = std::env::temp_dir().join("solum-llm-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("solum-llm.json");
        std::fs::write(
            &path,
            r#"{"base_url":"https://old/v1","api_key":"tp-old-9999"}"#,
        )
        .unwrap();
        std::env::set_var("SOLUM_LLM_CONFIG", &path);
        let c = resolve_llm_args(args("https://new/v1", "", "m")).unwrap();
        assert_eq!(c.api_key, "tp-old-9999");
        // No stored key at all → must ask for one.
        std::fs::remove_file(&path).unwrap();
        assert!(resolve_llm_args(args("https://new/v1", "", "m")).is_err());
        std::env::remove_var("SOLUM_LLM_CONFIG");
    }

    #[test]
    fn key_tail_masks() {
        assert_eq!(key_tail("tp-secret-zju9"), "…zju9");
        assert_eq!(key_tail(""), "");
    }

    #[test]
    fn daily_brief_gate_allows_only_one_emit_per_date() {
        let first = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let next = NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();
        assert!(daily_brief_is_due(None, first));
        assert!(!daily_brief_is_due(Some(first), first));
        assert!(daily_brief_is_due(Some(first), next));
    }
}

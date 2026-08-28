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

mod portable_snapshot;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use solum_core::recovery_code::RecoveryCodeTransport as _;

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Timelike, Utc};
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
    /// Device/profile root outside any mutable data generation. Restore
    /// readers may open sealed staging children here, never arbitrary paths.
    profile_root: std::path::PathBuf,
    active_generation: Option<String>,
    webview_restore: Mutex<Option<WebViewRestoreSession>>,
    /// One frozen encrypted upload attempt. It owns only ciphertext staging
    /// plus at most the attachment chunk currently being sealed; account or
    /// profile changes make every command discard it.
    backup_upload: Mutex<Option<BackupUploadAppSession>>,
    /// Authenticated cloud version bound to the sealed generation prepared by
    /// the restore downloader. It contains metadata only; a durable takeover
    /// journal replaces it once the user confirms publication.
    backup_restore: Mutex<Option<BackupRestoreAppSession>>,
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
    /// Login, registration and logout all replace the process-wide identity.
    /// Only one transition may issue or publish tokens at a time.
    account_transition: Arc<AtomicBool>,
}

struct WebViewRestoreSession {
    profile_id: String,
    reader: solum_core::portable_generation::PortableGenerationReader,
    ready: bool,
    publication_preview: Option<RestorePublicationBinding>,
}

struct BackupUploadAppSession {
    profile_id: String,
    account: solum_core::account::AccountSession,
    upload: solum_core::backup_upload::BackupUploadSession,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct BackupRestoreAppSession {
    profile_id: String,
    account: solum_core::account::AccountSession,
    backup_id: String,
    cloud_generation: u64,
    staged_generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestorePublicationBinding {
    token: String,
    sqlite_fingerprint: String,
    webview: LiveWebViewSummary,
}

struct AccountTransitionGuard<'a>(&'a AtomicBool);

impl<'a> AccountTransitionGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> CmdResult<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self(flag))
            .map_err(|_| "账号正在切换，请等待当前操作完成".to_string())
    }

    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    fn hold_until_restart(self) {
        std::mem::forget(self);
    }
}

impl Drop for AccountTransitionGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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

fn require_full_account() -> CmdResult<()> {
    if solum_core::account::AccountSession::load_full().is_some() {
        Ok(())
    } else {
        Err("这项完整能力需要先注册或登录 Solum 账号".into())
    }
}

fn tool_requires_full_account(tool: &str) -> bool {
    matches!(
        tool,
        "email_send"
            | "soulous_push_event"
            | "widget_delete"
            | "widget_record_delete"
            | "persona_clear"
            | "backup_delete"
    )
}

fn require_tool_account(tool: &str) -> CmdResult<()> {
    if tool_requires_full_account(tool) {
        require_full_account()?;
    }
    Ok(())
}

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
    ($state:expr) => {{
        let guard = $state
            .orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if $state.account_transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".to_string());
        }
        guard
    }};
}

fn core_err(e: solum_core::CoreError) -> String {
    e.to_string()
}

fn daily_brief_is_due(last_emitted: Option<NaiveDate>, today: NaiveDate) -> bool {
    last_emitted != Some(today)
}

/// The Android listener reads only this tiny local projection before appending
/// an inbox line. Missing/unreadable remains fail-closed during a concurrent
/// settings write; a valid policy defaults to all-app capture.
fn write_notification_capture_policy(
    db_path: &str,
    config: &NotificationIntelligenceConfig,
) -> CmdResult<()> {
    let parent = std::path::Path::new(db_path)
        .parent()
        .ok_or_else(|| "数据库路径没有父目录，无法写通知白名单".to_string())?;
    let path = parent.join("notif-policy.json");
    let projection = serde_json::json!({
        "capture_mode": config.capture_mode,
        "allowed_packages": config.allowed_packages,
    });
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
    if solum_core::account::AccountSession::load_full().is_none() {
        return app
            .notif_access()
            .stop_pipeline()
            .map_err(|e| e.to_string());
    }
    let relay_alerts_configured = solum_core::sync::SyncConfig::load().is_some();
    if config.capture_mode
        == solum_core::notification_intelligence::NotificationCaptureMode::Allowlist
        && config.allowed_packages.is_empty()
        && !relay_alerts_configured
    {
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
    /// `guest` or the immutable account UUID. The WebView uses the same value
    /// to partition chat/UI/IndexedDB state that never enters SQLite.
    local_profile_id: String,
    active_generation: Option<String>,
    web_storage: WebStorageNames,
    system_now: String,
    /// e.g. "http://100.79.74.121:8000/v1 · qwen3.5-2b · no key"; `None` = offline mode.
    llm: Option<String>,
}

#[tauri::command]
fn app_info(state: State<AppState>) -> CmdResult<AppInfo> {
    let local_profile_id = active_local_profile_id();
    Ok(AppInfo {
        db_path: state.db_path.clone(),
        web_storage: web_storage_names(&local_profile_id, state.active_generation.as_deref())?,
        local_profile_id,
        active_generation: state.active_generation.clone(),
        system_now: Local::now()
            .naive_local()
            .format("%Y-%m-%dT%H:%M")
            .to_string(),
        llm: state.llm_summary.lock().map(|g| g.clone()).unwrap_or(None),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct WebStorageNames {
    ui_state_key: String,
    chat_sessions_key: String,
    document_db_name: String,
}

fn web_storage_names(profile_id: &str, generation: Option<&str>) -> CmdResult<WebStorageNames> {
    if profile_id != "guest" && !solum_core::account::is_valid_user_id(profile_id) {
        return Err("WebView profile id 无效".into());
    }
    let suffix = match generation {
        None if profile_id == "guest" => String::new(),
        None => format!(".{profile_id}"),
        Some(value)
            if value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            format!(".{profile_id}.generation.{value}")
        }
        Some(_) => return Err("WebView generation id 无效".into()),
    };
    Ok(WebStorageNames {
        ui_state_key: format!("solum.ui-state.v1{suffix}"),
        chat_sessions_key: format!("pa.chat-sessions.v1{suffix}"),
        document_db_name: format!("pa-local-documents-v1{suffix}"),
    })
}

#[derive(Serialize)]
struct WebViewHydrationPlan {
    profile_id: String,
    generation: String,
    web_storage: WebStorageNames,
    bootstrap: solum_core::portable_generation::WebViewBootstrap,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct WebViewDocumentReceipt {
    id: String,
    plaintext_size: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebViewHydrationReceipt {
    profile_id: String,
    generation: String,
    documents: Vec<WebViewDocumentReceipt>,
}

#[derive(Serialize)]
struct WebViewHydrationStatus {
    generation: String,
    ready: bool,
}

/// Internal F17 seam: verify one sealed inactive generation and return only
/// its strict bootstrap plus exact, generation-isolated Web Storage names.
#[tauri::command]
fn restore_webview_begin(
    state: State<AppState>,
    generation_id: String,
) -> CmdResult<WebViewHydrationPlan> {
    let profile_id = active_local_profile_id();
    let reader = solum_core::portable_generation::PortableGenerationReader::open(
        &state.profile_root,
        &generation_id,
    )
    .map_err(core_err)?;
    let bootstrap = reader.bootstrap().clone();
    let plan = WebViewHydrationPlan {
        profile_id: profile_id.clone(),
        generation: generation_id.clone(),
        web_storage: web_storage_names(&profile_id, Some(&generation_id))?,
        bootstrap,
    };
    *state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())? =
        Some(WebViewRestoreSession {
            profile_id,
            reader,
            ready: false,
            publication_preview: None,
        });
    Ok(plan)
}

#[tauri::command]
fn restore_webview_document_chunk(
    state: State<AppState>,
    generation_id: String,
    document_id: String,
    offset: u64,
) -> CmdResult<solum_core::portable_generation::PortableDocumentChunk> {
    let session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())?;
    let session = session
        .as_ref()
        .filter(|session| {
            session.reader.bootstrap().generation == generation_id
                && session.profile_id == active_local_profile_id()
        })
        .ok_or_else(|| "WebView restore session 不存在或已失效".to_string())?;
    session
        .reader
        .read_document_chunk(&document_id, offset)
        .map_err(core_err)
}

#[tauri::command]
fn restore_webview_finish(
    state: State<AppState>,
    generation_id: String,
    receipt: WebViewHydrationReceipt,
) -> CmdResult<WebViewHydrationStatus> {
    let mut session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())?;
    let session = session
        .as_mut()
        .filter(|session| {
            session.reader.bootstrap().generation == generation_id
                && session.profile_id == active_local_profile_id()
        })
        .ok_or_else(|| "WebView restore session 不存在或已失效".to_string())?;
    let expected: Vec<_> = session
        .reader
        .bootstrap()
        .documents
        .iter()
        .map(|document| WebViewDocumentReceipt {
            id: document.id.clone(),
            plaintext_size: document.plaintext_size,
            sha256: document.sha256.clone(),
        })
        .collect();
    validate_webview_hydration_receipt(&session.profile_id, &generation_id, &expected, &receipt)?;
    session.ready = true;
    Ok(WebViewHydrationStatus {
        generation: generation_id,
        ready: true,
    })
}

fn validate_webview_hydration_receipt(
    profile_id: &str,
    generation: &str,
    expected: &[WebViewDocumentReceipt],
    receipt: &WebViewHydrationReceipt,
) -> CmdResult<()> {
    if receipt.profile_id != profile_id
        || receipt.generation != generation
        || receipt.documents != expected
    {
        return Err("WebView hydrate 回读结果与 sealed generation 不一致".into());
    }
    Ok(())
}

#[tauri::command]
fn restore_webview_status(
    state: State<AppState>,
    generation_id: String,
) -> CmdResult<WebViewHydrationStatus> {
    let session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())?;
    let session = session
        .as_ref()
        .filter(|session| {
            session.reader.bootstrap().generation == generation_id
                && session.profile_id == active_local_profile_id()
        })
        .ok_or_else(|| "WebView restore session 不存在或已失效".to_string())?;
    Ok(WebViewHydrationStatus {
        generation: generation_id,
        ready: session.ready,
    })
}

// ---- cloud LLM settings (§3.6; provider survey in docs/LLM-PROVIDERS.md) ----

/// The JSON file the settings UI reads/writes. Matches `LlmConfig::load`'s
/// fallback: explicit `SOLUM_LLM_CONFIG` first, then the active account profile
/// (or the historical app-data path for guest).
const MAX_REPLACEMENT_PREVIEW_ITEMS: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveWebViewSummary {
    profile_id: String,
    chat_sessions: usize,
    chat_messages: usize,
    drafts: usize,
    documents: usize,
    fingerprint: String,
}

#[derive(Debug, Serialize)]
struct RestorePublicationPreview {
    profile_id: String,
    generation: String,
    token: String,
    sqlite_counts: Vec<(String, usize)>,
    webview: LiveWebViewSummary,
    has_existing_data: bool,
}

fn validate_live_webview_summary(summary: &LiveWebViewSummary, profile_id: &str) -> CmdResult<()> {
    let total = summary
        .chat_sessions
        .checked_add(summary.chat_messages)
        .and_then(|value| value.checked_add(summary.drafts))
        .and_then(|value| value.checked_add(summary.documents))
        .ok_or_else(|| "WebView replacement summary count overflow".to_string())?;
    if summary.profile_id != profile_id
        || total > MAX_REPLACEMENT_PREVIEW_ITEMS
        || summary.fingerprint.len() != 64
        || !summary
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("WebView replacement summary is invalid".into());
    }
    Ok(())
}

fn random_publication_token() -> CmdResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "OS random source is unavailable".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
fn validate_publication_binding(
    binding: &RestorePublicationBinding,
    token: &str,
    sqlite_fingerprint: &str,
    webview: &LiveWebViewSummary,
) -> CmdResult<()> {
    if binding.token != token
        || binding.sqlite_fingerprint != sqlite_fingerprint
        || binding.webview != *webview
    {
        return Err("Replacement preview is stale; generate a new preview".into());
    }
    Ok(())
}

#[tauri::command]
fn restore_publish_preview(
    state: State<AppState>,
    generation_id: String,
    webview: LiveWebViewSummary,
) -> CmdResult<RestorePublicationPreview> {
    let profile_id = active_local_profile_id();
    validate_live_webview_summary(&webview, &profile_id)?;
    let _orch_guard = state
        .orch
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("Solum profile is switching; reopen the app to continue".into());
    }
    let sync_store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let sqlite = solum_core::export_v3::snapshot_portable_store(&sync_store).map_err(core_err)?;
    let token = random_publication_token()?;
    let mut session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session state is unavailable".to_string())?;
    let session = session
        .as_mut()
        .filter(|session| {
            session.ready
                && session.reader.bootstrap().generation == generation_id
                && session.profile_id == profile_id
        })
        .ok_or_else(|| "WebView restore generation is not ready for publication".to_string())?;
    session.publication_preview = Some(RestorePublicationBinding {
        token: token.clone(),
        sqlite_fingerprint: sqlite.fingerprint,
        webview: webview.clone(),
    });
    let sqlite_total = sqlite.counts.iter().try_fold(0usize, |total, (_, count)| {
        total
            .checked_add(*count)
            .ok_or_else(|| "SQLite replacement summary count overflow".to_string())
    })?;
    let webview_total =
        webview.chat_sessions + webview.chat_messages + webview.drafts + webview.documents;
    Ok(RestorePublicationPreview {
        profile_id,
        generation: generation_id,
        token,
        sqlite_counts: sqlite.counts,
        webview,
        has_existing_data: sqlite_total > 0 || webview_total > 0,
    })
}

#[cfg(target_os = "android")]
fn validate_restore_preview_is_current(
    state: &AppState,
    generation_id: &str,
    token: &str,
    webview: &LiveWebViewSummary,
) -> CmdResult<()> {
    let profile_id = active_local_profile_id();
    validate_live_webview_summary(webview, &profile_id)?;
    let _orch_guard = state
        .orch
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let sync_store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let sqlite = solum_core::export_v3::snapshot_portable_store(&sync_store).map_err(core_err)?;
    let session_guard = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session state is unavailable".to_string())?;
    let session = session_guard
        .as_ref()
        .filter(|session| {
            session.ready
                && session.reader.bootstrap().generation == generation_id
                && session.profile_id == profile_id
        })
        .ok_or_else(|| "WebView restore generation is not ready for publication".to_string())?;
    let binding = session
        .publication_preview
        .as_ref()
        .ok_or_else(|| "Replacement preview is missing or already consumed".to_string())?;
    validate_publication_binding(binding, token, &sqlite.fingerprint, webview)
}

#[cfg(target_os = "android")]
fn publish_restore_generation(
    state: &AppState,
    generation_id: &str,
    token: &str,
    webview: &LiveWebViewSummary,
) -> CmdResult<solum_core::portable_generation::PortablePublishOutcome> {
    let profile_id = active_local_profile_id();
    validate_live_webview_summary(webview, &profile_id)?;
    let _orch_guard = state
        .orch
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let sync_store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let sqlite = solum_core::export_v3::snapshot_portable_store(&sync_store).map_err(core_err)?;
    let binding = {
        let mut session_guard = state
            .webview_restore
            .lock()
            .map_err(|_| "WebView restore session state is unavailable".to_string())?;
        let session = session_guard
            .as_mut()
            .filter(|session| {
                session.ready
                    && session.reader.bootstrap().generation == generation_id
                    && session.profile_id == profile_id
            })
            .ok_or_else(|| "WebView restore generation is not ready for publication".to_string())?;
        session
            .publication_preview
            .take()
            .ok_or_else(|| "Replacement preview is missing or already consumed".to_string())?
    };
    validate_publication_binding(&binding, token, &sqlite.fingerprint, webview)?;
    solum_core::portable_generation::publish_portable_generation(&state.profile_root, generation_id)
        .map_err(core_err)
}

fn llm_config_file() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SOLUM_LLM_CONFIG") {
        return p.into();
    }
    solum_core::paths::resolve_profile_with_adoption("solum-llm.json")
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
    require_full_account()?;
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
    if solum_core::account::AccountSession::load_full().is_none() {
        return Err("完整云端能力需要先注册或登录 Solum 账号".into());
    }
    let c = resolve_llm_args(cfg)?;
    let json = serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?;
    let path = llm_config_file();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    solum_core::fsatomic::write_atomic(&path, &json).map_err(|e| e.to_string())?;
    let summary = c.masked_summary();
    // Direct vendor settings remain importable for advanced account users,
    // but the authenticated account proxy owns the running reasoner. Logging
    // out always returns to offline guest mode.
    let _ = state;
    Ok(format!("{summary}（已保存；当前账号代理保持生效）"))
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
    if solum_core::account::AccountSession::load_full().is_none() {
        return Err("测试云端连接前请先注册或登录 Solum 账号".into());
    }
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
    default_model: String,
    model_options: Vec<String>,
    usage: Option<solum_core::account::CloudUsage>,
    state_error: Option<String>,
    /// Where the session file lives.
    path: String,
}

async fn revoke_pending_session(session: solum_core::account::AccountSession) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::revoke_remote(&session);
    })
    .await;
}

fn schedule_pending_session_revoke(session: solum_core::account::AccountSession) {
    let _task = tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::revoke_remote(&session);
    });
}

#[tauri::command]
async fn account_status_get() -> CmdResult<AccountStatus> {
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
            server_url: String::new(),
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
fn restart_account_app(app: &tauri::AppHandle) -> CmdResult<String> {
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
fn restart_account_app(app: &tauri::AppHandle) -> CmdResult<String> {
    app.restart()
}

async fn account_auth(
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

/// Log in against a self-hosted solum-cloud (`server/`), prepare the target
/// profile, publish it behind the old profile's sync barrier, then restart.
#[tauri::command]
async fn account_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    username: String,
    password: String,
    model: String,
) -> CmdResult<String> {
    account_auth(state, app, server_url, username, password, model, false).await
}

/// Register once in the central account database, then enter the exact same
/// profile-switch and recovery flow as a normal login.
#[tauri::command]
async fn account_register(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    username: String,
    password: String,
    model: String,
) -> CmdResult<String> {
    account_auth(state, app, server_url, username, password, model, true).await
}

#[tauri::command]
async fn account_oauth_config_get(
    server_url: Option<String>,
) -> CmdResult<solum_core::account::OAuthConfig> {
    let server = server_url.unwrap_or_else(|| solum_core::account::SOLUM_CENTER_URL.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        solum_core::account::fetch_oauth_config(&server).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("获取 OAuth 配置任务失败: {e}"))?
}

#[tauri::command]
fn account_oauth_pkce_generate() -> CmdResult<solum_core::account::PkcePair> {
    solum_core::account::generate_pkce().map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct AccountOAuthLoginArgs {
    server_url: String,
    code: String,
    code_verifier: String,
    redirect_uri: String,
    model: String,
    recovery_code: Option<String>,
}

#[tauri::command]
async fn account_oauth_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    args: AccountOAuthLoginArgs,
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
        let session = solum_core::account::authenticate_via_oauth(
            &args.server_url,
            &args.code,
            &args.code_verifier,
            &args.redirect_uri,
            &args.model,
        )
        .map_err(|e| e.to_string())?;

        let user_id = session
            .stable_user_id()
            .ok_or_else(|| "账号服务器未返回不可变 user_id".to_string())?;
        let sync_path = solum_core::paths::account_profile_file(user_id, "solum-sync.json")
            .ok_or_else(|| "无法创建账号专属同步配置目录".to_string())?;

        // 1. Check if local sync config already exists for this user_id
        if let Some(existing_config) = solum_core::sync::load_account_sync_config(&sync_path) {
            let master_key = existing_config.key_bytes().map_err(|e| e.to_string())?;
            if let Err(error) =
                solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
            {
                solum_core::account::revoke_remote(&session);
                return Err(format!("本机加密空间初始化失败: {error}"));
            }
            return Ok::<_, String>(session);
        }

        // 2. Local config does not exist. Check if server has recovery code envelope.
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
                    "NEEDS_RECOVERY_CODE:此账号已有云端加密数据，请输入您的 20 字符恢复码以解密"
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
            key_bytes
        };

        solum_core::sync::save_account_sync_config_public(
            &sync_path,
            &session.server_url,
            &master_key,
        )
        .map_err(|e| e.to_string())?;

        if let Err(error) =
            solum_core::sync::verify_account_sync_provision(&session, &master_key, &sync_path)
        {
            solum_core::account::revoke_remote(&session);
            return Err(format!("加密空间准备失败: {error}"));
        }

        Ok::<_, String>(session)
    })
    .await
    .map_err(|e| format!("OAuth 登录后台任务失败: {e}"))??;

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

/// Local sign-out first-class: best-effort server-side revocation, then restart
/// into the offline guest profile. Legacy direct-key files never activate it.
#[tauri::command]
async fn account_logout(state: State<'_, AppState>, app: tauri::AppHandle) -> CmdResult<String> {
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
async fn recovery_code_status() -> CmdResult<serde_json::Value> {
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
async fn recovery_code_issue() -> CmdResult<String> {
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
        Ok(issuance.display_code)
    })
    .await
    .map_err(|e| format!("恢复码生成线程失败: {e}"))?
}

/// Sign in with username + recovery code when the password is forgotten.
/// Runs the full profile-preparation flow: fetch the envelope with an
/// authenticated session, unwrap the master key locally, and persist it so
/// the restored key matches what future backups will use.
#[tauri::command]
async fn recovery_code_login(
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

/// Change the account password while signed in. The client rewraps the
/// password envelope under the new password first; the server swaps the
/// credential hash and that envelope in one transaction, so "new password
/// live, envelope still old" cannot happen. Other devices must sign in again.
#[tauri::command]
async fn account_change_password(
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
async fn account_model_save(state: State<'_, AppState>, model: String) -> CmdResult<String> {
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

// ---- 首启隐私门 + 应用内隐私政策（自 Solum Harmony 回移）----------------------

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

/// Refusing the first-run policy must terminate the native app, including on
/// Android where closing a WebView window is not a reliable process-exit path.
#[cfg(not(mobile))]
#[tauri::command]
fn privacy_consent_decline(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg(mobile)]
#[tauri::command]
fn privacy_consent_decline(_app: tauri::AppHandle) {
    // On Android a WebView window is not the application task. Exit the native
    // process after the user explicitly declines so the button always has the
    // promised outcome.
    std::process::exit(0);
}

/// Request OS notification delivery only after the user has accepted the
/// in-app privacy policy. Notification-listener access remains separate.
#[tauri::command]
fn notification_permission_request(app: tauri::AppHandle) -> CmdResult<bool> {
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
struct ReminderDeliveryHealth {
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
fn reminder_delivery_health(app: tauri::AppHandle) -> CmdResult<ReminderDeliveryHealth> {
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
async fn debug_simulate_process_death(window: tauri::WebviewWindow) -> CmdResult<bool> {
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

#[derive(Serialize)]
struct CaptureConnectorView {
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
fn capture_entry_points(
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
fn capture_inbox_list(state: State<AppState>) -> CmdResult<Vec<CaptureDraftView>> {
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
fn capture_inbox_add(
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
fn capture_inbox_discard(state: State<AppState>, id: String) -> CmdResult<bool> {
    let mut inbox = state
        .capture_inbox
        .lock()
        .map_err(|_| "采集队列状态异常（锁中毒）".to_string())?;
    Ok(inbox.remove(&id))
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct AndroidShareItem {
    source: String,
    title: String,
    text: String,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct AndroidShareDrain {
    items: Vec<AndroidShareItem>,
    notices: Vec<String>,
}

#[derive(Serialize)]
struct AndroidShareDrainView {
    drafts: Vec<CaptureDraftView>,
    notices: Vec<String>,
}

/// Drain Android's bounded, RAM-only ACTION_SEND queue into the ordinary
/// candidate inbox. This is invoked on launch/focus rather than by a battery-
/// consuming poll loop. Desktop returns an empty list.
#[tauri::command]
async fn capture_android_share_drain(
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
async fn capture_android_voice_start(window: tauri::WebviewWindow) -> CmdResult<bool> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(false)
    }
    #[cfg(target_os = "android")]
    android_activity_bool(window, "startVoiceCapture").await
}

#[tauri::command]
async fn capture_android_ocr_pick(window: tauri::WebviewWindow) -> CmdResult<bool> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(false)
    }
    #[cfg(target_os = "android")]
    android_activity_bool(window, "startOcrPicker").await
}

#[cfg(target_os = "android")]
async fn android_activity_bool(
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
async fn calendar_access_status(window: tauri::WebviewWindow) -> CmdResult<bool> {
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
async fn calendar_access_request(window: tauri::WebviewWindow) -> CmdResult<bool> {
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
struct AndroidCalendarItem {
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
async fn calendar_candidates_import(
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
async fn calendar_open_insert(
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
fn email_config_remove(account_id: String) -> CmdResult<EmailSettings> {
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

/// Scan only recent INBOX envelope metadata and enqueue reviewable high-value events.
/// Message bodies and attachments are deliberately never fetched by this path.
#[tauri::command]
async fn email_high_value_scan(
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
struct EmailOAuthPoll {
    status: String,
}

#[tauri::command]
async fn email_oauth_poll(
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
fn widget_confirm_preview(
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
fn widget_discard_preview(state: State<AppState>, preview_id: String) -> CmdResult<()> {
    require_full_account()?;
    lock!(state)
        .discard_widget_preview(&preview_id)
        .map_err(core_err)
}

#[tauri::command]
fn widget_defs(state: State<AppState>) -> CmdResult<Vec<solum_core::widget::WidgetDefinition>> {
    require_full_account()?;
    lock!(state).widget_definitions().map_err(core_err)
}

#[tauri::command]
fn widget_records(
    state: State<AppState>,
    widget_id: i64,
) -> CmdResult<Vec<solum_core::widget::WidgetRecord>> {
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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

/// One paged ledger slice for the F12 UI: a single layer, keyset-paginated.
/// Returns (entries, next_cursor); a null cursor means the layer is done.
#[tauri::command]
fn ledger_page(
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
fn fact_update(state: State<AppState>, id: i64, content: String) -> CmdResult<()> {
    lock!(state).update_fact(id, &content).map_err(core_err)
}

/// §4 数据完全归你：把本机全部数据导出为一份 JSON 文件，写在数据库同目录，
/// 返回完整路径。纯只读、纯本地、不上云。
#[tauri::command]
fn export_data(state: State<AppState>, now: Option<String>) -> CmdResult<String> {
    let now = parse_now(now)?;
    let json = lock!(state).export_json(now).map_err(core_err)?;
    write_export_json(&state.db_path, &json, "solum-export")
}

/// Build a v3 main logical payload from a WebView/IndexedDB snapshot plus the
/// Rust-owned spool and non-secret configuration. This command is deliberately
/// not wired to the current v2 export/import UI: v3 restore stays fail-closed
/// until the platform-wide temporary-store transaction exists.
#[tauri::command]
fn export_portable_data(
    state: State<AppState>,
    app: tauri::AppHandle,
    snapshot: portable_snapshot::WebViewPortableSnapshot,
    now: Option<String>,
) -> CmdResult<String> {
    let now = parse_now(now)?;
    let json = collect_portable_json(&state, &app, snapshot, now)?;
    write_export_json(&state.db_path, &json, "solum-export-v3")
}

fn collect_portable_json(
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

fn current_backup_account(
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

fn require_current_privacy_consent() -> CmdResult<()> {
    if solum_core::privacy::PrivacyConsent::needs_consent() {
        return Err("请先阅读并同意当前隐私政策，再使用加密云备份".into());
    }
    Ok(())
}

fn backup_master_key(account: &solum_core::account::AccountSession) -> CmdResult<[u8; 32]> {
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

fn require_backup_delete_is_not_in_restore(state: &AppState, args: &str) -> CmdResult<()> {
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

fn take_backup_upload(state: &AppState, session_id: &str) -> CmdResult<BackupUploadAppSession> {
    let mut guard = state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())?;
    let matches = guard.as_ref().is_some_and(|session| {
        session.upload.session_id() == session_id
            && current_backup_account(&session.profile_id, &session.account).is_ok()
    });
    if !matches {
        return Err("备份上传会话不存在、已取消或已失效".into());
    }
    guard
        .take()
        .ok_or_else(|| "备份上传会话不存在、已取消或已失效".into())
}

fn restore_failed_backup_upload(state: &AppState, session: BackupUploadAppSession) {
    if current_backup_account(&session.profile_id, &session.account).is_err() {
        return;
    }
    if let Ok(mut guard) = state.backup_upload.lock() {
        if guard.is_none() {
            *guard = Some(session);
        }
    }
}

/// Manual F17 backup entry. The UI invokes it only from the backup center;
/// there is deliberately no scheduler in this phase.
#[tauri::command]
async fn backup_upload_begin(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    snapshot: portable_snapshot::WebViewPortableSnapshot,
) -> CmdResult<solum_core::backup_upload::BackupUploadBegin> {
    require_current_privacy_consent()?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("账号正在切换，已取消备份".into());
    }
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能创建加密云备份".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
        .to_string();
    if solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        &profile_id,
        &account.server_url,
    )
    .map_err(|error| error.to_string())?
    .is_some()
    {
        return Err("已有备份接管事务待完成，请先继续恢复".into());
    }
    portable_snapshot::validate_profile(&snapshot.profile_id, &profile_id)?;
    let master_key = backup_master_key(&account)?;
    let json = collect_portable_json(&state, &app, snapshot, Local::now().naive_local())?;
    // A fresh begin explicitly supersedes any older attempt before core clears
    // encrypted crash residue. If preparation fails, callers must begin again;
    // an old plan whose files were removed is never left looking resumable.
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    let profile_root = state.profile_root.clone();
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut cloud_account = account.clone();
    let worker_profile_id = profile_id.clone();
    let (cloud_account, upload) = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::backup_upload::HttpBackupCloudTransport::new();
        let authority = solum_core::backup_upload::load_or_initialize_device_generation(
            &profile_root,
            &worker_profile_id,
            &transport,
            &mut cloud_account,
        )
        .map_err(|error| error.to_string())?;
        let upload = solum_core::backup_upload::BackupUploadSession::begin(
            &profile_root,
            json.as_bytes(),
            &worker_profile_id,
            authority.generation,
            authority.writer_id,
            &created_at,
            master_key,
        )
        .map_err(|error| error.to_string())?;
        Ok::<_, String>((cloud_account, upload))
    })
    .await
    .map_err(|_| "备份准备任务异常终止".to_string())??;
    current_backup_account(&profile_id, &cloud_account)?;
    let begin = upload.begin_info();
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = Some(BackupUploadAppSession {
        profile_id,
        account: cloud_account,
        upload,
    });
    Ok(begin)
}

#[tauri::command]
fn backup_upload_attachment_chunk(
    state: State<AppState>,
    session_id: String,
    attachment_id: String,
    offset: u64,
    data_base64: String,
) -> CmdResult<()> {
    use base64::Engine as _;

    const MAX_ENCODED_CHARS: usize = solum_core::backup_upload::MAX_IPC_CHUNK_BYTES.div_ceil(3) * 4;
    if data_base64.len() > MAX_ENCODED_CHARS {
        return Err("备份附件分块超过 IPC 上限".into());
    }
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| "备份附件分块编码无效".to_string())?;
    if base64::engine::general_purpose::STANDARD.encode(&bytes) != data_base64 {
        return Err("备份附件分块编码不是规范 base64".into());
    }
    let mut guard = state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())?;
    let session = guard
        .as_mut()
        .filter(|session| {
            session.upload.session_id() == session_id
                && current_backup_account(&session.profile_id, &session.account).is_ok()
        })
        .ok_or_else(|| "备份上传会话不存在、已取消或已失效".to_string())?;
    let result = session
        .upload
        .append_attachment_chunk(&attachment_id, offset, &bytes)
        .map_err(|error| error.to_string());
    bytes.fill(0);
    result
}

#[tauri::command]
async fn backup_upload_attachment_finish(
    state: State<'_, AppState>,
    session_id: String,
    attachment_id: String,
) -> CmdResult<()> {
    let mut session = take_backup_upload(&state, &session_id)?;
    let (returned, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = session
            .upload
            .finish_attachment(&attachment_id)
            .map_err(|error| error.to_string());
        (session, result)
    })
    .await
    .map_err(|_| "备份附件加密任务异常终止".to_string())?;
    if let Err(error) = result {
        restore_failed_backup_upload(&state, returned);
        return Err(error);
    }
    restore_failed_backup_upload(&state, returned);
    Ok(())
}

#[tauri::command]
async fn backup_upload_commit(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<solum_core::backup_upload::BackupUploadOutcome> {
    require_current_privacy_consent()?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("账号正在切换，已取消备份提交".into());
    }
    let mut session = take_backup_upload(&state, &session_id)?;
    let (returned, result) = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::backup_upload::HttpBackupCloudTransport::new();
        let result = session
            .upload
            .upload(&transport, &mut session.account)
            .map_err(|error| error.to_string());
        (session, result)
    })
    .await
    .map_err(|_| "备份上传任务异常终止".to_string())?;
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            restore_failed_backup_upload(&state, returned);
            Err(error)
        }
    }
}

#[tauri::command]
fn backup_upload_cancel(state: State<AppState>, session_id: String) -> CmdResult<()> {
    let mut guard = state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())?;
    if guard
        .as_ref()
        .is_some_and(|session| session.upload.session_id() == session_id)
    {
        *guard = None;
    }
    Ok(())
}

/// F17 recovery catalog. It returns complete-version metadata only and
/// does not download, decrypt, stage, publish, or advance device generation.
#[tauri::command]
async fn backup_restore_catalog(
    state: State<'_, AppState>,
) -> CmdResult<solum_core::backup_download::BackupCatalog> {
    require_current_privacy_consent()?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("账号正在切换，已取消读取备份列表".into());
    }
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能读取加密云备份".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
        .to_string();
    if active_local_profile_id() != profile_id {
        return Err("备份读取已取消：账号或本地 profile 不一致".into());
    }
    let mut cloud_account = account.clone();
    let (cloud_account, catalog) = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::backup_download::HttpBackupCloudReader::new();
        let catalog =
            solum_core::backup_download::fetch_backup_catalog(&transport, &mut cloud_account, 20)
                .map_err(|error| error.to_string())?;
        Ok::<_, String>((cloud_account, catalog))
    })
    .await
    .map_err(|_| "备份列表任务异常终止".to_string())??;
    current_backup_account(&profile_id, &cloud_account)?;
    Ok(catalog)
}

/// Download and authenticate one complete version, then assemble a sealed but
/// inactive generation. Publication remains a separate preview/confirm flow.
#[tauri::command]
async fn backup_restore_stage(
    state: State<'_, AppState>,
    backup_id: String,
) -> CmdResult<solum_core::backup_download::StagedBackupOutcome> {
    require_current_privacy_consent()?;
    // Staging and hard deletion must never cross: either operation can begin
    // only after it owns the same profile-transition guard for its full
    // network/filesystem lifetime.
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能准备加密备份恢复".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
        .to_string();
    if active_local_profile_id() != profile_id {
        return Err("备份恢复已取消：账号或本地 profile 不一致".into());
    }
    if solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        &profile_id,
        &account.server_url,
    )
    .map_err(|error| error.to_string())?
    .is_some()
    {
        return Err("已有备份接管事务待完成，请从待继续项恢复".into());
    }
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let mut master_key = backup_master_key(&account)?;
    let profile_root = state.profile_root.clone();
    let worker_profile_id = profile_id.clone();
    let mut cloud_account = account.clone();
    let (cloud_account, active_generation, outcome) =
        tauri::async_runtime::spawn_blocking(move || {
            let transport = solum_core::backup_download::HttpBackupCloudReader::new();
            let result = (|| {
                let catalog = solum_core::backup_download::fetch_backup_catalog(
                    &transport,
                    &mut cloud_account,
                    100,
                )?;
                let active_generation = catalog.active_generation;
                let outcome = solum_core::backup_download::download_and_stage_backup(
                    solum_core::backup_download::BackupStageRequest {
                        profile_root: &profile_root,
                        catalog: &catalog,
                        backup_id: &backup_id,
                        user_id: &worker_profile_id,
                        master_key: &master_key,
                        now: Local::now().naive_local(),
                    },
                    &transport,
                    &mut cloud_account,
                )?;
                Ok::<_, solum_core::backup_download::BackupDownloadError>((
                    active_generation,
                    outcome,
                ))
            })()
            .map_err(|error| error.to_string());
            master_key.fill(0);
            result.map(|(active_generation, outcome)| (cloud_account, active_generation, outcome))
        })
        .await
        .map_err(|_| "备份下载与校验任务异常终止".to_string())??;
    current_backup_account(&profile_id, &cloud_account)?;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = Some(BackupRestoreAppSession {
        profile_id,
        account: cloud_account,
        backup_id: outcome.backup_id.clone(),
        cloud_generation: active_generation,
        staged_generation: outcome.generation_id.clone(),
    });
    Ok(outcome)
}

/// A durable takeover may survive a response loss or process restart. The UI
/// uses this metadata-only status to resume hydrate/preview without creating a
/// second transaction or downloading a different candidate.
#[tauri::command]
fn backup_restore_pending(
    state: State<AppState>,
) -> CmdResult<Option<solum_core::backup_takeover::BackupTakeoverStatus>> {
    require_current_privacy_consent()?;
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能继续加密备份恢复".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?;
    if active_local_profile_id() != profile_id {
        return Err("备份恢复已取消：账号或本地 profile 不一致".into());
    }
    solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        profile_id,
        &account.server_url,
    )
    .map(|pending| pending.map(|pending| pending.status()))
    .map_err(|error| error.to_string())
}

/// Advance cloud/device generation only after a fresh preview, then re-check
/// both live stores under their locks before atomically publishing the pointer.
/// A response-lost retry reuses the durable takeover UUID.
#[tauri::command]
async fn backup_restore_takeover_publish(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    generation_id: String,
    token: String,
    webview: LiveWebViewSummary,
) -> CmdResult<String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (state, app, generation_id, token, webview);
        Err("Backup takeover is available only in the Android app".into())
    }
    #[cfg(target_os = "android")]
    {
        require_current_privacy_consent()?;
        let transition = AccountTransitionGuard::acquire(&state.account_transition)?;
        let account = solum_core::account::AccountSession::load_full()
            .ok_or_else(|| "登录后才能接管加密备份".to_string())?;
        let profile_id = account
            .stable_user_id()
            .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
            .to_string();
        if active_local_profile_id() != profile_id {
            return Err("备份接管已取消：账号或本地 profile 不一致".into());
        }

        // This check deliberately leaves the token unspent. Network work must
        // happen without the SQLite locks; publication performs the same check
        // again and consumes the token only after the cloud generation is safe.
        validate_restore_preview_is_current(&state, &generation_id, &token, &webview)?;

        let pending_status = solum_core::backup_takeover::load_pending_takeover(
            &state.profile_root,
            &profile_id,
            &account.server_url,
        )
        .map_err(|error| error.to_string())?
        .map(|pending| pending.status());
        let (backup_id, expected_generation) = if let Some(status) = pending_status {
            if status.staged_generation != generation_id {
                return Err("另一个备份接管事务仍在进行".into());
            }
            (status.backup_id, status.expected_generation)
        } else {
            let restore = state
                .backup_restore
                .lock()
                .map_err(|_| "备份恢复会话状态异常".to_string())?;
            let restore = restore
                .as_ref()
                .filter(|restore| {
                    restore.profile_id == profile_id
                        && restore.staged_generation == generation_id
                        && current_backup_account(&restore.profile_id, &restore.account).is_ok()
                })
                .ok_or_else(|| "备份恢复候选已失效，请重新校验该版本".to_string())?;
            (restore.backup_id.clone(), restore.cloud_generation)
        };

        let profile_root = state.profile_root.clone();
        let worker_profile = profile_id.clone();
        let worker_server = account.server_url.clone();
        let worker_generation = generation_id.clone();
        let mut cloud_account = account.clone();
        let (cloud_account, pending) = tauri::async_runtime::spawn_blocking(move || {
            let mut pending = solum_core::backup_takeover::begin_or_resume_takeover(
                &profile_root,
                solum_core::backup_takeover::BackupTakeoverBinding {
                    user_id: &worker_profile,
                    server_url: &worker_server,
                    backup_id: &backup_id,
                    staged_generation: &worker_generation,
                    expected_generation,
                },
            )
            .map_err(|error| error.to_string())?;
            let transport = solum_core::backup_download::HttpBackupCloudReader::new();
            solum_core::backup_takeover::advance_takeover(
                &mut pending,
                &transport,
                &mut cloud_account,
            )
            .map_err(|error| error.to_string())?;
            Ok::<_, String>((cloud_account, pending))
        })
        .await
        .map_err(|_| "备份接管任务异常终止".to_string())??;
        current_backup_account(&profile_id, &cloud_account)?;

        let outcome = publish_restore_generation(&state, &generation_id, &token, &webview)?;
        if solum_core::backup_takeover::finish_published_takeover(pending, &generation_id).is_err()
        {
            // The pointer is already durable. Startup reconciliation owns this
            // cleanup path; never pretend publication can now be rolled back.
            eprintln!("backup takeover journal cleanup deferred to startup reconciliation");
        }
        if let Ok(mut restore) = state.backup_restore.lock() {
            *restore = None;
        }
        transition.hold_until_restart();
        if outcome.handoff_pending {
            eprintln!("restore pointer committed; startup must resume late notification handoff");
        }
        restart_account_app(&app)
    }
}

fn write_export_json(db_path: &str, json: &str, prefix: &str) -> CmdResult<String> {
    let dir = std::path::Path::new(db_path)
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
    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut file = None;
    for n in 1..=99 {
        let candidate = if n == 1 {
            dir.join(format!("{prefix}-{stamp}.json"))
        } else {
            dir.join(format!("{prefix}-{stamp}-{n}.json"))
        };
        if !candidate.exists() {
            file = Some(candidate);
            break;
        }
    }
    let file = file.ok_or_else(|| "同一秒已有过多导出文件，请稍后重试".to_string())?;
    // Atomic write so an interrupted export leaves no truncated file that
    // looks like a usable backup.
    solum_core::fsatomic::write_atomic(&file, json).map_err(|e| e.to_string())?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
    let dim: ProactivityDimension = dimension.parse().map_err(core_err)?;
    let lvl: ProactivityLevel = level.parse().map_err(core_err)?;
    lock!(state).set_proactivity(dim, lvl).map_err(core_err)
}

// ---- notification privacy (Phase 9) -----------------------------------------

#[tauri::command]
fn notif_cloud_get(state: State<AppState>) -> CmdResult<bool> {
    require_full_account()?;
    lock!(state).notif_cloud_enabled().map_err(core_err)
}

#[tauri::command]
fn notif_cloud_set(state: State<AppState>, enabled: bool) -> CmdResult<()> {
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
    lock!(state).acknowledge_capture_losses().map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_set_app(
    state: State<AppState>,
    app: tauri::AppHandle,
    package_name: String,
    enabled: bool,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
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

#[tauri::command]
fn notif_intelligence_set_capture_mode(
    state: State<AppState>,
    app: tauri::AppHandle,
    mode: solum_core::notification_intelligence::NotificationCaptureMode,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
    let target = {
        let o = lock!(state);
        let mut config = o.notification_intelligence_config().map_err(core_err)?;
        config.capture_mode = mode;
        config.normalized().map_err(core_err)?
    };
    write_notification_capture_policy(&state.db_path, &target)?;
    let config = lock!(state)
        .set_notification_capture_mode(mode)
        .map_err(core_err)?;
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
    require_full_account()?;
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
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state).auto_event_counts(now).map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_set_batch_interval(
    state: State<AppState>,
    minutes: u16,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .promote_notification_capture(id, now)
        .map_err(core_err)
}

#[tauri::command]
fn notif_intelligence_process_now(state: State<AppState>, now: Option<String>) -> CmdResult<usize> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .process_notification_batch(now)
        .map_err(core_err)
}

#[tauri::command]
fn notif_pipeline_status(app: tauri::AppHandle) -> CmdResult<solum_notif_access::PipelineStatus> {
    require_full_account()?;
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn notif_pipeline_request_battery_optimization(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
    app.notif_access()
        .request_ignore_battery_optimizations()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn notif_pipeline_open_battery_settings(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
    app.notif_access()
        .open_battery_settings()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn notif_pipeline_open_background_settings(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
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
    require_tool_account(&tool)?;
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
    require_tool_account(&tool)?;
    if tool == "backup_delete" {
        require_current_privacy_consent()?;
        require_backup_delete_is_not_in_restore(&state, &args)?;
    }
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
    require_tool_account(&tool)?;
    let backup_delete = tool == "backup_delete";
    let _backup_transition = if backup_delete {
        Some(AccountTransitionGuard::acquire(&state.account_transition)?)
    } else {
        None
    };
    if backup_delete {
        require_current_privacy_consent()?;
        require_backup_delete_is_not_in_restore(&state, &args)?;
    }
    let now = parse_now(now)?;
    let orch = state.orch.clone();
    let transition = state.account_transition.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if !backup_delete && transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
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
    require_full_account()?;
    let now = parse_now(now)?;
    Ok(lock!(state)
        .checkin_if_due(now)
        .map_err(core_err)?
        .map(checkin_resp))
}

// ---- suggestions (F10) --------------------------------------------------------------

#[tauri::command]
fn suggestions(state: State<AppState>) -> CmdResult<Vec<Suggestion>> {
    require_full_account()?;
    lock!(state).suggestions().map_err(core_err)
}

#[tauri::command]
fn suggest_generate(
    state: State<AppState>,
    days: i64,
    now: Option<String>,
) -> CmdResult<Vec<Suggestion>> {
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
    lock!(state).routines().map_err(core_err)
}

#[tauri::command]
fn routine_set_active(
    state: State<AppState>,
    id: i64,
    active: bool,
    now: Option<String>,
) -> CmdResult<()> {
    require_full_account()?;
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
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .update_routine(id, &title, &time_of_day, now)
        .map_err(core_err)
}

/// D4「一键已完成」：确认某条 routine 今天完成了（落行为日志，按日去重）。
#[tauri::command]
fn routine_done(state: State<AppState>, id: i64, now: Option<String>) -> CmdResult<String> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state).confirm_routine(id, now).map_err(core_err)
}

// ---- offline data review (D2) ---------------------------------------------------

#[tauri::command]
fn stats(state: State<AppState>, now: Option<String>) -> CmdResult<String> {
    require_full_account()?;
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
    require_full_account()?;
    let now = parse_now(now)?;
    let from = now - Duration::days(days.clamp(1, 365));
    let styled = styled.unwrap_or(false);
    let orch = state.orch.clone();
    let transition = state.account_transition.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
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
    require_full_account()?;
    lock!(state).rollback_persona(version).map_err(core_err)
}

// ---- notification-listener access (F1 companion) ---------------------------------

/// Whether Solum's notification-capture listener is enabled in OS settings.
/// Always `true` on desktop (no such permission exists there) so the
/// frontend only ever shows the nag banner where it's actually actionable.
#[tauri::command]
fn notif_access_status(app: tauri::AppHandle) -> CmdResult<bool> {
    require_full_account()?;
    app.notif_access().is_enabled().map_err(|e| e.to_string())
}

/// Jump to the notification-listener settings page (Android: Settings →
/// Notifications → Notification access). No runtime-request dialog exists
/// for this permission — the user must flip the toggle themselves.
#[tauri::command]
fn notif_access_open_settings(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
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
fn health_request(app: tauri::AppHandle) -> CmdResult<bool> {
    require_full_account()?;
    app.health_connect()
        .request_permissions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn health_samples(state: State<AppState>) -> CmdResult<Vec<HealthSample>> {
    require_full_account()?;
    lock!(state).health_samples().map_err(core_err)
}

#[tauri::command]
fn health_trends(
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
/// fallback: explicit `SOLUM_SYNC_CONFIG` first, then the active account profile
/// (or the historical app-data path for guest).
fn sync_config_file() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SOLUM_SYNC_CONFIG") {
        return p.into();
    }
    solum_core::paths::resolve_profile_with_adoption("solum-sync.json")
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
    account_logged_in: bool,
    device_id: String,
}

#[tauri::command]
fn sync_config_get(state: State<AppState>) -> CmdResult<SyncConfigSettings> {
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
struct SyncSaveArgs {
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
fn sync_config_save(
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
fn sync_status(state: State<AppState>) -> CmdResult<SyncStatusDto> {
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
fn sync_gap_acknowledge(state: State<AppState>) -> CmdResult<()> {
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
fn sync_round(state: &AppState) -> CmdResult<solum_core::sync::SyncOutcome> {
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
        if state.account_transition.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_secs(60));
            continue;
        }
        let full_account = solum_core::account::AccountSession::load_full().is_some();
        let should_emit_daily_brief = full_account
            && state
                .last_daily_brief_date
                .lock()
                .map(|last| daily_brief_is_due(*last, now.date()))
                .unwrap_or(false);
        let (fired_msgs, checkin, fresh, captured, notification_batch, daily_brief) =
            match state.orch.lock() {
                Ok(mut o) if !state.account_transition.load(Ordering::Acquire) => {
                    let captured = if full_account {
                        drain_capture_inbox(&mut o, &state.db_path, now)
                    } else {
                        Vec::new()
                    };
                    let notification_batch = if full_account {
                        o.notification_intelligence_config()
                            .ok()
                            .filter(|config| {
                                now.minute()
                                    .is_multiple_of(u32::from(config.batch_interval_minutes))
                            })
                            .and_then(|_| o.process_notification_batch(now).ok())
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    // D4: account routines materialize upcoming occurrences
                    // before delivery. Guest reminders remain local and manual.
                    if full_account {
                        let _ = o.materialize_routines(now);
                    }
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
                    let checkin = full_account
                        .then(|| o.checkin_if_due(now).ok().flatten())
                        .flatten();
                    let fresh = if full_account {
                        o.auto_generate_suggestions(now).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
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
                Ok(_) => (Vec::new(), None, Vec::new(), Vec::new(), 0, None),
            };

        if state.account_transition.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_secs(60));
            continue;
        }

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
        if full_account && tick_count.is_multiple_of(5) {
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
                            Ok(mut o) if !state.account_transition.load(Ordering::Acquire) => {
                                o.record_health_samples(samples, now)
                            }
                            Ok(_) => Err(solum_core::CoreError::Invalid(
                                "profile is switching".into(),
                            )),
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
        if full_account && tick_count.is_multiple_of(5) {
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
                    if let Some(reason) = &r.push_block {
                        eprintln!(
                            "[sync] 推送未完成：{reason}；本轮 {} 条本地变更未上传（拉取不受影响，下轮重试）",
                            r.push_skipped
                        );
                        let _ = app.emit("solum-sync-push-block", reason.clone());
                    } else if r.push_skipped > 0 {
                        eprintln!(
                            "[sync] {} 条本地变更因超过单条体积上限被滞留",
                            r.push_skipped
                        );
                    }
                    if let Some(reason) = &r.push_failed {
                        eprintln!("[sync] 推送失败（{reason}），本轮只完成拉取，下轮重试");
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

struct ResolvedProfilePaths {
    db_path: String,
    /// Device credentials/config stay at the profile root even after the user
    /// data plane switches to a restored generation.
    config_dir: std::path::PathBuf,
    active_generation: Option<String>,
}

/// Resolve both the active user-data SQLite and the device-config root.
/// `SOLUM_DB` always wins. Mobile follows the durable generation pointer when
/// present; desktop remains on the migration-era single profile path.
///
/// Both branches first adopt a `pa.sqlite` left by the 2026-07-20 rename, so an
/// existing store keeps its data rather than starting empty. On Android this is
/// effectively a no-op — the identifier changed too, so the old install's data
/// is in another package's private storage (see `adopt_legacy_db`).
fn resolve_profile_paths(
    #[cfg_attr(desktop, allow(unused_variables))] app: &tauri::App,
) -> solum_core::Result<ResolvedProfilePaths> {
    if let Ok(p) = std::env::var("SOLUM_DB") {
        let config_dir = std::path::Path::new(&p)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
            .to_path_buf();
        return Ok(ResolvedProfilePaths {
            db_path: p,
            config_dir,
            active_generation: None,
        });
    }
    #[cfg(mobile)]
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let profile_root = if let Some(profile) = account_profile_root(&dir) {
            profile
        } else {
            let current = dir.join("solum.sqlite");
            report_legacy_adoption(&dir.join("pa.sqlite"), &current);
            dir
        };
        let active = solum_core::profile_generation::resolve_active_data_dir(&profile_root)?;
        let active_generation = active.generation_id().map(str::to_owned);
        if let Some(generation) = active_generation.as_deref() {
            solum_core::portable_generation::resume_portable_handoff(&profile_root, generation)?;
        }
        if let Some(session) = solum_core::account::AccountSession::load_full() {
            if let Some(user_id) = session.stable_user_id() {
                solum_core::backup_takeover::reconcile_published_takeover(
                    &profile_root,
                    user_id,
                    &session.server_url,
                    active_generation.as_deref(),
                )
                .map_err(|_| {
                    solum_core::CoreError::Invalid(
                        "backup takeover journal failed startup reconciliation".into(),
                    )
                })?;
            }
        }
        return Ok(ResolvedProfilePaths {
            db_path: active
                .path()
                .join(solum_core::profile_generation::SQLITE_FILE)
                .to_string_lossy()
                .into_owned(),
            config_dir: profile_root,
            active_generation,
        });
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
    let base = solum_core::paths::app_data_dir();
    if let Some(profile) = base.as_deref().and_then(account_profile_root) {
        let db_path = profile.join("solum.sqlite").to_string_lossy().into_owned();
        return Ok(ResolvedProfilePaths {
            db_path,
            config_dir: profile,
            active_generation: None,
        });
    }
    let db_path = solum_core::paths::resolve_with_adoption("solum.sqlite")
        .to_string_lossy()
        .into_owned();
    let config_dir = std::path::Path::new(&db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""))
        .to_path_buf();
    Ok(ResolvedProfilePaths {
        db_path,
        config_dir,
        active_generation: None,
    })
}

fn active_local_profile_id() -> String {
    solum_core::account::AccountSession::load_full()
        .and_then(|session| session.stable_user_id().map(str::to_owned))
        .unwrap_or_else(|| "guest".to_string())
}

fn account_profile_root(base: &std::path::Path) -> Option<std::path::PathBuf> {
    let session = solum_core::account::AccountSession::load_full()?;
    solum_core::paths::account_profile_dir(base, session.stable_user_id()?)
}

/// Set a profile-scoped config path while preserving a path explicitly supplied
/// by the operator. The marker is inherited by `app.restart()`, allowing paths
/// created by Solum itself to follow the newly selected profile on next boot.
fn set_managed_profile_config(
    variable: &str,
    marker: &str,
    profile_dir: Option<&std::path::Path>,
    filename: &str,
) {
    let managed = std::env::var(marker).as_deref() == Ok("1");
    if !managed && std::env::var_os(variable).is_some() {
        return;
    }
    let Some(dir) = profile_dir else { return };
    std::env::set_var(variable, dir.join(filename));
    std::env::set_var(marker, "1");
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
            // The account session is device-global because it decides which
            // local profile to open. Point mobile at app-data before resolving
            // the database; doing this afterwards would always open `guest`.
            #[cfg(mobile)]
            if std::env::var("SOLUM_ACCOUNT_CONFIG").is_err() {
                if let Ok(dir) = app.path().app_data_dir() {
                    std::env::set_var("SOLUM_ACCOUNT_CONFIG", dir.join("solum-account.json"));
                }
            }
            let resolved_profile = resolve_profile_paths(app)?;
            let db_path = resolved_profile.db_path;
            let profile_root = resolved_profile.config_dir.clone();
            let active_generation = resolved_profile.active_generation;
            let profile_dir = Some(resolved_profile.config_dir.as_path());
            // Every business config follows the selected SQLite profile. The
            // managed marker lets an app.restart() recalculate our own env
            // override without replacing an operator-supplied path.
            set_managed_profile_config(
                "SOLUM_LLM_CONFIG",
                "SOLUM_LLM_CONFIG_MANAGED",
                profile_dir,
                "solum-llm.json",
            );
            set_managed_profile_config(
                "SOLUM_SOULOUS_CONFIG",
                "SOLUM_SOULOUS_CONFIG_MANAGED",
                profile_dir,
                "solum-soulous.json",
            );
            set_managed_profile_config(
                "SOLUM_EMAIL_CONFIG",
                "SOLUM_EMAIL_CONFIG_MANAGED",
                profile_dir,
                "solum-email.json",
            );
            set_managed_profile_config(
                "SOLUM_SYNC_CONFIG",
                "SOLUM_SYNC_CONFIG_MANAGED",
                profile_dir,
                "solum-sync.json",
            );
            // Consent is device-local too. Android has no usable working
            // directory, so it needs the same app-data injection as the
            // database and credential files above.
            #[cfg(mobile)]
            if std::env::var("SOLUM_PRIVACY_CONSENT").is_err() {
                if let Ok(dir) = app.path().app_data_dir() {
                    std::env::set_var(
                        "SOLUM_PRIVACY_CONSENT",
                        dir.join("solum-privacy-consent.json"),
                    );
                }
            }
            let mut orch = Orchestrator::open(&db_path)?;
            // The account proxy is the only cloud path. Guest mode is always
            // offline even when an older direct-key file remains on disk.
            let llm_summary =
                if let Some(session) = solum_core::account::AccountSession::load_full() {
                    let summary = format!("账号 · {}", session.masked_summary());
                    orch.set_reasoner(Box::new(solum_core::account::AccountReasoner::new(session)));
                    Some(summary)
                } else {
                    None
                };
            app.manage(AppState {
                orch: Arc::new(Mutex::new(orch)),
                db_path: db_path.clone(),
                profile_root,
                active_generation,
                webview_restore: Mutex::new(None),
                backup_upload: Mutex::new(None),
                backup_restore: Mutex::new(None),
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
                account_transition: Arc::new(AtomicBool::new(false)),
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
                if let Err(error) = sync_notification_pipeline(app.handle(), &config) {
                    // F16: a foreground-service/ROM failure may delay batch
                    // work, never the local database or alarm delivery path.
                    eprintln!("[notification-intelligence] foreground service: {error}");
                }
            }
            // POST_NOTIFICATIONS is requested by the frontend only after the
            // user accepts the in-app privacy policy.
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
            ledger_page,
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
            notif_intelligence_set_capture_mode,
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
            health_trends,
            llm_config_get,
            llm_config_save,
            llm_config_test,
            account_status_get,
            account_login,
            account_register,
            account_oauth_config_get,
            account_oauth_pkce_generate,
            account_oauth_login,
            account_logout,
            account_model_save,
            account_change_password,
            recovery_code_status,
            recovery_code_issue,
            recovery_code_login,
            privacy_consent_status,
            privacy_consent_accept,
            privacy_consent_decline,
            notification_permission_request,
            reminder_delivery_health,
            debug_simulate_process_death,
            capture_entry_points,
            capture_inbox_list,
            capture_inbox_add,
            capture_inbox_discard,
            capture_android_share_drain,
            capture_android_voice_start,
            capture_android_ocr_pick,
            calendar_access_status,
            calendar_access_request,
            calendar_candidates_import,
            calendar_open_insert,
            soulous_config_get,
            soulous_config_save,
            soulous_pull,
            email_config_get,
            email_config_save,
            email_config_remove,
            email_folders,
            email_messages,
            email_high_value_scan,
            email_search,
            email_message,
            email_oauth_begin,
            email_oauth_poll,
            snooze,
            fact_update,
            export_data,
            export_portable_data,
            backup_upload_begin,
            backup_upload_attachment_chunk,
            backup_upload_attachment_finish,
            backup_upload_commit,
            backup_upload_cancel,
            backup_restore_catalog,
            backup_restore_stage,
            backup_restore_pending,
            backup_restore_takeover_publish,
            restore_webview_begin,
            restore_webview_document_chunk,
            restore_webview_finish,
            restore_webview_status,
            restore_publish_preview,
            event_reschedule,
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

    /// The native listener and Rust must select the same account directory.
    /// A root-level spool while SQLite lives under `profiles/<uuid>` silently
    /// strands notifications and makes a same-profile backup impossible.
    #[test]
    fn android_listener_publishes_spool_and_overflow_under_one_frozen_profile() {
        let kotlin = include_str!(
            "../gen/android/app/src/main/java/dev/solum/app/SolumNotificationListener.kt"
        );
        assert!(kotlin.contains("handleNotificationPostedLocked(sbn, profileRoot)"));
        assert!(kotlin.contains("channel.lock().use"));
        assert!(kotlin.contains("active-generation.lock"));
        assert!(kotlin.contains("val profileDir = activeGenerationDir(profileRoot) ?: return"));
        assert!(kotlin.contains("activeGenerationDir(profileRoot)"));
        assert!(kotlin.contains("active-generation.json"));
        assert!(kotlin.contains("generation-manifest.json"));
        assert!(kotlin.contains("Files.notExists(pointerPath"));
        assert!(kotlin.contains("File(profileDir, SPOOL_DIR)"));
        assert!(kotlin.contains("File(profileDir, OVERFLOW_DIR)"));
        assert!(!kotlin.contains("File(dataDir, SPOOL_DIR)"));
        assert!(!kotlin.contains("File(dataDir, OVERFLOW_DIR)"));
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

    #[test]
    fn web_storage_names_keep_legacy_compatible_and_isolate_generations() {
        let user = "11111111-1111-4111-8111-111111111111";
        let generation = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            web_storage_names("guest", None).unwrap(),
            WebStorageNames {
                ui_state_key: "solum.ui-state.v1".into(),
                chat_sessions_key: "pa.chat-sessions.v1".into(),
                document_db_name: "pa-local-documents-v1".into(),
            }
        );
        assert_eq!(
            web_storage_names(user, None).unwrap().chat_sessions_key,
            format!("pa.chat-sessions.v1.{user}")
        );
        let restored = web_storage_names(user, Some(generation)).unwrap();
        assert_eq!(
            restored.document_db_name,
            format!("pa-local-documents-v1.{user}.generation.{generation}")
        );
        assert_ne!(restored, web_storage_names(user, None).unwrap());
        assert!(web_storage_names(user, Some("../escape")).is_err());
    }

    #[test]
    fn webview_hydration_receipt_requires_exact_profile_generation_and_ordered_set() {
        let expected = vec![WebViewDocumentReceipt {
            id: "document-1".into(),
            plaintext_size: 7,
            sha256: "01".repeat(32),
        }];
        let valid = WebViewHydrationReceipt {
            profile_id: "guest".into(),
            generation: "0123456789abcdef0123456789abcdef".into(),
            documents: vec![WebViewDocumentReceipt {
                id: "document-1".into(),
                plaintext_size: 7,
                sha256: "01".repeat(32),
            }],
        };
        assert!(validate_webview_hydration_receipt(
            "guest",
            "0123456789abcdef0123456789abcdef",
            &expected,
            &valid
        )
        .is_ok());
        let missing = WebViewHydrationReceipt {
            documents: Vec::new(),
            ..valid
        };
        assert!(validate_webview_hydration_receipt(
            "guest",
            "0123456789abcdef0123456789abcdef",
            &expected,
            &missing
        )
        .is_err());
    }

    #[test]
    fn frontend_hydrates_only_inactive_generation_and_does_not_publish_pointer() {
        let js = include_str!("../dist/app.js");
        assert!(js.contains("hydratePortableGenerationWebView(generationId)"));
        assert!(js.contains("restore_webview_begin"));
        assert!(js.contains("restore_webview_document_chunk"));
        assert!(js.contains("restore_webview_finish"));
        assert!(js.contains("store.clear()"));
        assert!(js.contains("await sha256Hex(await actual.blob.arrayBuffer())"));
        assert!(js.contains("safeLocalGet(storage.chat_sessions_key) !== chatJson"));
        assert!(!js.contains("publish_profile_generation"));
    }

    #[test]
    fn publication_binding_rejects_token_sqlite_and_webview_changes() {
        let webview = LiveWebViewSummary {
            profile_id: "guest".into(),
            chat_sessions: 1,
            chat_messages: 2,
            drafts: 1,
            documents: 0,
            fingerprint: "11".repeat(32),
        };
        assert!(validate_live_webview_summary(&webview, "guest").is_ok());
        let binding = RestorePublicationBinding {
            token: "22".repeat(32),
            sqlite_fingerprint: "33".repeat(32),
            webview: webview.clone(),
        };
        assert!(validate_publication_binding(
            &binding,
            &"22".repeat(32),
            &"33".repeat(32),
            &webview
        )
        .is_ok());
        assert!(validate_publication_binding(
            &binding,
            &"44".repeat(32),
            &"33".repeat(32),
            &webview
        )
        .is_err());
        assert!(validate_publication_binding(
            &binding,
            &"22".repeat(32),
            &"55".repeat(32),
            &webview
        )
        .is_err());
        let mut changed_webview = webview;
        changed_webview.fingerprint = "66".repeat(32);
        assert!(validate_publication_binding(
            &binding,
            &"22".repeat(32),
            &"33".repeat(32),
            &changed_webview
        )
        .is_err());
    }

    #[test]
    fn frontend_publication_requires_fresh_exact_webview_snapshot() {
        let js = include_str!("../dist/app.js");
        assert!(js.contains("snapshotLiveWebViewSummary()"));
        assert!(js.contains("saveCurrentChatDraft(true)"));
        assert!(js.contains("chat_json: chatJson"));
        assert!(js.contains("ui_state_json: uiStateJson"));
        assert!(js.contains("await sha256Hex(await row.blob.arrayBuffer())"));
        assert!(js.contains("restore_publish_preview"));
        assert!(js.contains("backup_restore_takeover_publish"));
        assert!(!js.contains("restore_publish_confirm"));
        assert!(js.contains("const webview = await snapshotLiveWebViewSummary();"));
    }

    #[test]
    fn frontend_backup_seam_streams_bounded_chunks_and_commits_last() {
        let js = include_str!("../dist/app.js");
        assert!(js.contains("const BACKUP_UPLOAD_IPC_CHUNK_BYTES = 512 * 1024"));
        assert!(js.contains("async function createAndUploadPortableBackup()"));
        assert!(js.contains("dataBase64: encodeCanonicalBase64(chunk)"));
        assert!(js.contains("row.blob.size !== expected.plaintext_size"));
        let begin = js.find("backup_upload_begin").unwrap();
        let chunk = js.find("backup_upload_attachment_chunk").unwrap();
        let finish = js.find("backup_upload_attachment_finish").unwrap();
        let commit = js.find("backup_upload_commit").unwrap();
        assert!(begin < chunk && chunk < finish && finish < commit);
        assert!(js[commit..].contains("backup_upload_cancel"));
    }

    #[test]
    fn frontend_backup_center_is_manual_and_recovery_requires_preview_takeover() {
        let html = include_str!("../dist/index.html");
        let js = include_str!("../dist/app.js");
        assert!(html.contains("id=\"view-backup\""));
        assert!(html.contains("id=\"backupCreateBtn\""));
        assert!(js.contains("createAndUploadPortableBackup()"));
        assert!(!js.contains("setInterval(createAndUploadPortableBackup"));
        let stage = js.find("backup_restore_stage").unwrap();
        let hydrate = js[stage..]
            .find("hydratePortableGenerationWebView")
            .unwrap()
            + stage;
        let preview = js[hydrate..].find("prepareRestorePublication").unwrap() + hydrate;
        let takeover = js[preview..]
            .find("backup_restore_takeover_publish")
            .unwrap()
            + preview;
        assert!(stage < hydrate && hydrate < preview && preview < takeover);
        assert!(js.contains("替换本机并接管"));
        assert!(html.contains("尚未改动本机"));
        assert!(js.contains("tool: \"backup_delete\""));
        assert!(js.contains("plan_etag: version.plan_etag"));
        assert!(js.contains("永久删除此版本"));
        assert!(js.contains("onError: async () => { await refreshBackupCenter(); }"));
    }

    #[test]
    fn android_reminder_delivery_has_a_startup_tripwire_and_debug_process_death_seam() {
        let js = include_str!("../dist/app.js");
        let plugin =
            include_str!("../../solum-alarm/android/src/main/java/dev/solum/alarm/AlarmPlugin.kt");
        let activity =
            include_str!("../gen/android/app/src/main/java/dev/solum/app/MainActivity.kt");
        assert!(plugin.contains("getReceiverInfo"));
        assert!(plugin.contains("startupReceiverRegistered"));
        assert!(plugin.contains("P1: AlarmReceiver is missing or disabled"));
        assert!(js.contains("reminder_delivery_health"));
        assert!(js.contains("当前安装包缺少后台提醒接收器"));
        assert!(js.contains("debug_simulate_process_death"));
        assert!(activity.contains("if (!BuildConfig.DEBUG) return false"));
        assert!(activity.contains("android.os.Process.killProcess"));
        assert!(!activity.contains("forceStopPackage"));
    }

    #[test]
    fn local_capture_stays_available_to_guests_and_dynamic_styles_survive_csp() {
        let html = include_str!("../dist/index.html");
        let js = include_str!("../dist/app.js");
        let config = include_str!("../tauri.conf.json");
        assert!(js.contains(
            "const GUEST_ALLOWED_VIEWS = new Set([\"chat\", \"agenda\", \"notifs\", \"ledger\", \"journal\", \"search\", \"settings\", \"privacy\", \"capture\"])"
        ));
        assert!(config.contains("\"dangerousDisableAssetCspModification\": [\"style-src\"]"));
        assert!(config.contains("style-src 'self' 'unsafe-inline'"));
        assert!(!config.contains("script-src 'self' 'unsafe-inline'"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("<script src=\"app.js\"></script>"));
        assert!(js.contains("meta capture-preview"));
        assert!(js.contains("previewChars.slice(0, 240)"));
    }
}

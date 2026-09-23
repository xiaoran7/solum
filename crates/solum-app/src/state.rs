// crates/solum-app/src/state.rs
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::Serialize;
use solum_core::extract::Intent;
use solum_core::notification_intelligence::NotificationIntelligenceConfig;
use solum_core::Orchestrator;
use solum_notif_access::NotifAccessExt;

#[macro_export]
macro_rules! lock {
    ($state:expr) => {{
        let guard = $state
            .orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if $state
            .account_transition
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err("Solum profile is switching; reopen the app to continue".to_string());
        }
        guard
    }};
}

pub(crate) type CmdResult<T> = Result<T, String>;

pub(crate) struct AppState {
    /// `Arc` so cloud-bound commands can move a handle into
    /// `spawn_blocking` — cloud HTTP on the main thread froze the whole
    /// window (Windows「未响应」, 2026-07-18 走查发现 1).
    pub(crate) orch: Arc<Mutex<Orchestrator>>,
    pub(crate) db_path: String,
    /// Device/profile root outside any mutable data generation. Restore
    /// readers may open sealed staging children here, never arbitrary paths.
    pub(crate) profile_root: PathBuf,
    pub(crate) active_generation: Option<String>,
    pub(crate) webview_restore: Mutex<Option<WebViewRestoreSession>>,
    /// One frozen encrypted upload attempt. It owns only ciphertext staging
    /// plus at most the attachment chunk currently being sealed; account or
    /// profile changes make every command discard it.
    pub(crate) backup_upload: Mutex<Option<BackupUploadAppSession>>,
    /// Authenticated cloud version bound to the sealed generation prepared by
    /// the restore downloader. It contains metadata only; a durable takeover
    /// journal replaces it once the user confirms publication.
    pub(crate) backup_restore: Mutex<Option<BackupRestoreAppSession>>,
    /// Masked provider summary when the cloud reasoner is configured.
    /// Mutable: the settings UI can save/apply a new endpoint at runtime.
    pub(crate) llm_summary: Mutex<Option<String>>,
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
    pub(crate) health_since_ms: Mutex<i64>,
    /// Signature of the alarm set last pushed to the OS (Android AlarmManager
    /// mirror, see `resync_alarms`). 0 = nothing pushed yet.
    pub(crate) alarm_sig: Mutex<u64>,
    /// Date whose focus brief has already been offered to this running app.
    /// A brief is a transient UI projection, so this small delivery gate does
    /// not belong in the durable store or sync layer.
    pub(crate) last_daily_brief_date: Mutex<Option<NaiveDate>>,
    /// OAuth callback codes and PKCE verifiers are ephemeral. They never
    /// enter SQLite or the durable mail config; a completed/expired session is
    /// removed immediately after polling.
    pub(crate) email_oauth: Mutex<HashMap<String, EmailOAuthSession>>,
    /// A second SQLite connection used only by sync.
    ///
    /// Sync is network-bound and can block for as long as its request timeout;
    /// running it through `orch` meant a slow or half-dead relay held the one
    /// lock that reminders, the ticker, and every UI command also need — the
    /// app froze for reasons that had nothing to do with the user. SQLite in
    /// WAL mode is happy with concurrent connections, so sync gets its own and
    /// touches `orch` only for the brief cache reload afterwards.
    pub(crate) sync_store: Mutex<solum_core::store::Store>,
    /// 多入口采集的待确认队列（进程内，不落盘——见 `solum_core::capture`）。
    /// 放 `AppState` 而不是全局静态，是因为它的生命周期本来就该跟这个窗口一致。
    pub(crate) capture_inbox: Mutex<solum_core::capture::CaptureInbox>,
    /// Login, registration and logout all replace the process-wide identity.
    /// Only one transition may issue or publish tokens at a time.
    pub(crate) account_transition: Arc<AtomicBool>,
}

pub(crate) struct WebViewRestoreSession {
    pub(crate) profile_id: String,
    pub(crate) reader: solum_core::portable_generation::PortableGenerationReader,
    pub(crate) ready: bool,
    pub(crate) publication_preview: Option<RestorePublicationBinding>,
}

pub(crate) struct BackupUploadAppSession {
    pub(crate) profile_id: String,
    pub(crate) account: solum_core::account::AccountSession,
    pub(crate) upload: solum_core::backup_upload::BackupUploadSession,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) struct BackupRestoreAppSession {
    pub(crate) profile_id: String,
    pub(crate) account: solum_core::account::AccountSession,
    pub(crate) backup_id: String,
    pub(crate) cloud_generation: u64,
    pub(crate) staged_generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestorePublicationBinding {
    pub(crate) token: String,
    pub(crate) sqlite_fingerprint: String,
    pub(crate) webview: crate::commands::LiveWebViewSummary,
}

pub(crate) struct AccountTransitionGuard<'a>(pub(crate) &'a AtomicBool);

impl<'a> AccountTransitionGuard<'a> {
    pub(crate) fn acquire(flag: &'a AtomicBool) -> CmdResult<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self(flag))
            .map_err(|_| "账号正在切换，请等待当前操作完成".to_string())
    }

    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn hold_until_restart(self) {
        std::mem::forget(self);
    }
}

impl Drop for AccountTransitionGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) struct EmailOAuthCallback {
    pub(crate) code: String,
    pub(crate) state: String,
}

pub(crate) struct EmailOAuthSession {
    pub(crate) account_id: String,
    pub(crate) state: String,
    pub(crate) code_verifier: String,
    pub(crate) redirect_uri: String,
    pub(crate) receiver: mpsc::Receiver<CmdResult<EmailOAuthCallback>>,
}

/// Durable cursor for the Health Connect poll window (epoch ms). Device-local
/// like the sync cursors it sits beside — a poll position is not memory.
pub(crate) const HEALTH_SINCE_KEY: &str = "health_poll_since_ms";

pub(crate) fn require_full_account() -> CmdResult<()> {
    if solum_core::account::AccountSession::load_full().is_some() {
        Ok(())
    } else {
        Err("这项完整能力需要先注册或登录 Solum 账号".into())
    }
}

pub(crate) fn tool_requires_full_account(tool: &str) -> bool {
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

pub(crate) fn require_tool_account(tool: &str) -> CmdResult<()> {
    if tool_requires_full_account(tool) {
        require_full_account()?;
    }
    Ok(())
}

pub(crate) fn parse_now(now: Option<String>) -> CmdResult<NaiveDateTime> {
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

pub(crate) fn intent_str(intent: Intent) -> &'static str {
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

pub(crate) fn core_err(e: solum_core::CoreError) -> String {
    e.to_string()
}

pub(crate) fn daily_brief_is_due(last_emitted: Option<NaiveDate>, today: NaiveDate) -> bool {
    last_emitted != Some(today)
}

/// The Android listener reads only this tiny local projection before appending
/// an inbox line. Missing/unreadable remains fail-closed during a concurrent
/// settings write; a valid policy defaults to all-app capture.
pub(crate) fn write_notification_capture_policy(
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

pub(crate) fn sync_notification_pipeline(
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
pub(crate) struct AppInfo {
    pub(crate) db_path: String,
    /// `guest` or the immutable account UUID. The WebView uses the same value
    /// to partition chat/UI/IndexedDB state that never enters SQLite.
    pub(crate) local_profile_id: String,
    pub(crate) active_generation: Option<String>,
    pub(crate) web_storage: crate::commands::WebStorageNames,
    pub(crate) system_now: String,
    /// e.g. "https://api.example.com/v1 · default-model · no key"; `None` = offline mode.
    pub(crate) llm: Option<String>,
}

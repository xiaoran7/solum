#![allow(linker_messages)] // MSVC prints normal import-library progress to stdout.

//! solum-app — the Tauri 2 desktop shell for the Solum Phase 1 core.
//!
//! This is a thin adapter: every command locks the shared [] and
//! forwards to , exactly like  does. No business logic lives
//! here — the UI can only do what the core exposes, and the HITL guard's
//! compile-time guarantees carry over untouched (the frontend never sees a
//! ; confirming in the dialog calls  +  on the Rust
//! side).
//!
//! The clock is injectable end-to-end: every command accepts an optional 
//! string (from the UI's "模拟时钟"); absent means the real system clock.
//!
//! Phase 2 turns the shell into a resident host: a background ticker (system
//! clock only — the simulated clock stays a UI demo device) fires due
//! reminders, asks status check-ins, and auto-generates suggestions, surfacing
//! each through OS notifications and window events.
//!
//! Phase 3 makes the same crate the mobile shell: the crate is a library so
//! Tauri's Android runner can link it (), and the desktop
//! binary () is a thin wrapper around [].

mod portable_snapshot;
use solum_core::recovery_code::RecoveryCodeTransport as _;

#[macro_use]
pub(crate) mod state;
pub(crate) use state::*;

pub(crate) mod profile;
pub(crate) use profile::*;

pub(crate) mod ticker;
pub(crate) use ticker::*;

#[macro_use]
mod commands;
pub(crate) use commands::*;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use chrono::{Duration, Local, NaiveDateTime, SecondsFormat, Utc};
#[cfg(test)]
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use solum_alarm::AlarmExt;
use solum_health_connect::HealthConnectExt;
use solum_notif_access::NotifAccessExt;
use tauri::{Emitter, Manager, State};

use solum_core::classify::{ImportanceRule, LeadTime};
use solum_core::email::{
    EmailAccount, EmailAuth, EmailConfig, EmailEndpoints, EmailProvider, SmtpTls,
};
use solum_core::journal::BehaviorEntry;
use solum_core::llm::{ChatTurn, MAX_HISTORY_TURNS};
use solum_core::model::{Channel, Event, EventKind, MemoryEntry, Notification};
use solum_core::notification_intelligence::NotificationIntelligenceConfig;
use solum_core::persona::{PersonaDraft, PersonaProfile};
use solum_core::proactivity::{ProactivityDimension, ProactivityLevel};
use solum_core::suggest::{Suggestion, SuggestionStatus};
use solum_core::wearable::{HealthMetric, HealthSample};
use solum_core::Orchestrator;


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
            account_email_register,
            account_email_login,
            account_sms_send,
            account_phone_register,
            account_phone_login,
            account_huawei_config_get,
            account_huawei_login,
            account_delete,
            account_oauth_config_get,
            account_oauth_pkce_generate,
            account_oauth_login,
            account_oauth_android_drain,
            open_external_url,
            account_logout,
            account_model_save,
            account_change_password,
            recovery_code_status,
            recovery_code_issue,
            recovery_code_login,
            recovery_code_pending,
            recovery_code_pending_ack,
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
#[cfg(test)]
mod tests;

// crates/solum-app/src/ticker.rs
use std::sync::atomic::Ordering;

use chrono::{Local, NaiveDateTime, TimeZone, Timelike};
use solum_alarm::AlarmExt;
use solum_health_connect::HealthConnectExt;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use solum_core::notification_intelligence::{
    CaptureLane, CaptureState, NotificationCapture,
};
use solum_core::wearable::HealthSample;
use crate::state::*;
use crate::commands::*;

// ---- resident ticker --------------------------------------------------------------

pub(crate) fn system_now() -> NaiveDateTime {
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
pub(crate) fn drain_capture_inbox(
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
pub(crate) fn collect_overflow_markers(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
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
pub(crate) fn record_and_remove_overflow_markers(
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
pub(crate) fn parked_payload_count(db_path: &str) -> i64 {
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
pub(crate) fn spool_attempt(path: &std::path::Path) -> u32 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.rsplit_once(".try"))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(1)
}

/// The stem with any `.tryN` suffix removed.
pub(crate) fn spool_base(path: &std::path::Path) -> String {
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
pub(crate) struct IngestBatch {
    pub(crate) outcomes: Vec<String>,
    pub(crate) failed: usize,
}

/// The resident heartbeat: every minute, on the **system clock**, deliver due
/// reminders, ask a check-in when due, and auto-generate suggestions. The
/// UI's simulated clock never drives this — it remains a demo device.
pub(crate) fn ticker(app: tauri::AppHandle) {
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
pub(crate) fn resync_alarms(app: &tauri::AppHandle) {
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


//! notif privacy. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::now;
use super::llm::CountingReasoner;

#[test]
fn notification_allowlist_is_deduped_and_visible_offline() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_notification_capture_mode(
        crate::notification_intelligence::NotificationCaptureMode::Allowlist,
    )
    .unwrap();
    let urgent = NotificationCapture {
        package_name: "com.tencent.mm".into(),
        title: "@我 明天下午3点在会议室开会".into(),
        body: String::new(),
        received_at: now(),
    };
    // Empty whitelist is a hard intake gate: no raw text is stored.
    assert!(o.capture_notification(urgent.clone()).unwrap().is_none());
    assert!(o.ledger().unwrap().is_empty());

    o.set_notif_cloud_enabled(false).unwrap();
    o.set_notification_app_enabled("com.tencent.mm", true)
        .unwrap();
    // Capture consent alone no longer writes to the calendar (2026-07-21
    // decision): the second, narrower grant is what authorizes that.
    o.set_notification_app_auto_event("com.tencent.mm", true)
        .unwrap();
    let captured = o.capture_notification(urgent.clone()).unwrap().unwrap();
    assert_eq!(captured.lane, CaptureLane::Urgent);
    let processed = o
        .process_urgent_notification(captured.id.unwrap(), now())
        .unwrap();
    assert_eq!(processed.state, CaptureState::EventCreated);
    assert!(processed.local_only, "scope is fixed at capture time");

    // Exact same source/content inside 10 minutes never reaches LLM work.
    let duplicate = o.capture_notification(urgent).unwrap().unwrap();
    assert_eq!(duplicate.state, CaptureState::Deduplicated);

    let ordinary = o
        .capture_notification(NotificationCapture {
            package_name: "com.tencent.mm".into(),
            title: "群公告已更新".into(),
            body: String::new(),
            received_at: now(),
        })
        .unwrap()
        .unwrap();
    assert_eq!(ordinary.lane, CaptureLane::Batch);
    assert_eq!(o.process_notification_batch(now()).unwrap(), 1);
    assert_eq!(
        o.notification_captures()
            .unwrap()
            .into_iter()
            .find(|capture| capture.id == ordinary.id)
            .unwrap()
            .state,
        CaptureState::NeedsReview,
        "offline uncertainty is visible rather than silently discarded"
    );

    let mut config = o.notification_intelligence_config().unwrap();
    config.filter_rules.push(NotificationFilterRule {
        id: "marketing".into(),
        pattern: "促销".into(),
        package_name: Some("com.tencent.mm".into()),
        matcher: NotificationMatchKind::Substring,
        reason: "用户确认的营销过滤".into(),
    });
    o.set_notification_intelligence_config(config).unwrap();
    let filtered = o
        .capture_notification(NotificationCapture {
            package_name: "com.tencent.mm".into(),
            title: "今日促销".into(),
            body: String::new(),
            received_at: now() + Duration::minutes(11),
        })
        .unwrap()
        .unwrap();
    assert_eq!(filtered.state, CaptureState::Filtered);
    o.restore_notification_capture(filtered.id.unwrap())
        .unwrap();
    assert_eq!(
        o.notification_captures()
            .unwrap()
            .into_iter()
            .find(|capture| capture.id == filtered.id)
            .unwrap()
            .state,
        CaptureState::Queued
    );
    assert!(o
        .ledger()
        .unwrap()
        .iter()
        .any(|entry| entry.layer == MemoryLayer::NotificationCapture));
}

#[test]
fn notification_intelligence_never_calls_llm_when_notification_cloud_is_off() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(CountingReasoner(calls.clone())));
    o.set_notification_app_enabled("com.tencent.mm", true)
        .unwrap();
    o.set_notif_cloud_enabled(false).unwrap();
    o.capture_notification(NotificationCapture {
        package_name: "com.tencent.mm".into(),
        title: "群公告已更新".into(),
        body: String::new(),
        received_at: now(),
    })
    .unwrap();
    o.process_notification_batch(now()).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    o.set_notif_cloud_enabled(true).unwrap();
    o.capture_notification(NotificationCapture {
        package_name: "com.tencent.mm".into(),
        title: "另一条群公告".into(),
        body: String::new(),
        received_at: now() + Duration::minutes(11),
    })
    .unwrap();
    o.process_notification_batch(now() + Duration::minutes(11))
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn stopping_capture_removes_only_that_apps_unmodified_presets() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_notification_app_enabled("com.tencent.mm", true)
        .unwrap();
    let user_rule = o
        .add_notification_priority_rule("紧急".into(), None, NotificationMatchKind::Substring)
        .unwrap();

    o.set_notification_app_enabled("com.tencent.mm", false)
        .unwrap();

    let remaining = o.rule_table().notification_priority_rules();
    assert!(
        remaining
            .iter()
            .all(|rule| !rule.id.starts_with("preset:com.tencent.mm:")),
        "stopping capture must retract the app's injected presets"
    );
    assert!(
        remaining.iter().any(|rule| rule.id == user_rule.id),
        "a user-created global rule must survive stopping capture"
    );
}

/// §3.8 v10 decoupling: notifications sync unconditionally (own relay,
/// end-to-end encrypted), but the capture-time cloud-LLM stamp travels with
/// the payload so a row barred on the capturing device stays barred on
/// every other device — otherwise the same leak reopens one hop away.
#[test]
fn local_only_stamp_travels_across_devices_while_the_row_still_syncs() {
    let mut a = Orchestrator::in_memory().unwrap();
    a.set_notif_cloud_enabled(false).unwrap();
    a.ingest_captured("张伟 明天下午3点在会议室开榛子品鉴会", "通知·com.x", now())
        .unwrap()
        .unwrap();

    let relay = crate::sync::tests::MemTransport::default();
    let cfg = crate::sync::tests::test_cfg();
    assert!(
        a.sync_now(&relay, &cfg).unwrap().pushed >= 3,
        "a local-only capture must still reach the user's own relay"
    );

    // B explicitly has the switch ON, so only the travelling stamp can
    // hold the line.
    let mut b = Orchestrator::in_memory().unwrap();
    b.set_notif_cloud_enabled(true).unwrap();
    assert!(b.notif_cloud_enabled().unwrap());
    assert!(b.sync_now(&relay, &cfg).unwrap().applied >= 3);

    assert!(
        b.ledger()
            .unwrap()
            .iter()
            .any(|e| e.summary.contains("榛子")),
        "the row itself must arrive on the second device"
    );
    assert!(
        b.recall("榛子", now()).unwrap().is_empty(),
        "capture-time cloud-LLM denial must survive the sync hop"
    );

    // The mirror case, which the old `[通知·` prefix guess gets wrong: a
    // capture allowed at intake must stay allowed after syncing, otherwise
    // enabling the switch on both devices still loses the context.
    a.set_notif_cloud_enabled(true).unwrap();
    a.ingest_captured("李雷 明天下午4点在会议室开栗子品鉴会", "通知·com.y", now())
        .unwrap()
        .unwrap();
    a.sync_now(&relay, &cfg).unwrap();
    b.sync_now(&relay, &cfg).unwrap();
    assert!(
        !b.recall("栗子", now()).unwrap().is_empty(),
        "a capture allowed at intake must remain LLM-eligible after syncing"
    );
}

/// PRIVACY.md §2: "先前标为仅本机的历史……不会因后来重新开启而被补传".
/// A capture stamped `local_only` at intake must never reach the cloud, even
/// if the user flips the switch on before the batch interval elapses.
#[test]
fn local_only_captures_are_never_backfilled_to_cloud_after_toggle() {
    struct RecordingReasoner(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl crate::extract::Reasoner for RecordingReasoner {
        fn complete(&self, _system: &str, user: &str) -> Result<String> {
            self.0.lock().unwrap().push(user.to_string());
            Ok(r#"{"decisions":[{"index":0,"kind":"keep"}]}"#.into())
        }
    }

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(RecordingReasoner(seen.clone())));
    o.set_notification_app_enabled("com.tencent.mm", true)
        .unwrap();

    // Cloud off at intake: this row is stamped local_only = 1.
    o.set_notif_cloud_enabled(false).unwrap();
    let capture = o
        .capture_notification(NotificationCapture {
            package_name: "com.tencent.mm".into(),
            title: "仅本机机密内容".into(),
            body: String::new(),
            received_at: now(),
        })
        .unwrap()
        .unwrap();

    // User re-enables before the batch timer fires. The queued row keeps its
    // capture-time scope, so it must stay out of the triage payload.
    o.set_notif_cloud_enabled(true).unwrap();
    o.process_notification_batch(now() + Duration::minutes(1))
        .unwrap();

    let capture_row = |o: &Orchestrator| {
        o.notification_captures()
            .unwrap()
            .into_iter()
            .find(|row| row.id == capture.id)
            .unwrap()
    };
    let first_reason = capture_row(&o).reason;
    assert!(
        first_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("捕获时关闭了「通知上云」")),
        "the initial local-only outcome must explain its immutable scope"
    );

    // A restore remains useful for deterministic extraction, but must not
    // turn into a hidden second chance to upload this captured text.
    o.restore_notification_capture(capture.id.unwrap()).unwrap();
    let restored = capture_row(&o);
    assert_eq!(restored.state, CaptureState::Queued);
    assert!(
        restored
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("仅重跑本机规则")),
        "restore must state that it only retries the offline path"
    );
    o.process_notification_batch(now() + Duration::minutes(2))
        .unwrap();

    let sent = seen.lock().unwrap().join("\n");
    assert!(
        !sent.contains("仅本机机密内容"),
        "local_only capture leaked into the cloud triage payload: {sent}"
    );
    let retried = capture_row(&o);
    assert_eq!(retried.state, CaptureState::NeedsReview);
    assert!(
        retried.reason.as_deref().is_some_and(
            |reason| reason.contains("仅本机规则未能确定") && reason.contains("不会发送到云端")
        ),
        "a failed local-only retry must explain both its outcome and privacy boundary"
    );
}

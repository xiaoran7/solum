//! `tests` -- undefined
//!
//! Part of the `notification_intelligence` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use chrono::NaiveDate;

use super::*;

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, 19)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

fn capture(pkg: &str, text: &str) -> NotificationCapture {
    NotificationCapture {
        package_name: pkg.into(),
        title: text.into(),
        body: String::new(),
        received_at: now(),
    }
}

#[test]
fn capture_defaults_to_all_and_allowlist_can_fail_closed() {
    let config = NotificationIntelligenceConfig::default();
    assert!(matches!(
        decide_intake(&capture("com.tencent.mm", "@我 开会"), &config, &[]),
        IntakeDecision::Lane(CaptureLane::Batch)
    ));

    let config = NotificationIntelligenceConfig {
        capture_mode: NotificationCaptureMode::Allowlist,
        ..Default::default()
    };
    assert_eq!(
        decide_intake(&capture("com.tencent.mm", "@我 开会"), &config, &[]),
        IntakeDecision::NotAllowed
    );

    let mut config = NotificationIntelligenceConfig {
        capture_mode: NotificationCaptureMode::Allowlist,
        ..Default::default()
    };
    config.allowed_packages.push("com.tencent.mm".into());
    let rules = priority_presets("com.tencent.mm");
    assert_eq!(
        decide_intake(
            &capture("com.tencent.mm", "@我 明天下午三点开会"),
            &config,
            &rules
        ),
        IntakeDecision::Lane(CaptureLane::Urgent)
    );
    assert_eq!(
        decide_intake(&capture("com.tencent.mm", "群公告已更新"), &config, &rules),
        IntakeDecision::Lane(CaptureLane::Batch)
    );
}

#[test]
fn confirmed_filter_beats_lane_and_fingerprint_is_stable() {
    let mut config = NotificationIntelligenceConfig::default();
    config.allowed_packages.push("com.x".into());
    config.filter_rules.push(NotificationFilterRule {
        id: "rule-1".into(),
        pattern: "促销".into(),
        package_name: Some("com.x".into()),
        matcher: NotificationMatchKind::Substring,
        reason: "营销".into(),
    });
    assert!(matches!(
        decide_intake(&capture("com.x", "今日促销"), &config, &[]),
        IntakeDecision::Filtered { .. }
    ));
    assert_eq!(content_hash("com.x", "a  b"), content_hash("com.x", "a b"));
}

#[test]
fn llm_batch_parser_has_no_ids_and_rejects_bad_indexes() {
    let captures = vec![
        capture("com.x", "明天下午三点开会"),
        capture("com.x", "促销"),
    ];
    let parsed = parse_batch_triage(
        r#"{"decisions":[{"index":0,"kind":"event","title":"开会","event_kind":"meeting","start":"2026-07-20T15:00:00","location":null,"people":[]},{"index":1,"kind":"filter","pattern":"促销","reason":"营销"},{"index":99,"kind":"keep"}]}"#,
        &captures,
        now(),
    );
    assert_eq!(parsed.len(), 2);
    assert!(matches!(parsed[0], LlmTriageDecision::Event { .. }));
    assert!(matches!(parsed[1], LlmTriageDecision::ProposeFilter { .. }));
    let action = parse_batch_triage(
        r#"{"decisions":[{"index":0,"kind":"action","action":"cancel_event","target":"开会","reason":"会议取消"}]}"#,
        &captures,
        now(),
    );
    assert!(matches!(
        action.as_slice(),
        [LlmTriageDecision::ProposeAction { intent, .. }]
            if intent.kind == NotificationActionKind::CancelEvent && intent.target == "开会"
    ));
    assert!(parse_batch_triage(
        r#"{"decisions":[{"index":0,"kind":"action","action":"cancel_event","target":"开会","id":7}]}"#,
        &captures,
        now(),
    )
    .is_empty());
    let (_, user) = batch_triage_prompt(&captures, now());
    assert!(!user.contains("id"));
}

#[test]
fn starter_rule_specs_and_fallback() {
    assert_eq!(
        starter_patterns_for_package("com.tencent.mm"),
        &["@我", "@所有人"]
    );
    assert_eq!(
        starter_patterns_for_package("com.alibaba.android.rimet"),
        &["DING", "紧急", "@"]
    );
    assert_eq!(
        starter_patterns_for_package("org.telegram.messenger"),
        DEFAULT_STARTER_PATTERNS
    );
}

//! `tests` -- undefined
//!
//! Part of the `genui` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

use crate::model::EventKind;
use crate::routine::Routine;
use chrono::NaiveDate;

fn now() -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, 14)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

fn text_env(json: &str) -> Option<UiEnvelope> {
    parse_envelope(json, ALLOWED_ACTIONS)
}

#[test]
fn parses_minimal_text_envelope_with_fences() {
    let raw =
        "```json\n{\"version\":1,\"components\":[{\"type\":\"text\",\"content\":\"你好\"}]}\n```";
    let env = text_env(raw).expect("valid");
    assert_eq!(env.text_summary(), "你好");
}

#[test]
fn rejects_unknown_component_version_and_junk() {
    // Unknown component type.
    assert!(text_env(r#"{"version":1,"components":[{"type":"iframe","content":"x"}]}"#).is_none());
    // Unknown extra field on a known component (deny_unknown_fields).
    assert!(text_env(
        r#"{"version":1,"components":[{"type":"text","content":"x","style":"red"}]}"#
    )
    .is_none());
    // Wrong version.
    assert!(text_env(r#"{"version":2,"components":[{"type":"text","content":"x"}]}"#).is_none());
    // Empty components / not JSON at all.
    assert!(text_env(r#"{"version":1,"components":[]}"#).is_none());
    assert!(text_env("hello there").is_none());
}

#[test]
fn action_whitelist_and_arg_schemas_enforced() {
    // Non-whitelisted command.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"删库","action":{"command":"drop_database","args":{}}}]}]}"#;
    assert!(text_env(raw).is_none());
    // Whitelisted command, wrong args shape.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"取消","action":{"command":"reminder_dismiss","args":{"id":"not-a-number"}}}]}]}"#;
    assert!(text_env(raw).is_none());
    // suggestion_set with an invalid status.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"x","action":{"command":"suggestion_set","args":{"id":1,"status":"deleted"}}}]}]}"#;
    assert!(text_env(raw).is_none());
    // A routine action needs a real boolean, not a stringly value.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"暂停","action":{"command":"routine_set_active","args":{"id":1,"active":"false"}}}]}]}"#;
    assert!(text_env(raw).is_none());
    // Correct one passes.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"取消","action":{"command":"reminder_dismiss","args":{"id":3}}}]}]}"#;
    assert!(text_env(raw).is_some());
}

#[test]
fn llm_action_subset_is_narrower() {
    assert_eq!(LLM_ACTIONS, ["ingest", "checkin_answer"]);
    // reminder_dismiss is fine for offline templates but not for the LLM.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"取消","action":{"command":"reminder_dismiss","args":{"id":3}}}]}]}"#;
    assert!(parse_envelope(raw, ALLOWED_ACTIONS).is_some());
    assert!(parse_envelope(raw, LLM_ACTIONS).is_none());
    // ingest is allowed in both.
    let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
        {"label":"记下来","action":{"command":"ingest","args":{"text":"明天9点开会"}}}]}]}"#;
    assert!(parse_envelope(raw, LLM_ACTIONS).is_some());
}

#[test]
fn bounds_enforced() {
    // 5 buttons > MAX_BUTTONS.
    let btn = r#"{"label":"a","action":{"command":"reminder_fire","args":{}}}"#;
    let raw = format!(
        r#"{{"version":1,"components":[{{"type":"button_group","buttons":[{}]}}]}}"#,
        [btn; 5].join(",")
    );
    assert!(text_env(&raw).is_none());
    // Choice needs >= 2 options.
    let raw = format!(r#"{{"version":1,"components":[{{"type":"choice","options":[{btn}]}}]}}"#);
    assert!(text_env(&raw).is_none());
    // Select field without options.
    let raw = r#"{"version":1,"components":[{"type":"form","fields":[
        {"name":"x","label":"X","type":"select"}],
        "submit":{"label":"存","action":{"command":"persona_import_save","args":{}}}}]}"#;
    assert!(text_env(raw).is_none());
}

#[test]
fn template_event_ingested_validates_and_carries_ids() {
    let mut ev = Event::new("开会", EventKind::Meeting, now(), "raw", now());
    ev.id = Some(7);
    let n = Notification {
        id: Some(42),
        event_id: 7,
        fire_at: now(),
        lead_label: "30m".into(),
        channels: vec![],
        status: crate::model::NotificationStatus::Pending,
        created_at: now(),
        fired_at: None,
    };
    let env = event_ingested(&ev, &[n]);
    assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains(r#""type":"event_card""#));
    assert!(json.contains(r#""id":42"#));
    // Round-trips through the defensive parser.
    assert!(parse_envelope(&json, ALLOWED_ACTIONS).is_some());
}

#[test]
fn template_routine_ingested_is_actionable_only_when_persisted() {
    let routine = Routine {
        id: Some(9),
        title: "刷牙".into(),
        time_of_day: "07:00".into(),
        source: Some("raw_input#1".into()),
        active: true,
        created_at: now(),
        scheduled_until: None,
    };
    let env = routine_ingested(&routine).expect("persisted routine renders a card");
    assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains(r#""type":"routine_card""#));
    assert!(json.contains(r#""command":"routine_set_active""#));
    assert!(json.contains(r#""active":false"#));

    assert!(routine_ingested(&Routine {
        id: None,
        ..routine
    })
    .is_none());
}

#[test]
fn template_checkin_and_persona_form_validate() {
    let env = checkin_prompt("现在在做什么？");
    assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
    assert_eq!(env.text_summary(), "现在在做什么？");

    let draft = PersonaDraft {
        nickname: None,
        tone: "干练".into(),
        catchphrases: vec!["稳了".into()],
        style_notes: None,
    };
    let env = persona_draft_form(&draft, Some("从聊天记录导入"));
    assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains("persona_import_save"));
}

#[test]
fn template_suggestions_skip_unpersisted() {
    use crate::suggest::{SuggestionKind, SuggestionStatus};
    let s = |id: Option<i64>| Suggestion {
        id,
        kind: SuggestionKind::ExamPrep,
        text: "开始复习".into(),
        dedup_key: "exam_prep:1".into(),
        source: None,
        status: SuggestionStatus::Pending,
        created_at: now(),
    };
    assert!(suggestions_prompt(&[s(None)]).is_none());
    let env = suggestions_prompt(&[s(Some(5))]).unwrap();
    assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
    assert!(serde_json::to_string(&env).unwrap().contains(r#""id":5"#));
}

#[test]
fn daily_brief_prompt_round_trips_and_validates() {
    use crate::brief::Brief;
    use crate::model::{Channel, NotificationStatus};
    use crate::suggest::{SuggestionKind, SuggestionStatus};

    let mut event = Event::new("晨会", EventKind::Meeting, now(), "raw", now());
    event.id = Some(7);
    let reminder = Notification {
        id: Some(12),
        event_id: 7,
        fire_at: now(),
        lead_label: "30m".into(),
        channels: vec![Channel::Push],
        status: NotificationStatus::Pending,
        created_at: now(),
        fired_at: None,
    };
    let suggestion = Suggestion {
        id: Some(3),
        created_at: now(),
        kind: SuggestionKind::ExamPrep,
        text: "先安排复习块".into(),
        dedup_key: "exam_prep:event#8".into(),
        source: Some("event#8".into()),
        status: SuggestionStatus::Pending,
    };
    let brief = Brief {
        date: now().date(),
        events_today: vec![event],
        due_reminders: vec![reminder],
        upcoming_reminders: vec![],
        top_suggestions: vec![suggestion],
    };

    let env = daily_brief_prompt(&brief).expect("brief has content");
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains(r#""type":"event_card""#));
    assert!(json.contains(r#""type":"reminder_card""#));
    assert!(json.contains(r#""type":"suggestion_card""#));
    let round_trip: UiEnvelope = serde_json::from_str(&json).unwrap();
    assert!(validate(&round_trip, ALLOWED_ACTIONS).is_ok());
}

#[test]
fn daily_brief_prompt_skips_an_empty_day() {
    let brief = Brief {
        date: now().date(),
        events_today: vec![],
        due_reminders: vec![],
        upcoming_reminders: vec![],
        top_suggestions: vec![],
    };
    assert!(daily_brief_prompt(&brief).is_none());
}

//! llm first. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};
// The canned reasoner lives with the widget tests that introduced it.
use super::widgets::FakeReasoner;

#[test]
fn notification_action_intent_is_rust_resolved_then_requires_f12_confirmation() {
    let mut o = Orchestrator::in_memory().unwrap();
    let event = o.ingest("明天下午3点开周会", now()).unwrap().event.unwrap();
    let event_id = event.id.unwrap();
    o.set_notif_cloud_enabled(true).unwrap();
    o.set_notification_app_enabled("com.tencent.mm", true)
        .unwrap();
    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"decisions":[{"index":0,"kind":"action","action":"cancel_event","target":"周会","reason":"通知称会议已取消"}]}"#.into(),
        }));
    let capture = o
        .capture_notification(NotificationCapture {
            package_name: "com.tencent.mm".into(),
            title: "会议变更".into(),
            body: "周会取消".into(),
            received_at: now(),
        })
        .unwrap()
        .unwrap();
    o.process_notification_batch(now()).unwrap();

    let proposal = o.notification_action_proposals().unwrap().remove(0);
    assert_eq!(
        proposal.event_id, event_id,
        "Rust—not the LLM—looked up the id"
    );
    assert_eq!(proposal.state, ActionProposalState::Pending);
    assert!(o.event(event_id).is_ok(), "proposal alone changes nothing");

    let message = o
        .resolve_notification_action_proposal(proposal.id.unwrap(), true, now())
        .unwrap();
    assert!(message.contains("已确认取消"));
    assert!(o.event(event_id).is_err());
    assert_eq!(
        o.notification_action_proposals().unwrap()[0].state,
        ActionProposalState::Accepted
    );
    assert_eq!(
        o.notification_captures()
            .unwrap()
            .into_iter()
            .find(|row| row.id == capture.id)
            .unwrap()
            .state,
        CaptureState::Resolved,
        "the source remains visible after a confirmed action"
    );
}

struct FailingReasoner;
impl crate::extract::Reasoner for FailingReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        Err(CoreError::Llm("network timeout".into()))
    }
    fn complete_with_tools(
        &self,
        _system: &str,
        _user: &str,
        _tools: &[serde_json::Value],
    ) -> Result<crate::extract::ReasonerResponse> {
        Err(CoreError::Llm("network timeout".into()))
    }
}

struct ToolCallEventReasoner {
    title: String,
    start: String,
    kind: String,
}
impl crate::extract::Reasoner for ToolCallEventReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        Ok(String::new())
    }
    fn complete_with_tools(
        &self,
        _system: &str,
        _user: &str,
        _tools: &[serde_json::Value],
    ) -> Result<crate::extract::ReasonerResponse> {
        Ok(crate::extract::ReasonerResponse {
            content: None,
            tool_calls: vec![crate::extract::ReasonerToolCall {
                name: crate::llm::TOOL_CREATE_EVENT.into(),
                arguments: serde_json::json!({
                    "title": self.title,
                    "kind": self.kind,
                    "start": self.start,
                    "location": null,
                    "people": []
                }),
            }],
        })
    }
}

#[test]
fn llm_first_ingest_event_preferred_when_online() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Online: configured with a tool-calling reasoner
    o.set_reasoner(Box::new(ToolCallEventReasoner {
        title: "洗澡".into(),
        start: "2026-07-06T10:10:00".into(),
        kind: "reminder".into(),
    }));

    let out = o.ingest("十分钟之后我要洗澡，记得提醒我", now()).unwrap();
    assert_eq!(out.intent, Intent::IngestEvent);
    let ev = out.event.expect("LLM-first extracted event");
    assert_eq!(ev.title, "洗澡");
    assert_eq!(ev.start, dt(2026, 7, 6, 10, 10));
    assert!(out.message.contains("（由云端解析）"));
    assert_eq!(o.all_events().unwrap().len(), 1);
}

#[test]
fn llm_first_rejects_joke_without_creating_event() {
    let mut o = Orchestrator::in_memory().unwrap();
    // LLM reasons that this joke/venting should not call create_event tool
    o.set_reasoner(Box::new(FakeReasoner {
        reply: "哈哈，祝你早日中大奖！到时候可别忘了这回事。".into(),
    }));

    let out = o.ingest("等我中了一千万记得提醒我买跑车", now()).unwrap();
    assert_eq!(out.intent, Intent::IngestEvent);
    // LLM returned Ok(None) - no event created!
    assert!(out.event.is_none());
    assert!(o.all_events().unwrap().is_empty());
    assert!(out.message.contains("已作为普通输入记录"));
}

#[test]
fn llm_network_error_gracefully_falls_back_to_offline_rules() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Online configuration, but network drops/times out
    o.set_reasoner(Box::new(FailingReasoner));

    let out = o.ingest("十分钟之后我要洗澡，记得提醒我", now()).unwrap();
    assert_eq!(out.intent, Intent::IngestEvent);
    let ev = out.event.expect("offline rule fallback event");
    assert!(ev.title.contains("洗澡"));
    assert_eq!(ev.start, dt(2026, 7, 6, 10, 10));
    // Falls back to offline rules, clearly noting that local rules saved the day
    assert!(out.message.contains("云端暂不可用，已通过本地规则为你创建"));
    assert_eq!(o.all_events().unwrap().len(), 1);
}

#[test]
fn offline_without_reasoner_uses_deterministic_rules() {
    let mut o = Orchestrator::in_memory().unwrap();
    assert!(!o.has_reasoner());

    let out = o.ingest("十分钟之后我要洗澡，记得提醒我", now()).unwrap();
    assert_eq!(out.intent, Intent::IngestEvent);
    let ev = out.event.expect("deterministic offline event");
    assert!(ev.title.contains("洗澡"));
    assert_eq!(ev.start, dt(2026, 7, 6, 10, 10));
    // No cloud note, purely offline
    assert!(!out.message.contains("云端解析"));
    assert!(!out.message.contains("云端暂不可用"));
    assert_eq!(o.all_events().unwrap().len(), 1);
}

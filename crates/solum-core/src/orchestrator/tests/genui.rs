//! genui. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::now;
// The canned reasoner lives with the widget tests that introduced it.
use super::widgets::FakeReasoner;

#[test]
fn ingest_event_carries_genui_confirmation_card() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("明天下午3点在会议室开会", now()).unwrap();
    let env = out.ui.expect("event ingest carries an envelope");
    assert!(crate::genui::validate(&env, crate::genui::ALLOWED_ACTIONS).is_ok());
    let json = serde_json::to_string(&env).unwrap();
    // Card snapshot + a dismiss button bound to the real notification id.
    assert!(json.contains(r#""type":"event_card""#));
    let nid = out.notifications[0].id.unwrap();
    assert!(json.contains(&format!(r#""id":{nid}"#)));
    // Non-event intents carry no envelope offline.
    assert!(o.ingest("我在护肤", now()).unwrap().ui.is_none());
    assert!(o.ingest("今天天气怎么样", now()).is_err());
}

#[test]
fn chat_genui_envelope_from_reasoner_and_degradation() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Valid envelope with an LLM-safe action → message + ui.
    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"version":1,"components":[
                {"type":"text","content":"要不要记下来？"},
                {"type":"button_group","buttons":[
                    {"label":"记下来","action":{"command":"ingest","args":{"text":"明天9点开会"}},"style":"primary"}]}]}"#
                .into(),
        }));
    let out = o.ingest("今天天气怎么样", now()).unwrap();
    assert_eq!(out.message, "要不要记下来？");
    assert!(out.ui.is_some());

    // Envelope smuggling a non-LLM action → whole thing degrades.
    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"version":1,"components":[
                {"type":"text","content":"帮你清理一下"},
                {"type":"button_group","buttons":[
                    {"label":"清理","action":{"command":"guard_request","args":{"tool":"demo_delete","args":"/photos"}}}]}]}"#
                .into(),
        }));
    let out = o.ingest("今天天气怎么样", now()).unwrap();
    assert!(out.ui.is_none());
    assert!(out.message.contains("格式异常"));

    // Plain prose stays a plain message.
    o.set_reasoner(Box::new(FakeReasoner {
        reply: "天气我看不到，但我在。".into(),
    }));
    let out = o.ingest("今天天气怎么样", now()).unwrap();
    assert_eq!(out.message, "天气我看不到，但我在。");
    assert!(out.ui.is_none());
}

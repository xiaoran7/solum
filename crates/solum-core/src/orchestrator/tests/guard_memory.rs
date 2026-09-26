//! guard memory. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};

#[test]
fn dangerous_ingest_never_executes() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("帮我删除所有照片", now()).unwrap();
    assert_eq!(out.intent, Intent::DangerousCommand);
    assert!(out.event.is_none());
}

#[test]
fn guard_flow_via_orchestrator() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Refused without confirmation.
    let refused = o.run_tool("demo_delete", "/photos", None, now());
    assert!(matches!(refused, Err(CoreError::GuardRefused(_))));
    // Confirmed path works.
    let pending = o
        .request_confirmation("demo_delete", "/photos", now())
        .unwrap();
    let token = o.confirm(&pending.id, now()).unwrap();
    let ok = o
        .run_tool("demo_delete", "/photos", Some(token), now())
        .unwrap();
    assert!(ok.contains("模拟"));
    // Audit persisted: one refusal + one execution.
    let audit = o.audit_log().unwrap();
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].decision, "refused");
    assert_eq!(audit[1].decision, "executed");
}

#[test]
fn proactivity_persists() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_proactivity(ProactivityDimension::WeeklyReview, ProactivityLevel::Butler)
        .unwrap();
    assert_eq!(
        o.proactivity().level(ProactivityDimension::WeeklyReview),
        ProactivityLevel::Butler
    );
}

#[test]
fn dismiss_cancels_reminder() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("明天下午3点开会", now()).unwrap();
    let nid = out.notifications[0].id.unwrap();
    o.dismiss(nid).unwrap();
    // Dismissed reminder never becomes due, even at its fire time.
    assert!(o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap().is_empty());
    // Dismissing again (no longer pending) errors.
    assert!(o.dismiss(nid).is_err());
}

#[test]
fn status_answer_lands_in_journal() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("我在护肤", now()).unwrap();
    assert_eq!(out.intent, Intent::StatusAnswer);
    assert!(out.message.contains("护肤"));
    let log = o.behavior_log().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, BehaviorKind::Status);
    assert_eq!(log[0].content, "护肤");
    // Journal entries show up in the F12 ledger and are deletable.
    let ledger = o.ledger().unwrap();
    assert!(ledger.iter().any(|m| m.layer == MemoryLayer::Behavior));
    o.forget(MemoryLayer::Behavior, log[0].id.unwrap()).unwrap();
    assert!(o.behavior_log().unwrap().is_empty());
}

#[test]
fn fired_reminder_lands_in_journal() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开会", now()).unwrap();
    o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap();
    let log = o.behavior_log().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, BehaviorKind::ReminderFired);
    assert!(log[0].content.contains("开会"));
}

#[test]
fn memory_write_stores_fact_and_ledger_delete_works() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("记住我不吃辣", now()).unwrap();
    assert_eq!(out.intent, Intent::MemoryWrite);
    assert!(out.message.contains("已记住：我不吃辣"));
    let facts = o.facts().unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].content, "我不吃辣");
    // Duplicate is acknowledged, not double-stored.
    let out2 = o.ingest("记住我不吃辣", now()).unwrap();
    assert!(out2.message.contains("已经记着了"));
    assert_eq!(o.facts().unwrap().len(), 1);
    // F12: facts appear in the ledger and are deletable.
    let ledger = o.ledger().unwrap();
    let entry = ledger
        .iter()
        .find(|m| m.layer == MemoryLayer::Fact)
        .unwrap();
    o.forget(MemoryLayer::Fact, entry.id).unwrap();
    assert!(o.facts().unwrap().is_empty());
    // "记住明天下午3点开会" stays an event, not a fact.
    let out3 = o.ingest("记住明天下午3点开会", now()).unwrap();
    assert_eq!(out3.intent, Intent::IngestEvent);
}

#[test]
fn recall_prefers_matching_fact_and_excludes_captured() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_notif_cloud_enabled(false).unwrap();
    o.ingest("记住我对花生过敏", now()).unwrap();
    o.ingest("我在健身", now()).unwrap();
    // A captured third-party event must never enter the recall corpus.
    o.ingest_captured("张伟 明天下午3点在会议室开花生品鉴会", "通知·com.x", now())
        .unwrap()
        .unwrap();
    let hits = o.recall("花生", now()).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().any(|s| s.content.contains("花生过敏")));
    assert!(
        hits.iter().all(|s| !s.content.contains("品鉴会")),
        "通知捕获的事件不得进入 recall 语料"
    );
}

/// M1: the second chat call carries the first exchange in its context.
#[test]
fn chat_history_travels_and_is_capped() {
    use std::sync::Mutex;
    struct CaptureSystem {
        seen: std::sync::Arc<Mutex<Vec<String>>>,
    }
    impl crate::extract::Reasoner for CaptureSystem {
        fn complete(&self, system: &str, _user: &str) -> Result<String> {
            self.seen.lock().unwrap().push(system.to_string());
            Ok("好的。".into())
        }
    }
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(CaptureSystem { seen: seen.clone() }));
    o.ingest("给我讲个笑话", now()).unwrap();
    o.ingest("再来一个", now()).unwrap();
    let all = seen.lock().unwrap();
    assert!(!all[0].contains("最近对话"), "首轮没有历史");
    assert!(all[1].contains("最近对话"));
    assert!(all[1].contains("给我讲个笑话"));
}

#[test]
fn replacing_chat_history_keeps_only_the_selected_sessions_recent_turns() {
    let mut o = Orchestrator::in_memory().unwrap();
    let turns = (0..6)
        .map(|n| ChatTurn {
            user: format!("会话输入{n}"),
            assistant: format!("会话回复{n}"),
        })
        .collect();
    o.replace_chat_history(turns);
    assert_eq!(o.chat_history.len(), MAX_HISTORY_TURNS);
    assert_eq!(o.chat_history.front().unwrap().user, "会话输入2");
    assert_eq!(o.chat_history.back().unwrap().assistant, "会话回复5");

    o.replace_chat_history(vec![ChatTurn {
        user: "另一段会话".into(),
        assistant: "另一段回复".into(),
    }]);
    assert_eq!(o.chat_history.len(), 1);
    assert_eq!(o.chat_history.front().unwrap().user, "另一段会话");
}

#[test]
fn editing_a_rule_replans_only_future_pending_reminders() {
    let mut o = Orchestrator::in_memory().unwrap();
    let created = o.ingest("明天下午3点开会", now()).unwrap();
    assert_eq!(created.notifications[0].fire_at, dt(2026, 7, 7, 14, 30));

    let mut rule = o.rule_table().rule(EventKind::Meeting);
    rule.lead_times = vec![crate::classify::LeadTime::parse("1h").unwrap()];
    rule.channels = vec![crate::model::Channel::Push, crate::model::Channel::Banner];
    assert_eq!(o.set_importance_rule(rule, now()).unwrap(), 1);

    let notifications = o.all_notifications().unwrap();
    assert_eq!(notifications.len(), 1, "old pending plan is replaced");
    assert_eq!(notifications[0].fire_at, dt(2026, 7, 7, 14, 0));
    assert_eq!(
        notifications[0].channels,
        vec![crate::model::Channel::Push, crate::model::Channel::Banner]
    );
}

/// D3: the purge request produces a guard entrance, and the full
/// confirm→token→execute flow really deletes rows + audits.
#[test]
fn ledger_purge_end_to_end() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Two old journal entries + one new.
    o.ingest("我在护肤", dt(2026, 7, 1, 8, 0)).unwrap();
    o.ingest("我在跑步", dt(2026, 7, 2, 8, 0)).unwrap();
    o.ingest("我在看书", dt(2026, 7, 6, 8, 0)).unwrap();
    assert_eq!(o.behavior_log().unwrap().len(), 3);

    // The chat entrance: message names the layer and the real count.
    let out = o.ingest("清空三天前的行为日志", now()).unwrap();
    assert_eq!(out.intent, Intent::DangerousCommand);
    assert!(out.message.contains("2 条"), "message: {}", out.message);
    let ui = out.ui.expect("purge request carries a guard entrance");
    let json = serde_json::to_string(&ui).unwrap();
    assert!(json.contains("guard_request"));
    assert!(json.contains("ledger_purge"));

    // No token → refused, nothing deleted, refusal audited.
    let args = r#"{"layer":"behavior","before":"2026-07-03T10:00:00"}"#;
    assert!(o.run_tool("ledger_purge", args, None, now()).is_err());
    assert_eq!(o.behavior_log().unwrap().len(), 3);

    // Full flow: preview shows the real count, then execution deletes.
    let pending = o.request_confirmation("ledger_purge", args, now()).unwrap();
    assert!(pending.request.effect_preview.contains("2 条"));
    let token = o.confirm(&pending.id, now()).unwrap();
    let msg = o
        .run_tool("ledger_purge", args, Some(token), now())
        .unwrap();
    assert!(msg.contains("已永久删除 2 条"));
    assert_eq!(o.behavior_log().unwrap().len(), 1);
    // Audit: one refusal + one execution.
    let audit = o.audit_log().unwrap();
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[1].decision, "executed");
}

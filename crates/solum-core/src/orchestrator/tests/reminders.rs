//! reminders. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};

#[test]
fn exam_lead_three_days() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("7月20号上午九点期末考试", now()).unwrap();
    let n = &out.notifications[0];
    assert_eq!(n.lead_label, "3d");
    assert_eq!(n.fire_at, dt(2026, 7, 17, 9, 0));
}

#[test]
fn fire_due_delivers_once() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开会", now()).unwrap();
    // Nothing due yet.
    assert!(o.fire_due(now()).unwrap().is_empty());
    // At fire time it delivers, and only once.
    let fired = o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap();
    assert_eq!(fired.len(), 1);
    assert!(o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap().is_empty());
}

/// Regression tests for the TODO "three multi-step writes lacked a
/// transaction" item: each step pair must be one unit, so an injected
/// failure in the second write rolls back the first and the caller sees
/// the error instead of half-applied state.
#[test]
fn event_persist_failure_rolls_back_with_its_reminders() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.store.arm_insert_tripwire("notifications");
    o.ingest("明天下午3点开事务回滚测试会", now())
        .expect_err("injected failure must surface");
    o.store.disarm_insert_tripwire("notifications");

    // The event itself must be gone with its failed reminder plan — no
    // visibly scheduled, silently never-firing orphan.
    let titles: Vec<String> = o
        .all_events()
        .unwrap()
        .into_iter()
        .map(|e| e.title)
        .collect();
    assert!(
        !titles.iter().any(|t| t.contains("事务回滚测试")),
        "event must roll back with its reminders, got {titles:?}"
    );
    assert!(o.all_notifications().unwrap().is_empty());

    // After disarming the same utterance succeeds normally.
    o.ingest("明天下午3点开事务回滚测试会", now()).unwrap();
    assert_eq!(o.all_events().unwrap().len(), 1);
    assert_eq!(o.all_notifications().unwrap().len(), 1);
}

#[test]
fn fire_due_journal_failure_leaves_no_fired_mark() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开提醒回滚会议", now()).unwrap();
    o.store.arm_insert_tripwire("behavior_log");
    let err = o
        .fire_due(dt(2026, 7, 7, 14, 30))
        .expect_err("injected journal failure must surface");
    assert!(err.to_string().contains("injected test failure"));
    o.store.disarm_insert_tripwire("behavior_log");

    // The reminder must NOT stay marked fired: mark + journal are one
    // unit, so after rollback it is still pending and will deliver again.
    let n = &o.all_notifications().unwrap()[0];
    assert_eq!(n.status.as_str(), "pending");
    let fired = o.fire_due(dt(2026, 7, 7, 14, 30)).unwrap();
    assert_eq!(fired.len(), 1);
    // Now both the fired mark and the journal entry exist together.
    assert_eq!(o.all_notifications().unwrap()[0].status.as_str(), "fired");
    assert!(o
        .store
        .list_behavior()
        .unwrap()
        .iter()
        .any(|b| b.content.contains("提醒回滚会议")));
}

#[test]
fn replan_rule_failure_keeps_old_pending_reminders() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开重排规则会议", now()).unwrap();
    let before = &o.all_notifications().unwrap()[0];

    let mut rule = o.rule_table().rule(EventKind::Meeting);
    rule.lead_times = vec![crate::classify::LeadTime::parse("2h").unwrap()];
    o.store.arm_insert_tripwire("notifications");
    let err = o
        .set_importance_rule(rule, now())
        .expect_err("injected replan failure must surface");
    assert!(err.to_string().contains("injected test failure"));
    o.store.disarm_insert_tripwire("notifications");

    // The old pending reminder survived untouched (delete+reinsert is one
    // unit per event) and still fires at the original time.
    let after = &o.all_notifications().unwrap()[0];
    assert_eq!(after.id, before.id);
    assert_eq!(after.fire_at, before.fire_at);
    assert_eq!(after.status.as_str(), "pending");

    // Disarmed, the same replan applies cleanly with the new lead time.
    let mut rule = o.rule_table().rule(EventKind::Meeting);
    rule.lead_times = vec![crate::classify::LeadTime::parse("2h").unwrap()];
    assert_eq!(o.set_importance_rule(rule, now()).unwrap(), 1);
    assert_eq!(
        o.all_notifications().unwrap()[0].fire_at,
        dt(2026, 7, 7, 13, 0),
        "disarmed replan must produce the new lead time"
    );
}

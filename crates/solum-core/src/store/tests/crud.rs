//! crud. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::events::sample_event;
use super::sync_core::dt;

#[test]
pub(crate) fn event_round_trip() {
    let s = Store::open_in_memory().unwrap();
    let id = s.insert_event(&sample_event(), None).unwrap();
    let got = s.get_event(id).unwrap();
    assert_eq!(got.title, "开会");
    assert_eq!(got.kind, EventKind::Meeting);
    assert_eq!(got.location.as_deref(), Some("会议室"));
    assert_eq!(got.people, vec!["张伟".to_string()]);
    assert_eq!(got.start, dt(2026, 7, 7, 15, 0));
}

#[test]
pub(crate) fn notification_due_and_fire() {
    let s = Store::open_in_memory().unwrap();
    let ev_id = s.insert_event(&sample_event(), None).unwrap();
    let n = Notification {
        id: None,
        event_id: ev_id,
        fire_at: dt(2026, 7, 7, 14, 30),
        lead_label: "30m".into(),
        channels: vec![Channel::Push],
        status: NotificationStatus::Pending,
        created_at: dt(2026, 7, 6, 10, 0),
        fired_at: None,
    };
    let nid = s.insert_notification(&n).unwrap();
    assert!(s
        .due_notifications(dt(2026, 7, 7, 14, 0))
        .unwrap()
        .is_empty());
    assert_eq!(
        s.due_notifications(dt(2026, 7, 7, 14, 30)).unwrap().len(),
        1
    );
    s.mark_fired(nid, dt(2026, 7, 7, 14, 30)).unwrap();
    assert!(s
        .due_notifications(dt(2026, 7, 7, 15, 0))
        .unwrap()
        .is_empty());
}

#[test]
pub(crate) fn snooze_rearms_pending_and_fired_but_not_dismissed() {
    let s = Store::open_in_memory().unwrap();
    let ev_id = s.insert_event(&sample_event(), None).unwrap();
    let n = Notification {
        id: None,
        event_id: ev_id,
        fire_at: dt(2026, 7, 7, 14, 30),
        lead_label: "30m".into(),
        channels: vec![Channel::Push],
        status: NotificationStatus::Pending,
        created_at: dt(2026, 7, 6, 10, 0),
        fired_at: None,
    };
    let nid = s.insert_notification(&n).unwrap();

    // pending → postpone: not due at the old time, due at the new one.
    s.snooze_notification(nid, dt(2026, 7, 7, 14, 45)).unwrap();
    assert!(s
        .due_notifications(dt(2026, 7, 7, 14, 30))
        .unwrap()
        .is_empty());
    assert_eq!(
        s.due_notifications(dt(2026, 7, 7, 14, 45)).unwrap().len(),
        1
    );

    // fired → re-arm: back to pending with fired_at cleared.
    s.mark_fired(nid, dt(2026, 7, 7, 14, 45)).unwrap();
    s.snooze_notification(nid, dt(2026, 7, 7, 14, 55)).unwrap();
    let got = s
        .list_notifications()
        .unwrap()
        .into_iter()
        .find(|x| x.id == Some(nid))
        .unwrap();
    assert_eq!(got.status, NotificationStatus::Pending);
    assert_eq!(got.fire_at, dt(2026, 7, 7, 14, 55));
    assert!(got.fired_at.is_none());

    // dismissed stays cancelled — snooze must not resurrect it.
    s.dismiss_notification(nid).unwrap();
    assert!(s.snooze_notification(nid, dt(2026, 7, 7, 15, 5)).is_err());
}

#[test]
pub(crate) fn fact_update_edits_in_place_and_rejects_duplicates() {
    let s = Store::open_in_memory().unwrap();
    let now = dt(2026, 7, 6, 10, 0);
    let fact = |content: &str| crate::memory::MemoryFact {
        id: None,
        content: content.into(),
        source: "manual".into(),
        created_at: now,
        last_used_at: None,
    };
    let (f1, f2) = (fact("我对花生过敏"), fact("我不吃辣"));
    let id1 = s.insert_fact_if_new(&f1).unwrap().unwrap();
    s.insert_fact_if_new(&f2).unwrap().unwrap();

    s.update_fact(id1, " 我对花生和腰果都过敏 ").unwrap();
    let facts = s.list_facts().unwrap();
    assert!(facts.iter().any(|f| f.content == "我对花生和腰果都过敏"));

    // Editing into another fact's exact wording is rejected, not merged.
    assert!(s.update_fact(id1, "我不吃辣").is_err());
    // Empty content is rejected.
    assert!(s.update_fact(id1, "   ").is_err());
    // Missing row.
    assert!(s.update_fact(9999, "x").is_err());
}

//! scene wellness. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};

/// D5/F13: scenes silence check-ins (sleep hours, mid-event) but never
/// block reminder delivery.
#[test]
fn scene_gates_checkins_but_not_reminders() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_proactivity(
        ProactivityDimension::StatusCheckins,
        ProactivityLevel::Butler,
    )
    .unwrap();
    // In-event: a meeting covering 10:00.
    o.ingest("今天上午10点开会", dt(2026, 7, 6, 8, 0)).unwrap();
    assert_eq!(
        o.current_scene(dt(2026, 7, 6, 10, 30)).unwrap(),
        crate::scene::Scene::InEvent
    );
    assert!(o.checkin_if_due(dt(2026, 7, 6, 10, 30)).unwrap().is_none());
    // Normal time asks fine.
    assert!(o.checkin_if_due(dt(2026, 7, 6, 15, 0)).unwrap().is_some());
    // Reminders still fire regardless of scene (10:00 meeting reminder
    // fires at 09:30 — inside no event, but the point is the pipeline
    // has no scene check at all).
    assert_eq!(o.fire_due(dt(2026, 7, 6, 9, 30)).unwrap().len(), 1);
}

/// D5/F11: wellness rules stay silent until the 14-day data gate opens.
#[test]
fn wellness_respects_data_gate_and_baseline() {
    use crate::wearable::{HealthMetric, HealthSample};
    let mut o = Orchestrator::in_memory().unwrap();
    // 14 days of sleep baseline at 420 min…
    let mut samples = Vec::new();
    for d in 1..=14u32 {
        samples.push(HealthSample::new(
            HealthMetric::Sleep,
            dt(2026, 7, d, 6, 0),
            dt(2026, 7, d, 6, 30),
            420.0,
            "hc",
        ));
    }
    // …and last night far below baseline.
    samples.push(HealthSample::new(
        HealthMetric::Sleep,
        dt(2026, 7, 15, 6, 0),
        dt(2026, 7, 15, 6, 30),
        240.0,
        "hc",
    ));
    o.record_health_samples(samples, dt(2026, 7, 15, 7, 0))
        .unwrap();
    let fresh = o.generate_suggestions(dt(2026, 7, 15, 9, 0), 1).unwrap();
    let wellness: Vec<_> = fresh
        .iter()
        .filter(|s| s.kind == suggest::SuggestionKind::Wellness)
        .collect();
    assert_eq!(wellness.len(), 1);
    assert!(wellness[0].text.contains("昨晚只睡了 4.0 小时"));
    // Steps/HR gates are closed (no data) → no other wellness noise.
    assert!(wellness[0].dedup_key.starts_with("wellness_sleep:"));
}

#[test]
fn facts_and_routines_sync_between_devices() {
    use crate::sync::tests::{test_cfg, MemTransport};
    let cfg = test_cfg();
    let relay = MemTransport::default();
    let mut a = Orchestrator::in_memory().unwrap();
    let mut b = Orchestrator::in_memory().unwrap();

    a.ingest("记住我不吃辣", now()).unwrap();
    a.store
        .insert_routine_if_new(&crate::routine::Routine {
            id: None,
            title: "护肤".into(),
            time_of_day: "07:20".into(),
            source: None,
            active: true,
            created_at: now(),
            scheduled_until: None,
        })
        .unwrap();
    a.materialize_routines(now()).unwrap();

    a.sync_now(&relay, &cfg).unwrap();
    let ob = b.sync_now(&relay, &cfg).unwrap();
    assert!(ob.applied > 0);
    assert_eq!(b.facts().unwrap().len(), 1);
    assert_eq!(b.routines().unwrap().len(), 1);

    // A schedule edit on B must retract A's old pending projection too;
    // routine payloads are synced independently from their occurrences.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let rid = b.routines().unwrap()[0].id.unwrap();
    b.update_routine(rid, "晨间护肤", "08:30", now()).unwrap();
    b.sync_now(&relay, &cfg).unwrap();
    a.sync_now(&relay, &cfg).unwrap();
    let edited = a.routines().unwrap();
    assert_eq!(edited[0].title, "晨间护肤");
    assert_eq!(edited[0].time_of_day, "08:30");
    assert!(a.due(dt(2026, 7, 7, 7, 20)).unwrap().is_empty());
    assert_eq!(a.due(dt(2026, 7, 7, 8, 30)).unwrap().len(), 1);

    // Deleting the fact on B propagates back to A.
    let fid = b.facts().unwrap()[0].id.unwrap();
    b.forget(MemoryLayer::Fact, fid).unwrap();
    b.sync_now(&relay, &cfg).unwrap();
    a.sync_now(&relay, &cfg).unwrap();
    assert!(a.facts().unwrap().is_empty());
}

#[test]
fn checkin_respects_level_and_interval() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Default status_checkins is passive → never asks.
    assert!(o.checkin_if_due(now()).unwrap().is_none());

    o.set_proactivity(
        ProactivityDimension::StatusCheckins,
        ProactivityLevel::Butler,
    )
    .unwrap();
    // First ask fires (10:00 is within waking hours) and is journaled.
    let q = o.checkin_if_due(now()).unwrap();
    assert!(q.is_some());
    // Asking again immediately stays quiet; after the 2h butler interval it fires.
    assert!(o.checkin_if_due(dt(2026, 7, 6, 11, 0)).unwrap().is_none());
    assert!(o.checkin_if_due(dt(2026, 7, 6, 12, 0)).unwrap().is_some());
    assert_eq!(
        o.behavior_log()
            .unwrap()
            .iter()
            .filter(|b| b.kind == BehaviorKind::CheckinAsked)
            .count(),
        2
    );
}

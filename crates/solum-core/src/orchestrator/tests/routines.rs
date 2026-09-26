//! routines. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};

#[test]
fn explicit_daily_input_creates_a_routine_not_a_one_shot_event() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("每天晚上8点提醒我吃药", now()).unwrap();

    assert!(out.event.is_none());
    assert!(out.message.contains("每天 20:00"));
    assert!(out.message.contains("吃药"));
    let ui = serde_json::to_string(out.ui.as_ref().expect("routine confirmation card")).unwrap();
    assert!(ui.contains(r#""type":"routine_card""#));
    assert!(ui.contains(r#""command":"routine_set_active""#));
    let routines = o.routines().unwrap();
    assert_eq!(routines.len(), 1);
    assert_eq!(routines[0].title, "吃药");
    assert_eq!(routines[0].time_of_day, "20:00");
    assert_eq!(
        routines[0].source.as_deref(),
        Some(format!("raw_input#{}", out.raw_input_id).as_str())
    );
    // Ingest materializes today and tomorrow immediately, so the current
    // occurrence already travels through the normal delivery path.
    assert_eq!(o.all_events().unwrap().len(), 2);
    assert_eq!(o.due(dt(2026, 7, 6, 20, 0)).unwrap().len(), 1);

    let duplicate = o.ingest("每日晚上8点提醒我吃药", now()).unwrap();
    assert!(duplicate.message.contains("未重复创建"));
    assert_eq!(o.routines().unwrap().len(), 1);
    assert_eq!(o.all_events().unwrap().len(), 2);

    let morning = o.ingest("每早7点提醒我刷牙", now()).unwrap();
    assert!(morning.message.contains("每天 07:00"));
    let morning_routine = o
        .routines()
        .unwrap()
        .into_iter()
        .find(|routine| routine.title == "刷牙")
        .unwrap();
    assert_eq!(morning_routine.time_of_day, "07:00");
    // At 10:00, the 07:00 occurrence for today has passed; only tomorrow
    // is materialized, while the existing 20:00 routine retains two events.
    assert_eq!(o.all_events().unwrap().len(), 3);
    assert!(o
        .all_events()
        .unwrap()
        .iter()
        .any(|event| { event.title == "刷牙" && event.start == dt(2026, 7, 7, 7, 0) }));
}

#[test]
fn unsupported_recurrence_stays_explicit() {
    assert!(is_daily_recurrence("每天晚上8点提醒我吃药"));
    assert!(recurrence_caveat("每天晚上8点提醒我吃药").is_none());
    assert!(is_daily_recurrence("每早7点提醒我刷牙"));
    assert!(recurrence_caveat("每早7点提醒我刷牙").is_none());
    assert!(is_daily_recurrence("每晚8点提醒我吃药"));
    assert!(recurrence_caveat("每晚8点提醒我吃药").is_none());
    assert!(recurrence_caveat("每周一上午10点开会").is_some());
    assert!(recurrence_caveat("每月1号交房租").is_some());
    assert!(recurrence_caveat("每小时提醒我喝水").is_some());
}

/// A standing routine, created the way the accept path creates one.
fn test_routine(o: &Orchestrator, title: &str, time_of_day: &str) -> i64 {
    o.store
        .insert_routine_if_new(&Routine {
            id: None,
            title: title.into(),
            time_of_day: time_of_day.into(),
            source: Some("test".into()),
            active: true,
            created_at: now(),
            scheduled_until: None,
        })
        .unwrap()
        .unwrap()
}

/// The deletions that used to be bare IPC commands are now Guard
/// tools. What matters is the *structural* property: without a token they
/// do not run, and the refusal is audited.
#[test]
fn irreversible_deletions_cannot_run_without_a_token() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("记住我不吃辣", now()).unwrap();
    let fact_id = o.facts().unwrap()[0].id.unwrap();
    let args = format!(r#"{{"layer":"fact","id":{fact_id}}}"#);

    assert!(o.run_tool("memory_forget", &args, None, now()).is_err());
    assert_eq!(
        o.facts().unwrap().len(),
        1,
        "must not have deleted anything"
    );
    assert!(o.run_tool("persona_clear", "{}", None, now()).is_err());
    assert!(o
        .run_tool(
            "widget_record_delete",
            r#"{"widget_id":1,"record_id":1}"#,
            None,
            now()
        )
        .is_err());
    let event_id = o
        .ingest("明天下午3点开会", now())
        .unwrap()
        .event
        .unwrap()
        .id
        .unwrap();
    assert!(o
        .run_tool(
            "event_cancel",
            &format!(r#"{{"event_id":{event_id}}}"#),
            None,
            now()
        )
        .is_err());
    assert_eq!(o.all_events().unwrap().len(), 1);

    let backup_args = serde_json::json!({
        "backup_id": "11111111-1111-4111-8111-111111111111",
        "plan_etag": "00".repeat(32),
        "committed_at": "2026-01-02T03:05:06.000Z",
        "total_bytes": 4096,
        "object_count": 2,
    })
    .to_string();
    let pending = o
        .request_confirmation("backup_delete", &backup_args, now())
        .unwrap();
    assert!(pending.request.effect_preview.contains("2 个密文对象"));
    assert!(pending.request.effect_preview.contains("不能"));
    assert!(o
        .run_tool("backup_delete", &backup_args, None, now())
        .is_err());

    let audit = o.audit_log().unwrap();
    assert_eq!(audit.len(), 5, "every refusal is audited");
    assert!(audit.iter().all(|e| e.decision == "refused"));
}

/// …and the full flow still works: preview names the real blast radius,
/// confirmation mints a token, execution deletes exactly that.
#[test]
fn forget_through_the_guard_previews_the_cascade_and_then_deletes() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点在会议室开会", now()).unwrap();
    let raw_id = o
        .ledger()
        .unwrap()
        .into_iter()
        .find(|e| e.layer == MemoryLayer::RawInput)
        .map(|e| e.id)
        .expect("raw input in ledger");
    let args = format!(r#"{{"layer":"raw_input","id":{raw_id}}}"#);

    let pending = o
        .request_confirmation("memory_forget", &args, now())
        .unwrap();
    // The preview must state what goes *with* it, not just the one row —
    // that is the number the effect digest then binds the token to.
    assert!(
        pending.request.effect_preview.contains("派生"),
        "preview should name the cascade, got: {}",
        pending.request.effect_preview
    );

    let token = o.confirm(&pending.id, now()).unwrap();
    o.run_tool("memory_forget", &args, Some(token), now())
        .unwrap();

    assert!(o.all_events().unwrap().is_empty(), "derived event went too");
    assert!(
        o.all_notifications().unwrap().is_empty(),
        "derived reminders went too"
    );
}

/// P1 regression: a routine occurrence whose event was written but whose
/// notification was not (crash between the two inserts) must be repaired on
/// the next materialization — not treated as done forever.
#[test]
fn a_routine_occurrence_missing_its_reminder_gets_repaired() {
    let mut o = Orchestrator::in_memory().unwrap();
    let id = test_routine(&o, "护肤", "07:20");

    assert_eq!(o.materialize_routines(now()).unwrap(), 1);
    let start = dt(2026, 7, 7, 7, 20);
    let ev_id = o
        .store
        .routine_occurrence_event(id, start)
        .unwrap()
        .expect("event materialized");

    // Simulate the crash: the event survived, its reminder did not.
    o.store
        .delete_pending_notifications_for_event(ev_id)
        .unwrap();
    assert!(o.store.routine_occurrence_needs_work(id, start).unwrap());

    // Re-running must notice and repair, reusing the existing event rather
    // than creating a second one.
    assert_eq!(o.materialize_routines(now()).unwrap(), 1);
    assert!(!o.store.routine_occurrence_needs_work(id, start).unwrap());
    assert_eq!(
        o.all_events().unwrap().len(),
        1,
        "repair must not duplicate the event"
    );
    // …and it is genuinely schedulable again.
    assert_eq!(o.fire_due(start).unwrap().len(), 1);
}

/// P2 regression: merely saying words that match a routine's title is not a
/// completion. Only an entry whose provenance is the routine counts.
#[test]
fn mentioning_a_routine_is_not_confirming_it() {
    let mut o = Orchestrator::in_memory().unwrap();
    let id = test_routine(&o, "护肤", "07:20");

    // An ordinary status utterance that happens to contain the title.
    o.ingest("我在护肤", dt(2026, 7, 6, 21, 0)).unwrap();

    // Not counted as today's confirmation…
    let msg = o.confirm_routine(id, dt(2026, 7, 6, 22, 0)).unwrap();
    assert!(msg.contains("已记录"), "got {msg}");
    // …but the real confirmation is, and is idempotent for the day.
    let again = o.confirm_routine(id, dt(2026, 7, 6, 22, 5)).unwrap();
    assert!(again.contains("已经确认过"), "got {again}");
}

/// P2 regression: the user's "no" is final. A stale card must not be able
/// to flip a dismissed suggestion into accepted, because accepting creates
/// or pauses a routine.
#[test]
fn a_dismissed_suggestion_cannot_be_accepted_later() {
    let mut o = Orchestrator::in_memory().unwrap();
    for d in 3..6 {
        o.ingest("我在护肤", dt(2026, 7, d, 7, 20)).unwrap();
    }
    let habit = o
        .generate_suggestions(now(), 3)
        .unwrap()
        .into_iter()
        .find(|s| s.kind == suggest::SuggestionKind::HabitReminder)
        .expect("habit detected");
    let sid = habit.id.unwrap();

    o.set_suggestion_status(sid, SuggestionStatus::Dismissed, now())
        .unwrap();

    // The replay from an old card.
    let replay = o
        .set_suggestion_status(sid, SuggestionStatus::Accepted, now())
        .unwrap();
    assert!(replay.unwrap().contains("已经处理过"));
    assert!(
        o.routines().unwrap().is_empty(),
        "a dismissed suggestion must not have created a routine"
    );
    assert_eq!(
        o.suggestions()
            .unwrap()
            .iter()
            .find(|s| s.id == Some(sid))
            .unwrap()
            .status,
        SuggestionStatus::Dismissed
    );
}

/// D4: accept a habit suggestion → routine + fact; materialization feeds
/// the normal reminder pipeline; a week of silence → pause suggestion;
/// accepting that pauses the routine.
#[test]
fn habit_accept_creates_routine_and_pause_brake_works() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Three clustered mornings of 护肤 → habit suggestion.
    for d in 3..6 {
        o.ingest("我在护肤", dt(2026, 7, d, 7, 20)).unwrap();
    }
    let fresh = o.generate_suggestions(now(), 3).unwrap();
    let habit = fresh
        .iter()
        .find(|s| s.kind == suggest::SuggestionKind::HabitReminder)
        .expect("habit detected");

    let msg = o
        .set_suggestion_status(habit.id.unwrap(), SuggestionStatus::Accepted, now())
        .unwrap()
        .expect("routine created");
    assert!(msg.contains("固定提醒"));
    let routines = o.routines().unwrap();
    assert_eq!(routines.len(), 1);
    assert_eq!(routines[0].title, "护肤");
    assert_eq!(routines[0].time_of_day, "07:20");
    assert!(routines[0].active);
    // The habit is also solidified as a semantic fact (M2 ②).
    assert!(o.facts().unwrap().iter().any(|f| f.source == "habit"));
    // Accepting again must not duplicate. It is now refused one step
    // earlier than it used to be: the status transition itself is
    // `pending`-only, so the side effect is never re-entered at all rather
    // than being re-entered and deduped inside routine creation.
    let again = o
        .set_suggestion_status(habit.id.unwrap(), SuggestionStatus::Accepted, now())
        .unwrap()
        .unwrap();
    assert!(again.contains("已经处理过"), "got {again}");
    assert_eq!(o.routines().unwrap().len(), 1);

    // Materialize (now = 10:00 → today's 07:20 already passed, tomorrow's
    // occurrence is created) and fire it through the normal pipeline.
    let created = o.materialize_routines(now()).unwrap();
    assert_eq!(created, 1);
    // Idempotent.
    assert_eq!(o.materialize_routines(now()).unwrap(), 0);
    let fired = o.fire_due(dt(2026, 7, 7, 7, 20)).unwrap();
    assert_eq!(fired.len(), 1);

    // Anti-nag brake: 8 days later with zero confirmations → pause offer.
    let later = dt(2026, 7, 14, 12, 0);
    let fresh = o.generate_suggestions(later, 3).unwrap();
    let pause = fresh
        .iter()
        .find(|s| s.kind == suggest::SuggestionKind::RoutinePause)
        .expect("pause suggested after 7 silent days");
    let msg = o
        .set_suggestion_status(pause.id.unwrap(), SuggestionStatus::Accepted, later)
        .unwrap()
        .unwrap();
    assert!(msg.contains("已暂停"));
    assert!(!o.routines().unwrap()[0].active);
    // Paused routines materialize nothing.
    assert_eq!(o.materialize_routines(later).unwrap(), 0);
}

#[test]
fn disabling_or_deleting_a_routine_retracts_pending_occurrences() {
    let mut o = Orchestrator::in_memory().unwrap();
    let routine = Routine {
        id: None,
        title: "拉伸".into(),
        time_of_day: "08:00".into(),
        source: Some("test".into()),
        active: true,
        created_at: now(),
        scheduled_until: None,
    };
    let first = o.store.insert_routine_if_new(&routine).unwrap().unwrap();
    assert_eq!(o.materialize_routines(now()).unwrap(), 1);
    assert_eq!(o.due(dt(2026, 7, 8, 8, 0)).unwrap().len(), 1);

    o.set_routine_active(first, false, now()).unwrap();
    assert!(o.due(dt(2026, 7, 8, 8, 0)).unwrap().is_empty());
    assert!(o
        .all_events()
        .unwrap()
        .iter()
        .all(|event| event.title != "拉伸"));

    let second = o
        .store
        .insert_routine_if_new(&Routine {
            title: "冥想".into(),
            ..routine
        })
        .unwrap()
        .unwrap();
    o.materialize_routines(now()).unwrap();
    o.forget(MemoryLayer::Routine, second).unwrap();
    assert!(o.due(dt(2026, 7, 8, 8, 0)).unwrap().is_empty());
    assert!(o
        .all_events()
        .unwrap()
        .iter()
        .all(|event| event.title != "冥想"));
}

#[test]
fn reenabling_a_routine_recreates_retracted_pending_occurrences() {
    let mut o = Orchestrator::in_memory().unwrap();
    let routine = Routine {
        id: None,
        title: "拉伸".into(),
        time_of_day: "08:00".into(),
        source: Some("test".into()),
        active: true,
        created_at: now(),
        scheduled_until: None,
    };
    let id = o.store.insert_routine_if_new(&routine).unwrap().unwrap();

    // `now()` is after today's slot, so the first materialization creates
    // tomorrow only and records that date as the high-water mark.
    assert_eq!(o.materialize_routines(now()).unwrap(), 1);
    assert_eq!(
        o.routines().unwrap()[0].scheduled_until,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 7).unwrap())
    );

    o.set_routine_active(id, false, now()).unwrap();
    assert!(o.due(dt(2026, 7, 7, 8, 0)).unwrap().is_empty());

    o.set_routine_active(id, true, now()).unwrap();
    // Resuming clears the stale water mark left by the pause, so the
    // routine can create a fresh pending occurrence instead of waiting
    // until a date beyond the previously materialized horizon.
    assert_eq!(
        o.routines().unwrap()[0].scheduled_until,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 7).unwrap())
    );
    assert_eq!(o.due(dt(2026, 7, 7, 8, 0)).unwrap().len(), 1);
}

#[test]
fn editing_a_routine_replaces_pending_occurrences_but_keeps_fired_history() {
    let mut o = Orchestrator::in_memory().unwrap();
    let routine = Routine {
        id: None,
        title: "拉伸".into(),
        time_of_day: "10:00".into(),
        source: Some("test".into()),
        active: true,
        created_at: now(),
        scheduled_until: None,
    };
    let id = o.store.insert_routine_if_new(&routine).unwrap().unwrap();
    assert_eq!(o.materialize_routines(now()).unwrap(), 2);
    assert_eq!(o.fire_due(now()).unwrap().len(), 1);

    o.update_routine(id, "晨间拉伸", "09:00", now()).unwrap();
    let routines = o.routines().unwrap();
    assert_eq!(routines[0].title, "晨间拉伸");
    assert_eq!(routines[0].time_of_day, "09:00");
    assert_eq!(o.due(dt(2026, 7, 7, 9, 0)).unwrap().len(), 1);

    let events = o.all_events().unwrap();
    assert!(events
        .iter()
        .any(|event| event.title == "拉伸" && event.start == now()));
    assert!(events
        .iter()
        .any(|event| event.title == "晨间拉伸" && event.start == dt(2026, 7, 7, 9, 0)));
    assert!(!events
        .iter()
        .any(|event| event.title == "拉伸" && event.start == dt(2026, 7, 7, 10, 0)));
}

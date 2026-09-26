//! review suggest. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};

#[test]
fn suggestions_generate_dedup_and_gate() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开会", now()).unwrap();
    o.ingest("后天上午九点期末考试", now()).unwrap();

    // Default life_suggestions is passive → auto stays silent.
    assert!(o.auto_generate_suggestions(now()).unwrap().is_empty());

    // Explicit generation finds the exam within 3 days.
    let fresh = o.generate_suggestions(now(), 3).unwrap();
    assert!(fresh.iter().any(|s| s.dedup_key.starts_with("exam_prep:")));
    // Rerun → nothing new (dedup).
    assert!(o.generate_suggestions(now(), 3).unwrap().is_empty());

    // Butler auto-generation works and also stays deduped.
    o.set_proactivity(
        ProactivityDimension::LifeSuggestions,
        ProactivityLevel::Butler,
    )
    .unwrap();
    assert!(o.auto_generate_suggestions(now()).unwrap().is_empty());

    // Status transitions persist; ledger shows the suggestion layer.
    let all = o.suggestions().unwrap();
    let id = all[0].id.unwrap();
    o.set_suggestion_status(id, SuggestionStatus::Dismissed, now())
        .unwrap();
    assert_eq!(
        o.suggestions()
            .unwrap()
            .iter()
            .find(|s| s.id == Some(id))
            .unwrap()
            .status,
        SuggestionStatus::Dismissed
    );
    assert!(o
        .ledger()
        .unwrap()
        .iter()
        .any(|m| m.layer == MemoryLayer::Suggestion));
    o.forget(MemoryLayer::Suggestion, id).unwrap();
    assert!(!o.suggestions().unwrap().iter().any(|s| s.id == Some(id)));
}

#[test]
fn daily_brief_aggregates_ingested_schedule_reminders_and_suggestions() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开会", now()).unwrap();
    o.ingest("后天上午九点期末考试", now()).unwrap();
    o.generate_suggestions(now(), 3).unwrap();

    let brief = o.daily_brief(dt(2026, 7, 7, 12, 0)).unwrap();
    assert_eq!(brief.events_today.len(), 1);
    assert_eq!(brief.events_today[0].title, "开会");
    // The exam's three-day lead time is pending and therefore overdue by
    // the next day, while the meeting's 14:30 reminder is still upcoming.
    assert_eq!(brief.due_reminders.len(), 1);
    assert_eq!(brief.upcoming_reminders.len(), 1);
    assert_eq!(brief.top_suggestions.len(), 1);
    assert!(brief.top_suggestions[0].text.contains("期末考试"));
}

#[test]
fn habit_suggestion_from_journal() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("我在护肤", dt(2026, 7, 3, 7, 15)).unwrap();
    o.ingest("我在护肤", dt(2026, 7, 4, 7, 25)).unwrap();
    o.ingest("我在护肤", dt(2026, 7, 5, 7, 20)).unwrap();
    let fresh = o.generate_suggestions(now(), 3).unwrap();
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].dedup_key, "habit_reminder:护肤");
    assert!(fresh[0].text.contains("07:20"));
}

#[test]
fn forget_removes_from_ledger() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("明天下午3点开会", now()).unwrap();
    let ev_id = out.event.unwrap().id.unwrap();
    o.forget(MemoryLayer::Event, ev_id).unwrap();
    // Event and its notification gone; only the raw input remains.
    assert!(o.agenda(now()).unwrap().is_empty());
    assert_eq!(o.ledger().unwrap().len(), 1);
}

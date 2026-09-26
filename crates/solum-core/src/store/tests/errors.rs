//! errors. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

/// CoreError must keep the SQLite result code: callers (and incident
/// logs) need BUSY vs CONSTRAINT vs READONLY to decide retry vs report,
/// and `Display` alone drops that distinction. Uses the same trigger
/// fault-injection as the orchestrator rollback tests to force a real
/// constraint violation through the `From` impl.
#[test]
pub(crate) fn storage_errors_keep_the_sqlite_error_code() {
    let s = Store::open_in_memory().unwrap();
    s.conn
        .execute_batch(
            "CREATE TRIGGER test_tripwire_events BEFORE INSERT ON events
                 BEGIN SELECT RAISE(ABORT, 'injected test failure'); END;",
        )
        .unwrap();
    let err = s
        .insert_event_with_scope(
            &crate::model::Event {
                id: None,
                title: "会被拦截".into(),
                kind: crate::model::EventKind::Meeting,
                start: dt(2026, 7, 7, 15, 0),
                end: None,
                location: None,
                people: Vec::new(),
                raw_input: String::new(),
                created_at: dt(2026, 7, 6, 10, 0),
            },
            None,
            false,
            None,
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("ConstraintViolation"),
        "error must carry the SQLite code, got: {text}"
    );
    assert!(
        matches!(err, CoreError::Storage(_)),
        "variant stays Storage for existing matchers"
    );
}

/// A corrupt people_json row must not silently read as "no participants":
/// list_events keeps going (one bad row cannot blank the whole ledger)
/// but the corruption is visible on stderr with the row id.
#[test]
pub(crate) fn corrupt_people_json_is_reported_not_silently_swallowed() {
    let s = Store::open_in_memory().unwrap();
    let raw = s
        .insert_raw_input(
            "明天下午3点开损坏JSON会议",
            "schedule_event",
            dt(2026, 7, 6, 10, 0),
        )
        .unwrap();
    s.conn
            .execute_batch(&format!(
                "INSERT INTO events(title, kind, start, people_json, raw_input_id, created_at)
                 VALUES ('损坏JSON会议', 'meeting', '2026-07-07T15:00:00', '{{oops', {raw}, '2026-07-06T10:00:00');"
            ))
            .unwrap();

    let events = s.list_events().unwrap();
    assert_eq!(events.len(), 1, "the bad row still surfaces");
    assert!(events[0].people.is_empty(), "fallback stays empty");
}

/// Same policy for a notification whose channels_json is corrupt.
#[test]
pub(crate) fn corrupt_channels_json_is_reported_not_silently_swallowed() {
    let s = Store::open_in_memory().unwrap();
    s.conn
            .execute_batch(
                "INSERT INTO events(title, kind, start, created_at)
                 VALUES ('渠道损坏会议', 'meeting', '2026-07-07T15:00:00', '2026-07-06T10:00:00');
                 INSERT INTO notifications(event_id, fire_at, lead_label, channels_json, status, created_at)
                 VALUES (1, '2026-07-07T14:00:00', '1h', '[broken', 'pending', '2026-07-06T10:00:00');",
            )
            .unwrap();

    let notifications = s.list_notifications().unwrap();
    assert_eq!(notifications.len(), 1);
    assert!(notifications[0].channels.is_empty());
}

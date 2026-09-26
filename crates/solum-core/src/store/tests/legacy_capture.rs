//! legacy capture. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::events::sample_event;
use super::sync_core::dt;

#[test]
pub(crate) fn phase9_legacy_capture_migration_runs_once() {
    let s = Store::open_in_memory().unwrap();
    let add_notification = |event_id| {
        s.insert_notification(&Notification {
            id: None,
            event_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        })
        .unwrap()
    };

    // Simulate a v6 database that had notification-source rows before
    // this setting existed. The migration preserves the old local-only
    // boundary and retracts its unsent oplog entries.
    let old_raw = s
        .insert_raw_input(
            "[通知·legacy] 明天下午3点开会",
            "event",
            dt(2026, 7, 6, 10, 0),
        )
        .unwrap();
    let old_event = s
        .insert_event_with_scope(&sample_event(), Some(old_raw), false, None)
        .unwrap();
    let old_notification = add_notification(old_event);
    s.conn
        .execute(
            "UPDATE meta SET value = '6' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    s.migrate().unwrap();
    assert!(s.raw_input_is_local_only(old_raw).unwrap());
    assert_eq!(
        s.list_raw_inputs()
            .unwrap()
            .into_iter()
            .find(|row| row.id == old_raw)
            .unwrap()
            .source_id,
        "notification"
    );
    assert!(s.event_is_local_only(old_event).unwrap());
    assert_eq!(
        s.conn
            .query_row(
                "SELECT local_only FROM notifications WHERE id = ?1",
                params![old_notification],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    // v10: the legacy rows keep their local_only stamp (no cloud LLM) but
    // are bootstrapped into the oplog, since sync no longer depends on it.
    assert!(
        !s.local_ops_after(0).unwrap().is_empty(),
        "v9→v10 must bootstrap previously unsynced captures into the oplog"
    );

    // A later open is not allowed to re-mark captures made under an
    // enabled setting; their scope is chosen at ingestion time.
    let new_raw = s
        .insert_raw_input("[通知·new] 明天下午4点开会", "event", dt(2026, 7, 6, 10, 0))
        .unwrap();
    let new_event = s
        .insert_event_with_scope(&sample_event(), Some(new_raw), false, None)
        .unwrap();
    let new_notification = add_notification(new_event);
    // A later schema-only upgrade must not re-run the Phase 9 privacy
    // migration over captures whose scope was already chosen at ingest.
    s.conn
        .execute(
            "UPDATE meta SET value = '8' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    s.migrate().unwrap();
    assert!(!s.raw_input_is_local_only(new_raw).unwrap());
    assert_eq!(
        s.list_raw_inputs()
            .unwrap()
            .into_iter()
            .find(|row| row.id == new_raw)
            .unwrap()
            .source_id,
        "notification"
    );
    assert!(!s.event_is_local_only(new_event).unwrap());
    assert_eq!(
        s.conn
            .query_row(
                "SELECT local_only FROM notifications WHERE id = ?1",
                params![new_notification],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

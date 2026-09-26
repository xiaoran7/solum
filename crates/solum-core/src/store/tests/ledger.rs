//! ledger. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::events::sample_event;
use super::sync_core::dt;

#[test]
pub(crate) fn ledger_lists_and_cascade_deletes() {
    let s = Store::open_in_memory().unwrap();
    let raw = s
        .insert_raw_input("明天下午3点开会", "ingest_event", dt(2026, 7, 6, 10, 0))
        .unwrap();
    let ev_id = s.insert_event(&sample_event(), Some(raw)).unwrap();
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
    s.insert_notification(&n).unwrap();

    let ledger = s.memory_ledger().unwrap();
    // raw input + event + notification = 3 entries.
    assert_eq!(ledger.len(), 3);

    // Deleting the raw input cascades to event and notification.
    s.delete_memory(MemoryLayer::RawInput, raw).unwrap();
    assert!(s.memory_ledger().unwrap().is_empty());
    assert!(s.list_events().unwrap().is_empty());
    assert!(s.list_notifications().unwrap().is_empty());
}

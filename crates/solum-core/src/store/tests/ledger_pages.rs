//! ledger pages. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

/// F12 ledger paging: pages walk every row exactly once in the same
/// newest-first order the full ledger produces, cursors chain cleanly,
/// and an exhausted layer returns no cursor.
#[test]
pub(crate) fn ledger_pages_match_the_full_ledger_order() {
    let s = Store::open_in_memory().unwrap();
    // Seed three layers with known timestamps, including same-second
    // ties that exercise the id tiebreak.
    for i in 0..5 {
        s.insert_raw_input(
            &format!("原始输入{i}"),
            "schedule_event",
            dt(2026, 7, 6, 10, i % 2),
        )
        .unwrap();
    }
    for i in 0..4 {
        s.insert_fact_if_new(&crate::memory::MemoryFact {
            id: None,
            content: format!("事实{i}"),
            source: "chat".into(),
            created_at: dt(2026, 7, 6, 9, i),
            last_used_at: None,
        })
        .unwrap()
        .unwrap();
    }
    s.conn
        .execute_batch(
            "INSERT INTO behavior_log(ts, kind, content) VALUES
                 ('2026-07-06T08:00:00', 'status', '状态一'),
                 ('2026-07-06T08:00:00', 'status', '状态二'),
                 ('2026-07-06T08:01:00', 'reminder_fired', '触发一');",
        )
        .unwrap();

    let full = s.memory_ledger().unwrap();

    for (layer, expected_len) in [
        (MemoryLayer::RawInput, 5usize),
        (MemoryLayer::Fact, 4),
        (MemoryLayer::Behavior, 3),
    ] {
        let expected: Vec<(String, i64)> = full
            .iter()
            .filter(|e| e.layer == layer)
            .map(|e| (e.summary.clone(), e.id))
            .collect();
        assert_eq!(expected.len(), expected_len);

        // Page through two-at-a-time and compare against the full view.
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (entries, next) = s.memory_ledger_page(layer, 2, cursor.as_deref()).unwrap();
            assert!(entries.len() <= 2);
            for e in &entries {
                seen.push((e.summary.clone(), e.id));
            }
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, expected, "paged {layer:?} must equal full order");
    }

    // A page size larger than the table just returns everything at once.
    let (all, next) = s.memory_ledger_page(MemoryLayer::Fact, 500, None).unwrap();
    assert_eq!(all.len(), 4);
    assert!(next.is_none());
}

/// Cursor validation is fail-closed: garbage input must be rejected, not
/// silently read as "first page".
#[test]
pub(crate) fn ledger_page_rejects_malformed_cursors_and_limits() {
    let s = Store::open_in_memory().unwrap();
    assert!(s.memory_ledger_page(MemoryLayer::Event, 0, None).is_err());
    assert!(s.memory_ledger_page(MemoryLayer::Event, 501, None).is_err());
    assert!(s
        .memory_ledger_page(MemoryLayer::Event, 10, Some("not-a-cursor"))
        .is_err());
    assert!(s
        .memory_ledger_page(MemoryLayer::Event, 10, Some("2026-07-06T10:00:00|x"))
        .is_err());
}

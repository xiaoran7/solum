//! quarantine. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

/// Park an op the way an older build would have: it named a table that
/// build did not know.
pub(super) fn park(s: &Store, tbl: &str, guid: &str, payload: &str, hlc: &str) {
    s.conn
        .execute(
            "INSERT INTO sync_quarantine(tbl, guid, op, payload, hlc, origin, reason, held_at)
                 VALUES (?1, ?2, 'upsert', ?3, ?4, 'peer', 'unknown sync table', ?4)",
            params![tbl, guid, payload, hlc],
        )
        .unwrap();
}

pub(super) fn event_payload(title: &str) -> String {
    serde_json::json!({
        "title": title,
        "kind": "meeting",
        "start": crate::model::fmt_ts(&dt(2026, 7, 9, 15, 0)),
        "people_json": "[]",
        "created_at": crate::model::fmt_ts(&dt(2026, 7, 7, 10, 0)),
    })
    .to_string()
}

/// The whole reason quarantine beats "just skip it": an op held by an
/// older build must still land after the upgrade that teaches us the
/// table. Skipping would have advanced the cursor past it forever.
#[test]
pub(crate) fn quarantined_ops_replay_once_this_build_knows_the_table() {
    let s = Store::open_in_memory().unwrap();
    park(
        &s,
        "events",
        &"a".repeat(32),
        &event_payload("升级后补上的会议"),
        "2026-07-07T10:00:00.000",
    );
    assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));

    // Re-running migrate is what an app restart after an upgrade does.
    s.migrate().unwrap();

    let titles: Vec<String> = s
        .list_events()
        .unwrap()
        .iter()
        .map(|e| e.title.clone())
        .collect();
    assert_eq!(titles, vec!["升级后补上的会议".to_string()]);
    assert_eq!(s.sync_quarantine_stats().unwrap(), (0, 0));
}

/// A table *this* build still does not know stays parked — it is not
/// dropped just because a migration ran for other reasons.
///
/// This used to use `widget_defs` as the stand-in for a future table.
/// That table shipped in v13, so the name has to be one no build knows,
/// or the test silently starts asserting the opposite of its own name.
#[test]
pub(crate) fn a_still_unknown_table_stays_parked_across_migrations() {
    let s = Store::open_in_memory().unwrap();
    park(
        &s,
        "a_table_from_a_future_version",
        &"b".repeat(32),
        r#"{"name":"日程表"}"#,
        "2026-07-07T10:00:00.000",
    );

    s.migrate().unwrap();
    s.migrate().unwrap();

    assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
}

/// A known table with an unusable payload must not take the batch down
/// with it, and the savepoint must leave nothing half-written.
#[test]
pub(crate) fn a_malformed_known_table_op_is_quarantined_without_partial_writes() {
    let s = Store::open_in_memory().unwrap();
    let good = crate::sync::SyncOp {
        tbl: "events".into(),
        guid: "c".repeat(32),
        op: "upsert".into(),
        payload: Some(event_payload("完好的会议")),
        hlc: "2026-07-07T10:00:00.000".into(),
        origin: "peer".into(),
    };
    // `kind` is required; without it the arm fails while building params.
    let broken = crate::sync::SyncOp {
        tbl: "events".into(),
        guid: "d".repeat(32),
        op: "upsert".into(),
        payload: Some(r#"{"title":"缺字段的会议"}"#.into()),
        hlc: "2026-07-07T10:00:01.000".into(),
        origin: "peer".into(),
    };

    let counts = s.apply_remote_ops(&[good, broken]).unwrap();

    assert_eq!(
        counts,
        MergeCounts {
            applied: 1,
            skipped: 0,
            quarantined: 1
        }
    );
    // The good sibling survived; the broken one wrote nothing.
    let titles: Vec<String> = s
        .list_events()
        .unwrap()
        .iter()
        .map(|e| e.title.clone())
        .collect();
    assert_eq!(titles, vec!["完好的会议".to_string()]);
    assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
}

/// The same op arriving twice must be held once — replaying it twice
/// would double-apply after an upgrade.
#[test]
pub(crate) fn re_holding_the_same_op_does_not_duplicate_it() {
    let s = Store::open_in_memory().unwrap();
    let op = crate::sync::SyncOp {
        tbl: "widget_defs".into(),
        guid: "e".repeat(32),
        op: "upsert".into(),
        payload: Some(r#"{"name":"日程表"}"#.into()),
        hlc: "2026-07-07T10:00:00.000".into(),
        origin: "peer".into(),
    };
    s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
    s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
    assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
}

/// A device that never upgrades must not grow the hold list without end;
/// the overflow is dropped *and counted*, never silently.
#[test]
pub(crate) fn quarantine_overflow_drops_oldest_and_counts_it() {
    let s = Store::open_in_memory().unwrap();
    for i in 0..(MAX_QUARANTINE_OPS + 3) {
        let op = crate::sync::SyncOp {
            tbl: "widget_defs".into(),
            guid: format!("{i:032}"),
            op: "upsert".into(),
            payload: Some("{}".into()),
            hlc: format!("2026-07-07T10:00:{:02}.000", i % 60),
            origin: "peer".into(),
        };
        s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
    }
    assert_eq!(
        s.sync_quarantine_stats().unwrap(),
        (MAX_QUARANTINE_OPS, 3),
        "held should cap out, and the drops must be visible"
    );
}

//! sync core. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::events::sample_event;

pub(super) fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
}

/// P2 regression: a peer stamping ops years ahead used to win LWW forever,
/// freezing that row against every later local correction. Such an op is
/// now quarantined — held and visible, not applied and not lost.
#[test]
pub(crate) fn an_op_stamped_far_in_the_future_is_quarantined_not_applied() {
    let s = Store::open_in_memory().unwrap();
    let counts = s
        .apply_remote_ops(&[crate::sync::SyncOp {
            tbl: "events".into(),
            guid: "guid-future".into(),
            op: "upsert".into(),
            payload: Some(
                serde_json::json!({
                    "guid": "guid-future",
                    "title": "来自未来的会议",
                    "kind": "meeting",
                    "start": "2026-07-09T15:00:00",
                    "created_at": "2026-07-07T10:00:00",
                })
                .to_string(),
            ),
            hlc: "2099-01-01T00:00:00.000".into(),
            origin: "hostile-peer".into(),
        }])
        .unwrap();

    assert_eq!(counts.applied, 0);
    assert_eq!(counts.quarantined, 1);
    assert!(s.list_events().unwrap().is_empty());
    assert_eq!(s.sync_quarantine_stats().unwrap().0, 1, "held, not dropped");

    // …and the hostile stamp must not have dragged this device's own
    // logical clock into the future with it.
    let hlc_last = s.get_meta("hlc_last").unwrap().unwrap_or_default();
    assert!(
        hlc_last.as_str() < "2099",
        "local clock absorbed the forged stamp: {hlc_last}"
    );
}

/// Follow-up review P2: `MAX(wall, hlc_last)` is only *non-decreasing*.
/// Two updates to the same row inside one millisecond got the identical
/// `(hlc, origin)`, and `apply_remote_ops` requires strictly newer — so a
/// peer applied the first and silently dropped the second, diverging
/// forever. Empirically `strftime('now')` twice in one statement always
/// returns the same value, so this was certain, not rare.
#[test]
pub(crate) fn rapid_updates_to_one_row_get_strictly_increasing_stamps() {
    let s = Store::open_in_memory().unwrap();
    let id = s.insert_event(&sample_event(), None).unwrap();
    // Hammer the same row; all of these land within a millisecond or two.
    for h in 0..20 {
        s.update_event_times(id, dt(2026, 7, 7, h % 24, 0), None)
            .unwrap();
    }
    let mut stamps = s
        .conn
        .prepare("SELECT hlc FROM sync_oplog WHERE tbl = 'events' ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(stamps.len() >= 20, "expected one op per update");
    let total = stamps.len();
    stamps.dedup();
    assert_eq!(
        stamps.len(),
        total,
        "duplicate (hlc, origin) means a peer would silently drop an update"
    );
}

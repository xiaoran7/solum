//! open adopt. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::fixtures::scratch;
use super::sync_core::dt;

/// Writer-vs-writer collisions between the shell's two long-lived
/// connections (`orch`, `sync_store`) must wait, not error: with SQLite's
/// default zero timeout a losing write surfaced as SQLITE_BUSY mid-`fire_due`
/// and vanished into the ticker's best-effort logging, silently dropping
/// half of a reminder fire. If this value drifts back to 0, that loss
/// quietly comes back.
#[test]
pub(crate) fn open_sets_a_busy_timeout_so_writer_collisions_wait_instead_of_erroring() {
    let dir = scratch("busy-timeout");
    let store = Store::open(dir.join("solum.sqlite").to_str().unwrap()).unwrap();
    let ms: i64 = store
        .conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        ms, 5000,
        "busy_timeout drifted from the configured 5s writer-wait"
    );
}

/// A crash between parking an oversized op and advancing the push cursor
/// makes the next round re-read and re-park the same op. That must be a
/// no-op, not a growing pile of hold rows.
#[test]
pub(crate) fn hold_push_op_is_idempotent_per_oplog_id() {
    let store = Store::open_in_memory().unwrap();
    let op = crate::sync::SyncOp {
        tbl: "events".into(),
        guid: "e".repeat(32),
        op: "upsert".into(),
        payload: Some(r#"{"title":"超大"}"#.into()),
        hlc: "2026-08-21T10:00:00.000".into(),
        origin: "self".into(),
    };
    store.hold_push_op(7, &op, "单条超限").unwrap();
    store.hold_push_op(7, &op, "单条超限").unwrap();
    assert_eq!(store.push_hold_count().unwrap(), 1);
    store.hold_push_op(8, &op, "单条超限").unwrap();
    assert_eq!(store.push_hold_count().unwrap(), 2);
}

/// The P1 this guards: data committed but still living in the WAL must
/// survive adoption. The old file-by-file rename could move the main
/// database and leave the WAL behind, and because the next launch then saw
/// a database already at the new path it never retried — the committed
/// rows were simply gone, silently and permanently.
#[test]
pub(crate) fn adopting_a_pre_rename_database_keeps_data_still_in_the_wal() {
    let dir = scratch("moves");
    let legacy = dir.join("pa.sqlite");
    let current = dir.join("solum.sqlite");

    // A real WAL-mode database with a committed row that has *not* been
    // checkpointed into the main file yet.
    {
        let store = Store::open(legacy.to_str().unwrap()).unwrap();
        store
            .insert_event(
                &Event::new(
                    "改名前的会议",
                    crate::model::EventKind::Meeting,
                    dt(2026, 7, 9, 15, 0),
                    "src",
                    dt(2026, 7, 7, 10, 0),
                ),
                None,
            )
            .unwrap();
        assert!(
            dir.join("pa.sqlite-wal").exists(),
            "precondition: the commit is in the WAL"
        );
    }

    assert!(adopt_legacy_db(&legacy, &current).unwrap());
    assert!(!legacy.exists(), "legacy database is moved, not copied");

    // The adopted database must still contain the row.
    let adopted = Store::open(current.to_str().unwrap()).unwrap();
    let titles: Vec<String> = adopted
        .list_events()
        .unwrap()
        .into_iter()
        .map(|e| e.title)
        .collect();
    assert_eq!(titles, vec!["改名前的会议".to_string()]);
    drop(adopted);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dangerous case: adopting must never clobber a database the user has
/// already built up under the new name.
#[test]
pub(crate) fn adopting_never_overwrites_an_existing_database() {
    let dir = scratch("keeps");
    let legacy = dir.join("pa.sqlite");
    let current = dir.join("solum.sqlite");
    std::fs::write(&legacy, b"old").unwrap();
    std::fs::write(&current, b"new").unwrap();

    assert!(!adopt_legacy_db(&legacy, &current).unwrap());

    assert_eq!(std::fs::read(&current).unwrap(), b"new");
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        b"old",
        "the legacy file stays put for the user to inspect"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
pub(crate) fn adopting_is_a_no_op_on_a_clean_install() {
    let dir = scratch("clean");
    assert!(!adopt_legacy_db(&dir.join("pa.sqlite"), &dir.join("solum.sqlite")).unwrap());
    assert!(!dir.join("solum.sqlite").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

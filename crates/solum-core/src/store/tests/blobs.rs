//! blobs. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;

/// Follow-up review P2: a peer with a mismatched key can emit unreadable
/// blobs indefinitely. Keeping every one of them would let it fill this
/// device's disk.
#[test]
pub(crate) fn parked_unopenable_blobs_are_bounded_and_the_loss_is_visible() {
    let s = Store::open_in_memory().unwrap();
    for seq in 1..=(MAX_BAD_BLOBS + 25) {
        s.hold_bad_blob(seq, "peer", "AAAA", "wrong key").unwrap();
    }
    let (held, dropped) = s.bad_blob_stats().unwrap();
    assert!(held <= MAX_BAD_BLOBS, "held {held} exceeds the cap");
    assert_eq!(dropped, 25, "the dropped count must be visible, not silent");

    // An oversized payload keeps the record but not the bytes.
    let huge = "A".repeat(MAX_BAD_BLOB_BYTES + 10);
    s.hold_bad_blob(99_999, "peer", &huge, "wrong key").unwrap();
    let kept: String = s
        .conn
        .query_row(
            "SELECT blob_b64 FROM sync_bad_blobs WHERE seq = 99999",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(kept.is_empty(), "oversized ciphertext should not be stored");
}

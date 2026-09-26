//! health. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

/// P3 regression: `dedup_key` excludes the value on purpose, so a platform
/// correcting a reading re-sends the same key with a new number. Under
/// `INSERT OR IGNORE` that correction was dropped and the first number
/// kept forever — the opposite of what `dedup_key`'s doc comment claims.
#[test]
pub(crate) fn a_corrected_health_reading_replaces_the_original() {
    let s = Store::open_in_memory().unwrap();
    let mut sample = crate::wearable::HealthSample::new(
        crate::wearable::HealthMetric::Steps,
        dt(2026, 7, 12, 8, 0),
        dt(2026, 7, 12, 9, 0),
        1000.0,
        "health_connect",
    );
    assert!(s.insert_health_sample_if_new(&sample).unwrap().is_some());

    // Same interval, same source, corrected value.
    sample.value = 1234.0;
    s.insert_health_sample_if_new(&sample).unwrap();

    let stored = s.list_health_samples().unwrap();
    assert_eq!(stored.len(), 1, "a correction must not add a second row");
    assert_eq!(stored[0].value, 1234.0, "the correction should have landed");
}

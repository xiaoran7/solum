//! health. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::now;

#[test]
fn health_samples_dedup_and_land_in_ledger() {
    use crate::wearable::{HealthMetric, HealthSample};
    let mut o = Orchestrator::in_memory().unwrap();
    let hr = HealthSample::new(
        HealthMetric::HeartRate,
        now(),
        now(),
        72.0,
        "health_connect",
    );
    let n = o.record_health_samples(vec![hr.clone()], now()).unwrap();
    assert_eq!(n, 1);
    // Re-polling the same instant (overlapping platform window) must not
    // duplicate the row.
    let n2 = o.record_health_samples(vec![hr], now()).unwrap();
    assert_eq!(n2, 0);
    assert_eq!(o.health_samples().unwrap().len(), 1);

    let ledger = o.ledger().unwrap();
    assert!(ledger
        .iter()
        .any(|e| e.layer == MemoryLayer::Wearable && e.summary.contains("心率")));

    let id = o.health_samples().unwrap()[0].id.unwrap();
    o.forget(MemoryLayer::Wearable, id).unwrap();
    assert!(o.health_samples().unwrap().is_empty());
}

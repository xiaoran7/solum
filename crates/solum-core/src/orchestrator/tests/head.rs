//! head. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

//! Shared date fixtures, the one thing every section file needs.

use chrono::{NaiveDate, NaiveDateTime};

pub(crate) fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
}

pub(crate) fn now() -> NaiveDateTime {
    dt(2026, 7, 6, 10, 0)
}

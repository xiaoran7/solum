//! events. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

pub(super) fn sample_event() -> Event {
    let mut ev = Event::new(
        "开会",
        EventKind::Meeting,
        dt(2026, 7, 7, 15, 0),
        "明天下午3点开会",
        dt(2026, 7, 6, 10, 0),
    );
    ev.location = Some("会议室".into());
    ev.people = vec!["张伟".into()];
    ev
}

//! Intent routing and natural-language → structured event extraction.
//!
//! Two layers:
//! - [`route_intent`] — a fast, offline classifier that decides what kind of
//!   input we're dealing with (chat / event / status answer / dangerous
//!   command). It never executes anything; dangerous routing is only a hint,
//!   the real enforcement is the HITL guard (see [`crate::guard`]).
//! - [`Extractor`] — turns an event-shaped utterance into a structured
//!   [`Event`]. The default [`RuleBasedExtractor`] works entirely offline; a
//!   cloud [`Reasoner`] can be layered on later behind the same trait.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use std::sync::OnceLock;

use chrono::NaiveDateTime;
use regex::Regex;

use crate::error::Result;
use crate::model::{Event, EventKind};
use crate::time_parse::parse_datetime;
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use cancel::parse_cancel;
pub use extractor::{Extractor, RuleBasedExtractor};
pub use intent::route_intent;
pub use kind::Intent;
pub use reasoner::{NullReasoner, Reasoner, ReasonerResponse, ReasonerToolCall};
pub use reschedule::{parse_reschedule, RescheduleRequest};
pub use widget::might_request_widget;

// -- macro, hoisted: `#[macro_use]` on a `mod` reaches DOWNWARD only, and
// -- the section declarations are anchored on the first `use` -- so a macro
// -- below them can never be reached, however many attributes are added.
macro_rules! lazy_re {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex compiles"))
    }};
}

mod cancel;
mod eventwords;
mod extractor;
mod intent;
mod kind;
mod reasoner;
mod reschedule;
#[cfg(test)]
mod tests;
mod widget;
mod words;

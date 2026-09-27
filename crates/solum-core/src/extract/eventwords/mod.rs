//! `eventwords` -- the two-line gate in front of date parsing, deliberately separate from the word lists it reads: having an event word is necessary but not sufficient, and running the date regex first would make every time-flavored chat line an event.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::words::EVENT_WORDS;
use super::*;

fn has_event_word(lower: &str) -> bool {
    EVENT_WORDS.iter().any(|w| lower.contains(*w))
}

/// Whether an utterance is worth turning into a scheduled event: it must carry
/// an explicit clock time *or* an event keyword. A bare date word ("今天") on
/// its own is chat, not a calendar entry.
pub(super) fn is_schedulable(text: &str, now: NaiveDateTime) -> bool {
    let explicit_time = parse_datetime(text, now)
        .map(|p| p.explicit_time)
        .unwrap_or(false);
    explicit_time || has_event_word(&text.to_lowercase())
}

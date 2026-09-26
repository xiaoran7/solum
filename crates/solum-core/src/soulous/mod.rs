//! Soulous read-only fact source (Phase 8.1, ARCHITECTURE.md §3.11).
//!
//! This module is deliberately a *data source*, not a memory writer: fetched
//! rows stay in `soulous_facts`, never enter `memory_facts` or recall, and
//! network failures leave the prior complete snapshot untouched. All business
//! timestamps are supplied by the shell as `now`; solum-core never reads a clock.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

mod config;
mod domain;
mod http;
mod parse;
mod push;
mod review;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::classify::{ImportanceRule, RuleTable};
use crate::error::{CoreError, Result};
use crate::model::parse_ts;
use crate::store::Store;
use crate::suggest::{Suggestion, SuggestionKind, SuggestionStatus};
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use config::{SoulousConfig, SCHEDULE_EVENT_TYPE, SOLUM_SOURCE, SOURCE};
pub use domain::{
    build_status, classify_exams, generate_suggestions, SoulousExamImportance, SoulousFact,
    SoulousKind, SoulousStatus,
};
pub use http::{
    pull_configured, push_event_configured, HttpSoulousClient, PullOutcome, SoulousHttp,
    SoulousHttpError,
};
pub use push::{preview_event_push, PushOutcome};
pub use review::{review_material, ReviewMaterial};

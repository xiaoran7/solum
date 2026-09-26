//! Tests for the orchestrator, split by the concern under test so each
//! section file holds one area's regressions.
//!
//! `head.rs` carries the `dt`/`now` date fixtures every
//! other file uses; the shared imports live in this file. The rest mirror
//! the source sections in `orchestrator/`.

mod capture;
mod genui;
mod guard_memory;
mod head;
mod health;
mod import;
mod llm;
mod llm_first;
mod notif_privacy;
mod persona;
mod reminders;
mod review_suggest;
mod routines;
mod scene_wellness;
mod widgets;

// Every section file did `use super::*;` when the suite was one module,
// so the shared imports belong here, not in a single section.
use super::*;
use crate::journal::BehaviorKind;

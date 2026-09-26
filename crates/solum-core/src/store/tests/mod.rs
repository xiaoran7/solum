//! Tests for the store, split by the area under test so each section file
//! holds one subsystem's regressions.
//!
//! `fixtures.rs` carries the shared imports and helpers; the rest mirror the
//! source sections in `store/`.

mod audit_config;
mod blobs;
mod crud;
mod errors;
mod events;
mod fixtures;
mod health;
mod ledger;
mod ledger_pages;
mod legacy_capture;
mod migrations;
mod open_adopt;
mod quarantine;
mod sync_core;
mod widget_sync;

// Every section file did `use super::*;` when the suite was one module,
// so the shared imports belong here, not in one section.
use super::*;
use crate::guard::Decision;
use crate::model::{EventKind, RiskLevel};
use chrono::NaiveDate;

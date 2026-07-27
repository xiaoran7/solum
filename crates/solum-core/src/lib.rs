//! solum-core — the Solum (Personal Agent) orchestrator core.
//!
//! Storage- and UI-agnostic domain logic: intent routing, natural-language
//! event extraction, importance classification, notification scheduling, the
//! HITL safety guard, and the local memory store. See ../../docs/ARCHITECTURE.md.

pub mod account;
pub mod brief;
pub mod capture;
pub mod classify;
pub mod email;
pub mod error;
pub mod export;
pub mod extract;
pub mod fsatomic;
pub mod genui;
pub mod guard;
pub mod journal;
pub mod llm;
pub mod memory;
pub mod model;
pub mod net;
pub mod notification_intelligence;
pub mod orchestrator;
pub mod paths;
pub mod persona;
pub mod persona_import;
pub mod privacy;
pub mod proactivity;
pub mod recall;
pub mod review;
pub mod routine;
pub mod scene;
pub mod schedule;
pub mod soulous;
pub mod stats;
pub mod store;
pub mod suggest;
pub mod sync;
pub mod time_parse;
pub mod wearable;
pub mod widget;

pub use error::{CoreError, Result};
pub use orchestrator::{IngestOutcome, Orchestrator};

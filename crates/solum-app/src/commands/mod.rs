// Tauri commands domain modularization entry
use super::*;

#[macro_use]
pub(crate) mod system;
pub(crate) use system::*;

#[macro_use]
pub(crate) mod llm;
pub(crate) use llm::*;

#[macro_use]
pub(crate) mod account;
pub(crate) use account::*;

#[macro_use]
pub(crate) mod privacy_capture;
pub(crate) use privacy_capture::*;

#[macro_use]
pub(crate) mod email_calendar;
pub(crate) use email_calendar::*;

#[macro_use]
pub(crate) mod events_widgets;
pub(crate) use events_widgets::*;

#[macro_use]
pub(crate) mod backup;
pub(crate) use backup::*;

#[macro_use]
pub(crate) mod notifications;
pub(crate) use notifications::*;

#[macro_use]
pub(crate) mod guard_audit;
pub(crate) use guard_audit::*;

#[macro_use]
pub(crate) mod persona_insights;
pub(crate) use persona_insights::*;

#[macro_use]
pub(crate) mod health_sync;
pub(crate) use health_sync::*;

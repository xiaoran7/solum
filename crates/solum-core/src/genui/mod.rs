//! Generative UI protocol (F18, ARCHITECTURE.md §3.9) — the component catalog,
//! the action whitelist, and the defensive validator.
//!
//! Four hard rules, mirrored from the architecture doc:
//! 1. A UI description is **data, not code**: a JSON component tree constrained
//!    to this catalog. No scripts, no styling, no free-form layout.
//! 2. Generated UI can only *request* actions. Every action must name a
//!    whitelisted command with schema-checked args; the Rust command layer
//!    re-validates everything, and `dangerous` work still goes through the
//!    §3.3 guard — a generated confirm button is an entrance, not a token.
//! 3. Anything that fails validation degrades to plain text (F16): callers use
//!    [`parse_envelope`], which returns `None` instead of erroring.
//! 4. Envelopes are never persisted (对话即焚): they are one-shot renders of
//!    durable data. What lands in storage is the *result* of actions, through
//!    the existing tables/audit/ledger. See docs/MISC.md 2026-07-14.
//!
//! The renderer (solum-app's frontend) only understands this protocol and does
//! not care whether an envelope came from the offline template builders below
//! or from the cloud reasoner (`llm::chat_reply_ui`).
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use serde::{Deserialize, Serialize};

use crate::brief::Brief;
use crate::model::{fmt_ts, Event, Notification};
use crate::persona::PersonaDraft;
use crate::routine::Routine;
use crate::suggest::Suggestion;
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use parse::parse_envelope;
pub use templates::{
    cancel_confirm, checkin_prompt, daily_brief_prompt, event_ingested, guard_entry,
    persona_draft_form, reschedule_pick, routine_ingested, suggestions_prompt,
};
pub use types::{
    ButtonStyle, FieldKind, UiAction, UiButton, UiComponent, UiEnvelope, UiField, ALLOWED_ACTIONS,
    LLM_ACTIONS, UI_VERSION,
};
pub use validation::{validate, validate_action};

mod parse;
mod templates;
#[cfg(test)]
mod tests;
/// Protocol version. Bump only with a renderer upgrade; the validator rejects
/// anything else so an old renderer never sees components it can't draw.
mod types;
mod validation;

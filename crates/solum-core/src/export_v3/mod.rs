//! Platform-neutral additions carried by `solum-export` v3.
//!
//! The existing v2 `_restore` section remains the authority for guid-bearing
//! SQLite rows. This module covers persistence that v2 cannot represent and
//! validates it without writing any store or platform storage.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

mod build;
mod helpers;
#[cfg(test)]
mod tests;
mod types;
mod validate;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::classify::NotificationMatchKind;
use crate::error::{CoreError, Result};
use crate::model::{fmt_ts, parse_ts};
use crate::notification_intelligence::{ActionProposalState, CaptureState, NotificationActionKind};
use crate::store::Store;

// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use build::{build_portable_export, snapshot_portable_store, validate_portable_document};
pub use types::{
    DocumentManifest, EmailRestoreIntent, ExternalRestoreIntents, PendingNotificationInput,
    PlatformPortableData, PortableChat, PortableChatDraft, PortableChatMessage, PortableChatRole,
    PortableChatSession, PortableLlmSettings, PortablePendingNotification, PortableRestoreIntents,
    PortableSection, SoulousRestoreIntent,
};
pub(crate) use validate::{
    validate_chat, validate_documents, validate_llm, validate_restore_intents,
};

pub const PORTABLE_EXPORT_VERSION: u32 = 3;
pub const PORTABLE_FORMAT: &str = "solum-portable";
pub const PORTABLE_SCHEMA_VERSION: u32 = 1;

const MAX_CHAT_SESSIONS: usize = 40;
const MAX_MESSAGES_PER_SESSION: usize = 240;
const MAX_CHAT_TEXT_CHARS: usize = 2_400_000;
const MAX_MESSAGE_CHARS: usize = 12_000;
const MAX_TITLE_CHARS: usize = 80;
const MAX_DOCUMENTS: usize = 30;
const MAX_DOCUMENT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_DOCUMENT_TOTAL_BYTES: u64 = 600 * 1024 * 1024;
const MAX_PENDING_NOTIFICATIONS: usize = 10_000;
const MAX_NOTIFICATION_FIELD_CHARS: usize = 2_000;
const MAX_AUDIT_ROWS: usize = 100_000;
const MAX_JOURNAL_ROWS: usize = 100_000;

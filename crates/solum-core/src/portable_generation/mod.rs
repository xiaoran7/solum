//! Assemble a validated v3 recovery into one sealed, still-inactive profile generation.
//!
//! Publication is deliberately separate. A caller gets a sealed handle only
//! after SQLite, WebView bootstrap data, attachments, notification spool and
//! the fail-closed native policy have all been written and re-read.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

mod handoff;
mod reader;
mod stage;
mod types;
mod verify;

#[cfg(test)]
mod tests;

// The stage, verify and reader seams between this module and the app layer.
// These were `pub` before the split, and callers outside the crate still name
// them, so they have to stay `pub` -- a re-export cannot be wider than the
// item it re-exports. The section-private helpers that only a sibling needs
// are `pub(super)` and are imported by their consumer directly.
pub use handoff::{publish_portable_generation, resume_portable_handoff};
pub use reader::PortableGenerationReader;
pub use stage::stage_portable_generation;
pub use types::{PortableDocumentChunk, WebViewBootstrap};

use std::collections::BTreeSet;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use chrono::{DateTime, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};
use crate::export::ImportPlan;
use crate::export_v3::{
    DocumentManifest, PortableChat, PortableLlmSettings, PortableRestoreIntents,
};
use crate::profile_generation::{
    begin_profile_generation, publish_profile_generation_unlocked,
    resolve_active_data_dir_unlocked, resume_sealed_profile_generation,
    resume_sealed_profile_generation_unlocked, seal_profile_generation, sync_tree_directories,
    with_generation_lock, SealedProfileGeneration, SQLITE_FILE, STAGING_DIR,
};
use crate::restore_v3::{
    restore_portable_into_empty_store, PlatformRestoreBundle, PreparedPortableRestore,
    RestoredDocument,
};
use crate::store::{MergeCounts, Store};

pub const WEBVIEW_DIR: &str = "webview";
pub const WEBVIEW_BOOTSTRAP_FILE: &str = "bootstrap-v1.json";
pub const WEBVIEW_DOCUMENTS_DIR: &str = "documents";
pub const NOTIFICATION_POLICY_FILE: &str = "notif-policy.json";
pub const NOTIFICATION_SPOOL_DIR: &str = "notif-spool";
pub const FAILED_NOTIFICATION_DIR: &str = "failed";

const BOOTSTRAP_FORMAT: &str = "solum-webview-bootstrap";
const BOOTSTRAP_VERSION: u32 = 1;
const MAX_BOOTSTRAP_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SPOOL_FILE_BYTES: u64 = 64 * 1024;
const EMPTY_NOTIFICATION_POLICY: &str = "{\"capture_mode\":\"all\",\"allowed_packages\":[]}";
pub const WEBVIEW_DOCUMENT_CHUNK_BYTES: usize = 512 * 1024;
pub const RESTORE_HANDOFF_DIR: &str = "restore-handoff";
const MAX_HANDOFF_ITEMS: usize = 10_000;
const MAX_HANDOFF_FILE_BYTES: u64 = 64 * 1024;
const LEGACY_INBOX_FILES: &[&str] = &["notif-inbox.jsonl", "notif-inbox.processing.jsonl"];
const LEGACY_OVERFLOW_FILES: &[&str] = &["notif-spool.overflow", "notif-spool.overflow.taking"];

//! Strict opaque-cloud download and inactive portable-generation staging.
//!
//! No function in this module publishes an active profile pointer or advances
//! device generation. Server metadata is transport evidence only; the main
//! and attachment AEAD containers remain the authority for recovery data.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

//! Strict opaque-cloud download and inactive portable-generation staging.
//!
//! No function in this module publishes an active profile pointer or advances
//! device generation. Server metadata is transport evidence only; the main
//! and attachment AEAD containers remain the authority for recovery data.

mod api;
mod error;
mod http;
mod stage;
#[cfg(test)]
mod tests;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::account::AccountSession;
use crate::attachment::{self, AttachmentBinding};
use crate::backup::{self, BackupMetadata};
use crate::backup_upload::{attachment_object_id, BackupObjectKind, BackupObjectPlan, BackupPlan};
use crate::export_v3::{DocumentManifest, PORTABLE_EXPORT_VERSION};
use crate::restore_v3::{self, RestoredAttachment};

// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use api::{
    delete_backup_version, download_and_stage_backup, fetch_backup_catalog,
    validate_backup_delete_request,
};
pub use error::BackupDownloadError;
pub use http::HttpBackupCloudReader;
pub use types::{
    BackupCatalog, BackupCloudDelete, BackupCloudReader, BackupCloudTakeover, BackupDeleteRequest,
    BackupStageRequest, CloudTakeoverOutcome, StagedBackupOutcome,
};

pub const MAX_CLOUD_OBJECT_BYTES: u64 = 68 * 1024 * 1024;
pub const MAX_CLOUD_BACKUP_BYTES: u64 = 700 * 1024 * 1024;
pub const MAX_CLOUD_OBJECTS: usize = 64;
pub const MAX_CATALOG_BACKUPS: usize = 100;

const MAX_CATALOG_BYTES: usize = 512 * 1024;
const HTTP_TIMEOUT_SECS: u64 = 30;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

type DownloadResult<T> = std::result::Result<T, BackupDownloadError>;

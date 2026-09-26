//! mod. Part of the module split out of `backup-upload-original.rs`.
//!
//! See `mod.rs` for the module contract and the original file's doc header.

//! Encrypted-only staging and opaque cloud upload for portable backup v1.
//!
//! This coordinator deliberately has no Store-writing API. It freezes an
//! already recovery-plannable `solum-export` v3 document, seals the main
//! object and every manifest-bound attachment locally, then uploads the same
//! ciphertext bytes on every retry before committing the remote version.

mod error;
mod http;
mod session;
mod staging;
mod types;

#[cfg(test)]
mod tests;

// Every section reached the crate imports through `use super::*;` when this
// was one file. They belong in the parent, because that is what a sibling
// glob actually resolves to. A name no section uses is left out: a section
// that needs a derive macro imports it in its own scope.
use crate::account::AccountSession;
use crate::attachment::{self, AttachmentBinding};
use crate::backup::{self, BackupMetadata};
use crate::export_v3::{self, DocumentManifest};
use hkdf::Hkdf;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

// The error enum and the transport seam are the two faces of this module:
// the enum is in the public signature of every other section, and the trait
// is what the session programs against. Re-export them, plus the two entry
// points and the transport constructor the app layer names, so every path
// callers used before the split keeps working.
pub use error::BackupUploadError;
pub use http::HttpBackupCloudTransport;
pub use session::{load_or_initialize_device_generation, persist_taken_over_device_generation};
pub use staging::attachment_object_id;
pub use types::{
    AttachmentExpectation, BackupCloudTransport, BackupDeviceAuthority, BackupObjectKind,
    BackupObjectPlan, BackupPlan, BackupUploadBegin, BackupUploadOutcome, BackupUploadSession,
};

pub const DEVICE_STATE_FILE: &str = "solum-backup-device.json";
pub const UPLOAD_STAGING_DIR: &str = "backup-upload-staging";
pub const MAX_IPC_CHUNK_BYTES: usize = 512 * 1024;

const DEVICE_STATE_FORMAT: &str = "solum-backup-device";
const DEVICE_STATE_VERSION: u32 = 2;
const HTTP_TIMEOUT_SECS: u64 = 30;
const OBJECT_ID_KDF_SALT: &[u8] = b"solum-backup-object-id:hkdf-salt:v1";
const OBJECT_ID_KDF_INFO: &[u8] = b"solum-backup-object-id:info:v1";

type UploadResult<T> = std::result::Result<T, BackupUploadError>;

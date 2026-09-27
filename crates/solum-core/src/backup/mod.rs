//! Platform-neutral `solum-backup` v1 container codec.
//!
//! This module deliberately stops before persistence: it authenticates,
//! decompresses, parses, and import-plans an inner `solum-export` v2/v3 document,
//! then returns that verified document to a later recovery transaction. The
//! server only needs to store the returned container as opaque bytes.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use std::io::{Read, Write};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, SecondsFormat, Utc};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::export::{self, ImportPlan};
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use format::{
    BackupError, BackupHeader, BackupMetadata, VerifiedBackup, COMPRESSION, ENCRYPTION, FORMAT,
    MAX_CIPHERTEXT_BYTES, MAX_CONTAINER_BYTES, MAX_HEADER_BYTES, MAX_PLAINTEXT_BYTES,
    PAYLOAD_DIGEST, PROTOCOL_VERSION,
};
pub use open::open_backup;
pub use seal::{create_backup, create_backup_from_json_bytes};

mod format;
mod helpers;
mod open;
mod seal;
#[cfg(test)]
mod tests;
mod validate;

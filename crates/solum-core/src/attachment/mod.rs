//! Platform-neutral chunked `solum-attachment` v1 codec.
//!
//! Attachment metadata comes from an already authenticated `solum-export` v3
//! manifest. The server stores only this opaque container; account, backup,
//! attachment, and digest identities never appear in its clear header. Exact
//! byte length remains observable from chunk framing and ciphertext length.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use std::collections::BTreeSet;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::backup::BackupMetadata;
use crate::export_v3::DocumentManifest;
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use format::{
    AttachmentBinding, AttachmentError, AttachmentHeader, ENCRYPTION, FORMAT, KDF, MAX_ATTACHMENTS,
    MAX_ATTACHMENT_BYTES, MAX_CHUNKS, MAX_CHUNK_HEADER_BYTES, MAX_CONTAINER_BYTES,
    MAX_HEADER_BYTES, MAX_TOTAL_ATTACHMENT_BYTES, PRODUCTION_CHUNK_BYTES, PROTOCOL_VERSION,
};
pub use open::open_attachment;
pub use seal::{seal_attachment, validate_attachment_set};

mod format;
mod helpers;
mod open;
mod seal;
#[cfg(test)]
mod tests;

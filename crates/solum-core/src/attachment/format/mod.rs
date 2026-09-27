//! `format` -- the on-disk container format, stated rather than implied: every magic number, length, and size ceiling lives here as a named constant, alongside the binding and header types. A limit that is only written down in the code that enforces it is a limit nobody can check against, so the ceilings are `pub` and the coordinator enforces the same numbers the codec does.
//!
//! Part of the `attachment` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) const MAGIC: &[u8; 8] = b"SOLUMATT";
pub(super) const PREAMBLE_LEN: usize = 12;
pub(super) const RECORD_PREAMBLE_LEN: usize = 8;
pub(super) const TAG_LEN: usize = 16;
pub(super) const NONCE_LEN: usize = 24;
pub(super) const AAD_DOMAIN: &[u8] = b"solum-attachment:aad:v1\0";
pub(super) const KDF_SALT: &[u8] = b"solum-attachment:hkdf-salt:v1";
pub(super) const KDF_INFO_DOMAIN: &[u8] = b"solum-attachment:key:v1\0";

pub const FORMAT: &str = "solum-attachment";
pub const PROTOCOL_VERSION: u32 = 1;
pub const KDF: &str = "hkdf-sha256";
pub const ENCRYPTION: &str = "xchacha20-poly1305";
pub const PRODUCTION_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_HEADER_BYTES: usize = 1024;
pub const MAX_CHUNK_HEADER_BYTES: usize = 512;
pub const MAX_CHUNKS: usize = 4096;
pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ATTACHMENTS: usize = 30;
pub const MAX_TOTAL_ATTACHMENT_BYTES: u64 = 600 * 1024 * 1024;
pub const MAX_CONTAINER_BYTES: usize = PREAMBLE_LEN
    + MAX_HEADER_BYTES
    + MAX_ATTACHMENT_BYTES
    + MAX_CHUNKS * (RECORD_PREAMBLE_LEN + MAX_CHUNK_HEADER_BYTES + TAG_LEN);

pub(super) type AttachmentResult<T> = std::result::Result<T, AttachmentError>;

/// Secret-associated metadata obtained from the authenticated main backup.
/// It is serialized only for HKDF and AAD and is never emitted in the clear
/// attachment container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentBinding {
    pub user_id: String,
    pub device_generation: u64,
    pub backup_id: String,
    pub attachment_id: String,
    pub plaintext_size: u64,
    pub plaintext_sha256: String,
}

impl AttachmentBinding {
    pub fn from_manifest(metadata: &BackupMetadata, manifest: &DocumentManifest) -> Self {
        Self {
            user_id: metadata.user_id.clone(),
            device_generation: metadata.device_generation,
            backup_id: metadata.backup_id.clone(),
            attachment_id: manifest.id.clone(),
            plaintext_size: manifest.plaintext_size,
            plaintext_sha256: manifest.sha256.clone(),
        }
    }
}

/// The non-sensitive, authenticated top-level v1 header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentHeader {
    pub format: String,
    pub protocol_version: u32,
    pub kdf: String,
    pub encryption: String,
    pub chunk_plaintext_size: u32,
    pub chunk_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChunkHeader {
    pub(super) index: u32,
    pub(super) plaintext_offset: u64,
    pub(super) plaintext_size: u32,
    pub(super) ciphertext_size: u32,
    pub(super) nonce: String,
}

/// Errors never include caller-controlled binding values or plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AttachmentError {
    #[error("attachment container is truncated")]
    Truncated,
    #[error("attachment container framing, binding, or fields are invalid")]
    InvalidContainer,
    #[error("attachment exceeds the protocol size limit")]
    SizeLimit,
    #[error("unsupported attachment protocol version {0}")]
    UnsupportedVersion(u32),
    #[error("unsupported attachment {0} algorithm")]
    UnsupportedAlgorithm(&'static str),
    #[error("attachment authentication failed (wrong key, binding, or modified container)")]
    Authentication,
    #[error("attachment plaintext digest does not match its authenticated manifest")]
    DigestMismatch,
    #[error("operating-system random source is unavailable")]
    RandomSource,
}

impl AttachmentError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::InvalidContainer => "invalid_container",
            Self::SizeLimit => "size_limit",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::UnsupportedAlgorithm(_) => "unsupported_algorithm",
            Self::Authentication => "authentication",
            Self::DigestMismatch => "digest_mismatch",
            Self::RandomSource => "random_source",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DecodeLimits {
    pub(super) container: usize,
    pub(super) plaintext: usize,
    pub(super) chunks: usize,
}

pub(super) const PROTOCOL_LIMITS: DecodeLimits = DecodeLimits {
    container: MAX_CONTAINER_BYTES,
    plaintext: MAX_ATTACHMENT_BYTES,
    chunks: MAX_CHUNKS,
};

pub(super) struct ParsedChunk<'a> {
    pub(super) raw_header: &'a [u8],
    pub(super) ciphertext: &'a [u8],
    pub(super) nonce: [u8; NONCE_LEN],
    pub(super) plaintext_size: usize,
}

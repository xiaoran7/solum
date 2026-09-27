//! `format` -- what a `solum-backup` container *is*, stated rather than implied: every magic number, frame length and size ceiling as a named constant, plus the metadata, header and verified-backup shapes and the error type. A limit only written down in the code that enforces it is a limit nobody can check against, so the ceilings are `pub` and the codec enforces the same numbers its callers size their transport with.
//!
//! Part of the `backup` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) const MAGIC: &[u8; 8] = b"SOLUMBAK";
pub(super) const PREAMBLE_LEN: usize = 12;
pub(super) const TAG_LEN: usize = 16;
pub(super) const DIGEST_LEN: usize = 32;
pub(super) const NONCE_LEN: usize = 24;
pub(super) const AAD_DOMAIN: &[u8] = b"solum-backup:aad:v1\0";

pub const FORMAT: &str = "solum-backup";
pub const PROTOCOL_VERSION: u32 = 1;
pub const COMPRESSION: &str = "zlib-deflate";
pub const ENCRYPTION: &str = "xchacha20-poly1305";
pub const PAYLOAD_DIGEST: &str = "sha256";
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_CIPHERTEXT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PLAINTEXT_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_CONTAINER_BYTES: usize = PREAMBLE_LEN + MAX_HEADER_BYTES + MAX_CIPHERTEXT_BYTES;

pub(super) type BackupResult<T> = std::result::Result<T, BackupError>;

/// Stable, non-secret metadata authenticated by the backup container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupMetadata {
    pub user_id: String,
    pub device_generation: u64,
    pub backup_id: String,
    pub created_at: String,
}

/// The exact v1 JSON header. Field declaration order is the writer's stable
/// serialization order; readers authenticate the raw bytes and therefore do
/// not depend on reserialization order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupHeader {
    pub format: String,
    pub protocol_version: u32,
    pub user_id: String,
    pub device_generation: u64,
    pub backup_id: String,
    pub created_at: String,
    pub payload_format: String,
    pub payload_version: u32,
    pub payload_digest: String,
    pub compression: String,
    pub encryption: String,
    pub nonce: String,
    pub plaintext_size: u64,
    pub compressed_size: u64,
    pub ciphertext_size: u64,
}

/// Fully authenticated and import-planned output. Obtaining this value does
/// not write a [`crate::store::Store`].
#[derive(Debug, Clone)]
pub struct VerifiedBackup {
    pub header: BackupHeader,
    pub document: Value,
    pub normalized_payload: Vec<u8>,
    pub import_plan: ImportPlan,
}

/// Errors are intentionally coarse enough not to echo plaintext, keys, or
/// attacker-controlled document contents into logs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupError {
    #[error("backup container is truncated")]
    Truncated,
    #[error("backup container framing or fields are invalid")]
    InvalidContainer,
    #[error("backup exceeds the protocol size limit")]
    SizeLimit,
    #[error("unsupported backup protocol version {0}")]
    UnsupportedVersion(u32),
    #[error("unsupported backup {0} algorithm")]
    UnsupportedAlgorithm(&'static str),
    #[error("backup authentication failed (wrong key or modified container)")]
    Authentication,
    #[error("backup compression stream is invalid")]
    Decompression,
    #[error("backup payload is not a valid recovery-plannable solum-export document")]
    InvalidPayload,
    #[error("operating-system random source is unavailable")]
    RandomSource,
}

impl BackupError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::InvalidContainer => "invalid_container",
            Self::SizeLimit => "size_limit",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::UnsupportedAlgorithm(_) => "unsupported_algorithm",
            Self::Authentication => "authentication",
            Self::Decompression => "decompression",
            Self::InvalidPayload => "invalid_payload",
            Self::RandomSource => "random_source",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DecodeLimits {
    pub(super) ciphertext: usize,
    pub(super) plaintext: usize,
}

pub(super) const PROTOCOL_LIMITS: DecodeLimits = DecodeLimits {
    ciphertext: MAX_CIPHERTEXT_BYTES,
    plaintext: MAX_PLAINTEXT_BYTES,
};

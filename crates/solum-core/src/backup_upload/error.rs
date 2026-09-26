//! error. Part of the `backup_upload` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `backup_upload.rs`.

//! The log-safe error enum every other section returns.

// A `use` in the parent does not bring a *derive macro* into a child's own
// scope; the derive has to be in scope where the `#[derive]` sits.
use thiserror::Error;

/// Coarse, log-safe failures. No variant carries plaintext, keys, tokens,
/// document identifiers, response bodies, or filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupUploadError {
    #[error("backup device generation state is invalid")]
    InvalidDeviceState,
    #[error("backup device generation is stale")]
    StaleGeneration,
    #[error("backup cloud reported a generation or idempotency conflict")]
    CloudConflict,
    #[error("backup snapshot is not a valid portable export")]
    InvalidSnapshot,
    #[error("backup attachment stream is invalid")]
    InvalidAttachmentStream,
    #[error("backup attachment exceeds the protocol size limit")]
    SizeLimit,
    #[error("backup encryption failed")]
    Encryption,
    #[error("encrypted backup staging failed")]
    Staging,
    #[error("backup cloud authentication failed")]
    Authentication,
    #[error("backup cloud request failed")]
    Transport,
    #[error("backup cloud rejected the request with status {0}")]
    ServerStatus(u16),
    #[error("backup cloud response is invalid")]
    InvalidResponse,
    #[error("operating-system random source is unavailable")]
    RandomSource,
}

impl BackupUploadError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidDeviceState => "invalid_device_state",
            Self::StaleGeneration => "stale_generation",
            Self::CloudConflict => "cloud_conflict",
            Self::InvalidSnapshot => "invalid_snapshot",
            Self::InvalidAttachmentStream => "invalid_attachment_stream",
            Self::SizeLimit => "size_limit",
            Self::Encryption => "encryption",
            Self::Staging => "staging",
            Self::Authentication => "authentication",
            Self::Transport => "transport",
            Self::ServerStatus(_) => "server_status",
            Self::InvalidResponse => "invalid_response",
            Self::RandomSource => "random_source",
        }
    }
}

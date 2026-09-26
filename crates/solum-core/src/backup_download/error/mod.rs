//! `error` -- the log-safe error enum every other section returns.
//!
//! Part of the `backup_download` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// Log-safe failures. Variants never carry tokens, keys, plaintext, document
/// identifiers, response bodies, digests, or filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupDownloadError {
    #[error("backup catalog is invalid")]
    InvalidCatalog,
    #[error("requested backup is not present in the validated catalog")]
    BackupNotFound,
    #[error("unsupported backup protocol version {0}")]
    UnsupportedVersion(u32),
    #[error("backup download exceeds the protocol size limit")]
    SizeLimit,
    #[error("backup object transport integrity check failed")]
    TransportIntegrity,
    #[error("backup authentication or container validation failed")]
    Authentication,
    #[error("backup metadata does not match its authenticated container")]
    MetadataMismatch,
    #[error("backup attachment set is invalid")]
    AttachmentSet,
    #[error("backup payload cannot be prepared for portable restore")]
    InvalidPayload,
    #[error("inactive portable generation staging failed")]
    Staging,
    #[error("backup cloud authentication failed")]
    CloudAuthentication,
    #[error("backup cloud request failed")]
    Transport,
    #[error("backup cloud generation changed before takeover")]
    CloudConflict,
    #[error("backup version changed before deletion")]
    DeletePrecondition,
    #[error("backup cloud rejected the request with status {0}")]
    ServerStatus(u16),
    #[error("backup cloud response is invalid")]
    InvalidResponse,
}

impl BackupDownloadError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidCatalog => "invalid_catalog",
            Self::BackupNotFound => "backup_not_found",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::SizeLimit => "size_limit",
            Self::TransportIntegrity => "transport_integrity",
            Self::Authentication => "authentication",
            Self::MetadataMismatch => "metadata_mismatch",
            Self::AttachmentSet => "attachment_set",
            Self::InvalidPayload => "invalid_payload",
            Self::Staging => "staging",
            Self::CloudAuthentication => "cloud_authentication",
            Self::Transport => "transport",
            Self::CloudConflict => "cloud_conflict",
            Self::DeletePrecondition => "delete_precondition",
            Self::ServerStatus(_) => "server_status",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

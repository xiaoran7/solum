//! `validate` -- the checks the open path runs before it trusts anything: header shape, metadata shape, and the inner document's format tag. Separate from `open` because these are the rules a second reader of the format has to reimplement, and they are the part worth reading together.
//!
//! Part of the `backup` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{BackupResult, DecodeLimits, DIGEST_LEN, NONCE_LEN, TAG_LEN};
use super::helpers::decode_nonce;
use super::*;

pub(super) fn validate_header(
    header: &BackupHeader,
    actual_ciphertext_size: usize,
    limits: DecodeLimits,
) -> BackupResult<[u8; NONCE_LEN]> {
    if header.format != FORMAT {
        return Err(BackupError::InvalidContainer);
    }
    if header.protocol_version != PROTOCOL_VERSION {
        return Err(BackupError::UnsupportedVersion(header.protocol_version));
    }
    if header.compression != COMPRESSION {
        return Err(BackupError::UnsupportedAlgorithm("compression"));
    }
    if header.encryption != ENCRYPTION {
        return Err(BackupError::UnsupportedAlgorithm("encryption"));
    }
    if header.payload_digest != PAYLOAD_DIGEST {
        return Err(BackupError::UnsupportedAlgorithm("payload digest"));
    }
    validate_metadata(&BackupMetadata {
        user_id: header.user_id.clone(),
        device_generation: header.device_generation,
        backup_id: header.backup_id.clone(),
        created_at: header.created_at.clone(),
    })?;
    if header.payload_format != export::FORMAT
        || !(export::MIN_RESTORABLE_VERSION..=export::MAX_PLANNABLE_VERSION)
            .contains(&header.payload_version)
    {
        return Err(BackupError::InvalidContainer);
    }
    if header.plaintext_size > limits.plaintext as u64
        || header.ciphertext_size > limits.ciphertext as u64
    {
        return Err(BackupError::SizeLimit);
    }
    if header.ciphertext_size > actual_ciphertext_size as u64 {
        return Err(BackupError::Truncated);
    }
    if header.ciphertext_size < actual_ciphertext_size as u64
        || header
            .compressed_size
            .checked_add(DIGEST_LEN as u64)
            .ok_or(BackupError::InvalidContainer)?
            .checked_add(TAG_LEN as u64)
            .ok_or(BackupError::InvalidContainer)?
            != header.ciphertext_size
    {
        return Err(BackupError::InvalidContainer);
    }
    decode_nonce(&header.nonce)
}

pub(super) fn validate_metadata(metadata: &BackupMetadata) -> BackupResult<()> {
    if !crate::account::is_valid_user_id(&metadata.user_id)
        || !crate::account::is_valid_user_id(&metadata.backup_id)
        || metadata.device_generation == 0
    {
        return Err(BackupError::InvalidContainer);
    }
    let parsed = DateTime::parse_from_rfc3339(&metadata.created_at)
        .map_err(|_| BackupError::InvalidContainer)?
        .with_timezone(&Utc);
    if parsed.to_rfc3339_opts(SecondsFormat::Millis, true) != metadata.created_at {
        return Err(BackupError::InvalidContainer);
    }
    Ok(())
}

pub(super) fn validate_inner_document(document: &Value) -> BackupResult<ImportPlan> {
    if document.get("format").and_then(Value::as_str) != Some(export::FORMAT) {
        return Err(BackupError::InvalidPayload);
    }
    export::plan_import(document).map_err(|_| BackupError::InvalidPayload)
}

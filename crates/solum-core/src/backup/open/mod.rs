//! `open` -- container in, verified document out: authenticate, bounded-decompress, parse, and import-plan. Deliberately takes no `Store`, so a failure here cannot partially mutate local data — the verified document is handed back to a later recovery transaction instead.
//!
//! Part of the `backup` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{
    BackupResult, DecodeLimits, DIGEST_LEN, MAGIC, PREAMBLE_LEN, PROTOCOL_LIMITS, TAG_LEN,
};
use super::helpers::{aad, normalized_json};
use super::validate::{validate_header, validate_inner_document};
use super::*;

/// Authenticate, bounded-decompress, parse, and import-plan a backup. No Store
/// is accepted by this API, so failure cannot partially mutate local data.
pub fn open_backup(container: &[u8], master_key: &[u8; 32]) -> BackupResult<VerifiedBackup> {
    open_backup_with_limits(container, master_key, PROTOCOL_LIMITS)
}

pub(super) fn open_backup_with_limits(
    container: &[u8],
    master_key: &[u8; 32],
    limits: DecodeLimits,
) -> BackupResult<VerifiedBackup> {
    if limits.ciphertext > MAX_CIPHERTEXT_BYTES || limits.plaintext > MAX_PLAINTEXT_BYTES {
        return Err(BackupError::InvalidContainer);
    }
    if container.len() > MAX_CONTAINER_BYTES {
        return Err(BackupError::SizeLimit);
    }
    if container.len() < PREAMBLE_LEN {
        return Err(BackupError::Truncated);
    }
    if &container[..MAGIC.len()] != MAGIC {
        return Err(BackupError::InvalidContainer);
    }
    let header_len = u32::from_be_bytes(
        container[8..12]
            .try_into()
            .map_err(|_| BackupError::Truncated)?,
    ) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(BackupError::InvalidContainer);
    }
    let header_end = PREAMBLE_LEN
        .checked_add(header_len)
        .ok_or(BackupError::InvalidContainer)?;
    if header_end > container.len() {
        return Err(BackupError::Truncated);
    }

    // Bound the actual ciphertext before parsing attacker-controlled JSON.
    let ciphertext = &container[header_end..];
    if ciphertext.len() > limits.ciphertext {
        return Err(BackupError::SizeLimit);
    }
    if ciphertext.len() < TAG_LEN {
        return Err(BackupError::Truncated);
    }

    let raw_header = &container[PREAMBLE_LEN..header_end];
    let header: BackupHeader =
        serde_json::from_slice(raw_header).map_err(|_| BackupError::InvalidContainer)?;
    let nonce = validate_header(&header, ciphertext.len(), limits)?;
    let authenticated = XChaCha20Poly1305::new(master_key.into())
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: ciphertext,
                aad: &aad(raw_header),
            },
        )
        .map_err(|_| BackupError::Authentication)?;
    if authenticated.len() as u64
        != header
            .compressed_size
            .checked_add(DIGEST_LEN as u64)
            .ok_or(BackupError::InvalidContainer)?
    {
        return Err(BackupError::InvalidContainer);
    }

    let (expected_digest, compressed) = authenticated.split_at(DIGEST_LEN);
    let mut decoder = ZlibDecoder::new(compressed);
    let mut normalized_payload = Vec::new();
    {
        let mut bounded = decoder.by_ref().take(limits.plaintext as u64 + 1);
        bounded
            .read_to_end(&mut normalized_payload)
            .map_err(|_| BackupError::Decompression)?;
    }
    if normalized_payload.len() > limits.plaintext {
        return Err(BackupError::SizeLimit);
    }
    if decoder.total_in() != compressed.len() as u64 {
        return Err(BackupError::Decompression);
    }
    if normalized_payload.len() as u64 != header.plaintext_size {
        return Err(BackupError::InvalidContainer);
    }
    if Sha256::digest(&normalized_payload).as_slice() != expected_digest {
        return Err(BackupError::InvalidPayload);
    }

    let document: Value =
        serde_json::from_slice(&normalized_payload).map_err(|_| BackupError::InvalidPayload)?;
    if normalized_json(&document)? != normalized_payload {
        return Err(BackupError::InvalidPayload);
    }
    let import_plan = validate_inner_document(&document)?;
    if header.payload_format != export::FORMAT || header.payload_version != import_plan.version {
        return Err(BackupError::InvalidPayload);
    }
    Ok(VerifiedBackup {
        header,
        document,
        normalized_payload,
        import_plan,
    })
}

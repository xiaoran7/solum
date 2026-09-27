//! `seal` -- plaintext in, container out: normalize the document, mint a fresh OS-random nonce per call, compress, encrypt under the associated data, and assemble the framed container. The nonce-generating entry points and the fixed-nonce one they delegate to are both here, so the one caller that needs a reproducible container (the tests) is the only reason the nonce is ever a parameter.
//!
//! Part of the `backup` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{BackupResult, DIGEST_LEN, MAGIC, NONCE_LEN, PREAMBLE_LEN, TAG_LEN};
use super::helpers::{aad, encode_hex, normalized_json, to_u64};
use super::validate::{validate_inner_document, validate_metadata};
use super::*;

/// Normalize and seal a recovery-plannable `solum-export` document.
/// A fresh OS-random nonce is generated for every call.
pub fn create_backup(
    document: &Value,
    metadata: &BackupMetadata,
    master_key: &[u8; 32],
) -> BackupResult<Vec<u8>> {
    let normalized = normalized_json(document)?;
    create_backup_from_normalized(&normalized, metadata, master_key)
}

/// Parse, normalize, validate, and seal JSON bytes. This accepts bytes from a
/// platform adapter without allowing whitespace/key order to fork the v1
/// fixture representation.
pub fn create_backup_from_json_bytes(
    json_bytes: &[u8],
    metadata: &BackupMetadata,
    master_key: &[u8; 32],
) -> BackupResult<Vec<u8>> {
    if json_bytes.len() > MAX_PLAINTEXT_BYTES {
        return Err(BackupError::SizeLimit);
    }
    let document: Value =
        serde_json::from_slice(json_bytes).map_err(|_| BackupError::InvalidPayload)?;
    create_backup(&document, metadata, master_key)
}

fn create_backup_from_normalized(
    normalized: &[u8],
    metadata: &BackupMetadata,
    master_key: &[u8; 32],
) -> BackupResult<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| BackupError::RandomSource)?;
    create_backup_with_nonce(normalized, metadata, master_key, nonce)
}

pub(super) fn create_backup_with_nonce(
    normalized: &[u8],
    metadata: &BackupMetadata,
    master_key: &[u8; 32],
    nonce: [u8; NONCE_LEN],
) -> BackupResult<Vec<u8>> {
    if normalized.len() > MAX_PLAINTEXT_BYTES {
        return Err(BackupError::SizeLimit);
    }
    let document: Value =
        serde_json::from_slice(normalized).map_err(|_| BackupError::InvalidPayload)?;
    if normalized_json(&document)? != normalized {
        return Err(BackupError::InvalidPayload);
    }
    validate_inner_document(&document)?;
    validate_metadata(metadata)?;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder
        .write_all(normalized)
        .map_err(|_| BackupError::Decompression)?;
    let compressed = encoder.finish().map_err(|_| BackupError::Decompression)?;
    if compressed
        .len()
        .checked_add(DIGEST_LEN)
        .ok_or(BackupError::SizeLimit)?
        .checked_add(TAG_LEN)
        .ok_or(BackupError::SizeLimit)?
        > MAX_CIPHERTEXT_BYTES
    {
        return Err(BackupError::SizeLimit);
    }

    let payload_version = document
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(BackupError::InvalidPayload)?;
    let header = BackupHeader {
        format: FORMAT.to_string(),
        protocol_version: PROTOCOL_VERSION,
        user_id: metadata.user_id.clone(),
        device_generation: metadata.device_generation,
        backup_id: metadata.backup_id.clone(),
        created_at: metadata.created_at.clone(),
        payload_format: export::FORMAT.to_string(),
        payload_version,
        payload_digest: PAYLOAD_DIGEST.to_string(),
        compression: COMPRESSION.to_string(),
        encryption: ENCRYPTION.to_string(),
        nonce: encode_hex(&nonce),
        plaintext_size: to_u64(normalized.len())?,
        compressed_size: to_u64(compressed.len())?,
        ciphertext_size: to_u64(compressed.len() + DIGEST_LEN + TAG_LEN)?,
    };
    let mut authenticated_body = Vec::with_capacity(DIGEST_LEN + compressed.len());
    authenticated_body.extend_from_slice(&Sha256::digest(normalized));
    authenticated_body.extend_from_slice(&compressed);
    assemble_container(&header, &authenticated_body, master_key, nonce)
}

pub(super) fn assemble_container(
    header: &BackupHeader,
    authenticated_body: &[u8],
    master_key: &[u8; 32],
    nonce: [u8; NONCE_LEN],
) -> BackupResult<Vec<u8>> {
    let header_bytes = serde_json::to_vec(header).map_err(|_| BackupError::InvalidContainer)?;
    if header_bytes.is_empty() || header_bytes.len() > MAX_HEADER_BYTES {
        return Err(BackupError::SizeLimit);
    }
    let aad = aad(&header_bytes);
    let ciphertext = XChaCha20Poly1305::new(master_key.into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: authenticated_body,
                aad: &aad,
            },
        )
        .map_err(|_| BackupError::InvalidContainer)?;
    if ciphertext.len() != header.ciphertext_size as usize {
        return Err(BackupError::InvalidContainer);
    }

    let mut container = Vec::with_capacity(PREAMBLE_LEN + header_bytes.len() + ciphertext.len());
    container.extend_from_slice(MAGIC);
    container.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    container.extend_from_slice(&header_bytes);
    container.extend_from_slice(&ciphertext);
    Ok(container)
}

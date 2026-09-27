//! `seal` -- plaintext in, container out. `validate_attachment_set` is the pre-flight: it checks the whole logical set (count, aggregate bytes, one user / device generation / backup across all of them) *before* anything is written, so a set that spans two accounts is refused rather than half-written.
//!
//! Part of the `attachment` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{AttachmentResult, ChunkHeader, MAGIC, NONCE_LEN, PROTOCOL_LIMITS, TAG_LEN};
use super::helpers::{
    aad, binding_bytes, derive_attachment_key, encode_hex, expected_chunk_count, to_u32,
    validate_binding,
};
use super::*;

/// Seal one attachment using the production 1 MiB chunk size and a fresh
/// OS-random nonce per chunk.
pub fn seal_attachment(
    plaintext: &[u8],
    binding: &AttachmentBinding,
    master_key: &[u8; 32],
) -> AttachmentResult<Vec<u8>> {
    seal_attachment_inner(plaintext, binding, master_key, PRODUCTION_CHUNK_BYTES, None)
}

/// Validate one logical set before a coordinator writes any output. This
/// mirrors the v3 manifest count/aggregate limits and prevents cross-account
/// or cross-backup sets.
pub fn validate_attachment_set(bindings: &[AttachmentBinding]) -> AttachmentResult<()> {
    if bindings.len() > MAX_ATTACHMENTS {
        return Err(AttachmentError::SizeLimit);
    }
    let mut ids = BTreeSet::new();
    let mut total = 0u64;
    let first = bindings.first();
    for binding in bindings {
        validate_binding(binding, PROTOCOL_LIMITS.plaintext)?;
        if let Some(first) = first {
            if binding.user_id != first.user_id
                || binding.device_generation != first.device_generation
                || binding.backup_id != first.backup_id
            {
                return Err(AttachmentError::InvalidContainer);
            }
        }
        if !ids.insert(binding.attachment_id.as_str()) {
            return Err(AttachmentError::InvalidContainer);
        }
        total = total
            .checked_add(binding.plaintext_size)
            .ok_or(AttachmentError::SizeLimit)?;
    }
    if total > MAX_TOTAL_ATTACHMENT_BYTES {
        return Err(AttachmentError::SizeLimit);
    }
    Ok(())
}

pub(super) fn seal_attachment_inner(
    plaintext: &[u8],
    binding: &AttachmentBinding,
    master_key: &[u8; 32],
    chunk_size: usize,
    fixed_nonces: Option<&[[u8; NONCE_LEN]]>,
) -> AttachmentResult<Vec<u8>> {
    validate_binding(binding, MAX_ATTACHMENT_BYTES)?;
    if plaintext.len() > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentError::SizeLimit);
    }
    if plaintext.len() as u64 != binding.plaintext_size
        || encode_hex(&Sha256::digest(plaintext)) != binding.plaintext_sha256
    {
        return Err(AttachmentError::DigestMismatch);
    }
    let chunk_count = expected_chunk_count(binding.plaintext_size, chunk_size, MAX_CHUNKS)?;
    if let Some(nonces) = fixed_nonces {
        if nonces.len() != chunk_count {
            return Err(AttachmentError::InvalidContainer);
        }
    }
    let header = AttachmentHeader {
        format: FORMAT.to_string(),
        protocol_version: PROTOCOL_VERSION,
        kdf: KDF.to_string(),
        encryption: ENCRYPTION.to_string(),
        chunk_plaintext_size: to_u32(chunk_size)?,
        chunk_count: to_u32(chunk_count)?,
    };
    let raw_header = serde_json::to_vec(&header).map_err(|_| AttachmentError::InvalidContainer)?;
    if raw_header.is_empty() || raw_header.len() > MAX_HEADER_BYTES {
        return Err(AttachmentError::SizeLimit);
    }
    let binding_bytes = binding_bytes(binding)?;
    let key = derive_attachment_key(master_key, &binding_bytes)?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut used_nonces = BTreeSet::new();

    let mut out = Vec::with_capacity(plaintext.len().saturating_add(4096));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&to_u32(raw_header.len())?.to_be_bytes());
    out.extend_from_slice(&raw_header);

    for index in 0..chunk_count {
        let offset = index
            .checked_mul(chunk_size)
            .ok_or(AttachmentError::SizeLimit)?;
        let end = offset
            .checked_add(chunk_size)
            .map(|value| value.min(plaintext.len()))
            .ok_or(AttachmentError::SizeLimit)?;
        let chunk = &plaintext[offset.min(plaintext.len())..end];
        let nonce = if let Some(nonces) = fixed_nonces {
            let nonce = nonces[index];
            if !used_nonces.insert(nonce) {
                return Err(AttachmentError::InvalidContainer);
            }
            nonce
        } else {
            let mut selected = None;
            for _ in 0..3 {
                let mut candidate = [0u8; NONCE_LEN];
                getrandom::getrandom(&mut candidate).map_err(|_| AttachmentError::RandomSource)?;
                if used_nonces.insert(candidate) {
                    selected = Some(candidate);
                    break;
                }
            }
            selected.ok_or(AttachmentError::RandomSource)?
        };
        let chunk_header = ChunkHeader {
            index: to_u32(index)?,
            plaintext_offset: offset as u64,
            plaintext_size: to_u32(chunk.len())?,
            ciphertext_size: to_u32(
                chunk
                    .len()
                    .checked_add(TAG_LEN)
                    .ok_or(AttachmentError::SizeLimit)?,
            )?,
            nonce: encode_hex(&nonce),
        };
        let raw_chunk_header =
            serde_json::to_vec(&chunk_header).map_err(|_| AttachmentError::InvalidContainer)?;
        if raw_chunk_header.is_empty() || raw_chunk_header.len() > MAX_CHUNK_HEADER_BYTES {
            return Err(AttachmentError::SizeLimit);
        }
        let aad = aad(&binding_bytes, &raw_header, &raw_chunk_header)?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: chunk,
                    aad: &aad,
                },
            )
            .map_err(|_| AttachmentError::InvalidContainer)?;
        out.extend_from_slice(&to_u32(raw_chunk_header.len())?.to_be_bytes());
        out.extend_from_slice(&to_u32(ciphertext.len())?.to_be_bytes());
        out.extend_from_slice(&raw_chunk_header);
        out.extend_from_slice(&ciphertext);
    }
    if out.len() > MAX_CONTAINER_BYTES {
        return Err(AttachmentError::SizeLimit);
    }
    Ok(out)
}

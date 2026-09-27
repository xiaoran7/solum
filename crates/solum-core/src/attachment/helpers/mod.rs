//! `helpers` -- the pieces both directions share: key derivation, AAD construction, length-prefixed framing, and the bounds arithmetic that decides how many chunks a container should have. Kept apart from `seal` and `open` so neither owns the other's rules -- a chunk count that only the writer computed would be the writer's opinion, not the format's.
//!
//! Part of the `attachment` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{
    AttachmentResult, DecodeLimits, AAD_DOMAIN, KDF_INFO_DOMAIN, KDF_SALT, NONCE_LEN,
};
use super::*;

pub(super) fn validate_binding(
    binding: &AttachmentBinding,
    plaintext_limit: usize,
) -> AttachmentResult<()> {
    if !crate::account::is_valid_user_id(&binding.user_id)
        || !crate::account::is_valid_user_id(&binding.backup_id)
        || binding.device_generation == 0
        || binding.attachment_id.is_empty()
        || binding.attachment_id.len() > 128
        || !binding
            .attachment_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._:".contains(&byte))
        || !is_lower_hex(&binding.plaintext_sha256, 64)
    {
        return Err(AttachmentError::InvalidContainer);
    }
    if binding.plaintext_size > plaintext_limit as u64 {
        return Err(AttachmentError::SizeLimit);
    }
    Ok(())
}

pub(super) fn validate_header(
    header: &AttachmentHeader,
    binding: &AttachmentBinding,
    limits: DecodeLimits,
) -> AttachmentResult<()> {
    if header.format != FORMAT {
        return Err(AttachmentError::InvalidContainer);
    }
    if header.protocol_version != PROTOCOL_VERSION {
        return Err(AttachmentError::UnsupportedVersion(header.protocol_version));
    }
    if header.kdf != KDF {
        return Err(AttachmentError::UnsupportedAlgorithm("key derivation"));
    }
    if header.encryption != ENCRYPTION {
        return Err(AttachmentError::UnsupportedAlgorithm("encryption"));
    }
    let chunk_size = header.chunk_plaintext_size as usize;
    let expected = expected_chunk_count(binding.plaintext_size, chunk_size, limits.chunks)?;
    if header.chunk_count as usize != expected {
        return Err(AttachmentError::InvalidContainer);
    }
    Ok(())
}

pub(super) fn binding_bytes(binding: &AttachmentBinding) -> AttachmentResult<Vec<u8>> {
    serde_json::to_vec(binding).map_err(|_| AttachmentError::InvalidContainer)
}

pub(super) fn derive_attachment_key(
    master_key: &[u8; 32],
    binding_bytes: &[u8],
) -> AttachmentResult<[u8; 32]> {
    let mut info = Vec::with_capacity(KDF_INFO_DOMAIN.len() + binding_bytes.len());
    info.extend_from_slice(KDF_INFO_DOMAIN);
    info.extend_from_slice(binding_bytes);
    let hkdf = Hkdf::<Sha256>::new(Some(KDF_SALT), master_key);
    let mut key = [0u8; 32];
    hkdf.expand(&info, &mut key)
        .map_err(|_| AttachmentError::InvalidContainer)?;
    Ok(key)
}

pub(super) fn aad(
    binding_bytes: &[u8],
    raw_header: &[u8],
    raw_chunk_header: &[u8],
) -> AttachmentResult<Vec<u8>> {
    let mut out = Vec::with_capacity(
        AAD_DOMAIN.len() + 12 + binding_bytes.len() + raw_header.len() + raw_chunk_header.len(),
    );
    out.extend_from_slice(AAD_DOMAIN);
    append_len_prefixed(&mut out, binding_bytes)?;
    append_len_prefixed(&mut out, raw_header)?;
    append_len_prefixed(&mut out, raw_chunk_header)?;
    Ok(out)
}

pub(super) fn append_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) -> AttachmentResult<()> {
    out.extend_from_slice(&to_u32(bytes.len())?.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

pub(super) fn expected_chunk_count(
    plaintext_size: u64,
    chunk_size: usize,
    chunk_limit: usize,
) -> AttachmentResult<usize> {
    if chunk_size == 0 || chunk_size > PRODUCTION_CHUNK_BYTES {
        return Err(AttachmentError::InvalidContainer);
    }
    let count = if plaintext_size == 0 {
        1
    } else {
        let chunk_size = chunk_size as u64;
        plaintext_size
            .checked_add(chunk_size - 1)
            .ok_or(AttachmentError::SizeLimit)?
            / chunk_size
    };
    let count = usize::try_from(count).map_err(|_| AttachmentError::SizeLimit)?;
    if count == 0 || count > chunk_limit {
        return Err(AttachmentError::SizeLimit);
    }
    Ok(count)
}

pub(super) fn expected_plaintext_size(
    total: u64,
    chunk_size: usize,
    index: usize,
) -> AttachmentResult<usize> {
    let offset = index
        .checked_mul(chunk_size)
        .ok_or(AttachmentError::InvalidContainer)? as u64;
    let remaining = total
        .checked_sub(offset)
        .ok_or(AttachmentError::InvalidContainer)?;
    usize::try_from(remaining.min(chunk_size as u64)).map_err(|_| AttachmentError::SizeLimit)
}

pub(super) fn read_u32(bytes: &[u8]) -> AttachmentResult<u32> {
    Ok(u32::from_be_bytes(
        bytes.try_into().map_err(|_| AttachmentError::Truncated)?,
    ))
}

pub(super) fn to_u32(value: usize) -> AttachmentResult<u32> {
    u32::try_from(value).map_err(|_| AttachmentError::SizeLimit)
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(super) fn decode_nonce(encoded: &str) -> AttachmentResult<[u8; NONCE_LEN]> {
    if !is_lower_hex(encoded, NONCE_LEN * 2) {
        return Err(AttachmentError::InvalidContainer);
    }
    let mut nonce = [0u8; NONCE_LEN];
    for (index, output) in nonce.iter_mut().enumerate() {
        *output = u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16)
            .map_err(|_| AttachmentError::InvalidContainer)?;
    }
    Ok(nonce)
}

pub(super) fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

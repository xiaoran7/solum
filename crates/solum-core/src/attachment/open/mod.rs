//! `open` -- the other direction, and deliberately the strict one: every chunk is authenticated and the manifest digest checked before a single byte of plaintext reaches the caller. Nothing is decrypted-then-verified, because a byte returned before its tag is checked is a byte an attacker already chose.
//!
//! Part of the `attachment` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{
    AttachmentResult, ChunkHeader, DecodeLimits, ParsedChunk, MAGIC, PREAMBLE_LEN, PROTOCOL_LIMITS,
    RECORD_PREAMBLE_LEN, TAG_LEN,
};
use super::helpers::{
    aad, binding_bytes, decode_nonce, derive_attachment_key, encode_hex, expected_plaintext_size,
    read_u32, validate_binding, validate_header,
};
use super::*;

/// Authenticate every chunk and the final manifest digest before returning
/// any plaintext to the caller.
pub fn open_attachment(
    container: &[u8],
    binding: &AttachmentBinding,
    master_key: &[u8; 32],
) -> AttachmentResult<Vec<u8>> {
    open_attachment_with_limits(container, binding, master_key, PROTOCOL_LIMITS)
}

pub(super) fn open_attachment_with_limits(
    container: &[u8],
    binding: &AttachmentBinding,
    master_key: &[u8; 32],
    limits: DecodeLimits,
) -> AttachmentResult<Vec<u8>> {
    if limits.container > MAX_CONTAINER_BYTES
        || limits.plaintext > MAX_ATTACHMENT_BYTES
        || limits.chunks > MAX_CHUNKS
    {
        return Err(AttachmentError::InvalidContainer);
    }
    if container.len() > limits.container {
        return Err(AttachmentError::SizeLimit);
    }
    validate_binding(binding, limits.plaintext)?;
    if container.len() < PREAMBLE_LEN {
        return Err(AttachmentError::Truncated);
    }
    if &container[..MAGIC.len()] != MAGIC {
        return Err(AttachmentError::InvalidContainer);
    }
    let header_len = read_u32(&container[8..12])? as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(AttachmentError::InvalidContainer);
    }
    let header_end = PREAMBLE_LEN
        .checked_add(header_len)
        .ok_or(AttachmentError::InvalidContainer)?;
    if header_end > container.len() {
        return Err(AttachmentError::Truncated);
    }
    let raw_header = &container[PREAMBLE_LEN..header_end];
    let header: AttachmentHeader =
        serde_json::from_slice(raw_header).map_err(|_| AttachmentError::InvalidContainer)?;
    validate_header(&header, binding, limits)?;

    // Parse and validate every record before decrypting any attacker-controlled
    // bytes. Decrypted chunks remain private until final length/digest checks.
    let mut parsed = Vec::with_capacity(header.chunk_count as usize);
    let mut nonces = BTreeSet::new();
    let mut cursor = header_end;
    let mut expected_offset = 0u64;
    for expected_index in 0..header.chunk_count as usize {
        let record_prefix_end = cursor
            .checked_add(RECORD_PREAMBLE_LEN)
            .ok_or(AttachmentError::InvalidContainer)?;
        if record_prefix_end > container.len() {
            return Err(AttachmentError::Truncated);
        }
        let chunk_header_len = read_u32(&container[cursor..cursor + 4])? as usize;
        let ciphertext_len = read_u32(&container[cursor + 4..record_prefix_end])? as usize;
        if chunk_header_len == 0 || chunk_header_len > MAX_CHUNK_HEADER_BYTES {
            return Err(AttachmentError::InvalidContainer);
        }
        if ciphertext_len < TAG_LEN
            || ciphertext_len > header.chunk_plaintext_size as usize + TAG_LEN
        {
            return Err(AttachmentError::InvalidContainer);
        }
        let chunk_header_end = record_prefix_end
            .checked_add(chunk_header_len)
            .ok_or(AttachmentError::InvalidContainer)?;
        let ciphertext_end = chunk_header_end
            .checked_add(ciphertext_len)
            .ok_or(AttachmentError::InvalidContainer)?;
        if ciphertext_end > container.len() {
            return Err(AttachmentError::Truncated);
        }
        let raw_chunk_header = &container[record_prefix_end..chunk_header_end];
        let chunk_header: ChunkHeader = serde_json::from_slice(raw_chunk_header)
            .map_err(|_| AttachmentError::InvalidContainer)?;
        let expected_plaintext_size = expected_plaintext_size(
            binding.plaintext_size,
            header.chunk_plaintext_size as usize,
            expected_index,
        )?;
        if chunk_header.index as usize != expected_index
            || chunk_header.plaintext_offset != expected_offset
            || chunk_header.plaintext_size as usize != expected_plaintext_size
            || chunk_header.ciphertext_size as usize != ciphertext_len
            || ciphertext_len != expected_plaintext_size + TAG_LEN
        {
            return Err(AttachmentError::InvalidContainer);
        }
        let nonce = decode_nonce(&chunk_header.nonce)?;
        if !nonces.insert(nonce) {
            return Err(AttachmentError::InvalidContainer);
        }
        parsed.push(ParsedChunk {
            raw_header: raw_chunk_header,
            ciphertext: &container[chunk_header_end..ciphertext_end],
            nonce,
            plaintext_size: expected_plaintext_size,
        });
        expected_offset = expected_offset
            .checked_add(expected_plaintext_size as u64)
            .ok_or(AttachmentError::InvalidContainer)?;
        cursor = ciphertext_end;
    }
    if cursor != container.len() || expected_offset != binding.plaintext_size {
        return Err(AttachmentError::InvalidContainer);
    }

    let binding_bytes = binding_bytes(binding)?;
    let key = derive_attachment_key(master_key, &binding_bytes)?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let capacity =
        usize::try_from(binding.plaintext_size).map_err(|_| AttachmentError::SizeLimit)?;
    let mut plaintext = Vec::with_capacity(capacity);
    for chunk in parsed {
        let aad = aad(&binding_bytes, raw_header, chunk.raw_header)?;
        let opened = cipher
            .decrypt(
                XNonce::from_slice(&chunk.nonce),
                Payload {
                    msg: chunk.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| AttachmentError::Authentication)?;
        if opened.len() != chunk.plaintext_size {
            return Err(AttachmentError::InvalidContainer);
        }
        plaintext.extend_from_slice(&opened);
    }
    if plaintext.len() as u64 != binding.plaintext_size {
        return Err(AttachmentError::InvalidContainer);
    }
    if encode_hex(&Sha256::digest(&plaintext)) != binding.plaintext_sha256 {
        return Err(AttachmentError::DigestMismatch);
    }
    Ok(plaintext)
}

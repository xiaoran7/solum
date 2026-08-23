//! Platform-neutral chunked `solum-attachment` v1 codec.
//!
//! Attachment metadata comes from an already authenticated `solum-export` v3
//! manifest. The server stores only this opaque container; account, backup,
//! attachment, and digest identities never appear in its clear header. Exact
//! byte length remains observable from chunk framing and ciphertext length.

use std::collections::BTreeSet;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::backup::BackupMetadata;
use crate::export_v3::DocumentManifest;

const MAGIC: &[u8; 8] = b"SOLUMATT";
const PREAMBLE_LEN: usize = 12;
const RECORD_PREAMBLE_LEN: usize = 8;
const TAG_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const AAD_DOMAIN: &[u8] = b"solum-attachment:aad:v1\0";
const KDF_SALT: &[u8] = b"solum-attachment:hkdf-salt:v1";
const KDF_INFO_DOMAIN: &[u8] = b"solum-attachment:key:v1\0";

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

type AttachmentResult<T> = std::result::Result<T, AttachmentError>;

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
struct ChunkHeader {
    index: u32,
    plaintext_offset: u64,
    plaintext_size: u32,
    ciphertext_size: u32,
    nonce: String,
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
struct DecodeLimits {
    container: usize,
    plaintext: usize,
    chunks: usize,
}

const PROTOCOL_LIMITS: DecodeLimits = DecodeLimits {
    container: MAX_CONTAINER_BYTES,
    plaintext: MAX_ATTACHMENT_BYTES,
    chunks: MAX_CHUNKS,
};

struct ParsedChunk<'a> {
    raw_header: &'a [u8],
    ciphertext: &'a [u8],
    nonce: [u8; NONCE_LEN],
    plaintext_size: usize,
}

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

fn seal_attachment_inner(
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

/// Authenticate every chunk and the final manifest digest before returning
/// any plaintext to the caller.
pub fn open_attachment(
    container: &[u8],
    binding: &AttachmentBinding,
    master_key: &[u8; 32],
) -> AttachmentResult<Vec<u8>> {
    open_attachment_with_limits(container, binding, master_key, PROTOCOL_LIMITS)
}

fn open_attachment_with_limits(
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

fn validate_binding(binding: &AttachmentBinding, plaintext_limit: usize) -> AttachmentResult<()> {
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

fn validate_header(
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

fn binding_bytes(binding: &AttachmentBinding) -> AttachmentResult<Vec<u8>> {
    serde_json::to_vec(binding).map_err(|_| AttachmentError::InvalidContainer)
}

fn derive_attachment_key(
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

fn aad(
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

fn append_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) -> AttachmentResult<()> {
    out.extend_from_slice(&to_u32(bytes.len())?.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn expected_chunk_count(
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

fn expected_plaintext_size(total: u64, chunk_size: usize, index: usize) -> AttachmentResult<usize> {
    let offset = index
        .checked_mul(chunk_size)
        .ok_or(AttachmentError::InvalidContainer)? as u64;
    let remaining = total
        .checked_sub(offset)
        .ok_or(AttachmentError::InvalidContainer)?;
    usize::try_from(remaining.min(chunk_size as u64)).map_err(|_| AttachmentError::SizeLimit)
}

fn read_u32(bytes: &[u8]) -> AttachmentResult<u32> {
    Ok(u32::from_be_bytes(
        bytes.try_into().map_err(|_| AttachmentError::Truncated)?,
    ))
}

fn to_u32(value: usize) -> AttachmentResult<u32> {
    u32::try_from(value).map_err(|_| AttachmentError::SizeLimit)
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_nonce(encoded: &str) -> AttachmentResult<[u8; NONCE_LEN]> {
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

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_KEY: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    const FIXTURE_PLAINTEXT: &[u8] = b"Synthetic attachment fixture bytes only.";
    const FIXTURE_NONCES: [[u8; NONCE_LEN]; 3] = [
        [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
        ],
        [
            0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25,
            0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
        ],
        [
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d,
            0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
        ],
    ];

    fn binding_for(plaintext: &[u8]) -> AttachmentBinding {
        AttachmentBinding {
            user_id: "11111111-1111-4111-8111-111111111111".into(),
            device_generation: 7,
            backup_id: "22222222-2222-4222-8222-222222222222".into(),
            attachment_id: "document-public-fixture".into(),
            plaintext_size: plaintext.len() as u64,
            plaintext_sha256: encode_hex(&Sha256::digest(plaintext)),
        }
    }

    fn fixture_container() -> Vec<u8> {
        seal_attachment_inner(
            FIXTURE_PLAINTEXT,
            &binding_for(FIXTURE_PLAINTEXT),
            &FIXTURE_KEY,
            16,
            Some(&FIXTURE_NONCES),
        )
        .unwrap()
    }

    fn split_records(container: &[u8]) -> (AttachmentHeader, Vec<(usize, usize)>) {
        let header_len = read_u32(&container[8..12]).unwrap() as usize;
        let header_end = PREAMBLE_LEN + header_len;
        let header = serde_json::from_slice(&container[PREAMBLE_LEN..header_end]).unwrap();
        let mut records = Vec::new();
        let mut cursor = header_end;
        while cursor < container.len() {
            let chunk_header_len = read_u32(&container[cursor..cursor + 4]).unwrap() as usize;
            let ciphertext_len = read_u32(&container[cursor + 4..cursor + 8]).unwrap() as usize;
            let end = cursor + RECORD_PREAMBLE_LEN + chunk_header_len + ciphertext_len;
            records.push((cursor, end));
            cursor = end;
        }
        (header, records)
    }

    fn rewrite_header(container: &[u8], header: &AttachmentHeader) -> Vec<u8> {
        let old_header_len = read_u32(&container[8..12]).unwrap() as usize;
        let raw = serde_json::to_vec(header).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        out.extend_from_slice(&raw);
        out.extend_from_slice(&container[PREAMBLE_LEN + old_header_len..]);
        out
    }

    fn decode_fixture_hex(raw: &str) -> Vec<u8> {
        let compact: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert_eq!(compact.len() % 2, 0);
        (0..compact.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&compact[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn chunked_codec_round_trip_and_zero_byte_attachment() {
        let plaintext = vec![0x5a; PRODUCTION_CHUNK_BYTES + 23];
        let binding = binding_for(&plaintext);
        let container = seal_attachment(&plaintext, &binding, &FIXTURE_KEY).unwrap();
        assert_eq!(
            open_attachment(&container, &binding, &FIXTURE_KEY).unwrap(),
            plaintext
        );
        assert_eq!(split_records(&container).0.chunk_count, 2);

        let empty_binding = binding_for(&[]);
        let empty = seal_attachment(&[], &empty_binding, &FIXTURE_KEY).unwrap();
        assert_eq!(split_records(&empty).0.chunk_count, 1);
        assert!(open_attachment(&empty, &empty_binding, &FIXTURE_KEY)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn wrong_key_ciphertext_tag_and_binding_tampering_fail_authentication() {
        let container = fixture_container();
        let binding = binding_for(FIXTURE_PLAINTEXT);
        let mut wrong_key = FIXTURE_KEY;
        wrong_key[0] ^= 1;
        assert_eq!(
            open_attachment(&container, &binding, &wrong_key).unwrap_err(),
            AttachmentError::Authentication
        );

        let (_, records) = split_records(&container);
        let mut ciphertext = container.clone();
        *ciphertext.last_mut().unwrap() ^= 1;
        assert_eq!(
            open_attachment(&ciphertext, &binding, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::Authentication
        );
        let mut tag = container.clone();
        tag[records[0].1 - 1] ^= 1;
        assert_eq!(
            open_attachment(&tag, &binding, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::Authentication
        );

        let mut other_account = binding.clone();
        other_account.user_id = "33333333-3333-4333-8333-333333333333".into();
        assert_eq!(
            open_attachment(&container, &other_account, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::Authentication
        );
        let mut other_digest = binding;
        other_digest.plaintext_sha256 = "00".repeat(32);
        assert_eq!(
            open_attachment(&container, &other_digest, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::Authentication
        );

        let mut chunk_aad = container;
        let first_header_start = records[0].0 + RECORD_PREAMBLE_LEN;
        let first_header_len =
            read_u32(&chunk_aad[records[0].0..records[0].0 + 4]).unwrap() as usize;
        let mut first_header: ChunkHeader = serde_json::from_slice(
            &chunk_aad[first_header_start..first_header_start + first_header_len],
        )
        .unwrap();
        first_header.nonce = "48".repeat(NONCE_LEN);
        let raw_first = serde_json::to_vec(&first_header).unwrap();
        assert_eq!(raw_first.len(), first_header_len);
        chunk_aad[first_header_start..first_header_start + first_header_len]
            .copy_from_slice(&raw_first);
        assert_eq!(
            open_attachment(&chunk_aad, &binding_for(FIXTURE_PLAINTEXT), &FIXTURE_KEY).unwrap_err(),
            AttachmentError::Authentication
        );
    }

    #[test]
    fn reordered_duplicate_and_truncated_chunks_are_rejected() {
        let container = fixture_container();
        let binding = binding_for(FIXTURE_PLAINTEXT);
        let (_, records) = split_records(&container);
        let header_len = read_u32(&container[8..12]).unwrap() as usize;
        let header_end = PREAMBLE_LEN + header_len;

        let mut reordered = container[..header_end].to_vec();
        reordered.extend_from_slice(&container[records[1].0..records[1].1]);
        reordered.extend_from_slice(&container[records[0].0..records[0].1]);
        reordered.extend_from_slice(&container[records[2].0..records[2].1]);
        assert_eq!(
            open_attachment(&reordered, &binding, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::InvalidContainer
        );

        let mut duplicate_nonce = container.clone();
        let second_header_start = records[1].0 + RECORD_PREAMBLE_LEN;
        let second_header_len =
            read_u32(&container[records[1].0..records[1].0 + 4]).unwrap() as usize;
        let first_header_start = records[0].0 + RECORD_PREAMBLE_LEN;
        let first_header_len =
            read_u32(&container[records[0].0..records[0].0 + 4]).unwrap() as usize;
        let first_header: ChunkHeader = serde_json::from_slice(
            &container[first_header_start..first_header_start + first_header_len],
        )
        .unwrap();
        let mut second_header: ChunkHeader = serde_json::from_slice(
            &container[second_header_start..second_header_start + second_header_len],
        )
        .unwrap();
        second_header.nonce = first_header.nonce;
        let raw_second = serde_json::to_vec(&second_header).unwrap();
        assert_eq!(raw_second.len(), second_header_len);
        duplicate_nonce[second_header_start..second_header_start + second_header_len]
            .copy_from_slice(&raw_second);
        assert_eq!(
            open_attachment(&duplicate_nonce, &binding, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::InvalidContainer
        );

        for truncated in [
            &container[..0],
            &container[..11],
            &container[..container.len() - 1],
        ] {
            assert_eq!(
                open_attachment(truncated, &binding, &FIXTURE_KEY).unwrap_err(),
                AttachmentError::Truncated
            );
        }
    }

    #[test]
    fn future_versions_unknown_algorithms_and_trailing_bytes_are_rejected() {
        let container = fixture_container();
        let binding = binding_for(FIXTURE_PLAINTEXT);
        let (mut header, _) = split_records(&container);
        header.protocol_version = 2;
        assert_eq!(
            open_attachment(&rewrite_header(&container, &header), &binding, &FIXTURE_KEY)
                .unwrap_err(),
            AttachmentError::UnsupportedVersion(2)
        );
        let (mut header, _) = split_records(&container);
        header.kdf = "unknown-kdf".into();
        assert_eq!(
            open_attachment(&rewrite_header(&container, &header), &binding, &FIXTURE_KEY)
                .unwrap_err(),
            AttachmentError::UnsupportedAlgorithm("key derivation")
        );
        let (mut header, _) = split_records(&container);
        header.encryption = "unknown-aead".into();
        assert_eq!(
            open_attachment(&rewrite_header(&container, &header), &binding, &FIXTURE_KEY)
                .unwrap_err(),
            AttachmentError::UnsupportedAlgorithm("encryption")
        );
        let mut trailing = container;
        trailing.push(0);
        assert_eq!(
            open_attachment(&trailing, &binding, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::InvalidContainer
        );
    }

    #[test]
    fn ciphertext_plaintext_chunk_and_manifest_set_limits_are_enforced() {
        let container = fixture_container();
        let binding = binding_for(FIXTURE_PLAINTEXT);
        let mut wrong_digest = binding.clone();
        wrong_digest.plaintext_sha256 = "00".repeat(32);
        assert_eq!(
            seal_attachment(FIXTURE_PLAINTEXT, &wrong_digest, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::DigestMismatch
        );
        assert_eq!(
            open_attachment_with_limits(
                &container,
                &binding,
                &FIXTURE_KEY,
                DecodeLimits {
                    container: container.len() - 1,
                    plaintext: MAX_ATTACHMENT_BYTES,
                    chunks: MAX_CHUNKS,
                },
            )
            .unwrap_err(),
            AttachmentError::SizeLimit
        );
        assert_eq!(
            open_attachment_with_limits(
                &container,
                &binding,
                &FIXTURE_KEY,
                DecodeLimits {
                    container: MAX_CONTAINER_BYTES,
                    plaintext: FIXTURE_PLAINTEXT.len() - 1,
                    chunks: MAX_CHUNKS,
                },
            )
            .unwrap_err(),
            AttachmentError::SizeLimit
        );
        assert_eq!(
            open_attachment_with_limits(
                &container,
                &binding,
                &FIXTURE_KEY,
                DecodeLimits {
                    container: MAX_CONTAINER_BYTES,
                    plaintext: MAX_ATTACHMENT_BYTES,
                    chunks: 2,
                },
            )
            .unwrap_err(),
            AttachmentError::SizeLimit
        );

        let mut oversized = binding.clone();
        oversized.plaintext_size = MAX_ATTACHMENT_BYTES as u64 + 1;
        assert_eq!(
            seal_attachment(FIXTURE_PLAINTEXT, &oversized, &FIXTURE_KEY).unwrap_err(),
            AttachmentError::SizeLimit
        );

        let mut set = vec![binding.clone(); MAX_ATTACHMENTS + 1];
        for (index, item) in set.iter_mut().enumerate() {
            item.attachment_id = format!("document-{index}");
        }
        assert_eq!(
            validate_attachment_set(&set).unwrap_err(),
            AttachmentError::SizeLimit
        );
        assert!(validate_attachment_set(&[binding]).is_ok());
    }

    #[test]
    fn production_calls_use_distinct_chunk_nonces_and_ciphertexts() {
        let binding = binding_for(FIXTURE_PLAINTEXT);
        let first = seal_attachment(FIXTURE_PLAINTEXT, &binding, &FIXTURE_KEY).unwrap();
        let second = seal_attachment(FIXTURE_PLAINTEXT, &binding, &FIXTURE_KEY).unwrap();
        assert_ne!(first, second);
        let (_, first_records) = split_records(&first);
        let (_, second_records) = split_records(&second);
        let nonce = |container: &[u8], record: (usize, usize)| {
            let start = record.0 + RECORD_PREAMBLE_LEN;
            let len = read_u32(&container[record.0..record.0 + 4]).unwrap() as usize;
            serde_json::from_slice::<ChunkHeader>(&container[start..start + len])
                .unwrap()
                .nonce
        };
        assert_ne!(
            nonce(&first, first_records[0]),
            nonce(&second, second_records[0])
        );
    }

    #[test]
    fn manifest_constructor_and_errors_do_not_leak_sensitive_values() {
        let metadata = BackupMetadata {
            user_id: "11111111-1111-4111-8111-111111111111".into(),
            device_generation: 7,
            backup_id: "22222222-2222-4222-8222-222222222222".into(),
            created_at: "2026-01-02T03:04:05.000Z".into(),
        };
        let manifest = DocumentManifest {
            id: "document-public-fixture".into(),
            name: "synthetic.txt".into(),
            media_type: "text/plain".into(),
            plaintext_size: FIXTURE_PLAINTEXT.len() as u64,
            sha256: encode_hex(&Sha256::digest(FIXTURE_PLAINTEXT)),
        };
        let binding = AttachmentBinding::from_manifest(&metadata, &manifest);
        assert_eq!(binding, binding_for(FIXTURE_PLAINTEXT));
        let container = seal_attachment(FIXTURE_PLAINTEXT, &binding, &FIXTURE_KEY).unwrap();
        let clear_container = String::from_utf8_lossy(&container);
        assert!(!clear_container.contains(&binding.user_id));
        assert!(!clear_container.contains(&binding.backup_id));
        assert!(!clear_container.contains(&binding.attachment_id));
        assert!(!clear_container.contains(&binding.plaintext_sha256));
        let mut wrong_key = FIXTURE_KEY;
        wrong_key[0] ^= 1;
        let message = open_attachment(&container, &binding, &wrong_key)
            .unwrap_err()
            .to_string();
        assert!(!message.contains(&encode_hex(&FIXTURE_KEY)));
        assert!(!message.contains("Synthetic attachment"));
        assert!(!message.contains(&binding.plaintext_sha256));
        assert!(!message.contains(&binding.user_id));
    }

    #[test]
    fn fixed_cross_platform_fixture_is_byte_stable_and_readable() {
        let actual = fixture_container();
        let expected = decode_fixture_hex(include_str!(
            "../tests/fixtures/attachment-v1/container.hex"
        ));
        assert_eq!(
            actual,
            expected,
            "actual fixture hex: {}",
            encode_hex(&actual)
        );
        assert_eq!(
            open_attachment(&expected, &binding_for(FIXTURE_PLAINTEXT), &FIXTURE_KEY).unwrap(),
            FIXTURE_PLAINTEXT
        );
    }
}

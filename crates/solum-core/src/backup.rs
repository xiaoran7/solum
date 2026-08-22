//! Platform-neutral `solum-backup` v1 container codec.
//!
//! This module deliberately stops before persistence: it authenticates,
//! decompresses, parses, and import-plans an inner `solum-export` v2 document,
//! then returns that verified document to a later recovery transaction. The
//! server only needs to store the returned container as opaque bytes.

use std::io::{Read, Write};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, SecondsFormat, Utc};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::export::{self, ImportPlan};

const MAGIC: &[u8; 8] = b"SOLUMBAK";
const PREAMBLE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const DIGEST_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const AAD_DOMAIN: &[u8] = b"solum-backup:aad:v1\0";

pub const FORMAT: &str = "solum-backup";
pub const PROTOCOL_VERSION: u32 = 1;
pub const COMPRESSION: &str = "zlib-deflate";
pub const ENCRYPTION: &str = "xchacha20-poly1305";
pub const PAYLOAD_DIGEST: &str = "sha256";
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_CIPHERTEXT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PLAINTEXT_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_CONTAINER_BYTES: usize = PREAMBLE_LEN + MAX_HEADER_BYTES + MAX_CIPHERTEXT_BYTES;

type BackupResult<T> = std::result::Result<T, BackupError>;

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
    #[error("backup payload is not a valid restorable solum-export v2 document")]
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
struct DecodeLimits {
    ciphertext: usize,
    plaintext: usize,
}

const PROTOCOL_LIMITS: DecodeLimits = DecodeLimits {
    ciphertext: MAX_CIPHERTEXT_BYTES,
    plaintext: MAX_PLAINTEXT_BYTES,
};

/// Normalize and seal a `serde_json::Value` containing `solum-export` v2.
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

fn create_backup_with_nonce(
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

    let header = BackupHeader {
        format: FORMAT.to_string(),
        protocol_version: PROTOCOL_VERSION,
        user_id: metadata.user_id.clone(),
        device_generation: metadata.device_generation,
        backup_id: metadata.backup_id.clone(),
        created_at: metadata.created_at.clone(),
        payload_format: export::FORMAT.to_string(),
        payload_version: export::EXPORT_VERSION,
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

fn assemble_container(
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

/// Authenticate, bounded-decompress, parse, and import-plan a backup. No Store
/// is accepted by this API, so failure cannot partially mutate local data.
pub fn open_backup(container: &[u8], master_key: &[u8; 32]) -> BackupResult<VerifiedBackup> {
    open_backup_with_limits(container, master_key, PROTOCOL_LIMITS)
}

fn open_backup_with_limits(
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

fn validate_header(
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
    if header.payload_format != export::FORMAT || header.payload_version != export::EXPORT_VERSION {
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

fn validate_metadata(metadata: &BackupMetadata) -> BackupResult<()> {
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

fn validate_inner_document(document: &Value) -> BackupResult<ImportPlan> {
    if document.get("format").and_then(Value::as_str) != Some(export::FORMAT)
        || document.get("version").and_then(Value::as_u64) != Some(export::EXPORT_VERSION as u64)
    {
        return Err(BackupError::InvalidPayload);
    }
    export::plan_import(document).map_err(|_| BackupError::InvalidPayload)
}

fn normalized_json(value: &Value) -> BackupResult<Vec<u8>> {
    fn write_value(value: &Value, out: &mut Vec<u8>) -> BackupResult<()> {
        match value {
            Value::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    write_value(item, out)?;
                }
                out.push(b']');
            }
            Value::Object(object) => {
                out.push(b'{');
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)
                        .map_err(|_| BackupError::InvalidPayload)?;
                    out.push(b':');
                    write_value(&object[key], out)?;
                }
                out.push(b'}');
            }
            scalar => {
                serde_json::to_writer(out, scalar).map_err(|_| BackupError::InvalidPayload)?
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    write_value(value, &mut out)?;
    Ok(out)
}

fn aad(raw_header: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(AAD_DOMAIN.len() + raw_header.len());
    out.extend_from_slice(AAD_DOMAIN);
    out.extend_from_slice(raw_header);
    out
}

fn to_u64(value: usize) -> BackupResult<u64> {
    u64::try_from(value).map_err(|_| BackupError::SizeLimit)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_nonce(encoded: &str) -> BackupResult<[u8; NONCE_LEN]> {
    if encoded.len() != NONCE_LEN * 2
        || !encoded
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(BackupError::InvalidContainer);
    }
    let mut nonce = [0u8; NONCE_LEN];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        nonce[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(nonce)
}

fn hex_nibble(byte: u8) -> BackupResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(BackupError::InvalidContainer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{build_export, import_document};
    use crate::model::{Event, EventKind};
    use crate::store::Store;
    use chrono::{NaiveDate, NaiveDateTime};

    const FIXTURE_KEY: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    const FIXTURE_NONCE: [u8; 24] = [
        0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae,
        0xaf, 0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7,
    ];

    fn dt(day: u32, hour: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 1, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap()
    }

    fn metadata() -> BackupMetadata {
        BackupMetadata {
            user_id: "11111111-1111-4111-8111-111111111111".into(),
            device_generation: 7,
            backup_id: "22222222-2222-4222-8222-222222222222".into(),
            created_at: "2026-01-02T03:04:05.000Z".into(),
        }
    }

    fn fixture_document() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/backup-v1/payload.json")).unwrap()
    }

    fn split(container: &[u8]) -> (BackupHeader, &[u8]) {
        let header_len = u32::from_be_bytes(container[8..12].try_into().unwrap()) as usize;
        let end = PREAMBLE_LEN + header_len;
        (
            serde_json::from_slice(&container[PREAMBLE_LEN..end]).unwrap(),
            &container[end..],
        )
    }

    fn rebuild(header: &BackupHeader, ciphertext: &[u8]) -> Vec<u8> {
        let header_bytes = serde_json::to_vec(header).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(ciphertext);
        out
    }

    fn decode_fixture_hex(raw: &str) -> Vec<u8> {
        let compact: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert_eq!(compact.len() % 2, 0);
        compact
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(text, 16).unwrap()
            })
            .collect()
    }

    #[test]
    fn codec_round_trip_and_verified_document_restores_to_empty_store() {
        let source = Store::open_in_memory().unwrap();
        let now = dt(2, 3);
        let raw = source
            .insert_raw_input("synthetic fixture meeting", "ingest_event", now)
            .unwrap();
        source
            .insert_event(
                &Event::new(
                    "Synthetic meeting",
                    EventKind::Meeting,
                    dt(3, 9),
                    "synthetic fixture meeting",
                    now,
                ),
                Some(raw),
            )
            .unwrap();
        let document = build_export(&source, now).unwrap();

        let container = create_backup(&document, &metadata(), &FIXTURE_KEY).unwrap();
        let verified = open_backup(&container, &FIXTURE_KEY).unwrap();
        assert_eq!(verified.document, document);
        assert_eq!(verified.import_plan.version, export::EXPORT_VERSION);

        let restored = Store::open_in_memory().unwrap();
        import_document(&restored, &verified.document, dt(10, 12)).unwrap();
        assert_eq!(restored.list_events().unwrap().len(), 1);
        assert_eq!(restored.list_raw_inputs().unwrap().len(), 1);
        assert_eq!(
            restored.list_events().unwrap()[0].title,
            "Synthetic meeting"
        );
    }

    #[test]
    fn wrong_key_ciphertext_tag_and_aad_tampering_are_rejected() {
        let normalized = normalized_json(&fixture_document()).unwrap();
        let container =
            create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE)
                .unwrap();

        let mut wrong_key = FIXTURE_KEY;
        wrong_key[0] ^= 1;
        assert_eq!(
            open_backup(&container, &wrong_key).unwrap_err(),
            BackupError::Authentication
        );

        let mut changed_ciphertext = container.clone();
        let body_index = changed_ciphertext.len() - TAG_LEN - 1;
        changed_ciphertext[body_index] ^= 1;
        assert_eq!(
            open_backup(&changed_ciphertext, &FIXTURE_KEY).unwrap_err(),
            BackupError::Authentication
        );

        let mut changed_tag = container.clone();
        *changed_tag.last_mut().unwrap() ^= 1;
        assert_eq!(
            open_backup(&changed_tag, &FIXTURE_KEY).unwrap_err(),
            BackupError::Authentication
        );

        let (mut changed_header, ciphertext) = split(&container);
        changed_header.user_id = "11111111-1111-4111-8111-111111111112".into();
        let changed_aad = rebuild(&changed_header, ciphertext);
        assert_eq!(
            open_backup(&changed_aad, &FIXTURE_KEY).unwrap_err(),
            BackupError::Authentication
        );
    }

    #[test]
    fn truncation_future_version_and_unknown_algorithms_are_rejected() {
        let normalized = normalized_json(&fixture_document()).unwrap();
        let container =
            create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE)
                .unwrap();
        for truncated in [
            &container[..0],
            &container[..11],
            &container[..container.len() - 1],
        ] {
            assert_eq!(
                open_backup(truncated, &FIXTURE_KEY).unwrap_err(),
                BackupError::Truncated
            );
        }

        let (mut header, ciphertext) = split(&container);
        header.protocol_version = 0;
        assert_eq!(
            open_backup(&rebuild(&header, ciphertext), &FIXTURE_KEY).unwrap_err(),
            BackupError::UnsupportedVersion(0)
        );

        let (mut header, ciphertext) = split(&container);
        header.protocol_version = 2;
        assert_eq!(
            open_backup(&rebuild(&header, ciphertext), &FIXTURE_KEY).unwrap_err(),
            BackupError::UnsupportedVersion(2)
        );

        let (mut header, ciphertext) = split(&container);
        header.compression = "unknown-compression".into();
        assert_eq!(
            open_backup(&rebuild(&header, ciphertext), &FIXTURE_KEY).unwrap_err(),
            BackupError::UnsupportedAlgorithm("compression")
        );

        let (mut header, ciphertext) = split(&container);
        header.encryption = "unknown-encryption".into();
        assert_eq!(
            open_backup(&rebuild(&header, ciphertext), &FIXTURE_KEY).unwrap_err(),
            BackupError::UnsupportedAlgorithm("encryption")
        );

        let (mut header, ciphertext) = split(&container);
        header.payload_digest = "unknown-digest".into();
        assert_eq!(
            open_backup(&rebuild(&header, ciphertext), &FIXTURE_KEY).unwrap_err(),
            BackupError::UnsupportedAlgorithm("payload digest")
        );
    }

    #[test]
    fn ciphertext_and_bounded_decompression_limits_are_enforced() {
        let normalized = normalized_json(&fixture_document()).unwrap();
        let container =
            create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE)
                .unwrap();
        let (_, ciphertext) = split(&container);
        assert_eq!(
            open_backup_with_limits(
                &container,
                &FIXTURE_KEY,
                DecodeLimits {
                    ciphertext: ciphertext.len() - 1,
                    plaintext: MAX_PLAINTEXT_BYTES,
                },
            )
            .unwrap_err(),
            BackupError::SizeLimit
        );

        // A validly authenticated zlib bomb lies about its expected output
        // size. The streaming cap must stop it before JSON parsing or writes.
        let expanded = vec![b'a'; 4096];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&expanded).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut bomb_header = split(&container).0;
        bomb_header.plaintext_size = 64;
        bomb_header.compressed_size = compressed.len() as u64;
        bomb_header.ciphertext_size = (compressed.len() + DIGEST_LEN + TAG_LEN) as u64;
        let mut bomb_body = vec![0u8; DIGEST_LEN];
        bomb_body.extend_from_slice(&compressed);
        let bomb =
            assemble_container(&bomb_header, &bomb_body, &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
        assert_eq!(
            open_backup_with_limits(
                &bomb,
                &FIXTURE_KEY,
                DecodeLimits {
                    ciphertext: MAX_CIPHERTEXT_BYTES,
                    plaintext: 128,
                },
            )
            .unwrap_err(),
            BackupError::SizeLimit
        );
    }

    #[test]
    fn authenticated_compression_stream_with_trailing_bytes_is_rejected() {
        let normalized = normalized_json(&fixture_document()).unwrap();
        let container =
            create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE)
                .unwrap();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&normalized).unwrap();
        let mut compressed = encoder.finish().unwrap();
        compressed.push(0);

        let mut header = split(&container).0;
        header.compressed_size = compressed.len() as u64;
        header.ciphertext_size = (compressed.len() + DIGEST_LEN + TAG_LEN) as u64;
        let mut body = Sha256::digest(&normalized).to_vec();
        body.extend_from_slice(&compressed);
        let malformed = assemble_container(&header, &body, &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
        assert_eq!(
            open_backup(&malformed, &FIXTURE_KEY).unwrap_err(),
            BackupError::Decompression
        );
    }

    #[test]
    fn production_calls_use_distinct_nonces_and_ciphertexts() {
        let document = fixture_document();
        let first = create_backup(&document, &metadata(), &FIXTURE_KEY).unwrap();
        let second = create_backup(&document, &metadata(), &FIXTURE_KEY).unwrap();
        assert_ne!(split(&first).0.nonce, split(&second).0.nonce);
        assert_ne!(first, second);
    }

    #[test]
    fn fixed_cross_platform_fixture_is_byte_stable_and_readable() {
        let normalized = normalized_json(&fixture_document()).unwrap();
        let actual =
            create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE)
                .unwrap();
        let expected =
            decode_fixture_hex(include_str!("../tests/fixtures/backup-v1/container.hex"));
        assert_eq!(
            actual,
            expected,
            "actual fixture hex: {}",
            encode_hex(&actual)
        );

        let verified = open_backup(&expected, &FIXTURE_KEY).unwrap();
        assert_eq!(verified.document, fixture_document());
        assert_eq!(verified.header, split(&expected).0);
        assert_eq!(verified.import_plan.total(), 0);
    }

    #[test]
    fn byte_input_normalizes_to_the_same_fixed_container() {
        let document = fixture_document();
        let pretty = serde_json::to_vec_pretty(&document).unwrap();
        let normalized = normalized_json(&document).unwrap();
        let from_value =
            create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE)
                .unwrap();
        let parsed: Value = serde_json::from_slice(&pretty).unwrap();
        let from_bytes = create_backup_with_nonce(
            &normalized_json(&parsed).unwrap(),
            &metadata(),
            &FIXTURE_KEY,
            FIXTURE_NONCE,
        )
        .unwrap();
        assert_eq!(from_value, from_bytes);
    }

    #[test]
    fn errors_never_include_key_or_payload_content() {
        let document = fixture_document();
        let container = create_backup(&document, &metadata(), &FIXTURE_KEY).unwrap();
        let mut wrong_key = FIXTURE_KEY;
        wrong_key[0] ^= 1;
        let message = open_backup(&container, &wrong_key).unwrap_err().to_string();
        assert!(!message.contains(&encode_hex(&FIXTURE_KEY)));
        assert!(!message.contains("synthetic-fixture"));
    }
}

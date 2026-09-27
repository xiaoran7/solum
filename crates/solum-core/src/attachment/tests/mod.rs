//! `tests` -- undefined
//!
//! Part of the `attachment` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{
    ChunkHeader, DecodeLimits, MAGIC, NONCE_LEN, PREAMBLE_LEN, RECORD_PREAMBLE_LEN,
};
use super::helpers::{encode_hex, read_u32};
use super::open::open_attachment_with_limits;
use super::seal::seal_attachment_inner;
use super::*;

const FIXTURE_KEY: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];
const FIXTURE_PLAINTEXT: &[u8] = b"Synthetic attachment fixture bytes only.";
const FIXTURE_NONCES: [[u8; NONCE_LEN]; 3] = [
    [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
    ],
    [
        0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26,
        0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
    ],
    [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e,
        0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
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
    let first_header_len = read_u32(&chunk_aad[records[0].0..records[0].0 + 4]).unwrap() as usize;
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
    let second_header_len = read_u32(&container[records[1].0..records[1].0 + 4]).unwrap() as usize;
    let first_header_start = records[0].0 + RECORD_PREAMBLE_LEN;
    let first_header_len = read_u32(&container[records[0].0..records[0].0 + 4]).unwrap() as usize;
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
        open_attachment(&rewrite_header(&container, &header), &binding, &FIXTURE_KEY).unwrap_err(),
        AttachmentError::UnsupportedVersion(2)
    );
    let (mut header, _) = split_records(&container);
    header.kdf = "unknown-kdf".into();
    assert_eq!(
        open_attachment(&rewrite_header(&container, &header), &binding, &FIXTURE_KEY).unwrap_err(),
        AttachmentError::UnsupportedAlgorithm("key derivation")
    );
    let (mut header, _) = split_records(&container);
    header.encryption = "unknown-aead".into();
    assert_eq!(
        open_attachment(&rewrite_header(&container, &header), &binding, &FIXTURE_KEY).unwrap_err(),
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
        "../../../tests/fixtures/attachment-v1/container.hex"
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

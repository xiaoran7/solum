//! `tests` -- the round-trip, framing and tamper tests, including the byte-stable fixture and the canonical-JSON property both directions depend on.
//!
//! Part of the `backup` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{DecodeLimits, DIGEST_LEN, MAGIC, PREAMBLE_LEN, TAG_LEN};
use super::helpers::{encode_hex, normalized_json};
use super::open::open_backup_with_limits;
use super::seal::{assemble_container, create_backup_with_nonce};
use super::*;

use crate::export::{build_export, import_document};
use crate::export_v3::{
    build_portable_export, ExternalRestoreIntents, PlatformPortableData, PortableChat,
    PORTABLE_EXPORT_VERSION,
};
use crate::model::{Event, EventKind};
use crate::store::Store;
use chrono::{NaiveDate, NaiveDateTime};

const FIXTURE_KEY: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];
const FIXTURE_NONCE: [u8; 24] = [
    0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf,
    0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7,
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
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/backup-v1/payload.json"
    ))
    .unwrap()
}

fn fixture_v3_document() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/backup-v1/payload-v3.json"
    ))
    .unwrap()
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
        .as_chunks::<2>()
        .0
        .iter()
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
fn protocol_v1_authenticates_and_plans_a_v3_payload_while_v2_stays_compatible() {
    let store = Store::open_in_memory().unwrap();
    let document = build_portable_export(
        &store,
        dt(2, 3),
        PlatformPortableData {
            chat: PortableChat::default(),
            documents: Vec::new(),
            pending_notifications: Vec::new(),
            llm: None,
            external_restore_intents: ExternalRestoreIntents::default(),
        },
    )
    .unwrap();

    let container = create_backup(&document, &metadata(), &FIXTURE_KEY).unwrap();
    let verified = open_backup(&container, &FIXTURE_KEY).unwrap();
    assert_eq!(verified.header.protocol_version, PROTOCOL_VERSION);
    assert_eq!(verified.header.payload_version, PORTABLE_EXPORT_VERSION);
    assert_eq!(verified.import_plan.version, PORTABLE_EXPORT_VERSION);
    assert_eq!(verified.document, document);

    // The checked-in fixture remains the exact v2 byte contract.
    let fixture = decode_fixture_hex(include_str!(
        "../../../tests/fixtures/backup-v1/container.hex"
    ));
    assert_eq!(
        open_backup(&fixture, &FIXTURE_KEY)
            .unwrap()
            .header
            .payload_version,
        2
    );
}

#[test]
fn wrong_key_ciphertext_tag_and_aad_tampering_are_rejected() {
    let normalized = normalized_json(&fixture_document()).unwrap();
    let container =
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();

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
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
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
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
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
    let bomb = assemble_container(&bomb_header, &bomb_body, &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
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
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
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
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
    let expected = decode_fixture_hex(include_str!(
        "../../../tests/fixtures/backup-v1/container.hex"
    ));
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
fn fixed_v3_cross_platform_fixture_is_byte_stable_and_readable() {
    let document = fixture_v3_document();
    let normalized = normalized_json(&document).unwrap();
    let actual =
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
    let expected = decode_fixture_hex(include_str!(
        "../../../tests/fixtures/backup-v1/container-v3.hex"
    ));
    assert_eq!(
        actual,
        expected,
        "actual v3 fixture hex: {}",
        encode_hex(&actual)
    );
    assert_eq!(
        encode_hex(&Sha256::digest(&expected)),
        "dc010cfb93432f01e2903b584161330f603e42a83f817010567a6d4b59935d4e"
    );
    let verified = open_backup(&expected, &FIXTURE_KEY).unwrap();
    assert_eq!(verified.document, document);
    assert_eq!(verified.header.payload_version, PORTABLE_EXPORT_VERSION);
}

#[test]
fn byte_input_normalizes_to_the_same_fixed_container() {
    let document = fixture_document();
    let pretty = serde_json::to_vec_pretty(&document).unwrap();
    let normalized = normalized_json(&document).unwrap();
    let from_value =
        create_backup_with_nonce(&normalized, &metadata(), &FIXTURE_KEY, FIXTURE_NONCE).unwrap();
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

//! tests. Part of the `backup_upload` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `backup_upload.rs`.

//! Regression tests for the upload coordinator.

use super::staging::{hex, random_hex, sha256_hex};
use super::types::{BackupDevicePhase, BackupDeviceState};
use super::*;

use std::sync::Mutex;

use chrono::NaiveDate;
use serde_json::json;

use crate::export_v3::{ExternalRestoreIntents, PlatformPortableData, PortableChat};
use crate::store::Store;

const USER_ID: &str = "11111111-1111-4111-8111-111111111111";
const WRITER_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CREATED_AT: &str = "2026-01-02T03:04:05.000Z";
const KEY: [u8; 32] = [7; 32];

struct FakeTransport {
    generation: u64,
    writer: Mutex<Option<String>>,
    calls: Mutex<Vec<String>>,
    uploaded: Mutex<Vec<Vec<u8>>>,
}

impl FakeTransport {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            writer: Mutex::new(Some(WRITER_ID.into())),
            calls: Mutex::new(Vec::new()),
            uploaded: Mutex::new(Vec::new()),
        }
    }

    fn unclaimed(generation: u64) -> Self {
        Self {
            generation,
            writer: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
            uploaded: Mutex::new(Vec::new()),
        }
    }

    fn require_writer(&self, writer_id: &str) -> UploadResult<()> {
        if self.writer.lock().unwrap().as_deref() == Some(writer_id) {
            Ok(())
        } else {
            Err(BackupUploadError::CloudConflict)
        }
    }
}

impl BackupCloudTransport for FakeTransport {
    fn active_generation(&self, _session: &mut AccountSession) -> UploadResult<u64> {
        self.calls.lock().unwrap().push("generation".into());
        Ok(self.generation)
    }

    fn claim_initial_writer(
        &self,
        _session: &mut AccountSession,
        writer_id: &str,
    ) -> UploadResult<u64> {
        self.calls.lock().unwrap().push("claim_writer".into());
        if self.generation != 1 {
            return Err(BackupUploadError::CloudConflict);
        }
        let mut writer = self.writer.lock().unwrap();
        match writer.as_deref() {
            Some(existing) if existing != writer_id => Err(BackupUploadError::CloudConflict),
            Some(_) => Ok(1),
            None => {
                *writer = Some(writer_id.to_string());
                Ok(1)
            }
        }
    }

    fn create_plan(
        &self,
        _session: &mut AccountSession,
        writer_id: &str,
        _plan: &BackupPlan,
    ) -> UploadResult<()> {
        self.require_writer(writer_id)?;
        self.calls.lock().unwrap().push("plan".into());
        Ok(())
    }

    fn put_object(
        &self,
        _session: &mut AccountSession,
        writer_id: &str,
        _backup_id: &str,
        object: &BackupObjectPlan,
        ciphertext: &[u8],
    ) -> UploadResult<()> {
        self.require_writer(writer_id)?;
        self.calls
            .lock()
            .unwrap()
            .push(format!("put:{:?}", object.kind));
        self.uploaded.lock().unwrap().push(ciphertext.to_vec());
        Ok(())
    }

    fn commit(
        &self,
        _session: &mut AccountSession,
        writer_id: &str,
        plan: &BackupPlan,
    ) -> UploadResult<BackupUploadOutcome> {
        self.require_writer(writer_id)?;
        self.calls.lock().unwrap().push("commit".into());
        Ok(BackupUploadOutcome {
            backup_id: plan.backup_id.clone(),
            generation: plan.generation,
            object_count: plan.objects.len(),
        })
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solum-backup-upload-{label}-{}-{}",
        std::process::id(),
        random_hex(8).unwrap()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn account() -> AccountSession {
    AccountSession {
        server_url: "https://example.invalid".into(),
        user_id: USER_ID.into(),
        username: "fixture".into(),
        access_token: "access-test-only".into(),
        refresh_token: "refresh-test-only".into(),
        model: crate::account::DEFAULT_CLOUD_MODEL.into(),
    }
}

fn portable_document(documents: Vec<DocumentManifest>) -> Value {
    let store = Store::open_in_memory().unwrap();
    export_v3::build_portable_export(
        &store,
        NaiveDate::from_ymd_opt(2026, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap(),
        PlatformPortableData {
            chat: PortableChat::default(),
            documents,
            pending_notifications: Vec::new(),
            llm: None,
            external_restore_intents: ExternalRestoreIntents::default(),
        },
    )
    .unwrap()
}

fn manifest(id: &str, plaintext: &[u8]) -> DocumentManifest {
    DocumentManifest {
        id: id.into(),
        name: "fixture.txt".into(),
        media_type: "text/plain".into(),
        plaintext_size: plaintext.len() as u64,
        sha256: sha256_hex(plaintext),
    }
}

#[test]
fn generation_one_writer_claim_is_atomic_and_never_adopts_remote_advance() {
    let root = temp_dir("generation");
    let mut session = account();
    let transport = FakeTransport::unclaimed(1);
    let authority =
        load_or_initialize_device_generation(&root, USER_ID, &transport, &mut session).unwrap();
    assert_eq!(authority.generation, 1);
    let state: BackupDeviceState =
        serde_json::from_str(&std::fs::read_to_string(root.join(DEVICE_STATE_FILE)).unwrap())
            .unwrap();
    assert_eq!(state.phase, BackupDevicePhase::Active);
    assert_eq!(state.writer_id, authority.writer_id);
    assert!(!serde_json::to_string(&state)
        .unwrap()
        .contains("access-test-only"));

    let competing = temp_dir("generation-competing");
    assert_eq!(
        load_or_initialize_device_generation(&competing, USER_ID, &transport, &mut session)
            .unwrap_err(),
        BackupUploadError::CloudConflict
    );
    let competing_state: BackupDeviceState =
        serde_json::from_str(&std::fs::read_to_string(competing.join(DEVICE_STATE_FILE)).unwrap())
            .unwrap();
    assert_eq!(competing_state.phase, BackupDevicePhase::Claiming);
    assert_ne!(competing_state.writer_id, authority.writer_id);

    assert_eq!(
        load_or_initialize_device_generation(&root, USER_ID, &transport, &mut session).unwrap(),
        authority
    );

    let fresh = temp_dir("generation-fresh");
    assert_eq!(
        load_or_initialize_device_generation(
            &fresh,
            USER_ID,
            &FakeTransport::unclaimed(2),
            &mut session
        )
        .unwrap_err(),
        BackupUploadError::CloudConflict
    );
    let fresh_state: BackupDeviceState =
        serde_json::from_str(&std::fs::read_to_string(fresh.join(DEVICE_STATE_FILE)).unwrap())
            .unwrap();
    assert_eq!(fresh_state.phase, BackupDevicePhase::Claiming);
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(competing);
    let _ = std::fs::remove_dir_all(fresh);
}

#[test]
fn malformed_or_cross_account_generation_state_fails_closed() {
    let root = temp_dir("bad-generation");
    std::fs::write(root.join(DEVICE_STATE_FILE), "{\"version\":1}").unwrap();
    let transport = FakeTransport::new(1);
    assert_eq!(
        load_or_initialize_device_generation(&root, USER_ID, &transport, &mut account())
            .unwrap_err(),
        BackupUploadError::InvalidDeviceState
    );
    assert!(transport.calls.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn takeover_persists_missing_or_expected_generation_idempotently() {
    for initial in [None, Some(4)] {
        let root = temp_dir("takeover-generation");
        if let Some(device_generation) = initial {
            let state = BackupDeviceState {
                format: DEVICE_STATE_FORMAT.into(),
                version: DEVICE_STATE_VERSION,
                user_id: USER_ID.into(),
                device_generation,
                writer_id: WRITER_ID.into(),
                phase: BackupDevicePhase::Active,
            };
            std::fs::write(
                root.join(DEVICE_STATE_FILE),
                serde_json::to_string(&state).unwrap(),
            )
            .unwrap();
        }
        persist_taken_over_device_generation(&root, USER_ID, 4, 5, WRITER_ID).unwrap();
        persist_taken_over_device_generation(&root, USER_ID, 4, 5, WRITER_ID).unwrap();
        let state: BackupDeviceState =
            serde_json::from_str(&std::fs::read_to_string(root.join(DEVICE_STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(state.device_generation, 5);
        let _ = std::fs::remove_dir_all(root);
    }

    let root = temp_dir("takeover-stale");
    let state = BackupDeviceState {
        format: DEVICE_STATE_FORMAT.into(),
        version: DEVICE_STATE_VERSION,
        user_id: USER_ID.into(),
        device_generation: 3,
        writer_id: WRITER_ID.into(),
        phase: BackupDevicePhase::Active,
    };
    std::fs::write(
        root.join(DEVICE_STATE_FILE),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    assert_eq!(
        persist_taken_over_device_generation(&root, USER_ID, 4, 5, WRITER_ID).unwrap_err(),
        BackupUploadError::InvalidDeviceState
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn encrypted_session_uploads_main_then_manifest_order_and_commits_last() {
    let root = temp_dir("roundtrip");
    let first = b"public fixture alpha";
    let second = b"public fixture beta";
    let ids = ["doc-alpha", "doc-beta"];
    let document = portable_document(vec![manifest(ids[0], first), manifest(ids[1], second)]);
    let json = serde_json::to_vec(&document).unwrap();
    let mut upload =
        BackupUploadSession::begin(&root, &json, USER_ID, 1, WRITER_ID.into(), CREATED_AT, KEY)
            .unwrap();
    let begin = upload.begin_info();
    assert_eq!(begin.attachments.len(), 2);
    upload.append_attachment_chunk(ids[0], 0, first).unwrap();
    upload.finish_attachment(ids[0]).unwrap();
    upload.append_attachment_chunk(ids[1], 0, second).unwrap();
    upload.finish_attachment(ids[1]).unwrap();

    let transport = FakeTransport::new(1);
    let outcome = upload.upload(&transport, &mut account()).unwrap();
    assert_eq!(outcome.object_count, 3);
    assert_eq!(
        *transport.calls.lock().unwrap(),
        vec![
            "plan",
            "put:Main",
            "put:Attachment",
            "put:Attachment",
            "commit"
        ]
    );
    let uploaded = transport.uploaded.lock().unwrap();
    let verified = backup::open_backup(&uploaded[0], &KEY).unwrap();
    assert_eq!(verified.document, document);
    let metadata = BackupMetadata {
        user_id: USER_ID.into(),
        device_generation: 1,
        backup_id: outcome.backup_id,
        created_at: CREATED_AT.into(),
    };
    for (index, plaintext) in [first.as_slice(), second.as_slice()]
        .into_iter()
        .enumerate()
    {
        let binding = AttachmentBinding::from_manifest(&metadata, &manifest(ids[index], plaintext));
        assert_eq!(
            attachment::open_attachment(&uploaded[index + 1], &binding, &KEY).unwrap(),
            plaintext
        );
    }
    drop(uploaded);
    let staging = upload.staging_dir.clone();
    drop(upload);
    assert!(!staging.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attachment_stream_rejects_wrong_order_offset_size_and_digest() {
    let root = temp_dir("stream-errors");
    let plaintext = b"expected fixture bytes";
    let document = portable_document(vec![manifest("expected-id", plaintext)]);
    let mut upload = BackupUploadSession::begin(
        &root,
        &serde_json::to_vec(&document).unwrap(),
        USER_ID,
        1,
        WRITER_ID.into(),
        CREATED_AT,
        KEY,
    )
    .unwrap();
    assert_eq!(
        upload
            .append_attachment_chunk("wrong-id", 0, plaintext)
            .unwrap_err(),
        BackupUploadError::InvalidAttachmentStream
    );
    assert_eq!(
        upload
            .append_attachment_chunk("expected-id", 1, plaintext)
            .unwrap_err(),
        BackupUploadError::InvalidAttachmentStream
    );
    assert_eq!(
        upload
            .append_attachment_chunk("expected-id", 0, &vec![0; MAX_IPC_CHUNK_BYTES + 1])
            .unwrap_err(),
        BackupUploadError::SizeLimit
    );
    let mut wrong_digest = plaintext.to_vec();
    wrong_digest[0] ^= 1;
    upload
        .append_attachment_chunk("expected-id", 0, &wrong_digest)
        .unwrap();
    assert_eq!(
        upload.finish_attachment("expected-id").unwrap_err(),
        BackupUploadError::Encryption
    );
    assert_eq!(
        upload.plan().unwrap_err(),
        BackupUploadError::InvalidAttachmentStream
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_attempts_use_distinct_ids_and_ciphertext_and_cleanup_orphans() {
    let root = temp_dir("randomness");
    let document = portable_document(Vec::new());
    let encoded = serde_json::to_vec(&document).unwrap();
    let first = BackupUploadSession::begin(
        &root,
        &encoded,
        USER_ID,
        1,
        WRITER_ID.into(),
        CREATED_AT,
        KEY,
    )
    .unwrap();
    let first_plan = first.plan().unwrap();
    let first_bytes = std::fs::read(&first.staged[0].path).unwrap();
    let orphan = root.join(UPLOAD_STAGING_DIR).join("a".repeat(32));
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("opaque"), b"ciphertext-only fixture").unwrap();
    drop(first);
    std::fs::create_dir_all(&orphan).unwrap();

    let second = BackupUploadSession::begin(
        &root,
        &encoded,
        USER_ID,
        1,
        WRITER_ID.into(),
        CREATED_AT,
        KEY,
    )
    .unwrap();
    let second_plan = second.plan().unwrap();
    let second_bytes = std::fs::read(&second.staged[0].path).unwrap();
    assert_ne!(first_plan.backup_id, second_plan.backup_id);
    assert_ne!(
        first_plan.objects[0].object_id,
        second_plan.objects[0].object_id
    );
    assert_ne!(first_bytes, second_bytes);
    assert!(!orphan.exists());
    let unrelated = root.join(UPLOAD_STAGING_DIR).join("keep-me");
    std::fs::create_dir_all(&unrelated).unwrap();
    drop(second);
    assert!(unrelated.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attachment_object_uuid_is_keyed_version_scoped_and_stable() {
    let first = attachment_object_id(&KEY, USER_ID, "doc-alpha").unwrap();
    assert_eq!(first, "52e86d9a-4a62-863c-8e5d-beb01ae600dc");
    assert_eq!(
        first,
        attachment_object_id(&KEY, USER_ID, "doc-alpha").unwrap()
    );
    assert_eq!(first.as_bytes()[14], b'8');
    assert!(matches!(first.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    assert_ne!(
        first,
        attachment_object_id(&[8; 32], USER_ID, "doc-alpha").unwrap()
    );
    assert_ne!(
        first,
        attachment_object_id(&KEY, "22222222-2222-4222-8222-222222222222", "doc-alpha").unwrap()
    );
    assert_ne!(
        first,
        attachment_object_id(&KEY, USER_ID, "doc-beta").unwrap()
    );
    assert!(!first.contains("doc-alpha"));
    let mut fixture_key = [0u8; 32];
    for (index, byte) in fixture_key.iter_mut().enumerate() {
        *byte = index as u8;
    }
    assert_eq!(
        attachment_object_id(
            &fixture_key,
            "22222222-2222-4222-8222-222222222222",
            "document-public-fixture"
        )
        .unwrap(),
        "c7dce9ce-d820-8325-b878-4a49a3c8bc2a"
    );
    assert_eq!(
        attachment_object_id(&KEY, USER_ID, "private id").unwrap_err(),
        BackupUploadError::InvalidSnapshot
    );
}

#[test]
fn errors_do_not_echo_plaintext_key_or_identifiers() {
    let marker = "private-marker-never-log";
    let root = temp_dir("safe-errors");
    let document = portable_document(vec![manifest(marker, marker.as_bytes())]);
    let mut upload = BackupUploadSession::begin(
        &root,
        &serde_json::to_vec(&document).unwrap(),
        USER_ID,
        1,
        WRITER_ID.into(),
        CREATED_AT,
        KEY,
    )
    .unwrap();
    let message = upload
        .append_attachment_chunk(marker, 1, marker.as_bytes())
        .unwrap_err()
        .to_string();
    assert!(!message.contains(marker));
    assert!(!message.contains(USER_ID));
    assert!(!message.contains(&hex(&KEY)));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn plan_json_matches_server_wire_shape() {
    let plan = BackupPlan {
        backup_id: "11111111-1111-4111-8111-111111111111".into(),
        generation: 1,
        created_at: CREATED_AT.into(),
        protocol_version: 1,
        objects: vec![BackupObjectPlan {
            object_id: "22222222-2222-4222-8222-222222222222".into(),
            kind: BackupObjectKind::Main,
            bytes: 123,
            sha256: "00".repeat(32),
        }],
    };
    assert_eq!(
        serde_json::to_value(plan).unwrap(),
        json!({
            "backup_id": "11111111-1111-4111-8111-111111111111",
            "generation": 1,
            "created_at": CREATED_AT,
            "protocol_version": 1,
            "objects": [{
                "object_id": "22222222-2222-4222-8222-222222222222",
                "kind": "main",
                "bytes": 123,
                "sha256": "00".repeat(32),
            }],
        })
    );
}

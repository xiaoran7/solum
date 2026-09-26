//! `tests` -- the suite for all of the above.
//!
//! Part of the `backup_download` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::http::HttpFailure;
use super::stage::{lower_hex, validate_catalog};
use super::types::CloudBackupVersion;
use super::*;
use crate::export_v3::{self, ExternalRestoreIntents, PlatformPortableData, PortableChat};
use crate::profile_generation::POINTER_FILE;
use crate::store::Store;

const USER_ID: &str = "11111111-1111-4111-8111-111111111111";
const BACKUP_ID: &str = "22222222-2222-4222-8222-222222222222";
const MAIN_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
const CREATED_AT: &str = "2026-01-02T03:04:05.000Z";
const COMMITTED_AT: &str = "2026-01-02T03:05:06.000Z";
const KEY: [u8; 32] = [7; 32];
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct FakeReader {
    catalog: BackupCatalog,
    objects: BTreeMap<String, Vec<u8>>,
    calls: Mutex<Vec<String>>,
}

impl BackupCloudReader for FakeReader {
    fn list_backups(
        &self,
        _session: &mut AccountSession,
        _limit: usize,
    ) -> DownloadResult<BackupCatalog> {
        self.calls.lock().unwrap().push("list".into());
        Ok(self.catalog.clone())
    }

    fn download_object(
        &self,
        _session: &mut AccountSession,
        _backup_id: &str,
        object: &BackupObjectPlan,
    ) -> DownloadResult<Vec<u8>> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("get:{:?}", object.kind));
        self.objects
            .get(&object.object_id)
            .cloned()
            .ok_or(BackupDownloadError::BackupNotFound)
    }
}

impl BackupCloudDelete for FakeReader {
    fn delete_backup(
        &self,
        _session: &mut AccountSession,
        backup_id: &str,
        plan_etag: &str,
    ) -> DownloadResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("delete:{backup_id}:{plan_etag}"));
        Ok(())
    }
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

fn temp_dir(label: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "solum-backup-download-{label}-{}-{counter}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn manifest(id: &str, plaintext: &[u8]) -> DocumentManifest {
    DocumentManifest {
        id: id.into(),
        name: format!("{id}.txt"),
        media_type: "text/plain".into(),
        plaintext_size: plaintext.len() as u64,
        sha256: lower_hex(&Sha256::digest(plaintext)),
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

fn object_plan(object_id: &str, kind: BackupObjectKind, bytes: &[u8]) -> BackupObjectPlan {
    BackupObjectPlan {
        object_id: object_id.into(),
        kind,
        bytes: bytes.len() as u64,
        sha256: lower_hex(&Sha256::digest(bytes)),
    }
}

fn fixture(attachments: &[(&str, &[u8])]) -> FakeReader {
    let manifests: Vec<_> = attachments
        .iter()
        .map(|(id, plaintext)| manifest(id, plaintext))
        .collect();
    let document = portable_document(manifests.clone());
    let metadata = BackupMetadata {
        user_id: USER_ID.into(),
        device_generation: 1,
        backup_id: BACKUP_ID.into(),
        created_at: CREATED_AT.into(),
    };
    let main = backup::create_backup(&document, &metadata, &KEY).unwrap();
    let mut objects = BTreeMap::from([(MAIN_ID.to_string(), main.clone())]);
    let mut plans = vec![object_plan(MAIN_ID, BackupObjectKind::Main, &main)];
    for ((_, plaintext), manifest) in attachments.iter().zip(manifests.iter()) {
        let binding = AttachmentBinding::from_manifest(&metadata, manifest);
        let ciphertext = attachment::seal_attachment(plaintext, &binding, &KEY).unwrap();
        let id = attachment_object_id(&KEY, BACKUP_ID, &manifest.id).unwrap();
        plans.push(object_plan(&id, BackupObjectKind::Attachment, &ciphertext));
        objects.insert(id, ciphertext);
    }
    plans.sort_by(|left, right| left.object_id.cmp(&right.object_id));
    let total_bytes = plans.iter().map(|object| object.bytes).sum();
    let plan_etag = lower_hex(&Sha256::digest(
        serde_json::to_vec(&BackupPlan {
            backup_id: BACKUP_ID.into(),
            generation: 1,
            created_at: CREATED_AT.into(),
            protocol_version: 1,
            objects: plans.clone(),
        })
        .unwrap(),
    ));
    let catalog = BackupCatalog {
        active_generation: 1,
        backups: vec![CloudBackupVersion {
            backup_id: BACKUP_ID.into(),
            generation: 1,
            created_at: CREATED_AT.into(),
            committed_at: COMMITTED_AT.into(),
            protocol_version: 1,
            total_bytes,
            object_count: plans.len(),
            plan_etag,
            objects: plans,
        }],
    };
    FakeReader {
        catalog,
        objects,
        calls: Mutex::new(Vec::new()),
    }
}

fn refresh_plan_etag(version: &mut CloudBackupVersion) {
    version.plan_etag = lower_hex(&Sha256::digest(
        serde_json::to_vec(&BackupPlan {
            backup_id: version.backup_id.clone(),
            generation: version.generation,
            created_at: version.created_at.clone(),
            protocol_version: version.protocol_version,
            objects: version.objects.clone(),
        })
        .unwrap(),
    ));
}

fn delete_request(version: &CloudBackupVersion) -> BackupDeleteRequest {
    BackupDeleteRequest {
        backup_id: version.backup_id.clone(),
        plan_etag: version.plan_etag.clone(),
        committed_at: version.committed_at.clone(),
        total_bytes: version.total_bytes,
        object_count: version.object_count,
    }
}

#[test]
fn delete_revalidates_preview_metadata_and_binds_the_plan_etag() {
    let reader = fixture(&[("doc-alpha", b"public fixture alpha")]);
    let request = delete_request(&reader.catalog.backups[0]);
    delete_backup_version(&reader, &mut account(), &request).unwrap();
    assert_eq!(reader.calls.lock().unwrap().len(), 2);
    assert_eq!(reader.calls.lock().unwrap()[0], "list");
    assert_eq!(
        reader.calls.lock().unwrap()[1],
        format!("delete:{BACKUP_ID}:{}", request.plan_etag)
    );
}

#[test]
fn destructive_transport_failures_are_not_automatically_replayed() {
    let reader = HttpBackupCloudReader::new();
    let calls = Cell::new(0);
    let result = reader.request_without_transport_retry(&mut account(), |_session| {
        calls.set(calls.get() + 1);
        Err(HttpFailure::Transport)
    });
    assert_eq!(result.unwrap_err(), BackupDownloadError::Transport);
    assert_eq!(calls.get(), 1);
}

#[test]
fn delete_refuses_stale_or_malformed_preview_before_mutation() {
    let reader = fixture(&[]);
    let mut stale = delete_request(&reader.catalog.backups[0]);
    stale.total_bytes += 1;
    assert_eq!(
        delete_backup_version(&reader, &mut account(), &stale).unwrap_err(),
        BackupDownloadError::DeletePrecondition
    );
    assert_eq!(*reader.calls.lock().unwrap(), vec!["list"]);

    reader.calls.lock().unwrap().clear();
    let mut malformed = delete_request(&reader.catalog.backups[0]);
    malformed.plan_etag = "ABC".into();
    assert_eq!(
        delete_backup_version(&reader, &mut account(), &malformed).unwrap_err(),
        BackupDownloadError::InvalidCatalog
    );
    assert!(reader.calls.lock().unwrap().is_empty());
}

#[test]
fn catalog_then_main_first_roundtrip_stages_inactive_generation() {
    let reader = fixture(&[
        ("doc-alpha", b"public fixture alpha"),
        ("doc-beta", b"public fixture beta"),
    ]);
    let root = temp_dir("roundtrip");
    let mut session = account();
    let catalog = fetch_backup_catalog(&reader, &mut session, 20).unwrap();
    let outcome = download_and_stage_backup(
        BackupStageRequest {
            profile_root: &root,
            catalog: &catalog,
            backup_id: BACKUP_ID,
            user_id: USER_ID,
            master_key: &KEY,
            now: NaiveDate::from_ymd_opt(2026, 1, 3)
                .unwrap()
                .and_hms_opt(4, 5, 6)
                .unwrap(),
        },
        &reader,
        &mut session,
    )
    .unwrap();
    assert_eq!(outcome.backup_id, BACKUP_ID);
    assert_eq!(outcome.attachment_count, 2);
    assert_eq!(outcome.object_count, 3);
    assert_eq!(
        *reader.calls.lock().unwrap(),
        vec!["list", "get:Main", "get:Attachment", "get:Attachment"]
    );
    assert!(!root.join(POINTER_FILE).exists());
    let generation =
        crate::portable_generation::PortableGenerationReader::open(&root, &outcome.generation_id)
            .unwrap();
    assert_eq!(generation.bootstrap().documents.len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn wrong_key_corrupt_main_and_metadata_mismatch_fail_before_staging() {
    for mode in ["wrong-key", "corrupt-main", "metadata"] {
        let mut reader = fixture(&[("doc-alpha", b"public fixture alpha")]);
        let key = if mode == "wrong-key" { [8; 32] } else { KEY };
        if mode == "corrupt-main" {
            reader.objects.get_mut(MAIN_ID).unwrap()[0] ^= 1;
        }
        if mode == "metadata" {
            reader.catalog.backups[0].created_at = "2026-01-02T03:04:06.000Z".into();
            refresh_plan_etag(&mut reader.catalog.backups[0]);
        }
        let root = temp_dir(mode);
        let error = download_and_stage_backup(
            BackupStageRequest {
                profile_root: &root,
                catalog: &reader.catalog,
                backup_id: BACKUP_ID,
                user_id: USER_ID,
                master_key: &key,
                now: NaiveDate::from_ymd_opt(2026, 1, 3)
                    .unwrap()
                    .and_hms_opt(4, 5, 6)
                    .unwrap(),
            },
            &reader,
            &mut account(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            match mode {
                "corrupt-main" => BackupDownloadError::TransportIntegrity,
                "metadata" => BackupDownloadError::MetadataMismatch,
                _ => BackupDownloadError::Authentication,
            }
        );
        assert!(!root.join(POINTER_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn missing_extra_and_corrupt_attachment_fail_before_store_write() {
    for mode in ["missing", "extra", "corrupt"] {
        let mut reader = fixture(&[("doc-alpha", b"public fixture alpha")]);
        if mode == "missing" {
            reader.catalog.backups[0]
                .objects
                .retain(|object| object.kind == BackupObjectKind::Main);
        } else if mode == "extra" {
            reader.catalog.backups[0].objects.push(BackupObjectPlan {
                object_id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".into(),
                kind: BackupObjectKind::Attachment,
                bytes: 1,
                sha256: "00".repeat(32),
            });
            reader.catalog.backups[0]
                .objects
                .sort_by(|left, right| left.object_id.cmp(&right.object_id));
        } else {
            let attachment = reader.catalog.backups[0]
                .objects
                .iter()
                .find(|object| object.kind == BackupObjectKind::Attachment)
                .unwrap();
            reader.objects.get_mut(&attachment.object_id).unwrap()[0] ^= 1;
        }
        let version = &mut reader.catalog.backups[0];
        version.object_count = version.objects.len();
        version.total_bytes = version.objects.iter().map(|object| object.bytes).sum();
        refresh_plan_etag(version);
        let root = temp_dir(mode);
        let error = download_and_stage_backup(
            BackupStageRequest {
                profile_root: &root,
                catalog: &reader.catalog,
                backup_id: BACKUP_ID,
                user_id: USER_ID,
                master_key: &KEY,
                now: NaiveDate::from_ymd_opt(2026, 1, 3)
                    .unwrap()
                    .and_hms_opt(4, 5, 6)
                    .unwrap(),
            },
            &reader,
            &mut account(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            if mode == "corrupt" {
                BackupDownloadError::TransportIntegrity
            } else {
                BackupDownloadError::AttachmentSet
            }
        );
        assert!(!root.join(POINTER_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn future_version_and_catalog_shape_or_limits_fail_closed() {
    let mut future = fixture(&[]);
    future.catalog.backups[0].protocol_version = 2;
    refresh_plan_etag(&mut future.catalog.backups[0]);
    let future_root = temp_dir("future");
    assert_eq!(
        download_and_stage_backup(
            BackupStageRequest {
                profile_root: &future_root,
                catalog: &future.catalog,
                backup_id: BACKUP_ID,
                user_id: USER_ID,
                master_key: &KEY,
                now: NaiveDate::from_ymd_opt(2026, 1, 3)
                    .unwrap()
                    .and_hms_opt(4, 5, 6)
                    .unwrap(),
            },
            &future,
            &mut account(),
        )
        .unwrap_err(),
        BackupDownloadError::UnsupportedVersion(2)
    );
    assert!(future.calls.lock().unwrap().is_empty());

    let encoded = serde_json::to_value(&future.catalog).unwrap();
    let mut unknown = encoded.as_object().unwrap().clone();
    unknown.insert("unexpected".into(), json!(true));
    assert!(serde_json::from_value::<BackupCatalog>(Value::Object(unknown)).is_err());

    let mut oversized = fixture(&[]).catalog;
    let object = &mut oversized.backups[0].objects[0];
    object.bytes = backup::MAX_CONTAINER_BYTES as u64 + 1;
    oversized.backups[0].total_bytes = object.bytes;
    assert_eq!(
        validate_catalog(&oversized, 20).unwrap_err(),
        BackupDownloadError::InvalidCatalog
    );
}

#[test]
fn truncated_download_and_safe_errors_do_not_echo_sensitive_values() {
    let mut reader = fixture(&[]);
    reader.objects.get_mut(MAIN_ID).unwrap().pop();
    let root = temp_dir("truncated");
    let error = download_and_stage_backup(
        BackupStageRequest {
            profile_root: &root,
            catalog: &reader.catalog,
            backup_id: BACKUP_ID,
            user_id: USER_ID,
            master_key: &KEY,
            now: NaiveDate::from_ymd_opt(2026, 1, 3)
                .unwrap()
                .and_hms_opt(4, 5, 6)
                .unwrap(),
        },
        &reader,
        &mut account(),
    )
    .unwrap_err();
    assert_eq!(error, BackupDownloadError::TransportIntegrity);
    let message = error.to_string();
    assert!(!message.contains(USER_ID));
    assert!(!message.contains(BACKUP_ID));
    assert!(!message.contains(&lower_hex(&KEY)));
    assert!(!root.join(POINTER_FILE).exists());
    let _ = std::fs::remove_dir_all(root);
}

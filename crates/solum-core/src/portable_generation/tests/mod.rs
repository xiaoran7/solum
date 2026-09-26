//! `tests` -- the suite for all of the above.
//!
//! Part of the `portable_generation` module; see `mod.rs` for the
//! contract. Moved verbatim from the single-file `portable_generation.rs`.

use super::*;

use super::handoff::prepare_restore_handoff;
use super::stage::SpoolRecord;
use super::verify::lower_hex;

use chrono::NaiveDate;

use crate::export_v3::{
    build_portable_export, DocumentManifest, ExternalRestoreIntents, PendingNotificationInput,
    PlatformPortableData, PortableChatDraft, PortableChatMessage, PortableChatRole,
    PortableChatSession, PortableLlmSettings,
};
use crate::notification_intelligence::NotificationIntelligenceConfig;
use crate::profile_generation::{
    publish_profile_generation, resolve_active_data_dir, GENERATIONS_DIR, MANIFEST_FILE,
    POINTER_FILE, STAGING_DIR,
};
use crate::restore_v3::{prepare_portable_restore, RestoredAttachment};

fn dt(day: u32, hour: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 8, day)
        .unwrap()
        .and_hms_opt(hour, 0, 0)
        .unwrap()
}

fn fixture() -> (serde_json::Value, Vec<RestoredAttachment>) {
    let source = Store::open_in_memory().unwrap();
    source
        .insert_raw_input("Synthetic input", "fixture", dt(22, 8))
        .unwrap();
    source
        .save_notification_intelligence_config(&NotificationIntelligenceConfig {
            capture_mode: crate::notification_intelligence::NotificationCaptureMode::Allowlist,
            allowed_packages: vec!["com.example.calendar".into()],
            auto_event_packages: vec!["com.example.calendar".into()],
            batch_interval_minutes: 20,
            filter_rules: Vec::new(),
        })
        .unwrap();
    source.set_notif_cloud_enabled(true).unwrap();

    let plaintext: Vec<u8> = (0..WEBVIEW_DOCUMENT_CHUNK_BYTES + 17)
        .map(|index| (index % 251) as u8)
        .collect();
    let sha256 = lower_hex(&Sha256::digest(&plaintext));
    let document = build_portable_export(
        &source,
        dt(22, 12),
        PlatformPortableData {
            chat: PortableChat {
                sessions: vec![PortableChatSession {
                    id: "session-1".into(),
                    title: "Synthetic session".into(),
                    updated_at: "2026-08-22T12:00:00+08:00".into(),
                    messages: vec![PortableChatMessage {
                        role: PortableChatRole::User,
                        content: "Synthetic message".into(),
                        intent: String::new(),
                    }],
                }],
                drafts: vec![PortableChatDraft {
                    session_id: "session-1".into(),
                    content: "Synthetic draft".into(),
                }],
            },
            documents: vec![DocumentManifest {
                id: "document:1".into(),
                name: "fixture.txt".into(),
                media_type: "text/plain".into(),
                plaintext_size: plaintext.len() as u64,
                sha256,
            }],
            pending_notifications: vec![
                PendingNotificationInput {
                    package_name: "com.example.pending".into(),
                    title: "Pending fixture".into(),
                    body: "Synthetic pending body".into(),
                    received_at: "2026-08-22T11:00:00+08:00".into(),
                    failed: false,
                },
                PendingNotificationInput {
                    package_name: "com.example.failed".into(),
                    title: "Failed fixture".into(),
                    body: "Synthetic failed body".into(),
                    received_at: "2026-08-22T11:30:00+08:00".into(),
                    failed: true,
                },
            ],
            llm: Some(PortableLlmSettings {
                base_url: "https://example.invalid/v1".into(),
                model: "fixture-model".into(),
                temperature: Some(0.2),
                max_tokens: Some(128),
                timeout_secs: 30,
            }),
            external_restore_intents: ExternalRestoreIntents {
                health_sources: vec!["steps".into()],
                background_reminders: true,
                ..ExternalRestoreIntents::default()
            },
        },
    )
    .unwrap();
    (
        document,
        vec![RestoredAttachment {
            id: "document:1".into(),
            plaintext,
        }],
    )
}

fn test_root(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solum-portable-generation-{label}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn complete_platform_image_is_sealed_but_inactive_until_publish() {
    let root = test_root("complete");
    let (document, attachments) = fixture();
    let prepared = prepare_portable_restore(document, attachments).unwrap();
    let sealed = stage_portable_generation(&root, prepared, dt(23, 12)).unwrap();
    let staging = root.join(STAGING_DIR).join(sealed.generation_id());

    assert_eq!(
        sealed.plan().version,
        crate::export_v3::PORTABLE_EXPORT_VERSION
    );
    assert!(sealed.counts().applied > 0);
    assert!(staging.join(MANIFEST_FILE).is_file());
    assert_eq!(
        std::fs::read_to_string(staging.join(NOTIFICATION_POLICY_FILE)).unwrap(),
        EMPTY_NOTIFICATION_POLICY
    );
    assert!(staging
        .join(WEBVIEW_DIR)
        .join(WEBVIEW_BOOTSTRAP_FILE)
        .is_file());
    assert_eq!(
        std::fs::read_dir(staging.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(staging.join(NOTIFICATION_SPOOL_DIR))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry
                .path()
                .extension()
                .is_some_and(|value| value == "json"))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(
            staging
                .join(NOTIFICATION_SPOOL_DIR)
                .join(FAILED_NOTIFICATION_DIR)
        )
        .unwrap()
        .count(),
        1
    );
    assert!(resolve_active_data_dir(&root)
        .unwrap()
        .generation_id()
        .is_none());
    assert!(!root.join(POINTER_FILE).exists());

    let active = publish_profile_generation(sealed.generation()).unwrap();
    assert_eq!(active.generation_id(), Some(sealed.generation_id()));
    let restored = Store::open(active.path().join(SQLITE_FILE).to_str().unwrap()).unwrap();
    assert_eq!(restored.list_raw_inputs().unwrap().len(), 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn staged_platform_plaintext_is_not_placed_in_pointer_or_file_names() {
    let root = test_root("privacy");
    let (document, attachments) = fixture();
    let prepared = prepare_portable_restore(document, attachments).unwrap();
    let sealed = stage_portable_generation(&root, prepared, dt(23, 12)).unwrap();
    let staging = root.join(STAGING_DIR).join(sealed.generation_id());
    let names = walk_names(&staging);
    assert!(!names.contains("Synthetic"));
    assert!(!names.contains("document:1"));
    assert!(!root.join(POINTER_FILE).exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn sealed_reader_reconstructs_only_declared_documents_in_bounded_chunks() {
    let root = test_root("reader");
    let (document, attachments) = fixture();
    let expected = attachments[0].plaintext.clone();
    let sealed = stage_portable_generation(
        &root,
        prepare_portable_restore(document, attachments).unwrap(),
        dt(23, 12),
    )
    .unwrap();
    let reader = PortableGenerationReader::open(&root, sealed.generation_id()).unwrap();
    assert_eq!(reader.bootstrap().documents.len(), 1);
    assert!(reader.read_document_chunk("not-in-generation", 0).is_err());
    assert!(reader
        .read_document_chunk("document:1", expected.len() as u64 + 1)
        .is_err());

    let first = reader.read_document_chunk("document:1", 0).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&first.data_base64)
            .unwrap()
            .len(),
        WEBVIEW_DOCUMENT_CHUNK_BYTES
    );
    assert!(!first.done);
    let second = reader
        .read_document_chunk("document:1", WEBVIEW_DOCUMENT_CHUNK_BYTES as u64)
        .unwrap();
    assert!(second.done);
    let mut reconstructed = base64::engine::general_purpose::STANDARD
        .decode(first.data_base64)
        .unwrap();
    reconstructed.extend(
        base64::engine::general_purpose::STANDARD
            .decode(second.data_base64)
            .unwrap(),
    );
    assert_eq!(reconstructed, expected);
    assert!(!root.join(POINTER_FILE).exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tampering_after_seal_cannot_publish_the_candidate() {
    let root = test_root("tamper");
    let (document, attachments) = fixture();
    let prepared = prepare_portable_restore(document, attachments).unwrap();
    let sealed = stage_portable_generation(&root, prepared, dt(23, 12)).unwrap();
    let documents = root
        .join(STAGING_DIR)
        .join(sealed.generation_id())
        .join(WEBVIEW_DIR)
        .join(WEBVIEW_DOCUMENTS_DIR);
    let blob = std::fs::read_dir(documents)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(blob, b"tampered").unwrap();

    assert!(publish_profile_generation(sealed.generation()).is_err());
    assert!(!root.join(POINTER_FILE).exists());
    assert!(resolve_active_data_dir(&root)
        .unwrap()
        .generation_id()
        .is_none());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn publication_preserves_late_spool_and_overflow_before_switching_pointer() {
    let root = test_root("publish-handoff");
    let legacy = Store::open(root.join(SQLITE_FILE).to_str().unwrap()).unwrap();
    drop(legacy);
    let (document, attachments) = fixture();
    let sealed = stage_portable_generation(
        &root,
        prepare_portable_restore(document, attachments).unwrap(),
        dt(23, 12),
    )
    .unwrap();
    write_late_spool(&root, false, "late-live", "Live late body");
    write_late_spool(&root, true, "late-failed", "Failed late body");
    let overflow = root.join("notif-overflow");
    std::fs::create_dir_all(&overflow).unwrap();
    std::fs::write(overflow.join("late-input.mark"), []).unwrap();

    let outcome = publish_portable_generation(&root, sealed.generation_id()).unwrap();
    assert_eq!(outcome.generation, sealed.generation_id());
    assert_eq!(outcome.late_items, 3);
    assert!(!outcome.handoff_pending);
    let active = resolve_active_data_dir(&root).unwrap();
    assert_eq!(active.generation_id(), Some(sealed.generation_id()));
    assert_eq!(
        json_file_count(&active.path().join(NOTIFICATION_SPOOL_DIR)),
        2
    );
    assert_eq!(
        json_file_count(
            &active
                .path()
                .join(NOTIFICATION_SPOOL_DIR)
                .join(FAILED_NOTIFICATION_DIR)
        ),
        2
    );
    assert_eq!(
        std::fs::read_dir(active.path().join("notif-overflow"))
            .unwrap()
            .count(),
        1
    );
    assert!(!root
        .join(RESTORE_HANDOFF_DIR)
        .join(sealed.generation_id())
        .exists());
    // Publication never deletes the old generation's intake. The pointer
    // makes it inert; keeping it is the safe side of a crash boundary.
    assert!(root
        .join(NOTIFICATION_SPOOL_DIR)
        .join("late-live.json")
        .exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_late_spool_fails_before_pointer_publication() {
    let root = test_root("publish-invalid-handoff");
    let legacy = Store::open(root.join(SQLITE_FILE).to_str().unwrap()).unwrap();
    drop(legacy);
    let (document, attachments) = fixture();
    let sealed = stage_portable_generation(
        &root,
        prepare_portable_restore(document, attachments).unwrap(),
        dt(23, 12),
    )
    .unwrap();
    let spool = root.join(NOTIFICATION_SPOOL_DIR);
    std::fs::create_dir_all(&spool).unwrap();
    std::fs::write(
        spool.join("bad.json"),
        br#"{"ts":1,"pkg":"com.example","title":"x","text":"y","extra":true}"#,
    )
    .unwrap();

    assert!(publish_portable_generation(&root, sealed.generation_id()).is_err());
    assert!(!root.join(POINTER_FILE).exists());
    assert!(root.join(STAGING_DIR).join(sealed.generation_id()).is_dir());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn startup_resumes_a_pointer_committed_handoff_idempotently() {
    let root = test_root("publish-resume-handoff");
    let legacy = Store::open(root.join(SQLITE_FILE).to_str().unwrap()).unwrap();
    drop(legacy);
    let (document, attachments) = fixture();
    let sealed = stage_portable_generation(
        &root,
        prepare_portable_restore(document, attachments).unwrap(),
        dt(23, 12),
    )
    .unwrap();
    write_late_spool(&root, false, "late-resume", "Resume body");
    assert_eq!(
        prepare_restore_handoff(&root, sealed.generation_id(), &root).unwrap(),
        1
    );
    let active = publish_profile_generation(sealed.generation()).unwrap();
    assert!(root
        .join(RESTORE_HANDOFF_DIR)
        .join(sealed.generation_id())
        .is_dir());

    assert_eq!(
        resume_portable_handoff(&root, sealed.generation_id()).unwrap(),
        1
    );
    assert_eq!(
        json_file_count(&active.path().join(NOTIFICATION_SPOOL_DIR)),
        2
    );
    assert_eq!(
        resume_portable_handoff(&root, sealed.generation_id()).unwrap(),
        0
    );
    std::fs::remove_dir_all(root).ok();
}

fn write_late_spool(root: &Path, failed: bool, stem: &str, body: &str) {
    let mut directory = root.join(NOTIFICATION_SPOOL_DIR);
    if failed {
        directory.push(FAILED_NOTIFICATION_DIR);
    }
    std::fs::create_dir_all(&directory).unwrap();
    let record = SpoolRecord {
        ts: 1_777_777_777_000,
        pkg: "com.example.late".into(),
        title: "Synthetic late".into(),
        text: body.into(),
    };
    std::fs::write(
        directory.join(format!("{stem}.json")),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}

fn json_file_count(directory: &Path) -> usize {
    std::fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .count()
}

fn walk_names(root: &Path) -> String {
    let mut names = String::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            names.push_str(&entry.file_name().to_string_lossy());
            if entry.path().is_dir() {
                pending.push(entry.path());
            }
        }
    }
    names
}

#[test]
fn assembler_failure_does_not_touch_existing_active_generation() {
    let root = test_root("failure");
    let (document, attachments) = fixture();
    let first = stage_portable_generation(
        &root,
        prepare_portable_restore(document, attachments).unwrap(),
        dt(23, 12),
    )
    .unwrap();
    let first_active = publish_profile_generation(first.generation()).unwrap();

    std::fs::remove_dir_all(root.join(STAGING_DIR)).unwrap();
    std::fs::write(root.join(STAGING_DIR), "blocks staging creation").unwrap();
    let (document, attachments) = fixture();
    let prepared = prepare_portable_restore(document, attachments).unwrap();
    assert!(stage_portable_generation(&root, prepared, dt(23, 12)).is_err());
    assert_eq!(resolve_active_data_dir(&root).unwrap(), first_active);
    assert_eq!(
        std::fs::read_dir(root.join(GENERATIONS_DIR))
            .unwrap()
            .count(),
        1
    );
    std::fs::remove_dir_all(root).ok();
}

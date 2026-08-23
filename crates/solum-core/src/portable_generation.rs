//! Assemble a validated v3 recovery into one sealed, still-inactive profile generation.
//!
//! Publication is deliberately separate. A caller gets a sealed handle only
//! after SQLite, WebView bootstrap data, attachments, notification spool and
//! the fail-closed native policy have all been written and re-read.

use std::collections::BTreeSet;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use chrono::{DateTime, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};
use crate::export::ImportPlan;
use crate::export_v3::{
    DocumentManifest, PortableChat, PortableLlmSettings, PortableRestoreIntents,
};
use crate::profile_generation::{
    begin_profile_generation, resume_sealed_profile_generation, seal_profile_generation,
    SealedProfileGeneration, SQLITE_FILE, STAGING_DIR,
};
use crate::restore_v3::{
    restore_portable_into_empty_store, PlatformRestoreBundle, PreparedPortableRestore,
    RestoredDocument,
};
use crate::store::{MergeCounts, Store};

pub const WEBVIEW_DIR: &str = "webview";
pub const WEBVIEW_BOOTSTRAP_FILE: &str = "bootstrap-v1.json";
pub const WEBVIEW_DOCUMENTS_DIR: &str = "documents";
pub const NOTIFICATION_POLICY_FILE: &str = "notif-policy.json";
pub const NOTIFICATION_SPOOL_DIR: &str = "notif-spool";
pub const FAILED_NOTIFICATION_DIR: &str = "failed";

const BOOTSTRAP_FORMAT: &str = "solum-webview-bootstrap";
const BOOTSTRAP_VERSION: u32 = 1;
const MAX_BOOTSTRAP_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SPOOL_FILE_BYTES: u64 = 64 * 1024;
const EMPTY_NOTIFICATION_POLICY: &str = "{\"allowed_packages\":[]}";
pub const WEBVIEW_DOCUMENT_CHUNK_BYTES: usize = 512 * 1024;

#[derive(Debug)]
pub struct SealedPortableGeneration {
    generation: SealedProfileGeneration,
    plan: ImportPlan,
    counts: MergeCounts,
}

impl SealedPortableGeneration {
    pub fn generation_id(&self) -> &str {
        self.generation.id()
    }

    pub fn generation(&self) -> &SealedProfileGeneration {
        &self.generation
    }

    pub fn plan(&self) -> &ImportPlan {
        &self.plan
    }

    pub fn counts(&self) -> &MergeCounts {
        &self.counts
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebViewBootstrap {
    pub format: String,
    pub version: u32,
    pub generation: String,
    pub chat: PortableChat,
    pub documents: Vec<BootstrapDocument>,
    pub llm: Option<PortableLlmSettings>,
    pub restore_intents: PortableRestoreIntents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapDocument {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub plaintext_size: u64,
    pub sha256: String,
    pub blob: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortableDocumentChunk {
    pub generation: String,
    pub document_id: String,
    pub offset: u64,
    pub total_size: u64,
    pub data_base64: String,
    pub done: bool,
}

#[derive(Debug)]
pub struct PortableGenerationReader {
    generation_path: PathBuf,
    bootstrap: WebViewBootstrap,
}

impl PortableGenerationReader {
    /// Open only a sealed, inactive staging generation. Active/final
    /// generations are mutable and are never attachment sources for hydrate.
    pub fn open(profile_root: &Path, generation: &str) -> Result<Self> {
        resume_sealed_profile_generation(profile_root, generation)?;
        let generation_path = profile_root.join(STAGING_DIR).join(generation);
        ensure_directory(&generation_path, "inactive portable generation")?;
        let bootstrap: WebViewBootstrap = read_bounded_json(
            &generation_path
                .join(WEBVIEW_DIR)
                .join(WEBVIEW_BOOTSTRAP_FILE),
            MAX_BOOTSTRAP_BYTES,
            "WebView bootstrap",
        )?;
        validate_bootstrap(&generation_path, generation, &bootstrap)?;
        Ok(Self {
            generation_path,
            bootstrap,
        })
    }

    pub fn bootstrap(&self) -> &WebViewBootstrap {
        &self.bootstrap
    }

    pub fn read_document_chunk(
        &self,
        document_id: &str,
        offset: u64,
    ) -> Result<PortableDocumentChunk> {
        let document = self
            .bootstrap
            .documents
            .iter()
            .find(|document| document.id == document_id)
            .ok_or_else(|| invalid("portable document is not part of this generation"))?;
        if offset > document.plaintext_size {
            return Err(invalid("portable document chunk offset exceeds its size"));
        }
        let path = self.generation_path.join(WEBVIEW_DIR).join(&document.blob);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| storage("WebView document", error))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != document.plaintext_size
        {
            return Err(invalid("portable document changed after reader validation"));
        }
        let remaining = document.plaintext_size - offset;
        let take = remaining.min(WEBVIEW_DOCUMENT_CHUNK_BYTES as u64) as usize;
        let mut bytes = vec![0u8; take];
        let mut file =
            std::fs::File::open(&path).map_err(|error| storage("WebView document", error))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|error| storage("WebView document", error))?;
        file.read_exact(&mut bytes)
            .map_err(|_| invalid("portable document changed while reading a chunk"))?;
        let next = offset + bytes.len() as u64;
        Ok(PortableDocumentChunk {
            generation: self.bootstrap.generation.clone(),
            document_id: document.id.clone(),
            offset,
            total_size: document.plaintext_size,
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            done: next == document.plaintext_size,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpoolRecord {
    ts: i64,
    pkg: String,
    title: String,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedSpool {
    relative_path: PathBuf,
    record: SpoolRecord,
}

/// Restore a fresh SQLite Store and persist every platform-owned handoff into
/// one sealed staging generation. This function never writes the active pointer.
pub fn stage_portable_generation(
    profile_root: &Path,
    prepared: PreparedPortableRestore,
    now: NaiveDateTime,
) -> Result<SealedPortableGeneration> {
    let staged = begin_profile_generation(profile_root)?;
    let sqlite_path = staged.path().join(SQLITE_FILE);
    let sqlite_text = sqlite_path
        .to_str()
        .ok_or_else(|| invalid("temporary generation SQLite path is not UTF-8"))?;
    let store = Store::open(sqlite_text)?;
    let (plan, counts, bundle) = restore_portable_into_empty_store(&store, prepared, now)?;

    let bootstrap = write_platform_bundle(staged.path(), staged.id(), &bundle)?;
    verify_platform_bundle(staged.path(), staged.id(), &bundle, &bootstrap)?;
    store.checkpoint_for_profile_generation()?;
    drop(store);

    let generation = seal_profile_generation(staged)?;
    Ok(SealedPortableGeneration {
        generation,
        plan,
        counts,
    })
}

fn write_platform_bundle(
    root: &Path,
    generation: &str,
    bundle: &PlatformRestoreBundle,
) -> Result<WebViewBootstrap> {
    write_new(
        &root.join(NOTIFICATION_POLICY_FILE),
        EMPTY_NOTIFICATION_POLICY.as_bytes(),
        "notification policy",
    )?;

    let documents_dir = root.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR);
    std::fs::create_dir_all(&documents_dir)
        .map_err(|error| storage("WebView document directory", error))?;
    let mut documents = Vec::with_capacity(bundle.documents.len());
    for document in &bundle.documents {
        let blob = document_blob_name(&document.manifest.id);
        write_new(
            &documents_dir.join(&blob),
            &document.plaintext,
            "WebView document",
        )?;
        documents.push(BootstrapDocument {
            id: document.manifest.id.clone(),
            name: document.manifest.name.clone(),
            media_type: document.manifest.media_type.clone(),
            plaintext_size: document.manifest.plaintext_size,
            sha256: document.manifest.sha256.clone(),
            blob: format!("{WEBVIEW_DOCUMENTS_DIR}/{blob}"),
        });
    }

    let bootstrap = WebViewBootstrap {
        format: BOOTSTRAP_FORMAT.into(),
        version: BOOTSTRAP_VERSION,
        generation: generation.into(),
        chat: bundle.chat.clone(),
        documents,
        llm: bundle.llm.clone(),
        restore_intents: bundle.restore_intents.clone(),
    };
    let encoded = serde_json::to_vec(&bootstrap)?;
    if encoded.len() as u64 > MAX_BOOTSTRAP_BYTES {
        return Err(invalid("WebView bootstrap exceeds its size limit"));
    }
    write_new(
        &root.join(WEBVIEW_DIR).join(WEBVIEW_BOOTSTRAP_FILE),
        &encoded,
        "WebView bootstrap",
    )?;

    for row in &bundle.pending_notifications {
        let received_at = DateTime::parse_from_rfc3339(&row.received_at)
            .map_err(|_| invalid("pending notification timestamp is invalid"))?;
        let record = SpoolRecord {
            ts: received_at.timestamp_millis(),
            pkg: row.package_name.clone(),
            title: row.title.clone(),
            text: row.body.clone(),
        };
        let mut directory = root.join(NOTIFICATION_SPOOL_DIR);
        if row.failed {
            directory.push(FAILED_NOTIFICATION_DIR);
        }
        let path = directory.join(format!("restore-{}.json", row.id));
        let mut encoded = serde_json::to_vec(&record)?;
        encoded.push(b'\n');
        if encoded.len() as u64 > MAX_SPOOL_FILE_BYTES {
            return Err(invalid(
                "pending notification spool entry exceeds its size limit",
            ));
        }
        write_new(&path, &encoded, "pending notification spool")?;
    }
    Ok(bootstrap)
}

fn verify_platform_bundle(
    root: &Path,
    generation: &str,
    bundle: &PlatformRestoreBundle,
    expected_bootstrap: &WebViewBootstrap,
) -> Result<()> {
    if std::fs::read(root.join(NOTIFICATION_POLICY_FILE))
        .map_err(|error| storage("notification policy", error))?
        != EMPTY_NOTIFICATION_POLICY.as_bytes()
    {
        return Err(invalid("staged notification policy is not fail-closed"));
    }

    let bootstrap_path = root.join(WEBVIEW_DIR).join(WEBVIEW_BOOTSTRAP_FILE);
    let actual_bootstrap: WebViewBootstrap =
        read_bounded_json(&bootstrap_path, MAX_BOOTSTRAP_BYTES, "WebView bootstrap")?;
    if actual_bootstrap != *expected_bootstrap
        || actual_bootstrap.format != BOOTSTRAP_FORMAT
        || actual_bootstrap.version != BOOTSTRAP_VERSION
        || actual_bootstrap.generation != generation
    {
        return Err(invalid(
            "staged WebView bootstrap does not match the restore plan",
        ));
    }
    verify_documents(root, &bundle.documents, &actual_bootstrap.documents)?;

    let expected_spool = expected_spool(bundle)?;
    let actual_paths = spool_paths(root)?;
    let expected_paths: BTreeSet<_> = expected_spool
        .iter()
        .map(|item| item.relative_path.clone())
        .collect();
    if actual_paths != expected_paths {
        return Err(invalid(
            "staged notification spool set does not match the restore plan",
        ));
    }
    for expected in expected_spool {
        let actual: SpoolRecord = read_bounded_json(
            &root.join(expected.relative_path),
            MAX_SPOOL_FILE_BYTES,
            "pending notification spool",
        )?;
        if actual != expected.record {
            return Err(invalid(
                "staged notification spool does not match the restore plan",
            ));
        }
    }
    Ok(())
}

fn verify_documents(
    root: &Path,
    restored: &[RestoredDocument],
    documents: &[BootstrapDocument],
) -> Result<()> {
    if restored.len() != documents.len() {
        return Err(invalid("staged WebView document set is incomplete"));
    }
    let directory = root.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR);
    ensure_directory(&directory, "WebView document directory")?;
    let actual_names: BTreeSet<String> = std::fs::read_dir(&directory)
        .map_err(|error| storage("WebView document directory", error))?
        .map(|entry| {
            let entry = entry.map_err(|error| storage("WebView document directory", error))?;
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| storage("WebView document", error))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid("staged WebView document is not a regular file"));
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("staged WebView document name is not UTF-8"))
        })
        .collect::<Result<_>>()?;
    let expected_names: BTreeSet<_> = documents
        .iter()
        .map(|document| {
            document
                .blob
                .strip_prefix(&format!("{WEBVIEW_DOCUMENTS_DIR}/"))
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    if actual_names != expected_names || expected_names.len() != documents.len() {
        return Err(invalid(
            "staged WebView document files do not match bootstrap",
        ));
    }

    for (source, document) in restored.iter().zip(documents) {
        let expected_blob = format!(
            "{WEBVIEW_DOCUMENTS_DIR}/{}",
            document_blob_name(&source.manifest.id)
        );
        if document.id != source.manifest.id
            || document.name != source.manifest.name
            || document.media_type != source.manifest.media_type
            || document.plaintext_size != source.manifest.plaintext_size
            || document.sha256 != source.manifest.sha256
            || document.blob != expected_blob
        {
            return Err(invalid("staged WebView document metadata is inconsistent"));
        }
        let digest = digest_file(
            &root.join(WEBVIEW_DIR).join(&document.blob),
            source.manifest.plaintext_size,
        )?;
        if digest != source.manifest.sha256 {
            return Err(invalid("staged WebView document digest does not match"));
        }
    }
    Ok(())
}

fn validate_bootstrap(root: &Path, generation: &str, bootstrap: &WebViewBootstrap) -> Result<()> {
    if bootstrap.format != BOOTSTRAP_FORMAT
        || bootstrap.version != BOOTSTRAP_VERSION
        || bootstrap.generation != generation
    {
        return Err(invalid("invalid or unsupported WebView bootstrap"));
    }
    crate::export_v3::validate_chat(&bootstrap.chat)?;
    if let Some(llm) = &bootstrap.llm {
        crate::export_v3::validate_llm(llm)?;
    }
    crate::export_v3::validate_restore_intents(&bootstrap.restore_intents)?;
    let manifests: Vec<_> = bootstrap
        .documents
        .iter()
        .map(|document| DocumentManifest {
            id: document.id.clone(),
            name: document.name.clone(),
            media_type: document.media_type.clone(),
            plaintext_size: document.plaintext_size,
            sha256: document.sha256.clone(),
        })
        .collect();
    crate::export_v3::validate_documents(&manifests)?;

    let directory = root.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR);
    ensure_directory(&directory, "WebView document directory")?;
    let mut expected_names = BTreeSet::new();
    for document in &bootstrap.documents {
        let name = document_blob_name(&document.id);
        if document.blob != format!("{WEBVIEW_DOCUMENTS_DIR}/{name}")
            || !expected_names.insert(name)
        {
            return Err(invalid("invalid WebView bootstrap document path"));
        }
        if digest_file(
            &root.join(WEBVIEW_DIR).join(&document.blob),
            document.plaintext_size,
        )? != document.sha256
        {
            return Err(invalid("WebView bootstrap document digest does not match"));
        }
    }
    let actual_names: BTreeSet<String> = std::fs::read_dir(&directory)
        .map_err(|error| storage("WebView document directory", error))?
        .map(|entry| {
            let entry = entry.map_err(|error| storage("WebView document directory", error))?;
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| storage("WebView document", error))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid(
                    "WebView document directory contains an invalid entry",
                ));
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("WebView document name is not UTF-8"))
        })
        .collect::<Result<_>>()?;
    if actual_names != expected_names {
        return Err(invalid("WebView document files do not match bootstrap"));
    }
    Ok(())
}

fn expected_spool(bundle: &PlatformRestoreBundle) -> Result<Vec<ExpectedSpool>> {
    bundle
        .pending_notifications
        .iter()
        .map(|row| {
            let received_at = DateTime::parse_from_rfc3339(&row.received_at)
                .map_err(|_| invalid("pending notification timestamp is invalid"))?;
            let mut relative_path = PathBuf::from(NOTIFICATION_SPOOL_DIR);
            if row.failed {
                relative_path.push(FAILED_NOTIFICATION_DIR);
            }
            relative_path.push(format!("restore-{}.json", row.id));
            Ok(ExpectedSpool {
                relative_path,
                record: SpoolRecord {
                    ts: received_at.timestamp_millis(),
                    pkg: row.package_name.clone(),
                    title: row.title.clone(),
                    text: row.body.clone(),
                },
            })
        })
        .collect()
}

fn spool_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    let spool = root.join(NOTIFICATION_SPOOL_DIR);
    if !spool.exists() {
        return Ok(paths);
    }
    ensure_directory(&spool, "notification spool directory")?;
    for entry in
        std::fs::read_dir(&spool).map_err(|error| storage("notification spool directory", error))?
    {
        let entry = entry.map_err(|error| storage("notification spool directory", error))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| storage("notification spool entry", error))?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            insert_spool_path(root, &path, &mut paths)?;
        } else if entry.file_name() == FAILED_NOTIFICATION_DIR
            && metadata.is_dir()
            && !metadata.file_type().is_symlink()
        {
            collect_failed_spool_paths(root, &path, &mut paths)?;
        } else {
            return Err(invalid("notification spool contains an invalid entry"));
        }
    }
    Ok(paths)
}

fn collect_failed_spool_paths(
    root: &Path,
    directory: &Path,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .map_err(|error| storage("notification spool directory", error))?
    {
        let entry = entry.map_err(|error| storage("notification spool directory", error))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| storage("notification spool entry", error))?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            insert_spool_path(root, &path, out)?;
        } else {
            return Err(invalid("notification spool contains an invalid entry"));
        }
    }
    Ok(())
}

fn insert_spool_path(root: &Path, path: &Path, out: &mut BTreeSet<PathBuf>) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("notification spool escaped generation root"))?
        .to_path_buf();
    if !out.insert(relative) {
        return Err(invalid("notification spool contains a duplicate entry"));
    }
    Ok(())
}

fn document_blob_name(id: &str) -> String {
    format!("{}.blob", lower_hex(&Sha256::digest(id.as_bytes())))
}

fn write_new(path: &Path, bytes: &[u8], label: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| storage(label, error))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| storage(label, error))?;
    file.write_all(bytes)
        .map_err(|error| storage(label, error))?;
    file.sync_all().map_err(|error| storage(label, error))
}

fn read_bounded_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<T> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| storage(label, error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(invalid(format!("staged {label} is invalid or too large")));
    }
    let bytes = std::fs::read(path).map_err(|error| storage(label, error))?;
    if bytes.len() as u64 > limit {
        return Err(invalid(format!("staged {label} exceeds its size limit")));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid(format!("staged {label} is invalid")))
}

fn digest_file(path: &Path, expected_size: u64) -> Result<String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| storage("WebView document", error))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != expected_size
        || expected_size > crate::attachment::MAX_ATTACHMENT_BYTES as u64
    {
        return Err(invalid("staged WebView document has an invalid size"));
    }
    let mut file = std::fs::File::open(path).map_err(|error| storage("WebView document", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read_total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| storage("WebView document", error))?;
        if read == 0 {
            break;
        }
        read_total = read_total
            .checked_add(read as u64)
            .ok_or_else(|| invalid("staged WebView document size overflow"))?;
        if read_total > expected_size {
            return Err(invalid("staged WebView document changed while verifying"));
        }
        hasher.update(&buffer[..read]);
    }
    if read_total != expected_size {
        return Err(invalid("staged WebView document changed while verifying"));
    }
    Ok(lower_hex(&hasher.finalize()))
}

fn ensure_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| storage(label, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(format!("staged {label} is invalid")));
    }
    Ok(())
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn storage(label: &str, error: std::io::Error) -> CoreError {
    CoreError::Storage(format!("could not stage {label}: {error}"))
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}

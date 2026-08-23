//! Strict opaque-cloud download and inactive portable-generation staging.
//!
//! No function in this module publishes an active profile pointer or advances
//! device generation. Server metadata is transport evidence only; the main
//! and attachment AEAD containers remain the authority for recovery data.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::account::AccountSession;
use crate::attachment::{self, AttachmentBinding};
use crate::backup::{self, BackupMetadata};
use crate::backup_upload::{attachment_object_id, BackupObjectKind, BackupObjectPlan, BackupPlan};
use crate::export_v3::{DocumentManifest, PORTABLE_EXPORT_VERSION};
use crate::restore_v3::{self, RestoredAttachment};

pub const MAX_CLOUD_OBJECT_BYTES: u64 = 68 * 1024 * 1024;
pub const MAX_CLOUD_BACKUP_BYTES: u64 = 700 * 1024 * 1024;
pub const MAX_CLOUD_OBJECTS: usize = 64;
pub const MAX_CATALOG_BACKUPS: usize = 100;

const MAX_CATALOG_BYTES: usize = 512 * 1024;
const HTTP_TIMEOUT_SECS: u64 = 30;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

type DownloadResult<T> = std::result::Result<T, BackupDownloadError>;

/// Log-safe failures. Variants never carry tokens, keys, plaintext, document
/// identifiers, response bodies, digests, or filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupDownloadError {
    #[error("backup catalog is invalid")]
    InvalidCatalog,
    #[error("requested backup is not present in the validated catalog")]
    BackupNotFound,
    #[error("unsupported backup protocol version {0}")]
    UnsupportedVersion(u32),
    #[error("backup download exceeds the protocol size limit")]
    SizeLimit,
    #[error("backup object transport integrity check failed")]
    TransportIntegrity,
    #[error("backup authentication or container validation failed")]
    Authentication,
    #[error("backup metadata does not match its authenticated container")]
    MetadataMismatch,
    #[error("backup attachment set is invalid")]
    AttachmentSet,
    #[error("backup payload cannot be prepared for portable restore")]
    InvalidPayload,
    #[error("inactive portable generation staging failed")]
    Staging,
    #[error("backup cloud authentication failed")]
    CloudAuthentication,
    #[error("backup cloud request failed")]
    Transport,
    #[error("backup cloud generation changed before takeover")]
    CloudConflict,
    #[error("backup cloud rejected the request with status {0}")]
    ServerStatus(u16),
    #[error("backup cloud response is invalid")]
    InvalidResponse,
}

impl BackupDownloadError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidCatalog => "invalid_catalog",
            Self::BackupNotFound => "backup_not_found",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::SizeLimit => "size_limit",
            Self::TransportIntegrity => "transport_integrity",
            Self::Authentication => "authentication",
            Self::MetadataMismatch => "metadata_mismatch",
            Self::AttachmentSet => "attachment_set",
            Self::InvalidPayload => "invalid_payload",
            Self::Staging => "staging",
            Self::CloudAuthentication => "cloud_authentication",
            Self::Transport => "transport",
            Self::CloudConflict => "cloud_conflict",
            Self::ServerStatus(_) => "server_status",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupCatalog {
    pub active_generation: u64,
    pub backups: Vec<CloudBackupVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudBackupVersion {
    pub backup_id: String,
    pub generation: u64,
    pub created_at: String,
    pub committed_at: String,
    pub protocol_version: u32,
    pub total_bytes: u64,
    pub object_count: usize,
    pub plan_etag: String,
    pub objects: Vec<BackupObjectPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StagedBackupOutcome {
    pub backup_id: String,
    pub generation: u64,
    pub generation_id: String,
    pub object_count: usize,
    pub attachment_count: usize,
    pub restored_rows: usize,
}

pub struct BackupStageRequest<'a> {
    pub profile_root: &'a Path,
    pub catalog: &'a BackupCatalog,
    pub backup_id: &'a str,
    pub user_id: &'a str,
    pub master_key: &'a [u8; 32],
    pub now: NaiveDateTime,
}

/// Narrow read seam used by all platforms and by deterministic security tests.
pub trait BackupCloudReader {
    fn list_backups(
        &self,
        session: &mut AccountSession,
        limit: usize,
    ) -> DownloadResult<BackupCatalog>;

    fn download_object(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        object: &BackupObjectPlan,
    ) -> DownloadResult<Vec<u8>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudTakeoverOutcome {
    pub active_generation: u64,
    pub pruned_pending: usize,
    pub replay: bool,
}

pub trait BackupCloudTakeover {
    fn advance_generation(
        &self,
        session: &mut AccountSession,
        expected_generation: u64,
        takeover_id: &str,
        writer_id: &str,
    ) -> DownloadResult<CloudTakeoverOutcome>;
}

/// Fetch and strictly validate complete-only cloud metadata.
pub fn fetch_backup_catalog(
    transport: &dyn BackupCloudReader,
    session: &mut AccountSession,
    limit: usize,
) -> DownloadResult<BackupCatalog> {
    if !(1..=MAX_CATALOG_BACKUPS).contains(&limit) {
        return Err(BackupDownloadError::InvalidCatalog);
    }
    let catalog = transport.list_backups(session, limit)?;
    validate_catalog(&catalog, limit)?;
    Ok(catalog)
}

/// Download, authenticate and assemble one still-inactive sealed generation.
/// The catalog is revalidated so callers cannot fabricate a partially checked
/// selection. No live Store or active pointer is opened or modified.
pub fn download_and_stage_backup(
    request: BackupStageRequest<'_>,
    transport: &dyn BackupCloudReader,
    session: &mut AccountSession,
) -> DownloadResult<StagedBackupOutcome> {
    validate_catalog(request.catalog, MAX_CATALOG_BACKUPS)?;
    if !valid_uuid(request.user_id)
        || request.user_id != request.user_id.to_ascii_lowercase()
        || session.user_id != request.user_id
    {
        return Err(BackupDownloadError::MetadataMismatch);
    }
    let selected = request
        .catalog
        .backups
        .iter()
        .find(|backup| backup.backup_id == request.backup_id)
        .ok_or(BackupDownloadError::BackupNotFound)?;
    if selected.protocol_version != backup::PROTOCOL_VERSION {
        return Err(BackupDownloadError::UnsupportedVersion(
            selected.protocol_version,
        ));
    }

    let main = selected
        .objects
        .iter()
        .find(|object| object.kind == BackupObjectKind::Main)
        .ok_or(BackupDownloadError::InvalidCatalog)?;
    let main_bytes = transport.download_object(session, &selected.backup_id, main)?;
    verify_downloaded_object(main, &main_bytes)?;
    let verified = backup::open_backup(&main_bytes, request.master_key)
        .map_err(|_| BackupDownloadError::Authentication)?;
    if verified.header.user_id != request.user_id
        || verified.header.device_generation != selected.generation
        || verified.header.backup_id != selected.backup_id
        || verified.header.created_at != selected.created_at
        || verified.header.protocol_version != selected.protocol_version
    {
        return Err(BackupDownloadError::MetadataMismatch);
    }
    if verified.header.payload_version != PORTABLE_EXPORT_VERSION
        || verified.import_plan.version != PORTABLE_EXPORT_VERSION
    {
        return Err(BackupDownloadError::InvalidPayload);
    }

    let manifests: Vec<DocumentManifest> = serde_json::from_value(
        verified
            .document
            .get("_portable")
            .and_then(|value| value.get("documents"))
            .cloned()
            .ok_or(BackupDownloadError::InvalidPayload)?,
    )
    .map_err(|_| BackupDownloadError::InvalidPayload)?;
    if manifests.len() > attachment::MAX_ATTACHMENTS {
        return Err(BackupDownloadError::AttachmentSet);
    }

    let metadata = BackupMetadata {
        user_id: request.user_id.to_string(),
        device_generation: selected.generation,
        backup_id: selected.backup_id.clone(),
        created_at: selected.created_at.clone(),
    };
    let attachment_objects: BTreeMap<_, _> = selected
        .objects
        .iter()
        .filter(|object| object.kind == BackupObjectKind::Attachment)
        .map(|object| (object.object_id.as_str(), object))
        .collect();
    let mut expected_ids = BTreeSet::new();
    let mut manifest_objects = Vec::with_capacity(manifests.len());
    for manifest in &manifests {
        let object_id = attachment_object_id(request.master_key, &selected.backup_id, &manifest.id)
            .map_err(|_| BackupDownloadError::AttachmentSet)?;
        if object_id == main.object_id || !expected_ids.insert(object_id.clone()) {
            return Err(BackupDownloadError::AttachmentSet);
        }
        let object = attachment_objects
            .get(object_id.as_str())
            .copied()
            .ok_or(BackupDownloadError::AttachmentSet)?;
        manifest_objects.push((manifest, object));
    }
    if expected_ids.len() != attachment_objects.len() {
        return Err(BackupDownloadError::AttachmentSet);
    }

    let mut restored = Vec::with_capacity(manifest_objects.len());
    for (manifest, object) in manifest_objects {
        let ciphertext = transport.download_object(session, &selected.backup_id, object)?;
        verify_downloaded_object(object, &ciphertext)?;
        let binding = AttachmentBinding::from_manifest(&metadata, manifest);
        let plaintext = attachment::open_attachment(&ciphertext, &binding, request.master_key)
            .map_err(|_| BackupDownloadError::Authentication)?;
        restored.push(RestoredAttachment {
            id: manifest.id.clone(),
            plaintext,
        });
    }

    let prepared = restore_v3::prepare_portable_restore(verified.document, restored)
        .map_err(|_| BackupDownloadError::InvalidPayload)?;
    let sealed = crate::portable_generation::stage_portable_generation(
        request.profile_root,
        prepared,
        request.now,
    )
    .map_err(|_| BackupDownloadError::Staging)?;
    Ok(StagedBackupOutcome {
        backup_id: selected.backup_id.clone(),
        generation: selected.generation,
        generation_id: sealed.generation_id().to_string(),
        object_count: selected.object_count,
        attachment_count: manifests.len(),
        restored_rows: sealed.counts().applied,
    })
}

fn validate_catalog(catalog: &BackupCatalog, max_backups: usize) -> DownloadResult<()> {
    if catalog.active_generation == 0
        || catalog.active_generation > MAX_SAFE_INTEGER
        || catalog.backups.len() > max_backups
        || catalog.backups.len() > MAX_CATALOG_BACKUPS
    {
        return Err(BackupDownloadError::InvalidCatalog);
    }
    let mut backup_ids = BTreeSet::new();
    let mut previous: Option<(DateTime<Utc>, &str)> = None;
    for backup in &catalog.backups {
        let committed =
            strict_timestamp(&backup.committed_at).ok_or(BackupDownloadError::InvalidCatalog)?;
        if let Some((previous_time, previous_id)) = previous {
            if committed > previous_time
                || (committed == previous_time && backup.backup_id.as_str() >= previous_id)
            {
                return Err(BackupDownloadError::InvalidCatalog);
            }
        }
        previous = Some((committed, &backup.backup_id));
        if !valid_uuid(&backup.backup_id)
            || backup.backup_id != backup.backup_id.to_ascii_lowercase()
            || !backup_ids.insert(backup.backup_id.as_str())
            || backup.generation == 0
            || backup.generation > MAX_SAFE_INTEGER
            || backup.generation > catalog.active_generation
            || strict_timestamp(&backup.created_at).is_none()
            || strict_timestamp(&backup.created_at).is_some_and(|created| created > committed)
            || backup.protocol_version == 0
            || backup.total_bytes == 0
            || backup.total_bytes > MAX_CLOUD_BACKUP_BYTES
            || backup.object_count != backup.objects.len()
            || backup.objects.is_empty()
            || backup.objects.len() > MAX_CLOUD_OBJECTS
            || !valid_lower_hex(&backup.plan_etag, 32)
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let mut object_ids = BTreeSet::new();
        let mut main_count = 0usize;
        let mut total = 0u64;
        let mut previous_object: Option<&str> = None;
        for object in &backup.objects {
            if let Some(previous_id) = previous_object {
                if object.object_id.as_str() <= previous_id {
                    return Err(BackupDownloadError::InvalidCatalog);
                }
            }
            previous_object = Some(&object.object_id);
            if !valid_uuid(&object.object_id)
                || object.object_id != object.object_id.to_ascii_lowercase()
                || !object_ids.insert(object.object_id.as_str())
                || object.bytes == 0
                || object.bytes > MAX_CLOUD_OBJECT_BYTES
                || !valid_lower_hex(&object.sha256, 32)
            {
                return Err(BackupDownloadError::InvalidCatalog);
            }
            if object.kind == BackupObjectKind::Main {
                main_count += 1;
                if object.bytes > backup::MAX_CONTAINER_BYTES as u64 {
                    return Err(BackupDownloadError::InvalidCatalog);
                }
            } else if object.bytes > attachment::MAX_CONTAINER_BYTES as u64 {
                return Err(BackupDownloadError::InvalidCatalog);
            }
            total = total
                .checked_add(object.bytes)
                .ok_or(BackupDownloadError::InvalidCatalog)?;
        }
        if main_count != 1 || total != backup.total_bytes {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let canonical = BackupPlan {
            backup_id: backup.backup_id.clone(),
            generation: backup.generation,
            created_at: backup.created_at.clone(),
            protocol_version: backup.protocol_version,
            objects: backup.objects.clone(),
        };
        let encoded =
            serde_json::to_vec(&canonical).map_err(|_| BackupDownloadError::InvalidCatalog)?;
        if lower_hex(&Sha256::digest(encoded)) != backup.plan_etag {
            return Err(BackupDownloadError::InvalidCatalog);
        }
    }
    Ok(())
}

fn verify_downloaded_object(object: &BackupObjectPlan, bytes: &[u8]) -> DownloadResult<()> {
    if bytes.len() as u64 != object.bytes || lower_hex(&Sha256::digest(bytes)) != object.sha256 {
        return Err(BackupDownloadError::TransportIntegrity);
    }
    Ok(())
}

fn strict_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&Utc);
    (parsed.to_rfc3339_opts(SecondsFormat::Millis, true) == value).then_some(parsed)
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
        && value != "00000000-0000-0000-0000-000000000000"
}

fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// Production reader with bounded response bodies and the same one-refresh /
/// one-transport-retry account semantics as the uploader.
pub struct HttpBackupCloudReader {
    agent: ureq::Agent,
}

impl Default for HttpBackupCloudReader {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpBackupCloudReader {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build(),
        }
    }

    fn request<F>(&self, session: &mut AccountSession, send: F) -> DownloadResult<ureq::Response>
    where
        F: Fn(&AccountSession) -> std::result::Result<ureq::Response, HttpFailure>,
    {
        let mut refreshed = false;
        let mut retried_transport = false;
        loop {
            match send(session) {
                Ok(response) => return Ok(response),
                Err(HttpFailure::Status(401)) if !refreshed => {
                    *session = crate::account::refresh_and_save(session)
                        .map_err(|_| BackupDownloadError::CloudAuthentication)?;
                    refreshed = true;
                }
                Err(HttpFailure::Transport) if !retried_transport => retried_transport = true,
                Err(HttpFailure::Transport) => return Err(BackupDownloadError::Transport),
                Err(HttpFailure::Status(401)) => {
                    return Err(BackupDownloadError::CloudAuthentication)
                }
                Err(HttpFailure::Status(404)) => return Err(BackupDownloadError::BackupNotFound),
                Err(HttpFailure::Status(409)) => return Err(BackupDownloadError::CloudConflict),
                Err(HttpFailure::Status(status)) => {
                    return Err(BackupDownloadError::ServerStatus(status))
                }
            }
        }
    }

    fn url(session: &AccountSession, path: &str) -> String {
        format!("{}{}", session.server_url, path)
    }
}

#[derive(Serialize)]
struct TakeoverRequest<'a> {
    expected_generation: u64,
    takeover_id: &'a str,
    writer_id: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TakeoverResponse {
    active_generation: u64,
    pruned_pending: usize,
    replay: bool,
}

impl BackupCloudTakeover for HttpBackupCloudReader {
    fn advance_generation(
        &self,
        session: &mut AccountSession,
        expected_generation: u64,
        takeover_id: &str,
        writer_id: &str,
    ) -> DownloadResult<CloudTakeoverOutcome> {
        if expected_generation == 0
            || expected_generation >= MAX_SAFE_INTEGER
            || !valid_uuid(takeover_id)
            || !valid_uuid(writer_id)
            || writer_id != writer_id.to_ascii_lowercase()
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let request = TakeoverRequest {
            expected_generation,
            takeover_id,
            writer_id,
        };
        let response = self.request(session, |current| {
            self.agent
                .post(&Self::url(current, "/v1/backups/generation"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .send_json(&request)
                .map_err(HttpFailure::from)
        })?;
        if response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            != Some("application/json")
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        let bytes = read_bounded(response.into_reader(), 16 * 1024)?;
        let parsed: TakeoverResponse =
            serde_json::from_slice(&bytes).map_err(|_| BackupDownloadError::InvalidResponse)?;
        if parsed.active_generation != expected_generation + 1
            || parsed.pruned_pending > MAX_CATALOG_BACKUPS
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        Ok(CloudTakeoverOutcome {
            active_generation: parsed.active_generation,
            pruned_pending: parsed.pruned_pending,
            replay: parsed.replay,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum HttpFailure {
    Status(u16),
    Transport,
}

impl From<ureq::Error> for HttpFailure {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Status(status, _) => Self::Status(status),
            ureq::Error::Transport(_) => Self::Transport,
        }
    }
}

impl BackupCloudReader for HttpBackupCloudReader {
    fn list_backups(
        &self,
        session: &mut AccountSession,
        limit: usize,
    ) -> DownloadResult<BackupCatalog> {
        if !(1..=MAX_CATALOG_BACKUPS).contains(&limit) {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let path = format!("/v1/backups?limit={limit}");
        let response = self.request(session, |current| {
            self.agent
                .get(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(HttpFailure::from)
        })?;
        if response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            != Some("application/json")
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        let bytes = read_bounded(response.into_reader(), MAX_CATALOG_BYTES)?;
        serde_json::from_slice(&bytes).map_err(|_| BackupDownloadError::InvalidResponse)
    }

    fn download_object(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        object: &BackupObjectPlan,
    ) -> DownloadResult<Vec<u8>> {
        if !valid_uuid(backup_id)
            || !valid_uuid(&object.object_id)
            || object.bytes == 0
            || object.bytes > MAX_CLOUD_OBJECT_BYTES
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let path = format!("/v1/backups/{backup_id}/objects/{}", object.object_id);
        let response = self.request(session, |current| {
            self.agent
                .get(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(HttpFailure::from)
        })?;
        let content_type = response
            .header("Content-Type")
            .and_then(|value| value.split(';').next());
        let content_length = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok());
        let etag = response.header("ETag");
        let expected_etag = format!("\"{}\"", object.sha256);
        if content_type != Some("application/octet-stream")
            || response.header("Cache-Control") != Some("no-store")
            || content_length != Some(object.bytes)
            || etag != Some(expected_etag.as_str())
        {
            return Err(BackupDownloadError::InvalidResponse);
        }
        read_bounded(response.into_reader(), object.bytes as usize)
    }
}

fn read_bounded(mut reader: impl Read, max: usize) -> DownloadResult<Vec<u8>> {
    let limit = u64::try_from(max)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or(BackupDownloadError::SizeLimit)?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| BackupDownloadError::Transport)?;
    if bytes.len() > max {
        return Err(BackupDownloadError::SizeLimit);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use chrono::NaiveDate;
    use serde_json::{json, Value};

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
        let generation = crate::portable_generation::PortableGenerationReader::open(
            &root,
            &outcome.generation_id,
        )
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
}

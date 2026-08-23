//! Encrypted-only staging and opaque cloud upload for portable backup v1.
//!
//! This coordinator deliberately has no Store-writing API. It freezes an
//! already recovery-plannable `solum-export` v3 document, seals the main
//! object and every manifest-bound attachment locally, then uploads the same
//! ciphertext bytes on every retry before committing the remote version.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use hkdf::Hkdf;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::account::AccountSession;
use crate::attachment::{self, AttachmentBinding};
use crate::backup::{self, BackupMetadata};
use crate::export_v3::{self, DocumentManifest};

pub const DEVICE_STATE_FILE: &str = "solum-backup-device.json";
pub const UPLOAD_STAGING_DIR: &str = "backup-upload-staging";
pub const MAX_IPC_CHUNK_BYTES: usize = 512 * 1024;

const DEVICE_STATE_FORMAT: &str = "solum-backup-device";
const DEVICE_STATE_VERSION: u32 = 1;
const HTTP_TIMEOUT_SECS: u64 = 30;
const OBJECT_ID_KDF_SALT: &[u8] = b"solum-backup-object-id:hkdf-salt:v1";
const OBJECT_ID_KDF_INFO: &[u8] = b"solum-backup-object-id:info:v1";

type UploadResult<T> = std::result::Result<T, BackupUploadError>;

/// Coarse, log-safe failures. No variant carries plaintext, keys, tokens,
/// document identifiers, response bodies, or filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupUploadError {
    #[error("backup device generation state is invalid")]
    InvalidDeviceState,
    #[error("backup device generation is stale")]
    StaleGeneration,
    #[error("backup cloud reported a generation or idempotency conflict")]
    CloudConflict,
    #[error("backup snapshot is not a valid portable export")]
    InvalidSnapshot,
    #[error("backup attachment stream is invalid")]
    InvalidAttachmentStream,
    #[error("backup attachment exceeds the protocol size limit")]
    SizeLimit,
    #[error("backup encryption failed")]
    Encryption,
    #[error("encrypted backup staging failed")]
    Staging,
    #[error("backup cloud authentication failed")]
    Authentication,
    #[error("backup cloud request failed")]
    Transport,
    #[error("backup cloud rejected the request with status {0}")]
    ServerStatus(u16),
    #[error("backup cloud response is invalid")]
    InvalidResponse,
    #[error("operating-system random source is unavailable")]
    RandomSource,
}

impl BackupUploadError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidDeviceState => "invalid_device_state",
            Self::StaleGeneration => "stale_generation",
            Self::CloudConflict => "cloud_conflict",
            Self::InvalidSnapshot => "invalid_snapshot",
            Self::InvalidAttachmentStream => "invalid_attachment_stream",
            Self::SizeLimit => "size_limit",
            Self::Encryption => "encryption",
            Self::Staging => "staging",
            Self::Authentication => "authentication",
            Self::Transport => "transport",
            Self::ServerStatus(_) => "server_status",
            Self::InvalidResponse => "invalid_response",
            Self::RandomSource => "random_source",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupDeviceState {
    format: String,
    version: u32,
    user_id: String,
    device_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupObjectKind {
    Main,
    Attachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupObjectPlan {
    pub object_id: String,
    pub kind: BackupObjectKind,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPlan {
    pub backup_id: String,
    pub generation: u64,
    pub created_at: String,
    pub protocol_version: u32,
    pub objects: Vec<BackupObjectPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentExpectation {
    pub id: String,
    pub plaintext_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupUploadBegin {
    pub session_id: String,
    pub backup_id: String,
    pub generation: u64,
    pub created_at: String,
    pub attachments: Vec<AttachmentExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupUploadOutcome {
    pub backup_id: String,
    pub generation: u64,
    pub object_count: usize,
}

struct StagedObject {
    plan: BackupObjectPlan,
    path: PathBuf,
}

/// A single frozen backup attempt. Deliberately does not implement `Debug` or
/// `Clone`: it temporarily owns the master key and at most one plaintext
/// attachment buffer.
pub struct BackupUploadSession {
    session_id: String,
    metadata: BackupMetadata,
    master_key: [u8; 32],
    manifests: Vec<DocumentManifest>,
    next_attachment: usize,
    attachment_plaintext: Vec<u8>,
    staged: Vec<StagedObject>,
    staging_dir: PathBuf,
}

/// Narrow transport seam: fake implementations prove ordering and
/// pre-commit validation without a server; the production implementation
/// below performs bounded retry and access-token rotation.
pub trait BackupCloudTransport {
    fn active_generation(&self, session: &mut AccountSession) -> UploadResult<u64>;
    fn create_plan(&self, session: &mut AccountSession, plan: &BackupPlan) -> UploadResult<()>;
    fn put_object(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        object: &BackupObjectPlan,
        ciphertext: &[u8],
    ) -> UploadResult<()>;
    fn commit(
        &self,
        session: &mut AccountSession,
        plan: &BackupPlan,
    ) -> UploadResult<BackupUploadOutcome>;
}

/// Load local single-writer authority and compare it to the server. A missing
/// file can initialize generation 1 only; a higher remote generation is never
/// silently adopted by an old or reinstalled device.
pub fn load_or_initialize_device_generation(
    profile_root: &Path,
    user_id: &str,
    transport: &dyn BackupCloudTransport,
    session: &mut AccountSession,
) -> UploadResult<u64> {
    if !valid_uuid(user_id) || user_id != user_id.to_ascii_lowercase() {
        return Err(BackupUploadError::InvalidDeviceState);
    }
    let path = profile_root.join(DEVICE_STATE_FILE);
    let local = match std::fs::read_to_string(&path) {
        Ok(raw) => Some(
            serde_json::from_str::<BackupDeviceState>(&raw)
                .map_err(|_| BackupUploadError::InvalidDeviceState)?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(BackupUploadError::InvalidDeviceState),
    };
    if let Some(state) = local.as_ref() {
        if state.format != DEVICE_STATE_FORMAT
            || state.version != DEVICE_STATE_VERSION
            || state.user_id != user_id
            || state.device_generation == 0
        {
            return Err(BackupUploadError::InvalidDeviceState);
        }
    }

    let remote = transport.active_generation(session)?;
    if remote == 0 {
        return Err(BackupUploadError::InvalidResponse);
    }
    match local {
        Some(state) if state.device_generation == remote => Ok(remote),
        Some(_) => Err(BackupUploadError::StaleGeneration),
        None if remote == 1 => {
            let state = BackupDeviceState {
                format: DEVICE_STATE_FORMAT.to_string(),
                version: DEVICE_STATE_VERSION,
                user_id: user_id.to_string(),
                device_generation: 1,
            };
            let encoded =
                serde_json::to_string(&state).map_err(|_| BackupUploadError::InvalidDeviceState)?;
            crate::fsatomic::write_atomic_durable(&path, &encoded)
                .map_err(|_| BackupUploadError::InvalidDeviceState)?;
            Ok(1)
        }
        None => Err(BackupUploadError::StaleGeneration),
    }
}

impl BackupUploadSession {
    pub fn begin(
        profile_root: &Path,
        portable_json: &[u8],
        user_id: &str,
        device_generation: u64,
        created_at: &str,
        master_key: [u8; 32],
    ) -> UploadResult<Self> {
        let document: Value = serde_json::from_slice(portable_json)
            .map_err(|_| BackupUploadError::InvalidSnapshot)?;
        crate::export::plan_import(&document).map_err(|_| BackupUploadError::InvalidSnapshot)?;
        let manifests = portable_manifests(&document)?;

        let backup_id = random_uuid_v4()?;
        let metadata = BackupMetadata {
            user_id: user_id.to_string(),
            device_generation,
            backup_id,
            created_at: created_at.to_string(),
        };
        let bindings: Vec<_> = manifests
            .iter()
            .map(|manifest| AttachmentBinding::from_manifest(&metadata, manifest))
            .collect();
        attachment::validate_attachment_set(&bindings).map_err(map_attachment_error)?;

        let staging_root = profile_root.join(UPLOAD_STAGING_DIR);
        clean_encrypted_staging(&staging_root)?;
        std::fs::create_dir_all(&staging_root).map_err(|_| BackupUploadError::Staging)?;
        let session_id = random_hex(16)?;
        let staging_dir = staging_root.join(&session_id);
        std::fs::create_dir(&staging_dir).map_err(|_| BackupUploadError::Staging)?;

        let result = (|| {
            let main = backup::create_backup(&document, &metadata, &master_key)
                .map_err(|_| BackupUploadError::Encryption)?;
            let object_id = random_uuid_v4()?;
            let staged_main =
                stage_ciphertext(&staging_dir, object_id, BackupObjectKind::Main, &main)?;
            Ok(Self {
                session_id,
                metadata,
                master_key,
                manifests,
                next_attachment: 0,
                attachment_plaintext: Vec::new(),
                staged: vec![staged_main],
                staging_dir: staging_dir.clone(),
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging_dir);
        }
        result
    }

    pub fn begin_info(&self) -> BackupUploadBegin {
        BackupUploadBegin {
            session_id: self.session_id.clone(),
            backup_id: self.metadata.backup_id.clone(),
            generation: self.metadata.device_generation,
            created_at: self.metadata.created_at.clone(),
            attachments: self
                .manifests
                .iter()
                .map(|manifest| AttachmentExpectation {
                    id: manifest.id.clone(),
                    plaintext_size: manifest.plaintext_size,
                })
                .collect(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn append_attachment_chunk(
        &mut self,
        attachment_id: &str,
        plaintext_offset: u64,
        chunk: &[u8],
    ) -> UploadResult<()> {
        if chunk.len() > MAX_IPC_CHUNK_BYTES {
            return Err(BackupUploadError::SizeLimit);
        }
        let manifest = self
            .manifests
            .get(self.next_attachment)
            .ok_or(BackupUploadError::InvalidAttachmentStream)?;
        if attachment_id != manifest.id
            || plaintext_offset != self.attachment_plaintext.len() as u64
        {
            return Err(BackupUploadError::InvalidAttachmentStream);
        }
        let next_len = self
            .attachment_plaintext
            .len()
            .checked_add(chunk.len())
            .ok_or(BackupUploadError::SizeLimit)?;
        if next_len as u64 > manifest.plaintext_size || next_len > attachment::MAX_ATTACHMENT_BYTES
        {
            return Err(BackupUploadError::SizeLimit);
        }
        self.attachment_plaintext.extend_from_slice(chunk);
        Ok(())
    }

    pub fn finish_attachment(&mut self, attachment_id: &str) -> UploadResult<()> {
        let manifest = self
            .manifests
            .get(self.next_attachment)
            .ok_or(BackupUploadError::InvalidAttachmentStream)?;
        if attachment_id != manifest.id
            || self.attachment_plaintext.len() as u64 != manifest.plaintext_size
        {
            return Err(BackupUploadError::InvalidAttachmentStream);
        }
        let binding = AttachmentBinding::from_manifest(&self.metadata, manifest);
        let ciphertext =
            attachment::seal_attachment(&self.attachment_plaintext, &binding, &self.master_key)
                .map_err(map_attachment_error);
        self.attachment_plaintext.fill(0);
        self.attachment_plaintext.clear();
        let ciphertext = ciphertext?;
        let object_id =
            attachment_object_id(&self.master_key, &self.metadata.backup_id, &manifest.id)?;
        if self
            .staged
            .iter()
            .any(|item| item.plan.object_id == object_id)
        {
            return Err(BackupUploadError::Encryption);
        }
        let staged = stage_ciphertext(
            &self.staging_dir,
            object_id,
            BackupObjectKind::Attachment,
            &ciphertext,
        )?;
        self.staged.push(staged);
        self.next_attachment += 1;
        Ok(())
    }

    pub fn plan(&self) -> UploadResult<BackupPlan> {
        if self.next_attachment != self.manifests.len() || !self.attachment_plaintext.is_empty() {
            return Err(BackupUploadError::InvalidAttachmentStream);
        }
        Ok(BackupPlan {
            backup_id: self.metadata.backup_id.clone(),
            generation: self.metadata.device_generation,
            created_at: self.metadata.created_at.clone(),
            protocol_version: backup::PROTOCOL_VERSION,
            objects: self.staged.iter().map(|item| item.plan.clone()).collect(),
        })
    }

    /// Verify every staged byte against the frozen plan before the first
    /// request, then upload main-first / manifest-order and commit last.
    pub fn upload(
        &self,
        transport: &dyn BackupCloudTransport,
        account: &mut AccountSession,
    ) -> UploadResult<BackupUploadOutcome> {
        let plan = self.plan()?;
        for staged in &self.staged {
            verify_staged_object(staged)?;
        }

        transport.create_plan(account, &plan)?;
        for staged in &self.staged {
            // Only one opaque object is resident at a time. Recheck after the
            // pre-plan pass so an unexpected local modification cannot turn a
            // declared digest into different uploaded bytes.
            let mut ciphertext = Vec::new();
            File::open(&staged.path)
                .and_then(|mut file| file.read_to_end(&mut ciphertext))
                .map_err(|_| BackupUploadError::Staging)?;
            if ciphertext.len() as u64 != staged.plan.bytes
                || sha256_hex(&ciphertext) != staged.plan.sha256
            {
                return Err(BackupUploadError::Staging);
            }
            transport.put_object(account, &plan.backup_id, &staged.plan, &ciphertext)?;
        }
        let outcome = transport.commit(account, &plan)?;
        if outcome.backup_id != plan.backup_id
            || outcome.generation != plan.generation
            || outcome.object_count != plan.objects.len()
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(outcome)
    }
}

fn verify_staged_object(staged: &StagedObject) -> UploadResult<()> {
    let mut file = File::open(&staged.path).map_err(|_| BackupUploadError::Staging)?;
    if file
        .metadata()
        .map_err(|_| BackupUploadError::Staging)?
        .len()
        != staged.plan.bytes
    {
        return Err(BackupUploadError::Staging);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| BackupUploadError::Staging)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if hex(&digest.finalize()) != staged.plan.sha256 {
        return Err(BackupUploadError::Staging);
    }
    Ok(())
}

impl Drop for BackupUploadSession {
    fn drop(&mut self) {
        self.master_key.fill(0);
        self.attachment_plaintext.fill(0);
        let _ = std::fs::remove_dir_all(&self.staging_dir);
    }
}

fn portable_manifests(document: &Value) -> UploadResult<Vec<DocumentManifest>> {
    export_v3::validate_portable_document(document)
        .map_err(|_| BackupUploadError::InvalidSnapshot)?;
    serde_json::from_value(
        document
            .get("_portable")
            .and_then(|value| value.get("documents"))
            .cloned()
            .ok_or(BackupUploadError::InvalidSnapshot)?,
    )
    .map_err(|_| BackupUploadError::InvalidSnapshot)
}

fn stage_ciphertext(
    staging_dir: &Path,
    object_id: String,
    kind: BackupObjectKind,
    ciphertext: &[u8],
) -> UploadResult<StagedObject> {
    if ciphertext.is_empty() {
        return Err(BackupUploadError::Encryption);
    }
    let path = staging_dir.join(&object_id);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| BackupUploadError::Staging)?;
    file.write_all(ciphertext)
        .and_then(|_| file.sync_all())
        .map_err(|_| BackupUploadError::Staging)?;
    crate::fsatomic::sync_directory(staging_dir).map_err(|_| BackupUploadError::Staging)?;
    Ok(StagedObject {
        plan: BackupObjectPlan {
            object_id,
            kind,
            bytes: ciphertext.len() as u64,
            sha256: sha256_hex(ciphertext),
        },
        path,
    })
}

fn clean_encrypted_staging(root: &Path) -> UploadResult<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(BackupUploadError::Staging),
    };
    for entry in entries {
        let entry = entry.map_err(|_| BackupUploadError::Staging)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|_| BackupUploadError::Staging)?;
        if name.len() == 32
            && name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && metadata.file_type().is_dir()
            && !metadata.file_type().is_symlink()
        {
            std::fs::remove_dir_all(entry.path()).map_err(|_| BackupUploadError::Staging)?;
        }
    }
    Ok(())
}

fn map_attachment_error(error: attachment::AttachmentError) -> BackupUploadError {
    match error {
        attachment::AttachmentError::SizeLimit => BackupUploadError::SizeLimit,
        attachment::AttachmentError::RandomSource => BackupUploadError::RandomSource,
        _ => BackupUploadError::Encryption,
    }
}

fn random_hex(bytes: usize) -> UploadResult<String> {
    let mut random = vec![0u8; bytes];
    getrandom::getrandom(&mut random).map_err(|_| BackupUploadError::RandomSource)?;
    Ok(hex(&random))
}

fn random_uuid_v4() -> UploadResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| BackupUploadError::RandomSource)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
}

/// Derive the server-visible UUID for one manifest attachment without exposing
/// its local id. The random backup id makes the result unlinkable across
/// backup versions; the master key keeps the mapping opaque to the server.
pub fn attachment_object_id(
    master_key: &[u8; 32],
    backup_id: &str,
    attachment_id: &str,
) -> UploadResult<String> {
    if !valid_uuid(backup_id)
        || backup_id != backup_id.to_ascii_lowercase()
        || !valid_attachment_id(attachment_id)
    {
        return Err(BackupUploadError::InvalidSnapshot);
    }
    let backup_len = u32::try_from(backup_id.len()).map_err(|_| BackupUploadError::Encryption)?;
    let attachment_len =
        u32::try_from(attachment_id.len()).map_err(|_| BackupUploadError::Encryption)?;
    let mut info =
        Vec::with_capacity(OBJECT_ID_KDF_INFO.len() + 8 + backup_id.len() + attachment_id.len());
    info.extend_from_slice(OBJECT_ID_KDF_INFO);
    info.extend_from_slice(&backup_len.to_be_bytes());
    info.extend_from_slice(backup_id.as_bytes());
    info.extend_from_slice(&attachment_len.to_be_bytes());
    info.extend_from_slice(attachment_id.as_bytes());

    let hkdf = Hkdf::<Sha256>::new(Some(OBJECT_ID_KDF_SALT), master_key);
    let mut bytes = [0u8; 16];
    hkdf.expand(&info, &mut bytes)
        .map_err(|_| BackupUploadError::Encryption)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
}

fn valid_attachment_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._:".contains(&byte))
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
        && value != "00000000-0000-0000-0000-000000000000"
}

fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_server_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// Production HTTP adapter. A 401 rotates and durably saves the account once;
/// one transport failure is retried. Status responses are never read into an
/// error message, so the server cannot reflect sensitive content into logs.
pub struct HttpBackupCloudTransport {
    agent: ureq::Agent,
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

impl Default for HttpBackupCloudTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpBackupCloudTransport {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build(),
        }
    }

    fn request_json<T, F>(&self, session: &mut AccountSession, send: F) -> UploadResult<T>
    where
        T: DeserializeOwned,
        F: Fn(&AccountSession) -> std::result::Result<ureq::Response, HttpFailure>,
    {
        let response = self.request(session, send)?;
        response
            .into_json::<T>()
            .map_err(|_| BackupUploadError::InvalidResponse)
    }

    fn request<F>(&self, session: &mut AccountSession, send: F) -> UploadResult<ureq::Response>
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
                        .map_err(|_| BackupUploadError::Authentication)?;
                    refreshed = true;
                }
                Err(HttpFailure::Transport) if !retried_transport => {
                    retried_transport = true;
                }
                Err(HttpFailure::Transport) => return Err(BackupUploadError::Transport),
                Err(HttpFailure::Status(401)) => return Err(BackupUploadError::Authentication),
                Err(HttpFailure::Status(status)) => {
                    return Err(match status {
                        409 => BackupUploadError::CloudConflict,
                        413 => BackupUploadError::SizeLimit,
                        _ => BackupUploadError::ServerStatus(status),
                    })
                }
            }
        }
    }

    fn url(session: &AccountSession, path: &str) -> String {
        format!("{}{}", session.server_url, path)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationResponse {
    active_generation: u64,
    updated_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateResponse {
    backup_id: String,
    generation: u64,
    status: String,
    plan_etag: String,
    total_bytes: u64,
    object_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutResponse {
    backup_id: String,
    object_id: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitResponse {
    backup_id: String,
    generation: u64,
    created_at: String,
    committed_at: String,
    protocol_version: u32,
    total_bytes: u64,
    object_count: usize,
    plan_etag: String,
    objects: Vec<BackupObjectPlan>,
    status: String,
    pruned_versions: Vec<String>,
}

impl BackupCloudTransport for HttpBackupCloudTransport {
    fn active_generation(&self, session: &mut AccountSession) -> UploadResult<u64> {
        let response: GenerationResponse = self.request_json(session, |current| {
            self.agent
                .get(&Self::url(current, "/v1/backups/generation"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(HttpFailure::from)
        })?;
        if response.active_generation == 0 || !valid_server_timestamp(&response.updated_at) {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(response.active_generation)
    }

    fn create_plan(&self, session: &mut AccountSession, plan: &BackupPlan) -> UploadResult<()> {
        let response: CreateResponse = self.request_json(session, |current| {
            self.agent
                .post(&Self::url(current, "/v1/backups"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .send_json(plan)
                .map_err(HttpFailure::from)
        })?;
        let total_bytes = plan.objects.iter().try_fold(0u64, |sum, object| {
            sum.checked_add(object.bytes)
                .ok_or(BackupUploadError::SizeLimit)
        })?;
        if response.backup_id != plan.backup_id
            || response.generation != plan.generation
            || !matches!(response.status.as_str(), "pending" | "complete")
            || !valid_lower_hex(&response.plan_etag, 32)
            || response.total_bytes != total_bytes
            || response.object_count != plan.objects.len()
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(())
    }

    fn put_object(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        object: &BackupObjectPlan,
        ciphertext: &[u8],
    ) -> UploadResult<()> {
        if ciphertext.len() as u64 != object.bytes || sha256_hex(ciphertext) != object.sha256 {
            return Err(BackupUploadError::Staging);
        }
        let path = format!("/v1/backups/{backup_id}/objects/{}", object.object_id);
        let response: PutResponse = self.request_json(session, |current| {
            self.agent
                .put(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("Content-Type", "application/octet-stream")
                .set("Content-Length", &ciphertext.len().to_string())
                .send_bytes(ciphertext)
                .map_err(HttpFailure::from)
        })?;
        if response.backup_id != backup_id
            || response.object_id != object.object_id
            || response.sha256 != object.sha256
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(())
    }

    fn commit(
        &self,
        session: &mut AccountSession,
        plan: &BackupPlan,
    ) -> UploadResult<BackupUploadOutcome> {
        let path = format!("/v1/backups/{}/commit", plan.backup_id);
        let response: CommitResponse = self.request_json(session, |current| {
            self.agent
                .post(&Self::url(current, &path))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("Content-Length", "0")
                .send_bytes(&[])
                .map_err(HttpFailure::from)
        })?;
        let total_bytes = plan.objects.iter().map(|object| object.bytes).sum::<u64>();
        if response.backup_id != plan.backup_id
            || response.generation != plan.generation
            || response.created_at != plan.created_at
            || !valid_server_timestamp(&response.committed_at)
            || response.protocol_version != plan.protocol_version
            || response.total_bytes != total_bytes
            || response.object_count != plan.objects.len()
            || !valid_lower_hex(&response.plan_etag, 32)
            || !response.objects.is_empty()
            || response.status != "complete"
            || response
                .pruned_versions
                .iter()
                .any(|value| !valid_uuid(value) || value != &value.to_ascii_lowercase())
        {
            return Err(BackupUploadError::InvalidResponse);
        }
        Ok(BackupUploadOutcome {
            backup_id: response.backup_id,
            generation: response.generation,
            object_count: response.object_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::NaiveDate;
    use serde_json::json;

    use super::*;
    use crate::export_v3::{ExternalRestoreIntents, PlatformPortableData, PortableChat};
    use crate::store::Store;

    const USER_ID: &str = "11111111-1111-4111-8111-111111111111";
    const CREATED_AT: &str = "2026-01-02T03:04:05.000Z";
    const KEY: [u8; 32] = [7; 32];

    struct FakeTransport {
        generation: u64,
        calls: Mutex<Vec<String>>,
        uploaded: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeTransport {
        fn new(generation: u64) -> Self {
            Self {
                generation,
                calls: Mutex::new(Vec::new()),
                uploaded: Mutex::new(Vec::new()),
            }
        }
    }

    impl BackupCloudTransport for FakeTransport {
        fn active_generation(&self, _session: &mut AccountSession) -> UploadResult<u64> {
            self.calls.lock().unwrap().push("generation".into());
            Ok(self.generation)
        }

        fn create_plan(
            &self,
            _session: &mut AccountSession,
            _plan: &BackupPlan,
        ) -> UploadResult<()> {
            self.calls.lock().unwrap().push("plan".into());
            Ok(())
        }

        fn put_object(
            &self,
            _session: &mut AccountSession,
            _backup_id: &str,
            object: &BackupObjectPlan,
            ciphertext: &[u8],
        ) -> UploadResult<()> {
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
            plan: &BackupPlan,
        ) -> UploadResult<BackupUploadOutcome> {
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
    fn generation_initializes_only_at_one_and_never_adopts_remote_advance() {
        let root = temp_dir("generation");
        let mut session = account();
        assert_eq!(
            load_or_initialize_device_generation(
                &root,
                USER_ID,
                &FakeTransport::new(1),
                &mut session
            )
            .unwrap(),
            1
        );
        let state = std::fs::read_to_string(root.join(DEVICE_STATE_FILE)).unwrap();
        assert!(!state.contains("access-test-only"));
        assert_eq!(
            load_or_initialize_device_generation(
                &root,
                USER_ID,
                &FakeTransport::new(2),
                &mut session
            )
            .unwrap_err(),
            BackupUploadError::StaleGeneration
        );

        let fresh = temp_dir("generation-fresh");
        assert_eq!(
            load_or_initialize_device_generation(
                &fresh,
                USER_ID,
                &FakeTransport::new(2),
                &mut session
            )
            .unwrap_err(),
            BackupUploadError::StaleGeneration
        );
        assert!(!fresh.join(DEVICE_STATE_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
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
    fn encrypted_session_uploads_main_then_manifest_order_and_commits_last() {
        let root = temp_dir("roundtrip");
        let first = b"public fixture alpha";
        let second = b"public fixture beta";
        let ids = ["doc-alpha", "doc-beta"];
        let document = portable_document(vec![manifest(ids[0], first), manifest(ids[1], second)]);
        let json = serde_json::to_vec(&document).unwrap();
        let mut upload =
            BackupUploadSession::begin(&root, &json, USER_ID, 1, CREATED_AT, KEY).unwrap();
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
            let binding =
                AttachmentBinding::from_manifest(&metadata, &manifest(ids[index], plaintext));
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
        let first =
            BackupUploadSession::begin(&root, &encoded, USER_ID, 1, CREATED_AT, KEY).unwrap();
        let first_plan = first.plan().unwrap();
        let first_bytes = std::fs::read(&first.staged[0].path).unwrap();
        let orphan = root.join(UPLOAD_STAGING_DIR).join("a".repeat(32));
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("opaque"), b"ciphertext-only fixture").unwrap();
        drop(first);
        std::fs::create_dir_all(&orphan).unwrap();

        let second =
            BackupUploadSession::begin(&root, &encoded, USER_ID, 1, CREATED_AT, KEY).unwrap();
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
            attachment_object_id(&KEY, "22222222-2222-4222-8222-222222222222", "doc-alpha")
                .unwrap()
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
}

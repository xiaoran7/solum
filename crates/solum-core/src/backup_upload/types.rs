//! types. Part of the `backup_upload` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `backup_upload.rs`.

//! Wire and in-memory shapes: device authority, object plans, the frozen upload session, the transport seam.

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackupDeviceState {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) user_id: String,
    pub(super) device_generation: u64,
    pub(super) writer_id: String,
    pub(super) phase: BackupDevicePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BackupDevicePhase {
    Claiming,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupDeviceAuthority {
    pub generation: u64,
    pub writer_id: String,
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

pub(super) struct StagedObject {
    pub(super) plan: BackupObjectPlan,
    pub(super) path: PathBuf,
}

/// A single frozen backup attempt. Deliberately does not implement `Debug` or
/// `Clone`: it temporarily owns the master key and at most one plaintext
/// attachment buffer.
pub struct BackupUploadSession {
    pub(super) session_id: String,
    pub(super) metadata: BackupMetadata,
    pub(super) writer_id: String,
    pub(super) master_key: [u8; 32],
    pub(super) manifests: Vec<DocumentManifest>,
    pub(super) next_attachment: usize,
    pub(super) attachment_plaintext: Vec<u8>,
    pub(super) staged: Vec<StagedObject>,
    pub(super) staging_dir: PathBuf,
}

/// Narrow transport seam: fake implementations prove ordering and
/// pre-commit validation without a server; the production implementation
/// below performs bounded retry and access-token rotation.
pub trait BackupCloudTransport {
    fn active_generation(&self, session: &mut AccountSession) -> UploadResult<u64>;
    fn claim_initial_writer(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
    ) -> UploadResult<u64>;
    fn create_plan(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
        plan: &BackupPlan,
    ) -> UploadResult<()>;
    fn put_object(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
        backup_id: &str,
        object: &BackupObjectPlan,
        ciphertext: &[u8],
    ) -> UploadResult<()>;
    fn commit(
        &self,
        session: &mut AccountSession,
        writer_id: &str,
        plan: &BackupPlan,
    ) -> UploadResult<BackupUploadOutcome>;
}

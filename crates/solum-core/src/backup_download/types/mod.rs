//! `types` -- the catalog, delete-request and staged-outcome shapes, and the three cloud-transport traits.
//!
//! Part of the `backup_download` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

/// Exact complete-version metadata shown to the user before a destructive
/// cloud deletion. The plan ETag is never rendered, but remains in the Guard
/// fingerprint and the final HTTP precondition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupDeleteRequest {
    pub backup_id: String,
    pub plan_etag: String,
    pub committed_at: String,
    pub total_bytes: u64,
    pub object_count: usize,
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

/// Narrow destructive seam kept separate from the read API so tests and
/// future platform shells cannot accidentally grant deletion to a reader.
pub trait BackupCloudDelete {
    fn delete_backup(
        &self,
        session: &mut AccountSession,
        backup_id: &str,
        plan_etag: &str,
    ) -> DownloadResult<()>;
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

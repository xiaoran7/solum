//! Durable client-side transaction for cloud generation takeover.
//!
//! The journal makes a response-lost compare-and-increment retry reuse the
//! same takeover UUID. It contains metadata only and never publishes a profile
//! pointer; the Android adapter performs publication after a fresh live-data
//! preview and then calls [`finish_published_takeover`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::account::AccountSession;
use crate::backup_download::{BackupCloudTakeover, BackupDownloadError};
use crate::backup_upload;

pub const TAKEOVER_JOURNAL_FILE: &str = "solum-backup-takeover.json";

const JOURNAL_FORMAT: &str = "solum-backup-takeover";
const JOURNAL_VERSION: u32 = 2;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

type TakeoverResult<T> = std::result::Result<T, BackupTakeoverError>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupTakeoverError {
    #[error("backup takeover binding is invalid")]
    InvalidBinding,
    #[error("backup takeover journal is invalid")]
    InvalidJournal,
    #[error("another backup takeover is already in progress")]
    TakeoverInProgress,
    #[error("backup cloud generation changed before takeover")]
    CloudConflict,
    #[error("backup cloud takeover request failed")]
    Cloud,
    #[error("backup device generation could not be persisted")]
    DeviceState,
    #[error("backup takeover journal persistence failed")]
    Persistence,
    #[error("published generation does not match the backup takeover")]
    PublicationMismatch,
}

impl BackupTakeoverError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidBinding => "invalid_binding",
            Self::InvalidJournal => "invalid_journal",
            Self::TakeoverInProgress => "takeover_in_progress",
            Self::CloudConflict => "cloud_conflict",
            Self::Cloud => "cloud",
            Self::DeviceState => "device_state",
            Self::Persistence => "persistence",
            Self::PublicationMismatch => "publication_mismatch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverPhase {
    Planned,
    RemoteAdvanced,
    DeviceReady,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TakeoverJournal {
    format: String,
    version: u32,
    user_id: String,
    server_origin_sha256: String,
    backup_id: String,
    staged_generation: String,
    expected_generation: u64,
    takeover_id: String,
    writer_id: String,
    phase: TakeoverPhase,
    target_generation: Option<u64>,
}

pub struct BackupTakeoverBinding<'a> {
    pub user_id: &'a str,
    pub server_url: &'a str,
    pub backup_id: &'a str,
    pub staged_generation: &'a str,
    pub expected_generation: u64,
}

pub struct PendingBackupTakeover {
    profile_root: PathBuf,
    journal: TakeoverJournal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupTakeoverStatus {
    pub backup_id: String,
    pub staged_generation: String,
    pub expected_generation: u64,
    pub target_generation: Option<u64>,
    pub phase: TakeoverPhase,
}

impl PendingBackupTakeover {
    pub fn status(&self) -> BackupTakeoverStatus {
        BackupTakeoverStatus {
            backup_id: self.journal.backup_id.clone(),
            staged_generation: self.journal.staged_generation.clone(),
            expected_generation: self.journal.expected_generation,
            target_generation: self.journal.target_generation,
            phase: self.journal.phase,
        }
    }
}

pub fn begin_or_resume_takeover(
    profile_root: &Path,
    binding: BackupTakeoverBinding<'_>,
) -> TakeoverResult<PendingBackupTakeover> {
    validate_binding(&binding)?;
    let expected_origin = sha256_hex(binding.server_url.as_bytes());
    let path = profile_root.join(TAKEOVER_JOURNAL_FILE);
    let journal = match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let existing: TakeoverJournal =
                serde_json::from_str(&raw).map_err(|_| BackupTakeoverError::InvalidJournal)?;
            validate_journal(&existing)?;
            if existing.user_id != binding.user_id
                || existing.server_origin_sha256 != expected_origin
                || existing.backup_id != binding.backup_id
                || existing.staged_generation != binding.staged_generation
                || existing.expected_generation != binding.expected_generation
            {
                return Err(BackupTakeoverError::TakeoverInProgress);
            }
            existing
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let journal = TakeoverJournal {
                format: JOURNAL_FORMAT.into(),
                version: JOURNAL_VERSION,
                user_id: binding.user_id.into(),
                server_origin_sha256: expected_origin,
                backup_id: binding.backup_id.into(),
                staged_generation: binding.staged_generation.into(),
                expected_generation: binding.expected_generation,
                takeover_id: random_uuid_v4()?,
                writer_id: random_uuid_v4()?,
                phase: TakeoverPhase::Planned,
                target_generation: None,
            };
            persist(profile_root, &journal)?;
            journal
        }
        Err(_) => return Err(BackupTakeoverError::Persistence),
    };
    Ok(PendingBackupTakeover {
        profile_root: profile_root.to_path_buf(),
        journal,
    })
}

pub fn load_pending_takeover(
    profile_root: &Path,
    user_id: &str,
    server_url: &str,
) -> TakeoverResult<Option<PendingBackupTakeover>> {
    let path = profile_root.join(TAKEOVER_JOURNAL_FILE);
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(BackupTakeoverError::Persistence),
    };
    let journal: TakeoverJournal =
        serde_json::from_str(&raw).map_err(|_| BackupTakeoverError::InvalidJournal)?;
    validate_journal(&journal)?;
    if journal.user_id != user_id
        || journal.server_origin_sha256 != sha256_hex(server_url.as_bytes())
    {
        return Err(BackupTakeoverError::TakeoverInProgress);
    }
    Ok(Some(PendingBackupTakeover {
        profile_root: profile_root.to_path_buf(),
        journal,
    }))
}

pub fn advance_takeover(
    pending: &mut PendingBackupTakeover,
    transport: &dyn BackupCloudTakeover,
    session: &mut AccountSession,
) -> TakeoverResult<BackupTakeoverStatus> {
    if session.user_id != pending.journal.user_id
        || sha256_hex(session.server_url.as_bytes()) != pending.journal.server_origin_sha256
    {
        return Err(BackupTakeoverError::InvalidBinding);
    }
    if pending.journal.phase == TakeoverPhase::Planned {
        let outcome = transport
            .advance_generation(
                session,
                pending.journal.expected_generation,
                &pending.journal.takeover_id,
                &pending.journal.writer_id,
            )
            .map_err(map_cloud_error)?;
        let target = pending
            .journal
            .expected_generation
            .checked_add(1)
            .ok_or(BackupTakeoverError::InvalidJournal)?;
        if outcome.active_generation != target {
            return Err(BackupTakeoverError::Cloud);
        }
        pending.journal.target_generation = Some(target);
        pending.journal.phase = TakeoverPhase::RemoteAdvanced;
        persist(&pending.profile_root, &pending.journal)?;
    }

    let target = pending
        .journal
        .target_generation
        .ok_or(BackupTakeoverError::InvalidJournal)?;
    backup_upload::persist_taken_over_device_generation(
        &pending.profile_root,
        &pending.journal.user_id,
        pending.journal.expected_generation,
        target,
        &pending.journal.writer_id,
    )
    .map_err(|_| BackupTakeoverError::DeviceState)?;
    if pending.journal.phase != TakeoverPhase::DeviceReady {
        pending.journal.phase = TakeoverPhase::DeviceReady;
        persist(&pending.profile_root, &pending.journal)?;
    }
    Ok(pending.status())
}

pub fn finish_published_takeover(
    pending: PendingBackupTakeover,
    active_generation: &str,
) -> TakeoverResult<()> {
    if pending.journal.phase != TakeoverPhase::DeviceReady
        || pending.journal.staged_generation != active_generation
    {
        return Err(BackupTakeoverError::PublicationMismatch);
    }
    remove_journal(&pending.profile_root)
}

pub fn reconcile_published_takeover(
    profile_root: &Path,
    user_id: &str,
    server_url: &str,
    active_generation: Option<&str>,
) -> TakeoverResult<bool> {
    let Some(pending) = load_pending_takeover(profile_root, user_id, server_url)? else {
        return Ok(false);
    };
    if pending.journal.phase == TakeoverPhase::DeviceReady
        && active_generation == Some(pending.journal.staged_generation.as_str())
    {
        finish_published_takeover(
            pending,
            active_generation.ok_or(BackupTakeoverError::PublicationMismatch)?,
        )?;
        return Ok(true);
    }
    Ok(false)
}

fn validate_binding(binding: &BackupTakeoverBinding<'_>) -> TakeoverResult<()> {
    if !valid_uuid(binding.user_id)
        || !valid_uuid(binding.backup_id)
        || !valid_generation_id(binding.staged_generation)
        || binding.server_url.is_empty()
        || binding.expected_generation == 0
        || binding.expected_generation >= MAX_SAFE_INTEGER
    {
        return Err(BackupTakeoverError::InvalidBinding);
    }
    Ok(())
}

fn validate_journal(journal: &TakeoverJournal) -> TakeoverResult<()> {
    if journal.format != JOURNAL_FORMAT
        || journal.version != JOURNAL_VERSION
        || !valid_uuid(&journal.user_id)
        || !valid_lower_hex(&journal.server_origin_sha256, 32)
        || !valid_uuid(&journal.backup_id)
        || !valid_generation_id(&journal.staged_generation)
        || journal.expected_generation == 0
        || journal.expected_generation >= MAX_SAFE_INTEGER
        || !valid_uuid(&journal.takeover_id)
        || !valid_uuid(&journal.writer_id)
    {
        return Err(BackupTakeoverError::InvalidJournal);
    }
    let expected_target = journal.expected_generation + 1;
    match journal.phase {
        TakeoverPhase::Planned if journal.target_generation.is_none() => Ok(()),
        TakeoverPhase::RemoteAdvanced | TakeoverPhase::DeviceReady
            if journal.target_generation == Some(expected_target) =>
        {
            Ok(())
        }
        _ => Err(BackupTakeoverError::InvalidJournal),
    }
}

fn persist(profile_root: &Path, journal: &TakeoverJournal) -> TakeoverResult<()> {
    std::fs::create_dir_all(profile_root).map_err(|_| BackupTakeoverError::Persistence)?;
    let encoded = serde_json::to_string(journal).map_err(|_| BackupTakeoverError::Persistence)?;
    crate::fsatomic::write_atomic_durable(&profile_root.join(TAKEOVER_JOURNAL_FILE), &encoded)
        .map_err(|_| BackupTakeoverError::Persistence)
}

fn remove_journal(profile_root: &Path) -> TakeoverResult<()> {
    let path = profile_root.join(TAKEOVER_JOURNAL_FILE);
    match std::fs::remove_file(path) {
        Ok(()) => crate::fsatomic::sync_directory(profile_root)
            .map_err(|_| BackupTakeoverError::Persistence),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(BackupTakeoverError::Persistence),
    }
}

fn map_cloud_error(error: BackupDownloadError) -> BackupTakeoverError {
    match error {
        BackupDownloadError::CloudConflict => BackupTakeoverError::CloudConflict,
        _ => BackupTakeoverError::Cloud,
    }
}

fn random_uuid_v4() -> TakeoverResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| BackupTakeoverError::Persistence)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
        && value != "00000000-0000-0000-0000-000000000000"
}

fn valid_generation_id(value: &str) -> bool {
    valid_lower_hex(value, 16)
}

fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::backup_download::CloudTakeoverOutcome;
    use crate::backup_upload::DEVICE_STATE_FILE;

    const USER_ID: &str = "11111111-1111-4111-8111-111111111111";
    const BACKUP_ID: &str = "22222222-2222-4222-8222-222222222222";
    const GENERATION: &str = "00112233445566778899aabbccddeeff";
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct FakeTakeover {
        calls: Mutex<usize>,
        result: std::result::Result<CloudTakeoverOutcome, BackupDownloadError>,
    }

    impl BackupCloudTakeover for FakeTakeover {
        fn advance_generation(
            &self,
            _session: &mut AccountSession,
            expected_generation: u64,
            _takeover_id: &str,
            _writer_id: &str,
        ) -> std::result::Result<CloudTakeoverOutcome, BackupDownloadError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone().map(|mut outcome| {
                outcome.active_generation = expected_generation + 1;
                outcome
            })
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solum-backup-takeover-{label}-{}-{counter}",
            std::process::id()
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

    fn binding<'a>() -> BackupTakeoverBinding<'a> {
        BackupTakeoverBinding {
            user_id: USER_ID,
            server_url: "https://example.invalid",
            backup_id: BACKUP_ID,
            staged_generation: GENERATION,
            expected_generation: 4,
        }
    }

    #[test]
    fn response_lost_retry_reuses_journal_and_device_state_is_idempotent() {
        let root = temp_dir("roundtrip");
        let mut pending = begin_or_resume_takeover(&root, binding()).unwrap();
        let first_status = pending.status();
        assert_eq!(first_status.phase, TakeoverPhase::Planned);
        let transport = FakeTakeover {
            calls: Mutex::new(0),
            result: Ok(CloudTakeoverOutcome {
                active_generation: 0,
                pruned_pending: 0,
                replay: false,
            }),
        };
        let ready = advance_takeover(&mut pending, &transport, &mut account()).unwrap();
        assert_eq!(ready.phase, TakeoverPhase::DeviceReady);
        assert_eq!(ready.target_generation, Some(5));
        assert_eq!(*transport.calls.lock().unwrap(), 1);

        let mut reloaded = load_pending_takeover(&root, USER_ID, "https://example.invalid")
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.status(), ready);
        advance_takeover(&mut reloaded, &transport, &mut account()).unwrap();
        assert_eq!(*transport.calls.lock().unwrap(), 1);
        let device = std::fs::read_to_string(root.join(DEVICE_STATE_FILE)).unwrap();
        assert!(device.contains("\"device_generation\":5"));
        finish_published_takeover(reloaded, GENERATION).unwrap();
        assert!(!root.join(TAKEOVER_JOURNAL_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ambiguous_transport_failure_keeps_planned_journal_for_same_id_retry() {
        let root = temp_dir("transport");
        let mut pending = begin_or_resume_takeover(&root, binding()).unwrap();
        let transport = FakeTakeover {
            calls: Mutex::new(0),
            result: Err(BackupDownloadError::Transport),
        };
        assert_eq!(
            advance_takeover(&mut pending, &transport, &mut account()).unwrap_err(),
            BackupTakeoverError::Cloud
        );
        let reloaded = begin_or_resume_takeover(&root, binding()).unwrap();
        assert_eq!(reloaded.status().phase, TakeoverPhase::Planned);
        assert!(root.join(TAKEOVER_JOURNAL_FILE).exists());
        assert!(!root.join(DEVICE_STATE_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mismatched_candidate_account_and_publication_fail_closed() {
        let root = temp_dir("mismatch");
        let pending = begin_or_resume_takeover(&root, binding()).unwrap();
        let other = BackupTakeoverBinding {
            backup_id: "33333333-3333-4333-8333-333333333333",
            ..binding()
        };
        assert!(matches!(
            begin_or_resume_takeover(&root, other),
            Err(BackupTakeoverError::TakeoverInProgress)
        ));
        assert_eq!(
            finish_published_takeover(pending, "ffeeddccbbaa99887766554433221100").unwrap_err(),
            BackupTakeoverError::PublicationMismatch
        );
        assert!(root.join(TAKEOVER_JOURNAL_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn published_ready_journal_is_reconciled_after_restart() {
        let root = temp_dir("reconcile");
        let mut pending = begin_or_resume_takeover(&root, binding()).unwrap();
        let transport = FakeTakeover {
            calls: Mutex::new(0),
            result: Ok(CloudTakeoverOutcome {
                active_generation: 0,
                pruned_pending: 0,
                replay: false,
            }),
        };
        advance_takeover(&mut pending, &transport, &mut account()).unwrap();
        drop(pending);
        assert!(reconcile_published_takeover(
            &root,
            USER_ID,
            "https://example.invalid",
            Some(GENERATION)
        )
        .unwrap());
        assert!(!root.join(TAKEOVER_JOURNAL_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}

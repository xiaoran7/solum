//! Crash-safe publication of one complete profile data generation.
//!
//! The active pointer is the sole commit record. Staging directories and
//! unreferenced sealed generations are inert until that pointer is durably
//! replaced, so a crash cannot expose a half-restored SQLite/profile image.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "android")]
use std::os::fd::AsRawFd;

#[cfg(not(target_os = "android"))]
use fs2::FileExt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};

pub const POINTER_FILE: &str = "active-generation.json";
pub const GENERATIONS_DIR: &str = "generations";
pub const STAGING_DIR: &str = "restore-staging";
pub const MANIFEST_FILE: &str = "generation-manifest.json";
pub const SQLITE_FILE: &str = "solum.sqlite";
pub const GENERATION_LOCK_FILE: &str = "active-generation.lock";

const POINTER_FORMAT: &str = "solum-profile-generation";
const MANIFEST_FORMAT: &str = "solum-profile-image";
const PROTOCOL_VERSION: u32 = 1;
const MAX_POINTER_BYTES: u64 = 4096;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_GENERATION_FILES: usize = 12_000;
pub const MAX_GENERATION_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveProfileData {
    Legacy { path: PathBuf },
    Generation { id: String, path: PathBuf },
}

impl ActiveProfileData {
    pub fn path(&self) -> &Path {
        match self {
            Self::Legacy { path } | Self::Generation { path, .. } => path,
        }
    }

    pub fn generation_id(&self) -> Option<&str> {
        match self {
            Self::Legacy { .. } => None,
            Self::Generation { id, .. } => Some(id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StagedProfileGeneration {
    profile_root: PathBuf,
    id: String,
    path: PathBuf,
}

impl StagedProfileGeneration {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone)]
pub struct SealedProfileGeneration {
    profile_root: PathBuf,
    id: String,
}

impl SealedProfileGeneration {
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivePointer {
    format: String,
    version: u32,
    generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationManifest {
    format: String,
    version: u32,
    generation: String,
    artifacts: Vec<GenerationArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationArtifact {
    path: String,
    size: u64,
    sha256: String,
}

/// Resolve the sole committed data directory. A malformed pointer is an error,
/// never permission to fall back to legacy or create an empty database.
pub fn resolve_active_data_dir(profile_root: &Path) -> Result<ActiveProfileData> {
    with_generation_lock(profile_root, || {
        resolve_active_data_dir_unlocked(profile_root)
    })
}

pub(crate) fn resolve_active_data_dir_unlocked(profile_root: &Path) -> Result<ActiveProfileData> {
    let pointer_path = profile_root.join(POINTER_FILE);
    if !path_entry_exists(&pointer_path)? {
        return Ok(ActiveProfileData::Legacy {
            path: profile_root.to_path_buf(),
        });
    }
    let pointer: ActivePointer = read_json(&pointer_path, MAX_POINTER_BYTES, "generation pointer")?;
    validate_pointer(&pointer)?;
    let path = profile_root.join(GENERATIONS_DIR).join(&pointer.generation);
    validate_published_target(&path, &pointer.generation)?;
    Ok(ActiveProfileData::Generation {
        id: pointer.generation,
        path,
    })
}

/// Allocate a caller-writable staging directory under the same profile root as
/// the eventual generation. The id is always OS-random and path-safe.
pub fn begin_profile_generation(profile_root: &Path) -> Result<StagedProfileGeneration> {
    std::fs::create_dir_all(profile_root.join(STAGING_DIR)).map_err(storage)?;
    std::fs::create_dir_all(profile_root.join(GENERATIONS_DIR)).map_err(storage)?;
    for _ in 0..4 {
        let id = random_id()?;
        let path = profile_root.join(STAGING_DIR).join(&id);
        match std::fs::create_dir(&path) {
            Ok(()) => {
                return Ok(StagedProfileGeneration {
                    profile_root: profile_root.to_path_buf(),
                    id,
                    path,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(storage(error)),
        }
    }
    Err(invalid("could not allocate a unique profile generation"))
}

/// Fsync and inventory a closed temporary image. `solum.sqlite` must already
/// be checkpointed; WAL/SHM sidecars prove the Store is not ready to publish.
pub fn seal_profile_generation(staged: StagedProfileGeneration) -> Result<SealedProfileGeneration> {
    validate_staged_handle(&staged)?;
    let sqlite = staged.path.join(SQLITE_FILE);
    ensure_regular_file(&sqlite, "temporary generation is missing solum.sqlite")?;
    if staged.path.join(format!("{SQLITE_FILE}-wal")).exists()
        || staged.path.join(format!("{SQLITE_FILE}-shm")).exists()
    {
        return Err(invalid(
            "temporary generation still has SQLite WAL/SHM sidecars",
        ));
    }
    verify_sqlite_image(&sqlite)?;
    if staged.path.join(MANIFEST_FILE).exists() {
        return Err(invalid("temporary generation is already sealed"));
    }
    let artifacts = scan_artifacts(&staged.path)?;
    if !artifacts.iter().any(|item| item.path == SQLITE_FILE) {
        return Err(invalid("generation manifest does not include solum.sqlite"));
    }
    let manifest = GenerationManifest {
        format: MANIFEST_FORMAT.into(),
        version: PROTOCOL_VERSION,
        generation: staged.id.clone(),
        artifacts,
    };
    let encoded = serde_json::to_string(&manifest)?;
    if encoded.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid("generation manifest exceeds its size limit"));
    }
    crate::fsatomic::write_atomic_durable(&staged.path.join(MANIFEST_FILE), &encoded)?;
    sync_tree_directories(&staged.path)?;
    Ok(SealedProfileGeneration {
        profile_root: staged.profile_root,
        id: staged.id,
    })
}

/// Reconstruct a sealed handle after process death. The id must come from the
/// restore coordinator's own journal; directory discovery never activates an
/// arbitrary orphan by itself.
pub fn resume_sealed_profile_generation(
    profile_root: &Path,
    id: &str,
) -> Result<SealedProfileGeneration> {
    with_generation_lock(profile_root, || {
        resume_sealed_profile_generation_unlocked(profile_root, id)
    })
}

pub(crate) fn resume_sealed_profile_generation_unlocked(
    profile_root: &Path,
    id: &str,
) -> Result<SealedProfileGeneration> {
    validate_generation_id(id)?;
    let staging = profile_root.join(STAGING_DIR).join(id);
    let final_path = profile_root.join(GENERATIONS_DIR).join(id);
    match (staging.is_dir(), final_path.is_dir()) {
        (true, false) => verify_sealed_generation(&staging, id)?,
        (false, true) => {
            let pointer_path = profile_root.join(POINTER_FILE);
            if path_entry_exists(&pointer_path)? {
                let pointer: ActivePointer =
                    read_json(&pointer_path, MAX_POINTER_BYTES, "generation pointer")?;
                validate_pointer(&pointer)?;
                if pointer.generation == id {
                    validate_published_target(&final_path, id)?;
                } else {
                    verify_sealed_generation(&final_path, id)?;
                }
            } else {
                verify_sealed_generation(&final_path, id)?;
            }
        }
        (true, true) => {
            return Err(invalid(
                "generation exists in staging and final directories",
            ))
        }
        (false, false) => return Err(invalid("sealed generation directory is missing")),
    }
    Ok(SealedProfileGeneration {
        profile_root: profile_root.to_path_buf(),
        id: id.to_owned(),
    })
}

/// Verify a sealed image, make its directory durable, then atomically replace
/// the sole active pointer. If a crash happened after the directory rename,
/// calling this again resumes from the unreferenced final directory.
pub fn publish_profile_generation(sealed: &SealedProfileGeneration) -> Result<ActiveProfileData> {
    with_generation_lock(&sealed.profile_root, || {
        publish_profile_generation_unlocked(sealed)
    })
}

pub(crate) fn publish_profile_generation_unlocked(
    sealed: &SealedProfileGeneration,
) -> Result<ActiveProfileData> {
    validate_generation_id(&sealed.id)?;
    let pointer_path = sealed.profile_root.join(POINTER_FILE);
    if path_entry_exists(&pointer_path)? {
        let pointer: ActivePointer =
            read_json(&pointer_path, MAX_POINTER_BYTES, "generation pointer")?;
        validate_pointer(&pointer)?;
        if pointer.generation == sealed.id {
            return resolve_active_data_dir_unlocked(&sealed.profile_root);
        }
    }
    let staging = sealed.profile_root.join(STAGING_DIR).join(&sealed.id);
    let final_path = sealed.profile_root.join(GENERATIONS_DIR).join(&sealed.id);
    let source = match (staging.is_dir(), final_path.is_dir()) {
        (true, false) => &staging,
        (false, true) => &final_path,
        (true, true) => {
            return Err(invalid(
                "generation exists in staging and final directories",
            ))
        }
        (false, false) => return Err(invalid("sealed generation directory is missing")),
    };
    verify_sealed_generation(source, &sealed.id)?;
    if source == &staging {
        std::fs::rename(&staging, &final_path).map_err(storage)?;
        crate::fsatomic::sync_directory(&sealed.profile_root.join(GENERATIONS_DIR))?;
        // This is a cross-directory rename. Persist both the destination entry
        // and removal of the staging entry before the pointer can commit.
        crate::fsatomic::sync_directory(&sealed.profile_root.join(STAGING_DIR))?;
    }
    let pointer = ActivePointer {
        format: POINTER_FORMAT.into(),
        version: PROTOCOL_VERSION,
        generation: sealed.id.clone(),
    };
    crate::fsatomic::write_atomic_durable(&pointer_path, &serde_json::to_string(&pointer)?)?;
    Ok(ActiveProfileData::Generation {
        id: sealed.id.clone(),
        path: final_path,
    })
}

fn validate_staged_handle(staged: &StagedProfileGeneration) -> Result<()> {
    validate_generation_id(&staged.id)?;
    let expected = staged.profile_root.join(STAGING_DIR).join(&staged.id);
    if staged.path != expected || !staged.path.is_dir() {
        return Err(invalid("temporary generation handle is invalid"));
    }
    ensure_not_symlink(&staged.path, "temporary generation cannot be a symlink")
}

fn validate_pointer(pointer: &ActivePointer) -> Result<()> {
    if pointer.format != POINTER_FORMAT || pointer.version != PROTOCOL_VERSION {
        return Err(invalid("unsupported profile generation pointer"));
    }
    validate_generation_id(&pointer.generation)
}

fn validate_published_target(path: &Path, id: &str) -> Result<()> {
    ensure_directory(path, "active profile generation directory is missing")?;
    let manifest: GenerationManifest = read_json(
        &path.join(MANIFEST_FILE),
        MAX_MANIFEST_BYTES,
        "generation manifest",
    )?;
    validate_manifest_shape(&manifest, id)?;
    ensure_regular_file(
        &path.join(SQLITE_FILE),
        "active profile generation is missing solum.sqlite",
    )
}

fn verify_sqlite_image(path: &Path) -> Result<()> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| invalid("temporary generation SQLite cannot be opened"))?;
    let result: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| invalid("temporary generation SQLite integrity check failed"))?;
    if result != "ok" {
        return Err(invalid(
            "temporary generation SQLite integrity check failed",
        ));
    }
    let has_meta: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table' AND name = 'meta'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|_| invalid("temporary generation SQLite schema cannot be read"))?;
    if !has_meta {
        return Err(invalid("temporary generation SQLite schema is incomplete"));
    }
    Ok(())
}

fn verify_sealed_generation(path: &Path, id: &str) -> Result<()> {
    ensure_directory(path, "sealed generation directory is missing")?;
    let manifest: GenerationManifest = read_json(
        &path.join(MANIFEST_FILE),
        MAX_MANIFEST_BYTES,
        "generation manifest",
    )?;
    validate_manifest_shape(&manifest, id)?;
    let actual = scan_artifacts(path)?;
    if actual != manifest.artifacts {
        return Err(invalid(
            "sealed generation artifacts do not match the manifest",
        ));
    }
    Ok(())
}

fn validate_manifest_shape(manifest: &GenerationManifest, id: &str) -> Result<()> {
    if manifest.format != MANIFEST_FORMAT
        || manifest.version != PROTOCOL_VERSION
        || manifest.generation != id
        || manifest.artifacts.is_empty()
        || manifest.artifacts.len() > MAX_GENERATION_FILES
    {
        return Err(invalid("invalid or unsupported generation manifest"));
    }
    let mut paths = BTreeSet::new();
    let mut total = 0u64;
    for artifact in &manifest.artifacts {
        validate_relative_path(&artifact.path)?;
        if !paths.insert(artifact.path.as_str())
            || artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("invalid generation artifact entry"));
        }
        total = total
            .checked_add(artifact.size)
            .ok_or_else(|| invalid("generation artifact bytes overflow"))?;
        if total > MAX_GENERATION_BYTES {
            return Err(invalid("generation exceeds its total size limit"));
        }
    }
    if !paths.contains(SQLITE_FILE) {
        return Err(invalid("generation manifest does not include solum.sqlite"));
    }
    Ok(())
}

fn scan_artifacts(root: &Path) -> Result<Vec<GenerationArtifact>> {
    let mut artifacts = Vec::new();
    scan_directory(root, root, &mut artifacts)?;
    artifacts.sort();
    if artifacts.len() > MAX_GENERATION_FILES {
        return Err(invalid("generation exceeds its file-count limit"));
    }
    let total = artifacts.iter().try_fold(0u64, |sum, item| {
        sum.checked_add(item.size)
            .ok_or_else(|| invalid("generation artifact bytes overflow"))
    })?;
    if total > MAX_GENERATION_BYTES {
        return Err(invalid("generation exceeds its total size limit"));
    }
    Ok(artifacts)
}

fn scan_directory(root: &Path, directory: &Path, out: &mut Vec<GenerationArtifact>) -> Result<()> {
    ensure_directory(directory, "generation contains an invalid directory")?;
    let mut entries = std::fs::read_dir(directory)
        .map_err(storage)?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(storage)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(storage)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid("generation cannot contain symbolic links"));
        }
        if metadata.is_dir() {
            scan_directory(root, &path, out)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(invalid("generation can contain only regular files"));
        }
        let relative = relative_path(root, &path)?;
        if relative == MANIFEST_FILE {
            continue;
        }
        if relative == format!("{SQLITE_FILE}-wal") || relative == format!("{SQLITE_FILE}-shm") {
            return Err(invalid("generation contains SQLite WAL/SHM sidecars"));
        }
        // Windows rejects `sync_all` on a read-only handle even though Unix
        // accepts it. A generation artifact is caller-owned staging data, so
        // require a writable handle while sealing rather than silently omitting
        // the durability step on one platform.
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(storage)?;
        file.sync_all().map_err(storage)?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(storage)?;
            if read == 0 {
                break;
            }
            size = size
                .checked_add(read as u64)
                .ok_or_else(|| invalid("generation artifact bytes overflow"))?;
            if size > MAX_GENERATION_BYTES {
                return Err(invalid("generation exceeds its total size limit"));
            }
            hasher.update(&buffer[..read]);
        }
        out.push(GenerationArtifact {
            path: relative,
            size,
            sha256: lower_hex(&hasher.finalize()),
        });
        if out.len() > MAX_GENERATION_FILES {
            return Err(invalid("generation exceeds its file-count limit"));
        }
    }
    Ok(())
}

pub(crate) fn sync_tree_directories(root: &Path) -> Result<()> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        for entry in std::fs::read_dir(&directory).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(storage)?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        crate::fsatomic::sync_directory(&directory)?;
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("generation artifact escaped its root"))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("generation artifact path is invalid"));
        };
        parts.push(
            part.to_str()
                .ok_or_else(|| invalid("generation artifact path is not UTF-8"))?,
        );
    }
    let value = parts.join("/");
    validate_relative_path(&value)?;
    Ok(value)
}

fn validate_relative_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(value).is_absolute()
    {
        return Err(invalid("generation artifact path is invalid"));
    }
    Ok(())
}

fn validate_generation_id(id: &str) -> Result<()> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("profile generation id is invalid"));
    }
    Ok(())
}

fn ensure_directory(path: &Path, message: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid(message))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(message));
    }
    Ok(())
}

fn ensure_regular_file(path: &Path, message: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid(message))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid(message));
    }
    Ok(())
}

fn ensure_not_symlink(path: &Path, message: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid(message));
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage(error)),
    }
}

pub(crate) fn with_generation_lock<T>(
    profile_root: &Path,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    std::fs::create_dir_all(profile_root).map_err(storage)?;
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(profile_root.join(GENERATION_LOCK_FILE))
        .map_err(storage)?;
    lock_generation_file(&lock_file).map_err(storage)?;
    let result = operation();
    let unlock = unlock_generation_file(&lock_file).map_err(storage);
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

#[cfg(target_os = "android")]
fn set_generation_file_lock(
    file: &std::fs::File,
    command: libc::c_int,
    lock_type: libc::c_short,
) -> std::io::Result<()> {
    // Java FileChannel uses POSIX locks. OFD locks conflict with those even
    // inside the same Android process, unlike process-owned F_SETLKW locks.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = lock_type;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    loop {
        let result = unsafe { libc::fcntl(file.as_raw_fd(), command, &lock) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(target_os = "android")]
fn lock_generation_file(file: &std::fs::File) -> std::io::Result<()> {
    set_generation_file_lock(file, libc::F_OFD_SETLKW, libc::F_WRLCK as libc::c_short)
}

#[cfg(target_os = "android")]
fn unlock_generation_file(file: &std::fs::File) -> std::io::Result<()> {
    set_generation_file_lock(file, libc::F_OFD_SETLK, libc::F_UNLCK as libc::c_short)
}

#[cfg(not(target_os = "android"))]
fn lock_generation_file(file: &std::fs::File) -> std::io::Result<()> {
    FileExt::lock_exclusive(file)
}

#[cfg(not(target_os = "android"))]
fn unlock_generation_file(file: &std::fs::File) -> std::io::Result<()> {
    FileExt::unlock(file)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64, what: &str) -> Result<T> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| invalid(format!("{what} is missing")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(invalid(format!(
            "{what} is invalid or exceeds its size limit"
        )));
    }
    let bytes = std::fs::read(path).map_err(storage)?;
    serde_json::from_slice(&bytes).map_err(|_| invalid(format!("{what} is not valid JSON")))
}

fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| invalid("operating-system random source is unavailable"))?;
    Ok(lower_hex(&bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn storage(error: std::io::Error) -> CoreError {
    CoreError::Storage(error.to_string())
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "solum-generation-{label}-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            assert!(path.starts_with(std::env::temp_dir()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            if self.0.starts_with(std::env::temp_dir())
                && self
                    .0
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("solum-generation-"))
            {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn prepare(root: &Path, marker: &str) -> StagedProfileGeneration {
        let staged = begin_profile_generation(root).unwrap();
        {
            let db = staged.path().join(SQLITE_FILE);
            let store = crate::store::Store::open(db.to_str().unwrap()).unwrap();
            store
                .insert_raw_input(marker, "fixture", chrono::NaiveDateTime::default())
                .unwrap();
            store.checkpoint_for_profile_generation().unwrap();
        }
        std::fs::write(staged.path().join("platform-ready.json"), marker).unwrap();
        staged
    }

    #[test]
    fn missing_pointer_keeps_the_legacy_profile_active() {
        let root = TestRoot::new("legacy");
        let active = resolve_active_data_dir(root.path()).unwrap();
        assert_eq!(active.path(), root.path());
        assert_eq!(active.generation_id(), None);
    }

    #[test]
    fn sealed_generation_publishes_once_and_resolves_after_restart() {
        let root = TestRoot::new("publish");
        let sealed = seal_profile_generation(prepare(root.path(), "first")).unwrap();
        let id = sealed.id().to_owned();
        let published = publish_profile_generation(&sealed).unwrap();
        assert_eq!(published.generation_id(), Some(id.as_str()));
        assert!(published.path().join(SQLITE_FILE).is_file());

        let restarted = resolve_active_data_dir(root.path()).unwrap();
        assert_eq!(restarted, published);
        // Retrying after the live SQLite has changed must use the pointer as
        // the commit record, not compare it with the pre-publication hashes.
        let store = crate::store::Store::open(restarted.path().join(SQLITE_FILE).to_str().unwrap())
            .unwrap();
        store
            .insert_raw_input("after publish", "fixture", chrono::NaiveDateTime::default())
            .unwrap();
        drop(store);
        assert_eq!(publish_profile_generation(&sealed).unwrap(), published);
    }

    #[test]
    fn incomplete_staging_and_unreferenced_final_directory_are_inert() {
        let root = TestRoot::new("orphan");
        let staged = prepare(root.path(), "candidate");
        assert_eq!(
            resolve_active_data_dir(root.path())
                .unwrap()
                .generation_id(),
            None
        );
        let sealed = seal_profile_generation(staged).unwrap();
        let final_path = root.path().join(GENERATIONS_DIR).join(sealed.id());
        std::fs::rename(root.path().join(STAGING_DIR).join(sealed.id()), &final_path).unwrap();
        assert_eq!(
            resolve_active_data_dir(root.path())
                .unwrap()
                .generation_id(),
            None
        );

        let resumed = resume_sealed_profile_generation(root.path(), sealed.id()).unwrap();
        let active = publish_profile_generation(&resumed).unwrap();
        assert_eq!(active.generation_id(), Some(sealed.id()));
    }

    #[test]
    fn a_failed_second_publish_leaves_the_first_pointer_unchanged() {
        let root = TestRoot::new("rollback");
        let first = seal_profile_generation(prepare(root.path(), "first")).unwrap();
        let first_active = publish_profile_generation(&first).unwrap();

        let second = seal_profile_generation(prepare(root.path(), "second")).unwrap();
        std::fs::write(
            root.path()
                .join(STAGING_DIR)
                .join(second.id())
                .join("platform-ready.json"),
            "tampered after seal",
        )
        .unwrap();
        assert!(publish_profile_generation(&second).is_err());
        assert_eq!(resolve_active_data_dir(root.path()).unwrap(), first_active);
    }

    #[test]
    fn damaged_pointer_fails_closed_instead_of_opening_legacy() {
        let root = TestRoot::new("pointer");
        std::fs::write(root.path().join(SQLITE_FILE), "legacy bytes").unwrap();
        std::fs::write(root.path().join(POINTER_FILE), "{\"format\":\"wrong\"}").unwrap();
        assert!(resolve_active_data_dir(root.path()).is_err());

        std::fs::write(
            root.path().join(POINTER_FILE),
            r#"{"format":"solum-profile-generation","version":1,"generation":"../escape"}"#,
        )
        .unwrap();
        assert!(resolve_active_data_dir(root.path()).is_err());
    }

    #[test]
    fn wal_sidecars_and_post_seal_changes_are_rejected() {
        let root = TestRoot::new("integrity");
        let staged = prepare(root.path(), "wal");
        std::fs::write(staged.path().join(format!("{SQLITE_FILE}-wal")), "pending").unwrap();
        assert!(seal_profile_generation(staged).is_err());

        let invalid = begin_profile_generation(root.path()).unwrap();
        std::fs::write(invalid.path().join(SQLITE_FILE), "not sqlite").unwrap();
        assert!(seal_profile_generation(invalid).is_err());
    }

    #[test]
    fn profile_roots_publish_independently() {
        let alice = TestRoot::new("alice");
        let bob = TestRoot::new("bob");
        let alice_sealed = seal_profile_generation(prepare(alice.path(), "alice")).unwrap();
        let bob_sealed = seal_profile_generation(prepare(bob.path(), "bob")).unwrap();
        publish_profile_generation(&alice_sealed).unwrap();
        assert!(resolve_active_data_dir(bob.path())
            .unwrap()
            .generation_id()
            .is_none());
        publish_profile_generation(&bob_sealed).unwrap();
        assert_ne!(
            resolve_active_data_dir(alice.path())
                .unwrap()
                .generation_id(),
            resolve_active_data_dir(bob.path()).unwrap().generation_id()
        );
    }

    #[test]
    fn generation_lock_serializes_pointer_read_and_publish_boundaries() {
        let root = TestRoot::new("lock");
        let root_path = root.path().to_path_buf();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            with_generation_lock(&root_path, || {
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        locked_rx.recv().unwrap();

        let second_root = root.path().to_path_buf();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            with_generation_lock(&second_root, || {
                entered_tx.send(()).unwrap();
                Ok(())
            })
            .unwrap();
        });
        assert!(entered_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        release_tx.send(()).unwrap();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        holder.join().unwrap();
        waiter.join().unwrap();
    }
}

//! `verify` -- every check that decides whether something on disk is trustworthy: id shape, published target, sqlite image integrity, sealed-generation agreement, manifest shape, and the artifact scan that bounds a generation's file count and total bytes. Kept apart from `stage` and `publish` so the rules live in one place rather than being re-derived at each call site.
//!
//! Part of the `profile_generation` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::io::{
    ensure_directory, ensure_not_symlink, ensure_regular_file, invalid, lower_hex, read_json,
    relative_path, storage, validate_generation_id, validate_relative_path,
};
use super::types::{
    ActivePointer, GenerationArtifact, GenerationManifest, MANIFEST_FORMAT, MAX_MANIFEST_BYTES,
    POINTER_FORMAT, PROTOCOL_VERSION,
};
use super::*;

pub(super) fn validate_staged_handle(staged: &StagedProfileGeneration) -> Result<()> {
    validate_generation_id(&staged.id)?;
    let expected = staged.profile_root.join(STAGING_DIR).join(&staged.id);
    if staged.path != expected || !staged.path.is_dir() {
        return Err(invalid("temporary generation handle is invalid"));
    }
    ensure_not_symlink(&staged.path, "temporary generation cannot be a symlink")
}

pub(super) fn validate_pointer(pointer: &ActivePointer) -> Result<()> {
    if pointer.format != POINTER_FORMAT || pointer.version != PROTOCOL_VERSION {
        return Err(invalid("unsupported profile generation pointer"));
    }
    validate_generation_id(&pointer.generation)
}

pub(super) fn validate_published_target(path: &Path, id: &str) -> Result<()> {
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

pub(super) fn verify_sqlite_image(path: &Path) -> Result<()> {
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

pub(super) fn verify_sealed_generation(path: &Path, id: &str) -> Result<()> {
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

pub(super) fn scan_artifacts(root: &Path) -> Result<Vec<GenerationArtifact>> {
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

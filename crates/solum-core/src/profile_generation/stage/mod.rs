//! `stage` -- begin and seal. `begin_profile_generation` allocates a caller-writable staging directory under the *same* profile root as the eventual generation, with an OS-random path-safe id, so a restore writes beside its target rather than across the filesystem. `seal_profile_generation` is where the sqlite image is verified and the id stops being provisional.
//!
//! Part of the `profile_generation` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::io::{ensure_regular_file, invalid, random_id, storage};
use super::types::{GenerationManifest, MANIFEST_FORMAT, MAX_MANIFEST_BYTES, PROTOCOL_VERSION};
use super::verify::{scan_artifacts, validate_staged_handle, verify_sqlite_image};
use super::*;

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

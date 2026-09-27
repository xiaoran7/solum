//! `publish` -- resume and publish: the two steps that survive a process death mid-restore. `resume_sealed_profile_generation` rebuilds a sealed handle from an id the restore coordinator already recorded in its own journal -- directory discovery never activates an arbitrary orphan by itself, because an orphan directory is not evidence that anyone meant to restore it. `publish_profile_generation` is the atomic pointer swap.
//!
//! Part of the `profile_generation` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::io::{invalid, path_entry_exists, read_json, storage, validate_generation_id};
use super::types::{ActivePointer, MAX_POINTER_BYTES, POINTER_FORMAT, PROTOCOL_VERSION};
use super::verify::{validate_pointer, validate_published_target, verify_sealed_generation};
use super::*;

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

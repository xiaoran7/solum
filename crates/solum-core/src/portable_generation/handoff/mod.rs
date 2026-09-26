//! `handoff` -- publishing a sealed generation, and the notification handoff that survives it.
//!
//! Part of the `portable_generation` module; see `mod.rs` for the
//! contract. Moved verbatim from the single-file `portable_generation.rs`.

use super::*;

use super::stage::SpoolRecord;
use super::verify::{ensure_directory, invalid, lower_hex, read_bounded_json, storage, write_new};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortablePublishOutcome {
    pub generation: String,
    pub late_items: usize,
    /// The pointer is already committed when this is true. Startup must resume
    /// the durable handoff before opening the new Store.
    pub handoff_pending: bool,
}

/// Atomically publish one WebView-ready portable generation while preserving
/// notification intake that landed in the old active data directory. The
/// caller must already have frozen both live SQLite connections.
pub fn publish_portable_generation(
    profile_root: &Path,
    generation: &str,
) -> Result<PortablePublishOutcome> {
    with_generation_lock(profile_root, || {
        let sealed = resume_sealed_profile_generation_unlocked(profile_root, generation)?;
        let active_before = resolve_active_data_dir_unlocked(profile_root)?;
        let already_active = active_before.generation_id() == Some(generation);
        let late_items = if already_active {
            0
        } else {
            prepare_restore_handoff(profile_root, generation, active_before.path())?
        };
        let active = publish_profile_generation_unlocked(&sealed)?;
        let handoff_pending =
            merge_restore_handoff(profile_root, generation, active.path()).is_err();
        Ok(PortablePublishOutcome {
            generation: generation.to_owned(),
            late_items,
            handoff_pending,
        })
    })
}

/// Complete a pointer-committed handoff before the active Store or ticker is
/// opened. Missing handoff state is a normal no-op; malformed state is fatal.
pub fn resume_portable_handoff(profile_root: &Path, generation: &str) -> Result<usize> {
    with_generation_lock(profile_root, || {
        let active = resolve_active_data_dir_unlocked(profile_root)?;
        if active.generation_id() != Some(generation) {
            return Err(invalid(
                "restore handoff does not match the active profile generation",
            ));
        }
        merge_restore_handoff(profile_root, generation, active.path())
    })
}

pub(super) fn prepare_restore_handoff(
    profile_root: &Path,
    generation: &str,
    source_root: &Path,
) -> Result<usize> {
    let handoff = restore_handoff_path(profile_root, generation)?;
    let mut count = 0usize;
    collect_spool_handoff(source_root, &handoff, false, &mut count)?;
    collect_spool_handoff(source_root, &handoff, true, &mut count)?;
    collect_legacy_inbox_handoff(source_root, &handoff, &mut count)?;
    collect_overflow_handoff(source_root, &handoff, &mut count)?;
    if handoff.exists() {
        sync_tree_directories(&handoff)?;
        crate::fsatomic::sync_directory(
            handoff
                .parent()
                .ok_or_else(|| invalid("restore handoff parent is missing"))?,
        )?;
    }
    Ok(count)
}

fn collect_spool_handoff(
    source_root: &Path,
    handoff: &Path,
    failed: bool,
    count: &mut usize,
) -> Result<()> {
    let mut source = source_root.join(NOTIFICATION_SPOOL_DIR);
    if failed {
        source.push(FAILED_NOTIFICATION_DIR);
    }
    if !source.exists() {
        return Ok(());
    }
    ensure_directory(&source, "active notification spool directory")?;
    let mut entries = std::fs::read_dir(&source)
        .map_err(|error| storage("active notification spool directory", error))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| storage("active notification spool directory", error))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if !failed && entry.file_name() == FAILED_NOTIFICATION_DIR {
            continue;
        }
        let path = entry.path();
        if failed || entry.file_name() != FAILED_NOTIFICATION_DIR {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let record: SpoolRecord =
                read_bounded_json(&path, MAX_HANDOFF_FILE_BYTES, "active notification spool")?;
            stage_handoff_record(handoff, failed, &record, count)?;
        }
    }
    Ok(())
}

fn collect_legacy_inbox_handoff(
    source_root: &Path,
    handoff: &Path,
    count: &mut usize,
) -> Result<()> {
    for name in LEGACY_INBOX_FILES {
        let path = source_root.join(name);
        if !path.exists() {
            continue;
        }
        let bytes = read_bounded_bytes(&path, MAX_HANDOFF_FILE_BYTES, "legacy notification inbox")?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| invalid("legacy notification inbox is not UTF-8"))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: SpoolRecord = serde_json::from_str(line)
                .map_err(|_| invalid("legacy notification inbox is invalid"))?;
            stage_handoff_record(handoff, false, &record, count)?;
        }
    }
    Ok(())
}

fn collect_overflow_handoff(source_root: &Path, handoff: &Path, count: &mut usize) -> Result<()> {
    let directory = source_root.join("notif-overflow");
    if directory.exists() {
        ensure_directory(&directory, "active notification overflow directory")?;
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|error| storage("active notification overflow directory", error))?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|error| storage("active notification overflow directory", error))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "mark") {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| storage("active notification overflow marker", error))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid("active notification overflow marker is invalid"));
            }
            stage_handoff_marker(
                handoff,
                entry.file_name().to_string_lossy().as_bytes(),
                count,
            )?;
        }
    }
    for name in LEGACY_OVERFLOW_FILES {
        let path = source_root.join(name);
        if !path.exists() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| storage("legacy notification overflow", error))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_HANDOFF_ITEMS as u64
        {
            return Err(invalid(
                "legacy notification overflow is invalid or too large",
            ));
        }
        for index in 0..metadata.len() {
            stage_handoff_marker(handoff, format!("{name}#{index}").as_bytes(), count)?;
        }
    }
    Ok(())
}

fn stage_handoff_record(
    handoff: &Path,
    failed: bool,
    record: &SpoolRecord,
    count: &mut usize,
) -> Result<()> {
    reserve_handoff_item(count)?;
    let mut encoded = serde_json::to_vec(record)?;
    encoded.push(b'\n');
    let mut identity = Vec::with_capacity(encoded.len() + 1);
    identity.push(u8::from(failed));
    identity.extend_from_slice(&encoded);
    let name = format!("late-{}.json", lower_hex(&Sha256::digest(&identity)));
    let mut target = handoff.join(NOTIFICATION_SPOOL_DIR);
    if failed {
        target.push(FAILED_NOTIFICATION_DIR);
    }
    write_idempotent(&target.join(name), &encoded, "restore notification handoff")
}

fn stage_handoff_marker(handoff: &Path, identity: &[u8], count: &mut usize) -> Result<()> {
    reserve_handoff_item(count)?;
    let name = format!("late-{}.mark", lower_hex(&Sha256::digest(identity)));
    write_idempotent(
        &handoff.join("notif-overflow").join(name),
        &[],
        "restore overflow handoff",
    )
}

fn reserve_handoff_item(count: &mut usize) -> Result<()> {
    *count = count
        .checked_add(1)
        .ok_or_else(|| invalid("restore handoff item count overflow"))?;
    if *count > MAX_HANDOFF_ITEMS {
        return Err(invalid("restore handoff exceeds its item-count limit"));
    }
    Ok(())
}

fn merge_restore_handoff(
    profile_root: &Path,
    generation: &str,
    active_root: &Path,
) -> Result<usize> {
    let handoff = restore_handoff_path(profile_root, generation)?;
    if !handoff.exists() {
        return Ok(0);
    }
    ensure_directory(&handoff, "restore handoff directory")?;
    let mut count = 0usize;
    merge_handoff_spool(&handoff, active_root, false, &mut count)?;
    merge_handoff_spool(&handoff, active_root, true, &mut count)?;
    merge_handoff_markers(&handoff, active_root, &mut count)?;
    sync_tree_directories(active_root)?;
    let expected = profile_root.join(RESTORE_HANDOFF_DIR).join(generation);
    if handoff != expected {
        return Err(invalid("restore handoff cleanup target is invalid"));
    }
    std::fs::remove_dir_all(&handoff)
        .map_err(|error| storage("completed restore handoff", error))?;
    crate::fsatomic::sync_directory(&profile_root.join(RESTORE_HANDOFF_DIR))?;
    Ok(count)
}

fn merge_handoff_spool(
    handoff: &Path,
    active_root: &Path,
    failed: bool,
    count: &mut usize,
) -> Result<()> {
    let mut source = handoff.join(NOTIFICATION_SPOOL_DIR);
    let mut target = active_root.join(NOTIFICATION_SPOOL_DIR);
    if failed {
        source.push(FAILED_NOTIFICATION_DIR);
        target.push(FAILED_NOTIFICATION_DIR);
    }
    if !source.exists() {
        return Ok(());
    }
    ensure_directory(&source, "restore notification handoff directory")?;
    let mut entries = std::fs::read_dir(&source)
        .map_err(|error| storage("restore notification handoff directory", error))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| storage("restore notification handoff directory", error))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if !failed && entry.file_name() == FAILED_NOTIFICATION_DIR {
            continue;
        }
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("restore notification handoff name is not UTF-8"))?;
        if !name.starts_with("late-") || !name.ends_with(".json") {
            return Err(invalid(
                "restore notification handoff contains an invalid entry",
            ));
        }
        let bytes = read_bounded_bytes(
            &path,
            MAX_HANDOFF_FILE_BYTES,
            "restore notification handoff",
        )?;
        serde_json::from_slice::<SpoolRecord>(&bytes)
            .map_err(|_| invalid("restore notification handoff payload is invalid"))?;
        reserve_handoff_item(count)?;
        write_idempotent(&target.join(name), &bytes, "active notification spool")?;
    }
    Ok(())
}

fn merge_handoff_markers(handoff: &Path, active_root: &Path, count: &mut usize) -> Result<()> {
    let source = handoff.join("notif-overflow");
    if !source.exists() {
        return Ok(());
    }
    ensure_directory(&source, "restore overflow handoff directory")?;
    let mut entries = std::fs::read_dir(&source)
        .map_err(|error| storage("restore overflow handoff directory", error))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| storage("restore overflow handoff directory", error))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("restore overflow handoff name is not UTF-8"))?;
        if !name.starts_with("late-") || !name.ends_with(".mark") {
            return Err(invalid(
                "restore overflow handoff contains an invalid entry",
            ));
        }
        let bytes = read_bounded_bytes(&path, 0, "restore overflow handoff marker")?;
        reserve_handoff_item(count)?;
        write_idempotent(
            &active_root.join("notif-overflow").join(name),
            &bytes,
            "active notification overflow marker",
        )?;
    }
    Ok(())
}

fn restore_handoff_path(profile_root: &Path, generation: &str) -> Result<PathBuf> {
    if generation.len() != 32
        || !generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("restore handoff generation id is invalid"));
    }
    Ok(profile_root.join(RESTORE_HANDOFF_DIR).join(generation))
}

fn read_bounded_bytes(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| storage(label, error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(invalid(format!("{label} is invalid or too large")));
    }
    let bytes = std::fs::read(path).map_err(|error| storage(label, error))?;
    if bytes.len() as u64 > limit {
        return Err(invalid(format!("{label} exceeds its size limit")));
    }
    Ok(bytes)
}

fn write_idempotent(path: &Path, bytes: &[u8], label: &str) -> Result<()> {
    if path.exists() {
        let existing = read_bounded_bytes(path, bytes.len() as u64, label)?;
        if existing != bytes {
            return Err(invalid(format!("{label} conflicts with an existing file")));
        }
        return Ok(());
    }
    write_new(path, bytes, label)
}

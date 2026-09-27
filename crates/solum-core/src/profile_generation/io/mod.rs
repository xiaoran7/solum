//! `io` -- the filesystem primitives everything else is built on: the profile-root lock, path handling, and the small JSON/hex/id helpers. `with_generation_lock` is the only writer of the pointer, which is what makes the swap atomic in the sense that matters -- no second writer can interleave. The lock has two implementations because Android needs `F_OFD_SETLKW` and everything else uses `fs2`; both live here, adjacent, so the pair is edited together.
//!
//! Part of the `profile_generation` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

pub(super) fn relative_path(root: &Path, path: &Path) -> Result<String> {
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

pub(super) fn validate_relative_path(value: &str) -> Result<()> {
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

pub(super) fn validate_generation_id(id: &str) -> Result<()> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("profile generation id is invalid"));
    }
    Ok(())
}

pub(super) fn ensure_directory(path: &Path, message: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid(message))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(message));
    }
    Ok(())
}

pub(super) fn ensure_regular_file(path: &Path, message: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid(message))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid(message));
    }
    Ok(())
}

pub(super) fn ensure_not_symlink(path: &Path, message: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid(message));
    }
    Ok(())
}

pub(super) fn path_entry_exists(path: &Path) -> Result<bool> {
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

pub(super) fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: u64,
    what: &str,
) -> Result<T> {
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

pub(super) fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| invalid("operating-system random source is unavailable"))?;
    Ok(lower_hex(&bytes))
}

pub(super) fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn storage(error: std::io::Error) -> CoreError {
    CoreError::Storage(error.to_string())
}

pub(super) fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid(message.into())
}

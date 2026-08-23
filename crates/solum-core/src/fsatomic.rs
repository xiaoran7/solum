//! Durable file replacement — write to a temp file, fsync, rename.
//!
//! Every config Solum owns was written with a plain `std::fs::write`, which
//! truncates the target and *then* writes. Lose power, run out of disk, or get
//! killed in between, and the file on disk is a valid-looking but truncated
//! JSON document. What that costs depends on which file it was:
//!
//! - `solum-llm.json` / `solum-soulous.json` parse-fail → the feature silently
//!   reports itself as "not configured", and the user re-enters a key;
//! - the mail config → the account becomes unreadable;
//! - `notif-policy.json` → the native listener fails closed and capture stops.
//!
//! None of those are recoverable by the user, because nothing tells them the
//! file was damaged rather than never written. Renaming a fully-written temp
//! file over the target removes the window entirely: a reader sees either the
//! whole old file or the whole new one.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{CoreError, Result};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Replace `path` with `contents`, atomically.
///
/// The `fsync` before the rename is the part that is easy to omit and matters
/// on power loss: without it the rename can reach disk before the data it
/// points at, leaving a file that is atomically *empty*.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let fail = |what: &str, e: std::io::Error| {
        CoreError::Invalid(format!("{what} {} 失败: {e}", path.display()))
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| fail("创建目录", e))?;
    }
    // Same directory as the target: rename is only atomic within a filesystem,
    // and the system temp dir is often a different one.
    let tmp = tmp_sibling(path);
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| fail("创建临时文件", e))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| fail("写入临时文件", e))?;
        file.sync_all().map_err(|e| fail("同步临时文件", e))?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(fail("替换", e));
    }
    Ok(())
}

/// Atomically replace a file and, on Unix/Android, fsync the containing
/// directory so the rename itself is durable across sudden power loss.
///
/// Most ordinary configs can tolerate retrying after a lost directory entry;
/// a profile-generation pointer cannot, because it is the commit record for a
/// whole restored image. Keep the stronger (and potentially fallible) contract
/// explicit at those call sites.
pub fn write_atomic_durable(path: &Path, contents: &str) -> Result<()> {
    write_atomic(path, contents)?;
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        sync_directory(parent)?;
    }
    Ok(())
}

/// Persist directory entries on platforms where opening a directory and
/// calling fsync is supported. Windows has no equivalent through `std`; its
/// rename remains atomic, while Android/Linux receives the stronger durability
/// guarantee required by profile publication.
pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| CoreError::Invalid(format!("同步 generation 目录失败: {error}")))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Each call gets its own sibling. The PID separates processes and the counter
/// separates concurrent writers inside one process.
fn tmp_sibling(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_contents_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("solum-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("cfg.json");

        write_atomic(&target, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\":1}");

        // Overwriting an existing file is the case that used to truncate.
        write_atomic(&target, "{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\":2}");

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must not survive");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = std::env::temp_dir().join(format!("solum-atomic-mk-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let target = dir.join("nested").join("cfg.json");
        write_atomic(&target, "ok").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "ok");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn durable_replacement_is_readable_after_directory_sync() {
        let dir = std::env::temp_dir().join(format!("solum-atomic-durable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("pointer.json");
        write_atomic_durable(&target, "{\"generation\":1}").unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "{\"generation\":1}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_writers_do_not_share_a_temp_path() {
        let dir = std::env::temp_dir().join(format!("solum-atomic-race-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = std::sync::Arc::new(dir.join("cfg.json"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|index| {
                let target = target.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    write_atomic(&target, &format!("writer-{index}")).unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert!(std::fs::read_to_string(&*target)
            .unwrap()
            .starts_with("writer-"));
        assert!(std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp")));
        std::fs::remove_dir_all(&dir).ok();
    }
}

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

use crate::error::{CoreError, Result};

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

/// `foo.json` → `foo.json.<pid>.tmp`. The pid keeps two processes writing the
/// same config from clobbering each other's temp file mid-write.
fn tmp_sibling(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
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
}

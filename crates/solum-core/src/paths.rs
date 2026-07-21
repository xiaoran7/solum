//! Where Solum's data lives on desktop, and how it got there.
//!
//! The database and the credential-bearing configs used to default to the
//! **current working directory**. That made "which database am I using" a
//! function of how the app happened to be launched: a shortcut with a
//! different working directory, a double-click from Explorer, or an autostart
//! entry would each silently open (or create) a different, empty store. The
//! user has no way to tell that apart from data loss.
//!
//! So desktop now resolves to a per-user app-data directory, and — this is the
//! part that makes it safe to change — **`solum-cli` resolves to the same one
//! by calling this same function**. The point of the old cwd default was that
//! the CLI and the desktop shell shared one store; that property is preserved,
//! it just no longer depends on where you were standing when you launched.
//!
//! Deliberately not a dependency. `dirs`/`directories` would do this, but the
//! rule is a handful of environment variables per platform, and this project
//! has already paid for one avoidable native dependency (see PITFALLS
//! 2026-07-21 on `native-tls`). Mobile is unaffected: the shell keeps using
//! the platform-provided app-data directory.

use std::path::PathBuf;

/// Bundle identifier, matching `tauri.conf.json`. Desktop Tauri derives its own
/// app-data path from this, so using the same string keeps the two in step.
const APP_ID: &str = "dev.solum.app";

/// Per-user application data directory, created if missing.
///
/// Returns `None` only when the platform's home/app-data variables are all
/// absent, which in practice means a stripped environment. Callers fall back to
/// the old cwd behaviour there rather than failing to start.
pub fn app_data_dir() -> Option<PathBuf> {
    let base = platform_base()?;
    let dir = base.join(APP_ID);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn platform_base() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };

    #[cfg(target_os = "windows")]
    {
        // Roaming, matching Tauri's desktop `app_data_dir`.
        var("APPDATA")
    }
    #[cfg(target_os = "macos")]
    {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
    }
}

/// Resolve one file into app-data, adopting an existing copy from the legacy
/// cwd location on first run.
///
/// Adoption is the whole reason this is a function rather than a path join: a
/// user who already has `./solum.sqlite` must not launch the new build and be
/// greeted by an empty store. Moving is preferred over copying so there is
/// exactly one live file afterwards — two diverging stores is the failure this
/// change exists to prevent.
///
/// Falls back to `name` (i.e. cwd, previous behaviour) when app-data cannot be
/// resolved at all.
pub fn resolve_with_adoption(name: &str) -> PathBuf {
    let Some(dir) = app_data_dir() else {
        return PathBuf::from(name);
    };
    let target = dir.join(name);
    if target.exists() {
        return target;
    }
    let legacy = PathBuf::from(name);
    if legacy.is_file() {
        // Failure here is not fatal: we simply keep using app-data (empty) and
        // leave the legacy file untouched for the user to move by hand. Losing
        // the file to a half-completed move would be far worse than a visible
        // "why is this empty".
        match std::fs::rename(&legacy, &target) {
            Ok(()) => eprintln!("[paths] 已接管 {} → {}", legacy.display(), target.display()),
            Err(e) => eprintln!(
                "[paths] 无法把 {} 迁移到 {}（{e}）；旧文件保持原样，请手动移动",
                legacy.display(),
                target.display()
            ),
        }
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_data_dir_is_under_the_bundle_identifier() {
        // Only assert the shape; the base differs per platform and per machine.
        if let Some(dir) = app_data_dir() {
            assert!(dir.ends_with(APP_ID), "got {}", dir.display());
            assert!(dir.is_dir(), "app_data_dir must create the directory");
        }
    }

    #[test]
    fn resolving_without_a_legacy_file_just_points_into_app_data() {
        let name = format!("solum-paths-test-{}.json", std::process::id());
        let p = resolve_with_adoption(&name);
        assert!(p.ends_with(&name));
        assert!(
            !p.exists(),
            "resolution must not create the file, only the directory"
        );
    }
}

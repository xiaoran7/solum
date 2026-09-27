// crates/solum-app/src/profile.rs
use std::path::{Path, PathBuf};
// `app.path()` needs the `Manager` trait in scope, and every such call is
// inside a `#[cfg(mobile)]` arm. The desktop build has no user, so the import
// has to carry the same gate or `clippy -D warnings` fails on an import that is
// genuinely needed for the release target.
#[cfg(mobile)]
use tauri::Manager;

pub(crate) struct ResolvedProfilePaths {
    pub(crate) db_path: String,
    /// Device credentials/config stay at the profile root even after the user
    /// data plane switches to a restored generation.
    pub(crate) config_dir: PathBuf,
    pub(crate) active_generation: Option<String>,
}

/// Resolve both the active user-data SQLite and the device-config root.
/// `SOLUM_DB` always wins. Mobile follows the durable generation pointer when
/// present; desktop remains on the migration-era single profile path.
///
/// Both branches first adopt a `pa.sqlite` left by the 2026-07-20 rename, so an
/// existing store keeps its data rather than starting empty. On Android this is
/// effectively a no-op — the identifier changed too, so the old install's data
/// is in another package's private storage (see `adopt_legacy_db`).
pub(crate) fn resolve_profile_paths(
    #[cfg_attr(desktop, allow(unused_variables))] app: &tauri::App,
) -> solum_core::Result<ResolvedProfilePaths> {
    if let Ok(p) = std::env::var("SOLUM_DB") {
        let config_dir = std::path::Path::new(&p)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
            .to_path_buf();
        return Ok(ResolvedProfilePaths {
            db_path: p,
            config_dir,
            active_generation: None,
        });
    }
    #[cfg(mobile)]
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let profile_root = if let Some(profile) = account_profile_root(&dir) {
            profile
        } else {
            let current = dir.join("solum.sqlite");
            report_legacy_adoption(&dir.join("pa.sqlite"), &current);
            dir
        };
        let active = solum_core::profile_generation::resolve_active_data_dir(&profile_root)?;
        let active_generation = active.generation_id().map(str::to_owned);
        if let Some(generation) = active_generation.as_deref() {
            solum_core::portable_generation::resume_portable_handoff(&profile_root, generation)?;
        }
        if let Some(session) = solum_core::account::AccountSession::load_full() {
            if let Some(user_id) = session.stable_user_id() {
                solum_core::backup_takeover::reconcile_published_takeover(
                    &profile_root,
                    user_id,
                    &session.server_url,
                    active_generation.as_deref(),
                )
                .map_err(|_| {
                    solum_core::CoreError::Invalid(
                        "backup takeover journal failed startup reconciliation".into(),
                    )
                })?;
            }
        }
        return Ok(ResolvedProfilePaths {
            db_path: active
                .path()
                .join(solum_core::profile_generation::SQLITE_FILE)
                .to_string_lossy()
                .into_owned(),
            config_dir: profile_root,
            active_generation,
        });
    }
    // Desktop: app-data, not the working directory.
    //
    // Two adoptions can apply, in order: the pre-rename `pa.sqlite` (2026-07-20)
    // and the pre-app-data `./solum.sqlite` (2026-07-21). Run the rename
    // adoption first *in the old location*, so a user who skipped a release
    // still ends up with one store rather than two half-populated ones.
    //
    // `solum-cli` calls the same resolver, so the "desktop and CLI share one
    // database" property the cwd default existed for is preserved — it just no
    // longer depends on the launch directory.
    report_legacy_adoption(
        std::path::Path::new("pa.sqlite"),
        std::path::Path::new("solum.sqlite"),
    );
    let base = solum_core::paths::app_data_dir();
    if let Some(profile) = base.as_deref().and_then(account_profile_root) {
        let db_path = profile.join("solum.sqlite").to_string_lossy().into_owned();
        return Ok(ResolvedProfilePaths {
            db_path,
            config_dir: profile,
            active_generation: None,
        });
    }
    let db_path = solum_core::paths::resolve_with_adoption("solum.sqlite")
        .to_string_lossy()
        .into_owned();
    let config_dir = std::path::Path::new(&db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""))
        .to_path_buf();
    Ok(ResolvedProfilePaths {
        db_path,
        config_dir,
        active_generation: None,
    })
}

pub(crate) fn active_local_profile_id() -> String {
    solum_core::account::AccountSession::load_full()
        .and_then(|session| session.stable_user_id().map(str::to_owned))
        .unwrap_or_else(|| "guest".to_string())
}

pub(crate) fn account_profile_root(base: &Path) -> Option<PathBuf> {
    let session = solum_core::account::AccountSession::load_full()?;
    solum_core::paths::account_profile_dir(base, session.stable_user_id()?)
}

/// Set a profile-scoped config path while preserving a path explicitly supplied
/// by the operator. The marker is inherited by `app.restart()`, allowing paths
/// created by Solum itself to follow the newly selected profile on next boot.
pub(crate) fn set_managed_profile_config(
    variable: &str,
    marker: &str,
    profile_dir: Option<&Path>,
    filename: &str,
) {
    let managed = std::env::var(marker).as_deref() == Ok("1");
    if !managed && std::env::var_os(variable).is_some() {
        return;
    }
    let Some(dir) = profile_dir else { return };
    std::env::set_var(variable, dir.join(filename));
    std::env::set_var(marker, "1");
}

/// Adoption failing must not stop the app from starting — a fresh store is a
/// worse outcome than a loud log, but a dead shell is worse than both. Log and
/// carry on; the untouched legacy file is still on disk either way.
pub(crate) fn report_legacy_adoption(legacy: &Path, current: &Path) {
    match solum_core::store::adopt_legacy_db(legacy, current) {
        Ok(true) => eprintln!("[db] 已接管改名前的数据库 {legacy:?} → {current:?}"),
        Ok(false) => {}
        Err(e) => eprintln!("[db] 改名前数据库接管失败（将使用 {current:?}）: {e}"),
    }
}

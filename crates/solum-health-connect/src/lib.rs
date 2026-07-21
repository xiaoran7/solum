//! Tiny app-local Tauri plugin: F5 (Phase 4, ARCHITECTURE.md §3.7) reads
//! heart rate / steps / sleep from Android's Health Connect. Health Connect
//! permissions are a special category with no standard runtime dialog — the
//! grant flow is an `Intent` built from `PermissionController`'s
//! `ActivityResultContract`, which only the platform (Kotlin) side can drive.
//! Lives in this repo rather than crates.io because nothing outside solum-app
//! uses it — same rationale as `solum-notif-access`.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(target_os = "android")]
mod mobile;

mod error;
pub use error::{Error, Result};

#[cfg(not(target_os = "android"))]
use desktop::HealthConnect;
#[cfg(target_os = "android")]
use mobile::HealthConnect;
#[cfg(target_os = "android")]
pub use mobile::RawSample;

/// Desktop stub also exposes `RawSample` so `solum-app` can share one call site
/// across platforms without `#[cfg]`-gating the type import itself.
#[cfg(not(target_os = "android"))]
pub use desktop::RawSample;

pub trait HealthConnectExt<R: Runtime> {
    fn health_connect(&self) -> &HealthConnect<R>;
}

impl<R: Runtime, T: Manager<R>> HealthConnectExt<R> for T {
    fn health_connect(&self) -> &HealthConnect<R> {
        self.state::<HealthConnect<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("solum-health-connect")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let inst = mobile::init(app, api)?;
            #[cfg(not(target_os = "android"))]
            let inst = desktop::init(app, api)?;
            app.manage(inst);
            Ok(())
        })
        .build()
}

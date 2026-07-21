//! Tiny app-local Tauri plugin: Android's notification-listener permission
//! (needed by F1 notification capture, see `SolumNotificationListener.kt`) has
//! no runtime-request dialog — only a Settings deep link, and no way to read
//! its state except `NotificationManagerCompat.getEnabledListenerPackages`.
//! Both need a few lines of Kotlin, so — same as `tauri-plugin-notification`
//! — that native code has to be a real Tauri plugin (Cargo `links` + a
//! `tauri-plugin` build script) for `cargo tauri android build` to discover
//! and wire the Kotlin module in. It lives in this repo rather than
//! crates.io because nothing outside solum-app uses it.

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

/// Android foreground-service state. `supported` is an explicit capability
/// projection so the shared shell never presents Android-only controls on a
/// desktop target.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct PipelineStatus {
    pub supported: bool,
    pub running: bool,
    pub ignoring_battery_optimizations: bool,
}

/// A launchable Android application that can be selected as a notification
/// source. The package remains necessary for the listener's policy file, but
/// the shell should present `name` to people instead of asking them to know
/// Android implementation identifiers.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub name: String,
    pub package_name: String,
}

#[cfg(not(target_os = "android"))]
use desktop::NotifAccess;
#[cfg(target_os = "android")]
use mobile::NotifAccess;

pub trait NotifAccessExt<R: Runtime> {
    fn notif_access(&self) -> &NotifAccess<R>;
}

impl<R: Runtime, T: Manager<R>> NotifAccessExt<R> for T {
    fn notif_access(&self) -> &NotifAccess<R> {
        self.state::<NotifAccess<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("solum-notif-access")
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

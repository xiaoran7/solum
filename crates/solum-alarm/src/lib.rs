//! Tiny app-local Tauri plugin: on Android the in-process ticker dies with
//! the app, so reminders scheduled in SQLite would only fire on the next
//! launch. This plugin mirrors the pending reminder set into OS-level
//! `AlarmManager` alarms — the system wakes a `BroadcastReceiver` at
//! `fire_at` and posts the notification even when the Solum process is dead
//! (F2 delivery + F16 reliability). Alarms are re-armed after reboot from a
//! persisted schedule file. Desktop is a no-op stub: the resident ticker
//! already delivers there, and nothing kills it mid-run.
//!
//! Division of labor (see solum-app's ticker): AlarmManager owns the
//! OS-visible reminder toast on Android; the ticker/`fire_due` stays the
//! single writer of reminder *state* (mark-fired + journal) and never posts
//! OS notifications on Android — so there is exactly one delivery surface
//! per platform and no double-fire.

use serde::Serialize;
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
use desktop::Alarm;
#[cfg(target_os = "android")]
use mobile::Alarm;

/// One OS alarm to arm: `at_ms` is a wall-clock epoch instant; `title`/`body`
/// are the ready-to-show notification strings (the receiver renders them
/// verbatim — no logic lives on the Kotlin side).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlarmSpec {
    /// The solum-core notification row id — doubles as the OS request code, so
    /// re-syncing the same reminder replaces rather than duplicates it.
    pub id: i64,
    pub at_ms: i64,
    pub title: String,
    pub body: String,
}

pub trait AlarmExt<R: Runtime> {
    fn alarm(&self) -> &Alarm<R>;
}

impl<R: Runtime, T: Manager<R>> AlarmExt<R> for T {
    fn alarm(&self) -> &Alarm<R> {
        self.state::<Alarm<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("solum-alarm")
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

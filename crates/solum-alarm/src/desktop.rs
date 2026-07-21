//! Desktop (and any non-Android target) has no AlarmManager — and doesn't
//! need one: the resident ticker delivers reminders there and nothing kills
//! it mid-run. Report "unavailable" so solum-app skips the sync entirely.

use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::AlarmSpec;

pub struct Alarm<R: Runtime>(#[allow(dead_code)] AppHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<Alarm<R>> {
    Ok(Alarm(app.clone()))
}

impl<R: Runtime> Alarm<R> {
    pub fn is_available(&self) -> crate::Result<bool> {
        Ok(false)
    }

    /// No-op; returns `exact = true` so callers never see a degraded state
    /// on a platform where the concept doesn't apply.
    pub fn sync(&self, _alarms: Vec<AlarmSpec>) -> crate::Result<bool> {
        Ok(true)
    }
}

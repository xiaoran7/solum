use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::{AlarmHealth, AlarmSpec};

const PLUGIN_IDENTIFIER: &str = "dev.solum.alarm";

pub struct Alarm<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<Alarm<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "AlarmPlugin")?;
    Ok(Alarm(handle))
}

#[derive(Serialize)]
struct SyncArgs {
    alarms: Vec<AlarmSpec>,
}

#[derive(Serialize)]
struct HealthArgs {}

#[derive(Deserialize)]
struct SyncResponse {
    /// Whether exact scheduling was used (`false` = the user revoked the
    /// exact-alarm special access; alarms still fire, just fuzzily).
    exact: bool,
}

impl<R: Runtime> Alarm<R> {
    pub fn is_available(&self) -> crate::Result<bool> {
        self.health().map(|health| health.receiver_registered)
    }

    pub fn health(&self) -> crate::Result<AlarmHealth> {
        self.0
            .run_mobile_plugin::<AlarmHealth>("health", HealthArgs {})
            .map_err(Into::into)
    }

    /// Replace the whole OS alarm set with `alarms` (cancel + re-arm; the
    /// Kotlin side persists the set for re-arming after reboot). Returns
    /// whether exact scheduling was available.
    pub fn sync(&self, alarms: Vec<AlarmSpec>) -> crate::Result<bool> {
        self.0
            .run_mobile_plugin::<SyncResponse>("sync", SyncArgs { alarms })
            .map(|r| r.exact)
            .map_err(Into::into)
    }
}

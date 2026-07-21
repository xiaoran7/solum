use serde::{de::DeserializeOwned, Deserialize};
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

const PLUGIN_IDENTIFIER: &str = "dev.solum.notifaccess";

pub struct NotifAccess<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<NotifAccess<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "NotifAccessPlugin")?;
    Ok(NotifAccess(handle))
}

#[derive(Deserialize)]
struct EnabledResponse {
    enabled: bool,
}

#[derive(Deserialize)]
struct PipelineStatusResponse {
    running: bool,
    #[serde(rename = "ignoringBatteryOptimizations")]
    ignoring_battery_optimizations: bool,
}

#[derive(Deserialize)]
struct InstalledAppsResponse {
    apps: Vec<crate::InstalledApp>,
}

impl<R: Runtime> NotifAccess<R> {
    /// Whether Solum's `NotificationListenerService` is in the OS's enabled-
    /// listener set (Settings → Notifications → Notification access).
    pub fn is_enabled(&self) -> crate::Result<bool> {
        self.0
            .run_mobile_plugin::<EnabledResponse>("isEnabled", ())
            .map(|r| r.enabled)
            .map_err(Into::into)
    }

    /// Jump straight to the notification-listener settings page — Android
    /// has no runtime permission-request dialog for this, only this deep
    /// link (the user still has to tap the toggle themselves).
    pub fn open_settings(&self) -> crate::Result<()> {
        self.0
            .run_mobile_plugin::<()>("openSettings", ())
            .map_err(Into::into)
    }

    /// Returns the launchable apps visible to Android's package manager. The
    /// native layer supplies a human-readable label; callers keep package
    /// names internal when saving the notification listener policy.
    pub fn installed_apps(&self) -> crate::Result<Vec<crate::InstalledApp>> {
        self.0
            .run_mobile_plugin::<InstalledAppsResponse>("installedApps", ())
            .map(|response| response.apps)
            .map_err(Into::into)
    }

    pub fn pipeline_status(&self) -> crate::Result<crate::PipelineStatus> {
        self.0
            .run_mobile_plugin::<PipelineStatusResponse>("pipelineStatus", ())
            .map(|response| crate::PipelineStatus {
                supported: true,
                running: response.running,
                ignoring_battery_optimizations: response.ignoring_battery_optimizations,
            })
            .map_err(Into::into)
    }

    pub fn start_pipeline(&self) -> crate::Result<()> {
        self.0
            .run_mobile_plugin::<()>("startPipeline", ())
            .map_err(Into::into)
    }

    pub fn stop_pipeline(&self) -> crate::Result<()> {
        self.0
            .run_mobile_plugin::<()>("stopPipeline", ())
            .map_err(Into::into)
    }

    pub fn request_ignore_battery_optimizations(&self) -> crate::Result<()> {
        self.0
            .run_mobile_plugin::<()>("requestIgnoreBatteryOptimizations", ())
            .map_err(Into::into)
    }

    pub fn open_battery_settings(&self) -> crate::Result<()> {
        self.0
            .run_mobile_plugin::<()>("openBatterySettings", ())
            .map_err(Into::into)
    }

    pub fn open_app_background_settings(&self) -> crate::Result<()> {
        self.0
            .run_mobile_plugin::<()>("openAppBackgroundSettings", ())
            .map_err(Into::into)
    }
}

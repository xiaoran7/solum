//! Desktop (and any non-Android target) has no notification-listener
//! permission model — this is purely an Android concept. Report "enabled"
//! so callers don't show a nag banner, and make the settings jump a no-op.

use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

pub struct NotifAccess<R: Runtime>(#[allow(dead_code)] AppHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<NotifAccess<R>> {
    Ok(NotifAccess(app.clone()))
}

impl<R: Runtime> NotifAccess<R> {
    pub fn is_enabled(&self) -> crate::Result<bool> {
        Ok(true)
    }

    pub fn open_settings(&self) -> crate::Result<()> {
        Ok(())
    }

    /// Desktop does not have Android's notification-listener pipeline, so
    /// there is no honest installed-app picker to show here.
    pub fn installed_apps(&self) -> crate::Result<Vec<crate::InstalledApp>> {
        Ok(Vec::new())
    }

    pub fn pipeline_status(&self) -> crate::Result<crate::PipelineStatus> {
        Ok(crate::PipelineStatus {
            supported: false,
            running: false,
            ignoring_battery_optimizations: false,
        })
    }

    pub fn start_pipeline(&self) -> crate::Result<()> {
        Ok(())
    }
    pub fn stop_pipeline(&self) -> crate::Result<()> {
        Ok(())
    }
    pub fn request_ignore_battery_optimizations(&self) -> crate::Result<()> {
        Err(crate::Error::Unsupported(
            "电池优化设置只支持 Android".into(),
        ))
    }
    pub fn open_battery_settings(&self) -> crate::Result<()> {
        Err(crate::Error::Unsupported("电池设置只支持 Android".into()))
    }
    pub fn open_app_background_settings(&self) -> crate::Result<()> {
        Err(crate::Error::Unsupported(
            "应用后台设置只支持 Android".into(),
        ))
    }
}

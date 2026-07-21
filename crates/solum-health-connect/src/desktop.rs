//! Desktop (and any non-Android target) has no Health Connect — this is
//! purely an Android platform service. Report "unavailable" so callers
//! (the permission banner, the ticker's poll) skip themselves entirely
//! rather than showing a nag for a platform capability that can't exist.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tauri::{plugin::PluginApi, AppHandle, Runtime};

pub struct HealthConnect<R: Runtime>(#[allow(dead_code)] AppHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<HealthConnect<R>> {
    Ok(HealthConnect(app.clone()))
}

/// Mirrors `mobile::RawSample`'s shape so `solum-app` can share one call site
/// across platforms without `#[cfg]`-gating the type import.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawSample {
    pub kind: String,
    pub start: String,
    pub end: String,
    pub value: f64,
}

impl<R: Runtime> HealthConnect<R> {
    pub fn is_available(&self) -> crate::Result<bool> {
        Ok(false)
    }

    pub fn has_permissions(&self) -> crate::Result<bool> {
        Ok(false)
    }

    pub fn request_permissions(&self) -> crate::Result<bool> {
        Ok(false)
    }

    pub fn read_recent(&self, _since_epoch_ms: i64) -> crate::Result<Vec<RawSample>> {
        Ok(Vec::new())
    }
}

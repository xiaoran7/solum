use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

const PLUGIN_IDENTIFIER: &str = "dev.solum.healthconnect";

pub struct HealthConnect<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<HealthConnect<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "HealthConnectPlugin")?;
    Ok(HealthConnect(handle))
}

#[derive(Deserialize)]
struct AvailableResponse {
    available: bool,
}

#[derive(Deserialize)]
struct GrantedResponse {
    granted: bool,
}

/// One reading handed over by the Kotlin side. `kind` is one of
/// `"heart_rate" | "steps" | "sleep"`; `start`/`end` are ISO-8601 instants
/// (Health Connect's own `Instant.toString()`); `value` is beats-per-minute,
/// a step count, or sleep-session minutes depending on `kind`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawSample {
    pub kind: String,
    pub start: String,
    pub end: String,
    pub value: f64,
}

#[derive(Deserialize)]
struct ReadRecentResponse {
    samples: Vec<RawSample>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadRecentArgs {
    since_epoch_ms: i64,
}

impl<R: Runtime> HealthConnect<R> {
    /// Whether the Health Connect SDK/app is present on this device at all
    /// (distinct from whether we've been granted permission).
    pub fn is_available(&self) -> crate::Result<bool> {
        self.0
            .run_mobile_plugin::<AvailableResponse>("isAvailable", ())
            .map(|r| r.available)
            .map_err(Into::into)
    }

    /// Whether all three read permissions are currently granted.
    pub fn has_permissions(&self) -> crate::Result<bool> {
        self.0
            .run_mobile_plugin::<GrantedResponse>("hasPermissions", ())
            .map(|r| r.granted)
            .map_err(Into::into)
    }

    /// Launch Health Connect's own permission-grant screen and block until
    /// the user returns. Returns whether all three ended up granted.
    pub fn request_permissions(&self) -> crate::Result<bool> {
        self.0
            .run_mobile_plugin::<GrantedResponse>("requestPermissions", ())
            .map(|r| r.granted)
            .map_err(Into::into)
    }

    /// Heart rate / steps / sleep records with `start >= since_epoch_ms`.
    pub fn read_recent(&self, since_epoch_ms: i64) -> crate::Result<Vec<RawSample>> {
        self.0
            .run_mobile_plugin::<ReadRecentResponse>(
                "readRecent",
                ReadRecentArgs { since_epoch_ms },
            )
            .map(|r| r.samples)
            .map_err(Into::into)
    }
}

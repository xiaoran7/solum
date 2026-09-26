//! `config` -- the stored Soulous connection: the source constants and `SoulousConfig`, which owns the token, its expiry and the refresh.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::push::token_tail;
use super::*;

pub const SOURCE: &str = "soulous";
/// Source marker for the narrowly-scoped Solum → Soulous outbound contract.
/// It is intentionally distinct from [`SOURCE`], which marks read-only rows
/// pulled *from* Soulous into Solum.
pub const SOLUM_SOURCE: &str = "pa";
pub const SCHEDULE_EVENT_TYPE: &str = "schedule_event";

pub(super) fn default_timeout_secs() -> u64 {
    15
}

/// Local-only connection credentials for the user's own Soulous deployment.
/// The file (`solum-soulous.json`) is gitignored; this type never carries a
/// username/password because Solum reuses Soulous's existing dual-token session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoulousConfig {
    #[serde(alias = "base_url")]
    pub server_url: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// L2 outbound whitelist. It is deliberately opt-in and currently has
    /// only one category: a user-confirmed Solum schedule event.
    #[serde(default)]
    pub push_schedule_events: bool,
}

impl SoulousConfig {
    /// Load a complete config. Missing or malformed local configuration is an
    /// intentional silent-off state, consistent with `solum-llm.json`.
    pub fn load() -> Option<Self> {
        let raw = std::fs::read_to_string(Self::path()).ok()?;
        Self::from_json(&raw).ok()
    }

    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("SOLUM_SOULOUS_CONFIG") {
            return p.into();
        }
        crate::paths::resolve_profile_with_adoption("solum-soulous.json")
    }

    pub fn from_json(raw: &str) -> Result<Self> {
        let mut config: Self = serde_json::from_str(raw)?;
        config.normalize()?;
        Ok(config)
    }

    pub fn normalize(&mut self) -> Result<()> {
        self.server_url = crate::net::validate_endpoint(&self.server_url, "Soulous server_url")?;
        self.access_token = self.access_token.trim().to_string();
        self.refresh_token = self.refresh_token.trim().to_string();
        self.timeout_secs = self.timeout_secs.clamp(3, 120);
        if self.access_token.is_empty() || self.refresh_token.is_empty() {
            return Err(CoreError::Invalid(
                "solum-soulous.json 缺 access_token 或 refresh_token".into(),
            ));
        }
        Ok(())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        // Atomic: a half-written config silently degrades to "not configured".
        crate::fsatomic::write_atomic(path, &text)
            .map_err(|e| CoreError::Soulous(format!("写入 {} 失败: {e}", path.display())))
    }

    pub fn masked_summary(&self) -> String {
        format!(
            "{} · access:{} · refresh:{}",
            self.server_url,
            token_tail(&self.access_token),
            token_tail(&self.refresh_token)
        )
    }
}

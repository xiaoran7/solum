//! config. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

use super::*;

/// Connection settings for an OpenAI-compatible endpoint.
///
/// Per-provider quirks (docs/LLM-PROVIDERS.md) drive the optional fields:
/// OpenAI gpt-5-family rejects any non-default `temperature`, so `null` in
/// the JSON means "don't send the field at all"; thinking-mode models
/// (DeepSeek V4, GLM-5…) need timeouts well past the old hardcoded 30s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// e.g. "https://api.example.com/v1"
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Omitted → 0.3 (historic default). Explicit `null` → field not sent.
    #[serde(default = "default_temperature")]
    pub temperature: Option<f64>,
    /// Omitted → field not sent (provider default applies).
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_model() -> String {
    "gemini-3.8-flash-high".to_string()
}

fn default_temperature() -> Option<f64> {
    Some(0.3)
}

fn default_timeout_secs() -> u64 {
    30
}

impl LlmConfig {
    /// Load from `SOLUM_LLM_BASE_URL`/`SOLUM_LLM_API_KEY`/`SOLUM_LLM_MODEL` (plus the
    /// optional `SOLUM_LLM_TEMPERATURE` — "none" to omit the field —,
    /// `SOLUM_LLM_MAX_TOKENS`, `SOLUM_LLM_TIMEOUT_SECS`), falling back to a JSON
    /// file at `SOLUM_LLM_CONFIG` (default: active account profile, or guest
    /// app-data when logged out).
    /// `None` means "stay offline" — never an error.
    pub fn load() -> Option<LlmConfig> {
        let env = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
        if let (Some(base_url), Some(api_key)) =
            (env("SOLUM_LLM_BASE_URL"), env("SOLUM_LLM_API_KEY"))
        {
            let temperature = match env("SOLUM_LLM_TEMPERATURE") {
                None => default_temperature(),
                Some(s) if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("null") => None,
                Some(s) => s.parse().ok().map(Some).unwrap_or_else(default_temperature),
            };
            return Some(LlmConfig {
                base_url,
                api_key,
                model: env("SOLUM_LLM_MODEL").unwrap_or_else(default_model),
                temperature,
                max_tokens: env("SOLUM_LLM_MAX_TOKENS").and_then(|s| s.parse().ok()),
                timeout_secs: env("SOLUM_LLM_TIMEOUT_SECS")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(default_timeout_secs),
            });
        }
        let path: String = match env("SOLUM_LLM_CONFIG") {
            Some(p) => p,
            None => crate::paths::resolve_profile_with_adoption("solum-llm.json")
                .to_string_lossy()
                .into_owned(),
        };
        let text = std::fs::read_to_string(path).ok()?;
        Self::from_json(&text).ok()
    }

    pub fn from_json(s: &str) -> Result<LlmConfig> {
        let c: LlmConfig = serde_json::from_str(s)?;
        if c.base_url.trim().is_empty() || c.api_key.trim().is_empty() {
            return Err(CoreError::Invalid(
                "solum-llm.json 缺 base_url 或 api_key".into(),
            ));
        }
        Ok(c)
    }

    /// A displayable summary that never leaks the key.
    pub fn masked_summary(&self) -> String {
        let tail: String = self
            .api_key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{} · {} · key:…{}", self.base_url, self.model, tail)
    }
}

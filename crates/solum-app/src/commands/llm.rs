use super::*;

// ---- cloud LLM settings (§3.6; provider survey in docs/LLM-PROVIDERS.md) ----

/// The JSON file the settings UI reads/writes. Matches `LlmConfig::load`'s
/// fallback: explicit `SOLUM_LLM_CONFIG` first, then the active account profile
/// (or the historical app-data path for guest).
const MAX_REPLACEMENT_PREVIEW_ITEMS: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveWebViewSummary {
    pub(crate) profile_id: String,
    pub(crate) chat_sessions: usize,
    pub(crate) chat_messages: usize,
    pub(crate) drafts: usize,
    pub(crate) documents: usize,
    pub(crate) fingerprint: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RestorePublicationPreview {
    profile_id: String,
    generation: String,
    token: String,
    sqlite_counts: Vec<(String, usize)>,
    webview: LiveWebViewSummary,
    has_existing_data: bool,
}

pub(crate) fn validate_live_webview_summary(summary: &LiveWebViewSummary, profile_id: &str) -> CmdResult<()> {
    let total = summary
        .chat_sessions
        .checked_add(summary.chat_messages)
        .and_then(|value| value.checked_add(summary.drafts))
        .and_then(|value| value.checked_add(summary.documents))
        .ok_or_else(|| "WebView replacement summary count overflow".to_string())?;
    if summary.profile_id != profile_id
        || total > MAX_REPLACEMENT_PREVIEW_ITEMS
        || summary.fingerprint.len() != 64
        || !summary
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("WebView replacement summary is invalid".into());
    }
    Ok(())
}

pub(crate) fn random_publication_token() -> CmdResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "OS random source is unavailable".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) fn validate_publication_binding(
    binding: &RestorePublicationBinding,
    token: &str,
    sqlite_fingerprint: &str,
    webview: &LiveWebViewSummary,
) -> CmdResult<()> {
    if binding.token != token
        || binding.sqlite_fingerprint != sqlite_fingerprint
        || binding.webview != *webview
    {
        return Err("Replacement preview is stale; generate a new preview".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn restore_publish_preview(
    state: State<AppState>,
    generation_id: String,
    webview: LiveWebViewSummary,
) -> CmdResult<RestorePublicationPreview> {
    let profile_id = active_local_profile_id();
    validate_live_webview_summary(&webview, &profile_id)?;
    let _orch_guard = state
        .orch
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("Solum profile is switching; reopen the app to continue".into());
    }
    let sync_store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let sqlite = solum_core::export_v3::snapshot_portable_store(&sync_store).map_err(core_err)?;
    let token = random_publication_token()?;
    let mut session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session state is unavailable".to_string())?;
    let session = session
        .as_mut()
        .filter(|session| {
            session.ready
                && session.reader.bootstrap().generation == generation_id
                && session.profile_id == profile_id
        })
        .ok_or_else(|| "WebView restore generation is not ready for publication".to_string())?;
    session.publication_preview = Some(RestorePublicationBinding {
        token: token.clone(),
        sqlite_fingerprint: sqlite.fingerprint,
        webview: webview.clone(),
    });
    let sqlite_total = sqlite.counts.iter().try_fold(0usize, |total, (_, count)| {
        total
            .checked_add(*count)
            .ok_or_else(|| "SQLite replacement summary count overflow".to_string())
    })?;
    let webview_total =
        webview.chat_sessions + webview.chat_messages + webview.drafts + webview.documents;
    Ok(RestorePublicationPreview {
        profile_id,
        generation: generation_id,
        token,
        sqlite_counts: sqlite.counts,
        webview,
        has_existing_data: sqlite_total > 0 || webview_total > 0,
    })
}

#[cfg(target_os = "android")]
pub(crate) fn validate_restore_preview_is_current(
    state: &AppState,
    generation_id: &str,
    token: &str,
    webview: &LiveWebViewSummary,
) -> CmdResult<()> {
    let profile_id = active_local_profile_id();
    validate_live_webview_summary(webview, &profile_id)?;
    let _orch_guard = state
        .orch
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let sync_store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let sqlite = solum_core::export_v3::snapshot_portable_store(&sync_store).map_err(core_err)?;
    let session_guard = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session state is unavailable".to_string())?;
    let session = session_guard
        .as_ref()
        .filter(|session| {
            session.ready
                && session.reader.bootstrap().generation == generation_id
                && session.profile_id == profile_id
        })
        .ok_or_else(|| "WebView restore generation is not ready for publication".to_string())?;
    let binding = session
        .publication_preview
        .as_ref()
        .ok_or_else(|| "Replacement preview is missing or already consumed".to_string())?;
    validate_publication_binding(binding, token, &sqlite.fingerprint, webview)
}

#[cfg(target_os = "android")]
pub(crate) fn publish_restore_generation(
    state: &AppState,
    generation_id: &str,
    token: &str,
    webview: &LiveWebViewSummary,
) -> CmdResult<solum_core::portable_generation::PortablePublishOutcome> {
    let profile_id = active_local_profile_id();
    validate_live_webview_summary(webview, &profile_id)?;
    let _orch_guard = state
        .orch
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let sync_store = state
        .sync_store
        .lock()
        .map_err(|_| "sync store lock poisoned".to_string())?;
    let sqlite = solum_core::export_v3::snapshot_portable_store(&sync_store).map_err(core_err)?;
    let binding = {
        let mut session_guard = state
            .webview_restore
            .lock()
            .map_err(|_| "WebView restore session state is unavailable".to_string())?;
        let session = session_guard
            .as_mut()
            .filter(|session| {
                session.ready
                    && session.reader.bootstrap().generation == generation_id
                    && session.profile_id == profile_id
            })
            .ok_or_else(|| "WebView restore generation is not ready for publication".to_string())?;
        session
            .publication_preview
            .take()
            .ok_or_else(|| "Replacement preview is missing or already consumed".to_string())?
    };
    validate_publication_binding(&binding, token, &sqlite.fingerprint, webview)?;
    solum_core::portable_generation::publish_portable_generation(&state.profile_root, generation_id)
        .map_err(core_err)
}

pub(crate) fn llm_config_file() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("SOLUM_LLM_CONFIG") {
        return p.into();
    }
    solum_core::paths::resolve_profile_with_adoption("solum-llm.json")
}

/// Soulous uses the same local-only config convention as the LLM. On mobile
/// setup points `SOLUM_SOULOUS_CONFIG` next to the database, so the WebView's
/// meaningless cwd never decides where dual tokens are stored.
pub(crate) fn soulous_config_file() -> std::path::PathBuf {
    solum_core::soulous::SoulousConfig::path()
}

/// When both env credentials are set they win over the file at next launch —
/// the UI surfaces this so a save doesn't look mysteriously ignored.
pub(crate) fn llm_env_active() -> bool {
    let set = |k: &str| {
        std::env::var(k)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    };
    set("SOLUM_LLM_BASE_URL") && set("SOLUM_LLM_API_KEY")
}

pub(crate) fn key_tail(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        tail
    } else {
        format!("…{tail}")
    }
}

/// Everything the settings form needs — never the key itself.
#[derive(Serialize)]
pub(crate) struct LlmSettings {
    configured: bool,
    /// "env" | "file"; `None` when unconfigured.
    source: Option<&'static str>,
    /// Where saves land.
    path: String,
    base_url: String,
    model: String,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    timeout_secs: u64,
    /// Last 4 chars of the stored key, e.g. "…zju9".
    key_tail: String,
}

#[tauri::command]
pub(crate) fn llm_config_get() -> CmdResult<LlmSettings> {
    require_full_account()?;
    let path = llm_config_file().to_string_lossy().into_owned();
    match solum_core::llm::LlmConfig::load() {
        Some(c) => Ok(LlmSettings {
            configured: true,
            source: Some(if llm_env_active() { "env" } else { "file" }),
            path,
            base_url: c.base_url,
            model: c.model,
            temperature: c.temperature,
            max_tokens: c.max_tokens,
            timeout_secs: c.timeout_secs,
            key_tail: key_tail(&c.api_key),
        }),
        None => Ok(LlmSettings {
            configured: false,
            source: None,
            path,
            base_url: String::new(),
            model: String::new(),
            temperature: Some(0.3),
            max_tokens: None,
            timeout_secs: 30,
            key_tail: String::new(),
        }),
    }
}

#[derive(Deserialize)]
pub(crate) struct LlmSaveArgs {
    pub(crate) base_url: String,
    /// Empty → reuse the key already stored in the config file, so editing
    /// the model doesn't force re-entering the secret.
    #[serde(default)]
    pub(crate) api_key: String,
    pub(crate) model: String,
    /// `null` = don't send the field (OpenAI gpt-5 family rejects non-default).
    pub(crate) temperature: Option<f64>,
    pub(crate) max_tokens: Option<u32>,
    pub(crate) timeout_secs: u64,
}

pub(crate) fn resolve_llm_args(a: LlmSaveArgs) -> CmdResult<solum_core::llm::LlmConfig> {
    let base_url = solum_core::net::validate_endpoint(&a.base_url, "LLM base_url")
        .map_err(|e| e.to_string())?;
    let model = a.model.trim().to_string();
    if model.is_empty() {
        return Err("请填写模型名".into());
    }
    let api_key = a.api_key.trim().to_string();
    let api_key = if api_key.is_empty() {
        std::fs::read_to_string(llm_config_file())
            .ok()
            .and_then(|t| solum_core::llm::LlmConfig::from_json(&t).ok())
            .map(|c| c.api_key)
            .ok_or_else(|| "请填写 API Key（没有已保存的密钥可沿用）".to_string())?
    } else {
        api_key
    };
    Ok(solum_core::llm::LlmConfig {
        base_url,
        api_key,
        model,
        temperature: a.temperature,
        max_tokens: a.max_tokens,
        timeout_secs: a.timeout_secs.clamp(5, 600),
    })
}

/// Persist to the config file and hot-swap the running reasoner. Returns the
/// masked summary for the status footer.
#[tauri::command]
pub(crate) fn llm_config_save(state: State<AppState>, cfg: LlmSaveArgs) -> CmdResult<String> {
    if solum_core::account::AccountSession::load_full().is_none() {
        return Err("完整云端能力需要先注册或登录 Solum 账号".into());
    }
    let c = resolve_llm_args(cfg)?;
    let json = serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?;
    let path = llm_config_file();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    solum_core::fsatomic::write_atomic(&path, &json).map_err(|e| e.to_string())?;
    let summary = c.masked_summary();
    // Direct vendor settings remain importable for advanced account users,
    // but the authenticated account proxy owns the running reasoner. Logging
    // out always returns to offline guest mode.
    let _ = state;
    Ok(format!("{summary}（已保存；当前账号代理保持生效）"))
}

#[derive(Serialize)]
pub(crate) struct LlmTestResult {
    latency_ms: u64,
    reply: String,
}

/// One real round-trip with the (possibly unsaved) form values. Async so the
/// up-to-`timeout_secs` blocking HTTP call never freezes the UI thread.
#[tauri::command]
pub(crate) async fn llm_config_test(cfg: LlmSaveArgs) -> CmdResult<LlmTestResult> {
    if solum_core::account::AccountSession::load_full().is_none() {
        return Err("测试云端连接前请先注册或登录 Solum 账号".into());
    }
    let c = resolve_llm_args(cfg)?;
    tauri::async_runtime::spawn_blocking(move || {
        use solum_core::extract::Reasoner;
        let r = solum_core::llm::LlmReasoner::new(c);
        let t0 = std::time::Instant::now();
        let reply = r
            .complete("你是连通性测试。不管用户说什么，只回复一个词：pong", "ping")
            .map_err(|e| e.to_string())?;
        Ok(LlmTestResult {
            latency_ms: t0.elapsed().as_millis() as u64,
            reply,
        })
    })
    .await
    .map_err(|e| format!("测试线程失败: {e}"))?
}


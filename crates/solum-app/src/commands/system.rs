use super::*;

// Modular commands entry

#[tauri::command]
pub(crate) fn app_info(state: State<AppState>) -> CmdResult<AppInfo> {
    let local_profile_id = active_local_profile_id();
    Ok(AppInfo {
        db_path: state.db_path.clone(),
        web_storage: web_storage_names(&local_profile_id, state.active_generation.as_deref())?,
        local_profile_id,
        active_generation: state.active_generation.clone(),
        system_now: Local::now()
            .naive_local()
            .format("%Y-%m-%dT%H:%M")
            .to_string(),
        llm: state.llm_summary.lock().map(|g| g.clone()).unwrap_or(None),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WebStorageNames {
    pub(crate) ui_state_key: String,
    pub(crate) chat_sessions_key: String,
    pub(crate) document_db_name: String,
}

pub(crate) fn web_storage_names(
    profile_id: &str,
    generation: Option<&str>,
) -> CmdResult<WebStorageNames> {
    if profile_id != "guest" && !solum_core::account::is_valid_user_id(profile_id) {
        return Err("WebView profile id 无效".into());
    }
    let suffix = match generation {
        None if profile_id == "guest" => String::new(),
        None => format!(".{profile_id}"),
        Some(value)
            if value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            format!(".{profile_id}.generation.{value}")
        }
        Some(_) => return Err("WebView generation id 无效".into()),
    };
    Ok(WebStorageNames {
        ui_state_key: format!("solum.ui-state.v1{suffix}"),
        chat_sessions_key: format!("pa.chat-sessions.v1{suffix}"),
        document_db_name: format!("pa-local-documents-v1{suffix}"),
    })
}

#[derive(Serialize)]
pub(crate) struct WebViewHydrationPlan {
    profile_id: String,
    generation: String,
    web_storage: WebStorageNames,
    bootstrap: solum_core::portable_generation::WebViewBootstrap,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebViewDocumentReceipt {
    pub(crate) id: String,
    pub(crate) plaintext_size: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebViewHydrationReceipt {
    pub(crate) profile_id: String,
    pub(crate) generation: String,
    pub(crate) documents: Vec<WebViewDocumentReceipt>,
}

#[derive(Serialize)]
pub(crate) struct WebViewHydrationStatus {
    generation: String,
    ready: bool,
}

/// Internal F17 seam: verify one sealed inactive generation and return only
/// its strict bootstrap plus exact, generation-isolated Web Storage names.
#[tauri::command]
pub(crate) fn restore_webview_begin(
    state: State<AppState>,
    generation_id: String,
) -> CmdResult<WebViewHydrationPlan> {
    let profile_id = active_local_profile_id();
    let reader = solum_core::portable_generation::PortableGenerationReader::open(
        &state.profile_root,
        &generation_id,
    )
    .map_err(core_err)?;
    let bootstrap = reader.bootstrap().clone();
    let plan = WebViewHydrationPlan {
        profile_id: profile_id.clone(),
        generation: generation_id.clone(),
        web_storage: web_storage_names(&profile_id, Some(&generation_id))?,
        bootstrap,
    };
    *state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())? =
        Some(WebViewRestoreSession {
            profile_id,
            reader,
            ready: false,
            publication_preview: None,
        });
    Ok(plan)
}

#[tauri::command]
pub(crate) fn restore_webview_document_chunk(
    state: State<AppState>,
    generation_id: String,
    document_id: String,
    offset: u64,
) -> CmdResult<solum_core::portable_generation::PortableDocumentChunk> {
    let session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())?;
    let session = session
        .as_ref()
        .filter(|session| {
            session.reader.bootstrap().generation == generation_id
                && session.profile_id == active_local_profile_id()
        })
        .ok_or_else(|| "WebView restore session 不存在或已失效".to_string())?;
    session
        .reader
        .read_document_chunk(&document_id, offset)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn restore_webview_finish(
    state: State<AppState>,
    generation_id: String,
    receipt: WebViewHydrationReceipt,
) -> CmdResult<WebViewHydrationStatus> {
    let mut session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())?;
    let session = session
        .as_mut()
        .filter(|session| {
            session.reader.bootstrap().generation == generation_id
                && session.profile_id == active_local_profile_id()
        })
        .ok_or_else(|| "WebView restore session 不存在或已失效".to_string())?;
    let expected: Vec<_> = session
        .reader
        .bootstrap()
        .documents
        .iter()
        .map(|document| WebViewDocumentReceipt {
            id: document.id.clone(),
            plaintext_size: document.plaintext_size,
            sha256: document.sha256.clone(),
        })
        .collect();
    validate_webview_hydration_receipt(&session.profile_id, &generation_id, &expected, &receipt)?;
    session.ready = true;
    Ok(WebViewHydrationStatus {
        generation: generation_id,
        ready: true,
    })
}

pub(crate) fn validate_webview_hydration_receipt(
    profile_id: &str,
    generation: &str,
    expected: &[WebViewDocumentReceipt],
    receipt: &WebViewHydrationReceipt,
) -> CmdResult<()> {
    if receipt.profile_id != profile_id
        || receipt.generation != generation
        || receipt.documents != expected
    {
        return Err("WebView hydrate 回读结果与 sealed generation 不一致".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn restore_webview_status(
    state: State<AppState>,
    generation_id: String,
) -> CmdResult<WebViewHydrationStatus> {
    let session = state
        .webview_restore
        .lock()
        .map_err(|_| "WebView restore session 状态异常".to_string())?;
    let session = session
        .as_ref()
        .filter(|session| {
            session.reader.bootstrap().generation == generation_id
                && session.profile_id == active_local_profile_id()
        })
        .ok_or_else(|| "WebView restore session 不存在或已失效".to_string())?;
    Ok(WebViewHydrationStatus {
        generation: generation_id,
        ready: session.ready,
    })
}

use super::*;

// ---- behavior journal & check-ins (F3/F4) -----------------------------------------

#[tauri::command]
pub(crate) fn behavior_log(state: State<AppState>) -> CmdResult<Vec<BehaviorEntry>> {
    lock!(state).behavior_log().map_err(core_err)
}

#[derive(Serialize, Clone)]
pub(crate) struct CheckinResp {
    question: String,
    /// F18: tap-to-answer quick options (offline template).
    ui: solum_core::genui::UiEnvelope,
}

pub(crate) fn checkin_resp(question: String) -> CheckinResp {
    let ui = solum_core::genui::checkin_prompt(&question);
    CheckinResp { question, ui }
}

/// Manual check-in probe against the UI clock (the resident ticker does the
/// same automatically on the system clock).
#[tauri::command]
pub(crate) fn checkin_now(state: State<AppState>, now: Option<String>) -> CmdResult<Option<CheckinResp>> {
    require_full_account()?;
    let now = parse_now(now)?;
    Ok(lock!(state)
        .checkin_if_due(now)
        .map_err(core_err)?
        .map(checkin_resp))
}

// ---- suggestions (F10) --------------------------------------------------------------

#[tauri::command]
pub(crate) fn suggestions(state: State<AppState>) -> CmdResult<Vec<Suggestion>> {
    require_full_account()?;
    lock!(state).suggestions().map_err(core_err)
}

#[tauri::command]
pub(crate) fn suggest_generate(
    state: State<AppState>,
    days: i64,
    now: Option<String>,
) -> CmdResult<Vec<Suggestion>> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .generate_suggestions(now, days.clamp(1, 30))
        .map_err(core_err)
}

/// Returns an optional follow-up message: accepting a habit suggestion
/// auto-creates its routine (D4), accepting a pause suggestion deactivates
/// one — the UI toasts whatever comes back.
#[tauri::command]
pub(crate) fn suggest_set(
    state: State<AppState>,
    id: i64,
    status: String,
    now: Option<String>,
) -> CmdResult<Option<String>> {
    require_full_account()?;
    // 注入时钟（AGENTS.md 红线）：采纳习惯建议会创建 routine，created_at 驱动
    // 7 天暂停刹车——用真实时钟会让模拟时钟下的演示/验证行为漂移。
    let now = parse_now(now)?;
    let status: SuggestionStatus = status.parse().map_err(core_err)?;
    lock!(state)
        .set_suggestion_status(id, status, now)
        .map_err(core_err)
}

// ---- routines (F3 完全体, D4) --------------------------------------------------

#[tauri::command]
pub(crate) fn routines(state: State<AppState>) -> CmdResult<Vec<solum_core::routine::Routine>> {
    require_full_account()?;
    lock!(state).routines().map_err(core_err)
}

#[tauri::command]
pub(crate) fn routine_set_active(
    state: State<AppState>,
    id: i64,
    active: bool,
    now: Option<String>,
) -> CmdResult<()> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .set_routine_active(id, active, now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn routine_update(
    state: State<AppState>,
    id: i64,
    title: String,
    time_of_day: String,
    now: Option<String>,
) -> CmdResult<()> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .update_routine(id, &title, &time_of_day, now)
        .map_err(core_err)
}

/// D4「一键已完成」：确认某条 routine 今天完成了（落行为日志，按日去重）。
#[tauri::command]
pub(crate) fn routine_done(state: State<AppState>, id: i64, now: Option<String>) -> CmdResult<String> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state).confirm_routine(id, now).map_err(core_err)
}

// ---- offline data review (D2) ---------------------------------------------------

#[tauri::command]
pub(crate) fn stats(state: State<AppState>, now: Option<String>) -> CmdResult<String> {
    require_full_account()?;
    let now = parse_now(now)?;
    Ok(lock!(state).stats(now).map_err(core_err)?.render())
}

// ---- self-review (F14) ----------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct ReviewResp {
    text: String,
    /// Whether the text was rewritten in the persona voice by the cloud.
    styled: bool,
}

/// `async` for the same reason as [`ingest`]: the persona rewrite is a cloud
/// call and must not stall the UI thread.
#[tauri::command]
pub(crate) async fn review(
    state: State<'_, AppState>,
    days: i64,
    now: Option<String>,
    styled: Option<bool>,
) -> CmdResult<ReviewResp> {
    require_full_account()?;
    let now = parse_now(now)?;
    let from = now - Duration::days(days.clamp(1, 365));
    let styled = styled.unwrap_or(false);
    let orch = state.orch.clone();
    let transition = state.account_transition.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
        if styled {
            let (text, styled) = o.review_text(from, now).map_err(core_err)?;
            Ok(ReviewResp { text, styled })
        } else {
            let digest = o.review(from, now).map_err(core_err)?;
            Ok(ReviewResp {
                text: digest.render(),
                styled: false,
            })
        }
    })
    .await
    .map_err(|e| format!("后台任务失败：{e}"))?
}

/// Build today's read-only focus brief for an on-demand UI refresh.
#[tauri::command]
pub(crate) fn daily_brief(state: State<AppState>, now: Option<String>) -> CmdResult<solum_core::brief::Brief> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state).daily_brief(now).map_err(core_err)
}

// ---- persona (F9 v1 / F15) ---------------------------------------------------

#[derive(Serialize)]
pub(crate) struct PersonaDto {
    active: Option<PersonaProfile>,
    versions: Vec<PersonaProfile>,
}

#[tauri::command]
pub(crate) fn persona_get(state: State<AppState>) -> CmdResult<PersonaDto> {
    require_full_account()?;
    let o = lock!(state);
    Ok(PersonaDto {
        active: o.persona().cloned(),
        versions: o.persona_versions().map_err(core_err)?,
    })
}

#[tauri::command]
pub(crate) fn persona_set(
    state: State<AppState>,
    nickname: Option<String>,
    tone: Option<String>,
    catchphrases: Vec<String>,
    style_notes: Option<String>,
    note: Option<String>,
    now: Option<String>,
) -> CmdResult<PersonaProfile> {
    require_full_account()?;
    let now = parse_now(now)?;
    let draft = PersonaDraft {
        nickname,
        tone: tone.unwrap_or_default(),
        catchphrases,
        style_notes,
    };
    lock!(state).set_persona(draft, note, now).map_err(core_err)
}

#[derive(Serialize)]
pub(crate) struct ImportPreviewResp {
    report: solum_core::persona_import::ImportReport,
    /// F18: the draft as an in-place edit form (submit → persona_import_save).
    ui: solum_core::genui::UiEnvelope,
}

/// F9 §3.4: run the strictly-local chat-log extraction. Pure preview — the
/// raw log stays in the frontend, nothing is stored or sent to the cloud.
#[tauri::command]
pub(crate) fn persona_import_preview(
    state: State<AppState>,
    raw: String,
    me: String,
) -> CmdResult<ImportPreviewResp> {
    require_full_account()?;
    let report = lock!(state)
        .preview_persona_import(&raw, &me)
        .map_err(core_err)?;
    let ui = solum_core::genui::persona_draft_form(&report.suggested, Some("从聊天记录导入"));
    Ok(ImportPreviewResp { report, ui })
}

/// Save the user-confirmed (possibly edited) import draft as a new persona
/// version with `source = "import"`.
#[tauri::command]
pub(crate) fn persona_import_save(
    state: State<AppState>,
    nickname: Option<String>,
    tone: Option<String>,
    catchphrases: Vec<String>,
    style_notes: Option<String>,
    note: Option<String>,
    now: Option<String>,
) -> CmdResult<PersonaProfile> {
    require_full_account()?;
    let now = parse_now(now)?;
    let draft = PersonaDraft {
        nickname,
        tone: tone.unwrap_or_default(),
        catchphrases,
        style_notes,
    };
    lock!(state)
        .import_persona(draft, note, now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn persona_rollback(state: State<AppState>, version: i64) -> CmdResult<PersonaProfile> {
    require_full_account()?;
    lock!(state).rollback_persona(version).map_err(core_err)
}


use super::*;

// ---- HITL guard ---------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct ToolDto {
    name: String,
    risk: String,
}

#[tauri::command]
pub(crate) fn tools(state: State<AppState>) -> CmdResult<Vec<ToolDto>> {
    let o = lock!(state);
    Ok(o.tool_names()
        .into_iter()
        .map(|name| {
            let risk = o
                .tool_risk(&name)
                .map(|r| r.as_str().to_string())
                .unwrap_or_else(|| "?".into());
            ToolDto { name, risk }
        })
        .collect())
}

#[derive(Serialize)]
pub(crate) struct GuardRunResp {
    ok: bool,
    output: String,
}

/// Attempt a tool with **no** confirmation. For non-safe tools this is refused
/// by the guard and audited — the UI uses it to demonstrate the F7 hard stop.
#[tauri::command]
pub(crate) fn guard_run(
    state: State<AppState>,
    tool: String,
    args: String,
    now: Option<String>,
) -> CmdResult<GuardRunResp> {
    require_tool_account(&tool)?;
    let now = parse_now(now)?;
    match lock!(state).run_tool(&tool, &args, None, now) {
        Ok(output) => Ok(GuardRunResp { ok: true, output }),
        Err(e) => Ok(GuardRunResp {
            ok: false,
            output: e.to_string(),
        }),
    }
}

#[derive(Serialize)]
pub(crate) struct PendingDto {
    pending_id: String,
    tool: String,
    args: String,
    risk: String,
    summary: String,
    preview: String,
}

/// Step 1 of the HITL flow: describe the action, get a pending confirmation.
/// Nothing executes; no token exists yet.
#[tauri::command]
pub(crate) fn guard_request(
    state: State<AppState>,
    tool: String,
    args: String,
    now: Option<String>,
) -> CmdResult<PendingDto> {
    require_tool_account(&tool)?;
    if tool == "backup_delete" {
        require_current_privacy_consent()?;
        require_backup_delete_is_not_in_restore(&state, &args)?;
    }
    let now = parse_now(now)?;
    let pending = lock!(state)
        .request_confirmation(&tool, &args, now)
        .map_err(core_err)?;
    Ok(PendingDto {
        pending_id: pending.id.clone(),
        tool,
        args,
        risk: pending.request.risk.as_str().to_string(),
        summary: pending.request.summary.clone(),
        preview: pending.request.effect_preview.clone(),
    })
}

/// Steps 2+3, driven by the human clicking 确认 in the dialog: mint the
/// one-time token and immediately spend it on exactly this action.
#[tauri::command]
pub(crate) async fn guard_confirm(
    state: State<'_, AppState>,
    pending_id: String,
    tool: String,
    args: String,
    now: Option<String>,
) -> CmdResult<GuardRunResp> {
    require_tool_account(&tool)?;
    let backup_delete = tool == "backup_delete";
    let _backup_transition = if backup_delete {
        Some(AccountTransitionGuard::acquire(&state.account_transition)?)
    } else {
        None
    };
    if backup_delete {
        require_current_privacy_consent()?;
        require_backup_delete_is_not_in_restore(&state, &args)?;
    }
    let now = parse_now(now)?;
    let orch = state.orch.clone();
    let transition = state.account_transition.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut o = orch
            .lock()
            .map_err(|_| "内部状态异常（锁中毒）".to_string())?;
        if !backup_delete && transition.load(Ordering::Acquire) {
            return Err("Solum profile is switching; reopen the app to continue".into());
        }
        let token = o.confirm(&pending_id, now).map_err(core_err)?;
        match o.run_tool(&tool, &args, Some(token), now) {
            Ok(output) => Ok(GuardRunResp { ok: true, output }),
            Err(e) => Ok(GuardRunResp {
                ok: false,
                output: e.to_string(),
            }),
        }
    })
    .await
    .map_err(|e| format!("护栏后台执行失败：{e}"))?
}

#[derive(Serialize)]
pub(crate) struct AuditDto {
    id: i64,
    ts: String,
    tool: String,
    risk: String,
    summary: String,
    decision: String,
    detail: String,
}

#[tauri::command]
pub(crate) fn audit(state: State<AppState>) -> CmdResult<Vec<AuditDto>> {
    let rows = lock!(state).audit_log().map_err(core_err)?;
    Ok(rows
        .into_iter()
        .map(|r| AuditDto {
            id: r.id,
            ts: r.ts,
            tool: r.tool,
            risk: r.risk,
            summary: r.summary,
            decision: r.decision,
            detail: r.detail,
        })
        .collect())
}

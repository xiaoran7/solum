use super::*;

// ---- rules & proactivity -------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct RuleDto {
    kind: String,
    /// The name to show. For built-ins this is the fixed Chinese label; for
    /// user categories it is the name they typed. Defaults to `kind` so an
    /// older frontend that doesn't read the field still renders something.
    label: String,
    leads: Vec<String>,
    channels: Vec<String>,
}

#[tauri::command]
pub(crate) fn rules(state: State<AppState>) -> CmdResult<Vec<RuleDto>> {
    require_full_account()?;
    let o = lock!(state);
    let table = o.rule_table();
    // all_rule_kinds, not EventKind::all(): the list must also carry the
    // categories the user created, in the same built-ins-then-slug order the
    // Harmony settings page uses.
    Ok(table
        .all_rule_kinds()
        .iter()
        .map(|kind| {
            let r = table.rule_ref(kind);
            RuleDto {
                kind: kind.as_str().to_string(),
                label: table
                    .custom_label(kind.as_str())
                    .unwrap_or_else(|| kind.label())
                    .to_string(),
                leads: r.lead_times.iter().map(|l| l.label.clone()).collect(),
                channels: r.channels.iter().map(|c| c.as_str().to_string()).collect(),
            }
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuleEdit {
    kind: String,
    leads: Vec<String>,
    channels: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuleSaveResult {
    replanned_events: usize,
}

/// The rule editor deliberately accepts the compact labels users see (30m,
/// 1h, 3d) and validates them in the core type before anything reaches the
/// persisted rule table. Saving immediately re-plans only future, pending
/// reminders; historical delivery records stay an honest audit trail.
#[tauri::command]
pub(crate) fn rules_save(
    state: State<AppState>,
    rule: RuleEdit,
    now: Option<String>,
) -> CmdResult<RuleSaveResult> {
    require_full_account()?;
    let kind = EventKind::from_str(rule.kind.trim()).map_err(core_err)?;
    if rule.leads.is_empty() || rule.leads.len() > 4 {
        return Err("每类事件需要保留 1–4 个提前提醒".into());
    }
    let mut seen_leads = HashSet::new();
    let mut leads = Vec::with_capacity(rule.leads.len());
    for raw in rule.leads {
        let lead = LeadTime::parse(&raw).map_err(core_err)?;
        if lead.minutes > 365 * 24 * 60 {
            return Err("提前量不能超过 365 天".into());
        }
        if !seen_leads.insert(lead.minutes) {
            return Err("同一提前量不能重复".into());
        }
        leads.push(lead);
    }
    let mut seen_channels = HashSet::new();
    let mut channels = Vec::with_capacity(rule.channels.len());
    for raw in rule.channels {
        let channel = match raw.as_str() {
            "push" => Channel::Push,
            "banner" => Channel::Banner,
            _ => return Err("通知渠道只支持 push 或 banner".into()),
        };
        if seen_channels.insert(channel.as_str()) {
            channels.push(channel);
        }
    }
    if channels.is_empty() {
        return Err("至少选择一种通知渠道".into());
    }
    let replanned_events = lock!(state)
        .set_importance_rule(
            ImportanceRule {
                kind,
                lead_times: leads,
                channels,
            },
            parse_now(now)?,
        )
        .map_err(core_err)?;
    Ok(RuleSaveResult { replanned_events })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatTurnInput {
    user: String,
    assistant: String,
}

/// The browser/WebView owns durable, local-only conversation sessions. Before
/// a new cloud reply it restores the selected session's short context here;
/// the core clamps it again so an IPC caller cannot expand the cloud context.
#[tauri::command]
pub(crate) fn chat_context_set(state: State<AppState>, turns: Vec<ChatTurnInput>) -> CmdResult<()> {
    if turns.len() > MAX_HISTORY_TURNS {
        return Err(format!("会话上下文最多保留 {MAX_HISTORY_TURNS} 轮"));
    }
    let mut out = Vec::with_capacity(turns.len());
    for turn in turns {
        let user = turn.user.trim().to_string();
        let assistant = turn.assistant.trim().to_string();
        if user.is_empty() || assistant.is_empty() {
            return Err("会话上下文不能包含空消息".into());
        }
        if user.chars().count() > 4_000 || assistant.chars().count() > 4_000 {
            return Err("单条会话上下文不能超过 4000 个字符".into());
        }
        out.push(ChatTurn { user, assistant });
    }
    lock!(state).replace_chat_history(out);
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct ProactivityDto {
    dimension: String,
    level: String,
}

#[tauri::command]
pub(crate) fn proactivity_get(state: State<AppState>) -> CmdResult<Vec<ProactivityDto>> {
    require_full_account()?;
    let o = lock!(state);
    let p = o.proactivity();
    Ok(ProactivityDimension::all()
        .iter()
        .map(|&dim| ProactivityDto {
            dimension: dim.as_str().to_string(),
            level: p.level(dim).as_str().to_string(),
        })
        .collect())
}

#[tauri::command]
pub(crate) fn proactivity_set(
    state: State<AppState>,
    dimension: String,
    level: String,
) -> CmdResult<()> {
    require_full_account()?;
    let dim: ProactivityDimension = dimension.parse().map_err(core_err)?;
    let lvl: ProactivityLevel = level.parse().map_err(core_err)?;
    lock!(state).set_proactivity(dim, lvl).map_err(core_err)
}

// ---- notification privacy (Phase 9) -----------------------------------------

#[tauri::command]
pub(crate) fn notif_cloud_get(state: State<AppState>) -> CmdResult<bool> {
    require_full_account()?;
    lock!(state).notif_cloud_enabled().map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_cloud_set(state: State<AppState>, enabled: bool) -> CmdResult<()> {
    require_full_account()?;
    lock!(state)
        .set_notif_cloud_enabled(enabled)
        .map_err(core_err)
}

// ---- notification intelligence (F20) --------------------------------------

#[derive(Serialize)]
pub(crate) struct NotificationIntelligenceStatus {
    config: NotificationIntelligenceConfig,
    priority_rules: Vec<solum_core::classify::NotificationPriorityRule>,
    captures: Vec<solum_core::notification_intelligence::NotificationCaptureRecord>,
    filter_proposals: Vec<solum_core::notification_intelligence::NotificationFilterProposal>,
    action_proposals: Vec<NotificationActionProposalView>,
    pipeline: solum_notif_access::PipelineStatus,
    /// Notifications dropped by the native listener because the spool was full.
    intake_overflow: i64,
    /// Notifications this side could not hand to the store after repeated
    /// attempts; their payloads are parked in `notif-spool/failed/`, not gone.
    intake_undeliverable: i64,
}

/// User-facing projection of one locally installed Android app. Package names
/// remain an implementation detail of the native listener policy and never
/// have to be typed or displayed in the settings UI.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationAppDto {
    name: String,
    package_name: String,
}

#[tauri::command]
pub(crate) fn notif_intelligence_apps(app: tauri::AppHandle) -> CmdResult<Vec<NotificationAppDto>> {
    require_full_account()?;
    let apps = app
        .notif_access()
        .installed_apps()
        .map_err(|e| e.to_string())?;
    Ok(apps
        .into_iter()
        .map(|entry| NotificationAppDto {
            name: entry.name,
            package_name: entry.package_name,
        })
        .collect())
}

/// A read-only projection for F12. The proposal stores only the locally
/// resolved id; the current event snapshot is fetched at render time so the
/// confirmation card can honestly show what it would affect without a schema
/// change or making a deleted event break the entire notification panel.
#[derive(Serialize)]
pub(crate) struct NotificationActionProposalView {
    #[serde(flatten)]
    proposal: solum_core::notification_intelligence::NotificationActionProposal,
    event: Option<NotificationActionEventView>,
}

#[derive(Serialize)]
pub(crate) struct NotificationActionEventView {
    title: String,
    start: NaiveDateTime,
}

#[tauri::command]
pub(crate) fn notif_intelligence_status(
    state: State<AppState>,
    app: tauri::AppHandle,
) -> CmdResult<NotificationIntelligenceStatus> {
    require_full_account()?;
    let o = lock!(state);
    Ok(NotificationIntelligenceStatus {
        config: o.notification_intelligence_config().map_err(core_err)?,
        priority_rules: o.rule_table().notification_priority_rules().to_vec(),
        captures: o.notification_captures().map_err(core_err)?,
        filter_proposals: o.notification_filter_proposals().map_err(core_err)?,
        action_proposals: o
            .notification_action_proposals()
            .map_err(core_err)?
            .into_iter()
            .map(|proposal| NotificationActionProposalView {
                event: o
                    .event(proposal.event_id)
                    .ok()
                    .map(|event| NotificationActionEventView {
                        title: event.title,
                        start: event.start,
                    }),
                proposal,
            })
            .collect(),
        pipeline: app
            .notif_access()
            .pipeline_status()
            .map_err(|e| e.to_string())?,
        intake_overflow: o.capture_overflow_count().map_err(core_err)?,
        intake_undeliverable: parked_payload_count(&state.db_path),
    })
}

/// The user has seen the intake-loss notice; stop showing it.
#[tauri::command]
pub(crate) fn notif_intelligence_acknowledge_losses(state: State<AppState>) -> CmdResult<()> {
    require_full_account()?;
    lock!(state).acknowledge_capture_losses().map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_set_app(
    state: State<AppState>,
    app: tauri::AppHandle,
    package_name: String,
    enabled: bool,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
    // Native policy file first, database second.
    //
    // These are two stores of the same decision, and the listener reads only
    // the file. Committing the database first meant that if the file write
    // failed, the user had revoked an app in the UI while the listener kept
    // reading its notifications and writing them to the inbox — the core would
    // discard them later, but the sensitive text had already been read and put
    // on disk. Writing the file first is safe in *both* directions: a failure
    // now changes nothing anywhere, and if the database commit then fails, the
    // core's whitelist is the stricter of the two and drops the capture.
    let target = {
        let o = lock!(state);
        let mut config = o.notification_intelligence_config().map_err(core_err)?;
        let package = package_name.trim().to_string();
        config.allowed_packages.retain(|p| p != &package);
        if enabled {
            config.allowed_packages.push(package);
        }
        config.allowed_packages.sort();
        config
    };
    write_notification_capture_policy(&state.db_path, &target)?;

    let config = {
        let mut o = lock!(state);
        o.set_notification_app_enabled(&package_name, enabled)
            .map_err(core_err)?;
        o.notification_intelligence_config().map_err(core_err)?
    };
    // Re-write from the committed config so the file always reflects what the
    // database actually holds (validation may have normalized the package).
    write_notification_capture_policy(&state.db_path, &config)?;
    sync_notification_pipeline(&app, &config)?;
    Ok(config)
}

#[tauri::command]
pub(crate) fn notif_intelligence_set_capture_mode(
    state: State<AppState>,
    app: tauri::AppHandle,
    mode: solum_core::notification_intelligence::NotificationCaptureMode,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
    let target = {
        let o = lock!(state);
        let mut config = o.notification_intelligence_config().map_err(core_err)?;
        config.capture_mode = mode;
        config.normalized().map_err(core_err)?
    };
    write_notification_capture_policy(&state.db_path, &target)?;
    let config = lock!(state)
        .set_notification_capture_mode(mode)
        .map_err(core_err)?;
    write_notification_capture_policy(&state.db_path, &config)?;
    sync_notification_pipeline(&app, &config)?;
    Ok(config)
}

/// The **second** grant: this app may create calendar entries by itself.
/// Deliberately a separate command from `notif_intelligence_set_app`, because
/// it is a separate decision — see `auto_event_packages`.
#[tauri::command]
pub(crate) fn notif_intelligence_set_app_auto_event(
    state: State<AppState>,
    package_name: String,
    enabled: bool,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
    let mut o = lock!(state);
    o.set_notification_app_auto_event(&package_name, enabled)
        .map_err(core_err)?;
    o.notification_intelligence_config().map_err(core_err)
}

/// Per-app counts of events auto-created over the trailing week. The
/// after-the-fact discovery surface the user reviews; it authorizes nothing.
#[tauri::command]
pub(crate) fn notif_intelligence_auto_event_counts(
    state: State<AppState>,
    now: Option<String>,
) -> CmdResult<Vec<(String, i64)>> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state).auto_event_counts(now).map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_set_batch_interval(
    state: State<AppState>,
    minutes: u16,
) -> CmdResult<NotificationIntelligenceConfig> {
    require_full_account()?;
    let mut config = lock!(state)
        .notification_intelligence_config()
        .map_err(core_err)?;
    config.batch_interval_minutes = minutes;
    lock!(state)
        .set_notification_intelligence_config(config)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_add_priority_rule(
    state: State<AppState>,
    pattern: String,
    package_name: Option<String>,
    matcher: String,
) -> CmdResult<solum_core::classify::NotificationPriorityRule> {
    require_full_account()?;
    let matcher = match matcher.as_str() {
        "substring" => solum_core::classify::NotificationMatchKind::Substring,
        "regex" => solum_core::classify::NotificationMatchKind::Regex,
        _ => return Err("匹配方式只支持 substring 或 regex".into()),
    };
    lock!(state)
        .add_notification_priority_rule(pattern, package_name, matcher)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_remove_priority_rule(
    state: State<AppState>,
    id: String,
) -> CmdResult<()> {
    require_full_account()?;
    lock!(state)
        .remove_notification_priority_rule(&id)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_set_filter_proposal(
    state: State<AppState>,
    app: tauri::AppHandle,
    id: i64,
    accepted: bool,
) -> CmdResult<()> {
    require_full_account()?;
    let config = {
        let o = lock!(state);
        o.set_notification_filter_proposal(id, accepted)
            .map_err(core_err)?;
        o.notification_intelligence_config().map_err(core_err)?
    };
    write_notification_capture_policy(&state.db_path, &config)?;
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn notif_intelligence_set_action_proposal(
    state: State<AppState>,
    id: i64,
    accepted: bool,
    now: Option<String>,
) -> CmdResult<String> {
    require_full_account()?;
    lock!(state)
        .resolve_notification_action_proposal(id, accepted, parse_now(now)?)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_remove_filter_rule(
    state: State<AppState>,
    app: tauri::AppHandle,
    id: String,
) -> CmdResult<()> {
    require_full_account()?;
    let config = {
        let o = lock!(state);
        o.remove_notification_filter_rule(&id).map_err(core_err)?;
        o.notification_intelligence_config().map_err(core_err)?
    };
    write_notification_capture_policy(&state.db_path, &config)?;
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn notif_intelligence_restore_capture(state: State<AppState>, id: i64) -> CmdResult<()> {
    require_full_account()?;
    lock!(state)
        .restore_notification_capture(id)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_promote_capture(
    state: State<AppState>,
    id: i64,
    now: Option<String>,
) -> CmdResult<String> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .promote_notification_capture(id, now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_intelligence_process_now(
    state: State<AppState>,
    now: Option<String>,
) -> CmdResult<usize> {
    require_full_account()?;
    let now = parse_now(now)?;
    lock!(state)
        .process_notification_batch(now)
        .map_err(core_err)
}

#[tauri::command]
pub(crate) fn notif_pipeline_status(
    app: tauri::AppHandle,
) -> CmdResult<solum_notif_access::PipelineStatus> {
    require_full_account()?;
    app.notif_access()
        .pipeline_status()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn notif_pipeline_request_battery_optimization(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
    app.notif_access()
        .request_ignore_battery_optimizations()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn notif_pipeline_open_battery_settings(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
    app.notif_access()
        .open_battery_settings()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn notif_pipeline_open_background_settings(app: tauri::AppHandle) -> CmdResult<()> {
    require_full_account()?;
    app.notif_access()
        .open_app_background_settings()
        .map_err(|e| e.to_string())
}

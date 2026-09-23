use super::*;

pub(crate) fn take_backup_upload(
    state: &AppState,
    session_id: &str,
) -> CmdResult<BackupUploadAppSession> {
    let mut guard = state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())?;
    let matches = guard.as_ref().is_some_and(|session| {
        session.upload.session_id() == session_id
            && current_backup_account(&session.profile_id, &session.account).is_ok()
    });
    if !matches {
        return Err("备份上传会话不存在、已取消或已失效".into());
    }
    guard
        .take()
        .ok_or_else(|| "备份上传会话不存在、已取消或已失效".into())
}

pub(crate) fn restore_failed_backup_upload(state: &AppState, session: BackupUploadAppSession) {
    if current_backup_account(&session.profile_id, &session.account).is_err() {
        return;
    }
    if let Ok(mut guard) = state.backup_upload.lock() {
        if guard.is_none() {
            *guard = Some(session);
        }
    }
}

/// Manual F17 backup entry. The UI invokes it only from the backup center;
/// there is deliberately no scheduler in this phase.
#[tauri::command]
pub(crate) async fn backup_upload_begin(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    snapshot: portable_snapshot::WebViewPortableSnapshot,
) -> CmdResult<solum_core::backup_upload::BackupUploadBegin> {
    require_current_privacy_consent()?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("账号正在切换，已取消备份".into());
    }
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能创建加密云备份".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
        .to_string();
    if solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        &profile_id,
        &account.server_url,
    )
    .map_err(|error| error.to_string())?
    .is_some()
    {
        return Err("已有备份接管事务待完成，请先继续恢复".into());
    }
    portable_snapshot::validate_profile(&snapshot.profile_id, &profile_id)?;
    let master_key = backup_master_key(&account)?;
    let json = collect_portable_json(&state, &app, snapshot, Local::now().naive_local())?;
    // A fresh begin explicitly supersedes any older attempt before core clears
    // encrypted crash residue. If preparation fails, callers must begin again;
    // an old plan whose files were removed is never left looking resumable.
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = None;
    let profile_root = state.profile_root.clone();
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut cloud_account = account.clone();
    let worker_profile_id = profile_id.clone();
    let (cloud_account, upload) = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::backup_upload::HttpBackupCloudTransport::new();
        let authority = solum_core::backup_upload::load_or_initialize_device_generation(
            &profile_root,
            &worker_profile_id,
            &transport,
            &mut cloud_account,
        )
        .map_err(|error| error.to_string())?;
        let upload = solum_core::backup_upload::BackupUploadSession::begin(
            &profile_root,
            json.as_bytes(),
            &worker_profile_id,
            authority.generation,
            authority.writer_id,
            &created_at,
            master_key,
        )
        .map_err(|error| error.to_string())?;
        Ok::<_, String>((cloud_account, upload))
    })
    .await
    .map_err(|_| "备份准备任务异常终止".to_string())??;
    current_backup_account(&profile_id, &cloud_account)?;
    let begin = upload.begin_info();
    *state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())? = Some(BackupUploadAppSession {
        profile_id,
        account: cloud_account,
        upload,
    });
    Ok(begin)
}

#[tauri::command]
pub(crate) fn backup_upload_attachment_chunk(
    state: State<AppState>,
    session_id: String,
    attachment_id: String,
    offset: u64,
    data_base64: String,
) -> CmdResult<()> {
    use base64::Engine as _;

    const MAX_ENCODED_CHARS: usize = solum_core::backup_upload::MAX_IPC_CHUNK_BYTES.div_ceil(3) * 4;
    if data_base64.len() > MAX_ENCODED_CHARS {
        return Err("备份附件分块超过 IPC 上限".into());
    }
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| "备份附件分块编码无效".to_string())?;
    if base64::engine::general_purpose::STANDARD.encode(&bytes) != data_base64 {
        return Err("备份附件分块编码不是规范 base64".into());
    }
    let mut guard = state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())?;
    let session = guard
        .as_mut()
        .filter(|session| {
            session.upload.session_id() == session_id
                && current_backup_account(&session.profile_id, &session.account).is_ok()
        })
        .ok_or_else(|| "备份上传会话不存在、已取消或已失效".to_string())?;
    let result = session
        .upload
        .append_attachment_chunk(&attachment_id, offset, &bytes)
        .map_err(|error| error.to_string());
    bytes.fill(0);
    result
}

#[tauri::command]
pub(crate) async fn backup_upload_attachment_finish(
    state: State<'_, AppState>,
    session_id: String,
    attachment_id: String,
) -> CmdResult<()> {
    let mut session = take_backup_upload(&state, &session_id)?;
    let (returned, result) = tauri::async_runtime::spawn_blocking(move || {
        let result = session
            .upload
            .finish_attachment(&attachment_id)
            .map_err(|error| error.to_string());
        (session, result)
    })
    .await
    .map_err(|_| "备份附件加密任务异常终止".to_string())?;
    if let Err(error) = result {
        restore_failed_backup_upload(&state, returned);
        return Err(error);
    }
    restore_failed_backup_upload(&state, returned);
    Ok(())
}

#[tauri::command]
pub(crate) async fn backup_upload_commit(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<solum_core::backup_upload::BackupUploadOutcome> {
    require_current_privacy_consent()?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("账号正在切换，已取消备份提交".into());
    }
    let mut session = take_backup_upload(&state, &session_id)?;
    let (returned, result) = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::backup_upload::HttpBackupCloudTransport::new();
        let result = session
            .upload
            .upload(&transport, &mut session.account)
            .map_err(|error| error.to_string());
        (session, result)
    })
    .await
    .map_err(|_| "备份上传任务异常终止".to_string())?;
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            restore_failed_backup_upload(&state, returned);
            Err(error)
        }
    }
}

#[tauri::command]
pub(crate) fn backup_upload_cancel(state: State<AppState>, session_id: String) -> CmdResult<()> {
    let mut guard = state
        .backup_upload
        .lock()
        .map_err(|_| "备份上传会话状态异常".to_string())?;
    if guard
        .as_ref()
        .is_some_and(|session| session.upload.session_id() == session_id)
    {
        *guard = None;
    }
    Ok(())
}

/// F17 recovery catalog. It returns complete-version metadata only and
/// does not download, decrypt, stage, publish, or advance device generation.
#[tauri::command]
pub(crate) async fn backup_restore_catalog(
    state: State<'_, AppState>,
) -> CmdResult<solum_core::backup_download::BackupCatalog> {
    require_current_privacy_consent()?;
    if state.account_transition.load(Ordering::Acquire) {
        return Err("账号正在切换，已取消读取备份列表".into());
    }
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能读取加密云备份".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
        .to_string();
    if active_local_profile_id() != profile_id {
        return Err("备份读取已取消：账号或本地 profile 不一致".into());
    }
    let mut cloud_account = account.clone();
    let (cloud_account, catalog) = tauri::async_runtime::spawn_blocking(move || {
        let transport = solum_core::backup_download::HttpBackupCloudReader::new();
        let catalog =
            solum_core::backup_download::fetch_backup_catalog(&transport, &mut cloud_account, 20)
                .map_err(|error| error.to_string())?;
        Ok::<_, String>((cloud_account, catalog))
    })
    .await
    .map_err(|_| "备份列表任务异常终止".to_string())??;
    current_backup_account(&profile_id, &cloud_account)?;
    Ok(catalog)
}

/// Download and authenticate one complete version, then assemble a sealed but
/// inactive generation. Publication remains a separate preview/confirm flow.
#[tauri::command]
pub(crate) async fn backup_restore_stage(
    state: State<'_, AppState>,
    backup_id: String,
) -> CmdResult<solum_core::backup_download::StagedBackupOutcome> {
    require_current_privacy_consent()?;
    // Staging and hard deletion must never cross: either operation can begin
    // only after it owns the same profile-transition guard for its full
    // network/filesystem lifetime.
    let _transition = AccountTransitionGuard::acquire(&state.account_transition)?;
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能准备加密备份恢复".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
        .to_string();
    if active_local_profile_id() != profile_id {
        return Err("备份恢复已取消：账号或本地 profile 不一致".into());
    }
    if solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        &profile_id,
        &account.server_url,
    )
    .map_err(|error| error.to_string())?
    .is_some()
    {
        return Err("已有备份接管事务待完成，请从待继续项恢复".into());
    }
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = None;
    let mut master_key = backup_master_key(&account)?;
    let profile_root = state.profile_root.clone();
    let worker_profile_id = profile_id.clone();
    let mut cloud_account = account.clone();
    let (cloud_account, active_generation, outcome) =
        tauri::async_runtime::spawn_blocking(move || {
            let transport = solum_core::backup_download::HttpBackupCloudReader::new();
            let result = (|| {
                let catalog = solum_core::backup_download::fetch_backup_catalog(
                    &transport,
                    &mut cloud_account,
                    100,
                )?;
                let active_generation = catalog.active_generation;
                let outcome = solum_core::backup_download::download_and_stage_backup(
                    solum_core::backup_download::BackupStageRequest {
                        profile_root: &profile_root,
                        catalog: &catalog,
                        backup_id: &backup_id,
                        user_id: &worker_profile_id,
                        master_key: &master_key,
                        now: Local::now().naive_local(),
                    },
                    &transport,
                    &mut cloud_account,
                )?;
                Ok::<_, solum_core::backup_download::BackupDownloadError>((
                    active_generation,
                    outcome,
                ))
            })()
            .map_err(|error| error.to_string());
            master_key.fill(0);
            result.map(|(active_generation, outcome)| (cloud_account, active_generation, outcome))
        })
        .await
        .map_err(|_| "备份下载与校验任务异常终止".to_string())??;
    current_backup_account(&profile_id, &cloud_account)?;
    *state
        .backup_restore
        .lock()
        .map_err(|_| "备份恢复会话状态异常".to_string())? = Some(BackupRestoreAppSession {
        profile_id,
        account: cloud_account,
        backup_id: outcome.backup_id.clone(),
        cloud_generation: active_generation,
        staged_generation: outcome.generation_id.clone(),
    });
    Ok(outcome)
}

/// A durable takeover may survive a response loss or process restart. The UI
/// uses this metadata-only status to resume hydrate/preview without creating a
/// second transaction or downloading a different candidate.
#[tauri::command]
pub(crate) fn backup_restore_pending(
    state: State<AppState>,
) -> CmdResult<Option<solum_core::backup_takeover::BackupTakeoverStatus>> {
    require_current_privacy_consent()?;
    let account = solum_core::account::AccountSession::load_full()
        .ok_or_else(|| "登录后才能继续加密备份恢复".to_string())?;
    let profile_id = account
        .stable_user_id()
        .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?;
    if active_local_profile_id() != profile_id {
        return Err("备份恢复已取消：账号或本地 profile 不一致".into());
    }
    solum_core::backup_takeover::load_pending_takeover(
        &state.profile_root,
        profile_id,
        &account.server_url,
    )
    .map(|pending| pending.map(|pending| pending.status()))
    .map_err(|error| error.to_string())
}

/// Advance cloud/device generation only after a fresh preview, then re-check
/// both live stores under their locks before atomically publishing the pointer.
/// A response-lost retry reuses the durable takeover UUID.
#[tauri::command]
pub(crate) async fn backup_restore_takeover_publish(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    generation_id: String,
    token: String,
    webview: LiveWebViewSummary,
) -> CmdResult<String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (state, app, generation_id, token, webview);
        Err("Backup takeover is available only in the Android app".into())
    }
    #[cfg(target_os = "android")]
    {
        require_current_privacy_consent()?;
        let transition = AccountTransitionGuard::acquire(&state.account_transition)?;
        let account = solum_core::account::AccountSession::load_full()
            .ok_or_else(|| "登录后才能接管加密备份".to_string())?;
        let profile_id = account
            .stable_user_id()
            .ok_or_else(|| "当前账号缺少不可变用户标识".to_string())?
            .to_string();
        if active_local_profile_id() != profile_id {
            return Err("备份接管已取消：账号或本地 profile 不一致".into());
        }

        // This check deliberately leaves the token unspent. Network work must
        // happen without the SQLite locks; publication performs the same check
        // again and consumes the token only after the cloud generation is safe.
        validate_restore_preview_is_current(&state, &generation_id, &token, &webview)?;

        let pending_status = solum_core::backup_takeover::load_pending_takeover(
            &state.profile_root,
            &profile_id,
            &account.server_url,
        )
        .map_err(|error| error.to_string())?
        .map(|pending| pending.status());
        let (backup_id, expected_generation) = if let Some(status) = pending_status {
            if status.staged_generation != generation_id {
                return Err("另一个备份接管事务仍在进行".into());
            }
            (status.backup_id, status.expected_generation)
        } else {
            let restore = state
                .backup_restore
                .lock()
                .map_err(|_| "备份恢复会话状态异常".to_string())?;
            let restore = restore
                .as_ref()
                .filter(|restore| {
                    restore.profile_id == profile_id
                        && restore.staged_generation == generation_id
                        && current_backup_account(&restore.profile_id, &restore.account).is_ok()
                })
                .ok_or_else(|| "备份恢复候选已失效，请重新校验该版本".to_string())?;
            (restore.backup_id.clone(), restore.cloud_generation)
        };

        let profile_root = state.profile_root.clone();
        let worker_profile = profile_id.clone();
        let worker_server = account.server_url.clone();
        let worker_generation = generation_id.clone();
        let mut cloud_account = account.clone();
        let (cloud_account, pending) = tauri::async_runtime::spawn_blocking(move || {
            let mut pending = solum_core::backup_takeover::begin_or_resume_takeover(
                &profile_root,
                solum_core::backup_takeover::BackupTakeoverBinding {
                    user_id: &worker_profile,
                    server_url: &worker_server,
                    backup_id: &backup_id,
                    staged_generation: &worker_generation,
                    expected_generation,
                },
            )
            .map_err(|error| error.to_string())?;
            let transport = solum_core::backup_download::HttpBackupCloudReader::new();
            solum_core::backup_takeover::advance_takeover(
                &mut pending,
                &transport,
                &mut cloud_account,
            )
            .map_err(|error| error.to_string())?;
            Ok::<_, String>((cloud_account, pending))
        })
        .await
        .map_err(|_| "备份接管任务异常终止".to_string())??;
        current_backup_account(&profile_id, &cloud_account)?;

        let outcome = publish_restore_generation(&state, &generation_id, &token, &webview)?;
        if solum_core::backup_takeover::finish_published_takeover(pending, &generation_id).is_err()
        {
            // The pointer is already durable. Startup reconciliation owns this
            // cleanup path; never pretend publication can now be rolled back.
            eprintln!("backup takeover journal cleanup deferred to startup reconciliation");
        }
        if let Ok(mut restore) = state.backup_restore.lock() {
            *restore = None;
        }
        transition.hold_until_restart();
        if outcome.handoff_pending {
            eprintln!("restore pointer committed; startup must resume late notification handoff");
        }
        restart_account_app(&app)
    }
}

pub(crate) fn write_export_json(db_path: &str, json: &str, prefix: &str) -> CmdResult<String> {
    let dir = std::path::Path::new(db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // 文件名一律用真实墙钟：模拟时钟只影响导出内容里的数据视角，不该让
    // 磁盘上出现"未来"时间戳的文件（2026-07-17 走查小项）。
    // Second-resolution names collide: two exports in the same second (a
    // double-click, or a manual one landing on a scheduled one) used to have
    // the later silently overwrite the earlier. A backup that can be
    // clobbered by another backup is not a backup — take the first free name.
    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut file = None;
    for n in 1..=99 {
        let candidate = if n == 1 {
            dir.join(format!("{prefix}-{stamp}.json"))
        } else {
            dir.join(format!("{prefix}-{stamp}-{n}.json"))
        };
        if !candidate.exists() {
            file = Some(candidate);
            break;
        }
    }
    let file = file.ok_or_else(|| "同一秒已有过多导出文件，请稍后重试".to_string())?;
    // Atomic write so an interrupted export leaves no truncated file that
    // looks like a usable backup.
    solum_core::fsatomic::write_atomic(&file, json).map_err(|e| e.to_string())?;
    Ok(file.display().to_string())
}

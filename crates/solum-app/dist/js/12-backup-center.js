"use strict";
let backupCenterBusy = false;
let backupRestoreCandidate = null;
function backupDate(value) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "时间未知" : new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
  }).format(date);
}
function backupTakeoverPhaseLabel(value) {
  return ({ planned: "等待安全推进", remote_advanced: "云端代次已接管", device_ready: "等待替换本机" })[value]
    || "等待继续";
}
function backupSetBusy(busy, label) {
  backupCenterBusy = busy;
  const button = $("backupCreateBtn");
  if (!button) return;
  button.disabled = busy;
  button.textContent = busy ? (label || "处理中…") : "立即创建备份";
  $("backupVersions")?.querySelectorAll("button").forEach((item) => { item.disabled = busy; });
}
function backupLiveTotal(preview) {
  const sqlite = Object.values(preview.sqlite_counts || {}).reduce((sum, count) => sum + Number(count || 0), 0);
  const webview = preview.webview || {};
  return sqlite + Number(webview.chat_sessions || 0) + Number(webview.chat_messages || 0) +
    Number(webview.drafts || 0) + Number(webview.documents || 0);
}
function showBackupPreview(candidate) {
  backupRestoreCandidate = candidate;
  const preview = candidate.preview;
  const incomingRows = candidate.staged?.restored_rows;
  const incomingDocs = candidate.staged?.attachment_count;
  const body = $("backupPreviewBody");
  body.textContent = "";
  body.appendChild(el("p", { class: "hint", text: "候选版本已在隔离空间完成下载、解密、认证与回读；下方只是预览，当前本机尚未改变。" }));
  const grid = el("div", { class: "backup-preview-grid" }, [
    el("div", { class: "backup-preview-cell" }, [el("span", { text: "候选数据行" }), el("strong", { text: incomingRows == null ? "已校验" : String(incomingRows) })]),
    el("div", { class: "backup-preview-cell" }, [el("span", { text: "候选本地资料" }), el("strong", { text: incomingDocs == null ? "已校验" : String(incomingDocs) })]),
    el("div", { class: "backup-preview-cell" }, [el("span", { text: "本机现有项目" }), el("strong", { text: String(backupLiveTotal(preview)) })]),
    el("div", { class: "backup-preview-cell" }, [el("span", { text: "替换后设备代次" }), el("strong", { text: "+1" })]),
  ]);
  body.appendChild(grid);
  body.appendChild(el("p", { class: "hint", text: preview.has_existing_data
    ? "继续会替换当前本机数据，并让此设备接管新的云端代次；该操作不能在应用内一键撤销。"
    : "当前本机为空；继续仍会接管新的云端代次，并在完成后重新打开应用。" }));
  $("backupPreview").hidden = false;
  $("backupPreview").scrollIntoView({ behavior: "smooth", block: "start" });
}
async function prepareBackupRestore(backupId, stagedGeneration) {
  if (backupCenterBusy) return;
  backupSetBusy(true, stagedGeneration ? "恢复校验中…" : "下载校验中…");
  try {
    const staged = stagedGeneration ? null : await invoke("backup_restore_stage", { backupId });
    const generationId = stagedGeneration || staged.generation_id;
    await hydratePortableGenerationWebView(generationId);
    const preview = await prepareRestorePublication(generationId);
    showBackupPreview({ backupId, generationId, staged, preview });
    toast("候选版本已校验，本机尚未改变", "ok");
  } catch (error) {
    toast(safeUserError(error, "恢复准备暂时无法完成，本机数据未改变。"), "danger", 5200);
  } finally { backupSetBusy(false); }
}
async function confirmBackupTakeover() {
  const candidate = backupRestoreCandidate;
  if (!candidate || backupCenterBusy) return;
  const body = el("div", null, [
    el("p", { text: "将用已校验的封缄点替换当前本机数据，并让此设备接管新的云端代次。" }),
    el("p", { class: "hint", text: "应用会在指针安全落盘后重新打开。若网络结果不明确，同一接管事务会安全重试。" }),
  ]);
  openModal("替换本机并接管？", body, "替换本机并接管", async () => {
    backupSetBusy(true, "正在安全接管…");
    try {
      const webview = await snapshotLiveWebViewSummary();
      await invoke("backup_restore_takeover_publish", {
        generationId: candidate.generationId,
        token: candidate.preview.token,
        webview,
      });
    } catch (error) {
      backupSetBusy(false);
      toast(safeUserError(error, "接管未完成，请重新校验后重试。"), "danger", 6000);
      await refreshBackupCenter();
    }
  }, { icon: "shield" });
}
async function requestBackupDelete(version) {
  if (backupCenterBusy) return;
  await guardedDestructive({
    tool: "backup_delete",
    args: {
      backup_id: version.backup_id,
      plan_etag: version.plan_etag,
      committed_at: version.committed_at,
      total_bytes: Number(version.total_bytes || 0),
      object_count: Number(version.object_count || 0),
    },
    title: "人工确认：删除云端备份",
    confirmLabel: "永久删除此版本",
    note: "确认令牌同时绑定版本元数据与隐藏的 ETag；若列表已变化，服务端会拒绝删除。删除不会影响本机数据。",
    onDone: async () => { await refreshBackupCenter(); },
    onError: async () => { await refreshBackupCenter(); },
  });
}
async function refreshBackupCenter() {
  const versions = $("backupVersions");
  if (!versions || !accountLoggedIn) return;
  versions.textContent = "";
  versions.appendChild(emptyBox("refresh", "正在读取封缄点", "只读取版本元数据，不下载备份正文。"));
  try {
    const [catalog, pending] = await Promise.all([
      invoke("backup_restore_catalog"),
      invoke("backup_restore_pending"),
    ]);
    const backups = Array.isArray(catalog.backups) ? catalog.backups.slice() : [];
    backups.sort((left, right) =>
      String(right.committed_at).localeCompare(String(left.committed_at))
      || String(right.backup_id).localeCompare(String(left.backup_id)));
    $("backupCount").textContent = `${backups.length} 个版本`;
    const latest = backups[0];
    $("backupSealTitle").textContent = latest ? `最近封缄于 ${backupDate(latest.committed_at)}` : "还没有云端备份";
    $("backupSealMeta").textContent = latest
      ? `${readableDocumentSize(Number(latest.total_bytes || 0))} 密文 · 设备代次 ${latest.generation} · 不会自动备份`
      : `当前云端代次 ${catalog.active_generation || 1} · 只在你点击时创建`;
    versions.textContent = "";
    if (pending) {
      versions.appendChild(el("div", { class: "backup-version" }, [
        el("span", { class: "backup-pin" }),
        el("span", { class: "backup-version-copy" }, [
          el("span", { class: "backup-version-title", text: "接管事务待继续" }),
          el("span", { class: "backup-version-meta", text: `已保留安全进度 · ${backupTakeoverPhaseLabel(pending.phase)}` }),
        ]),
        el("button", { class: "btn sm", type: "button", text: "继续校验", onclick: () => prepareBackupRestore(pending.backup_id, pending.staged_generation) }),
      ]));
    }
    if (!backups.length && !pending) {
      versions.appendChild(emptyBox("cloud", "还没有封缄点", "点击“立即创建备份”，在本机加密后保存第一个版本。"));
      return;
    }
    for (const version of backups) {
      const isPending = pending && pending.backup_id === version.backup_id;
      const actions = el("span", { class: "backup-version-actions" }, [
        el("button", { class: "btn sm", type: "button", text: isPending ? "继续校验" : "校验并准备恢复", onclick: () => prepareBackupRestore(version.backup_id, isPending ? pending.staged_generation : "") }),
      ]);
      if (!isPending) actions.appendChild(el("button", {
        class: "btn sm danger", type: "button", text: "删除版本",
        onclick: () => requestBackupDelete(version),
      }));
      versions.appendChild(el("div", { class: "backup-version" }, [
        el("span", { class: "backup-pin" }),
        el("span", { class: "backup-version-copy" }, [
          el("span", { class: "backup-version-title", text: backupDate(version.committed_at) }),
          el("span", { class: "backup-version-meta", text: `${readableDocumentSize(Number(version.total_bytes || 0))} 密文 · ${version.object_count || 0} 个密文对象 · 协议 v${version.protocol_version}` }),
          el("span", { class: "backup-version-meta", text: `设备代次 ${version.generation}${isPending ? " · 接管待继续" : ""}` }),
        ]),
        actions,
      ]));
    }
  } catch (error) {
    $("backupCount").textContent = "读取失败";
    $("backupSealTitle").textContent = "暂时无法读取备份";
    $("backupSealMeta").textContent = "本机数据未受影响";
    versions.textContent = "";
    versions.appendChild(emptyBox("cloud", "封缄点读取失败", safeUserError(error, "封缄点暂时无法读取，请稍后重试。")));
  }
}
async function createBackupFromCenter() {
  if (backupCenterBusy) return;
  backupSetBusy(true, "正在封缄…");
  try {
    await createAndUploadPortableBackup();
    toast("加密备份已封缄并提交", "ok");
    await refreshBackupCenter();
  } catch (error) {
    toast(safeUserError(error, "备份未完成，本机数据已保留。"), "danger", 6000);
  } finally { backupSetBusy(false); }
}
$("backupSealIcon").appendChild(icon("lock", 20));
$("backupPrivacyIcon").appendChild(icon("shield", 18));
$("backupCreateBtn").addEventListener("click", createBackupFromCenter);
$("backupTakeoverBtn").addEventListener("click", confirmBackupTakeover);
$("backupPreviewCancelBtn").addEventListener("click", () => {
  backupRestoreCandidate = null;
  $("backupPreview").hidden = true;
});
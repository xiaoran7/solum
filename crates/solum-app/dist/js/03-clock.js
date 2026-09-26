"use strict";
/* ============================== 时钟（可注入） ============================== */
function nowParam() {
  if (!$("simToggle").checked) return null;
  return $("simClock").value || null;
}
function effectiveNow() {
  const p = nowParam();
  return p ? new Date(p) : new Date();
}
$("simToggle").addEventListener("change", () => {
  $("simClock").disabled = !$("simToggle").checked;
  refreshAll();
});
// Keep the arrow wrapper: `refreshAll` is declared in 44-navigation.js, and
// every module here is its own classic script, so the name is not bound yet at
// this point. Wrapping defers the lookup to event time.
$("simClock").addEventListener("change", () => refreshAll());

async function initClock() {
  try {
    const info = await invoke("app_info");
    selectLocalProfile(info.local_profile_id, info.web_storage);
    $("simClock").value = info.system_now;
    $("dbPath").textContent = info.db_path;
    $("dbPath").title = info.db_path;
    const on = !!info.llm;
    $("llmDot").className = "sdot " + (on ? "ok" : "");
    $("llmText").textContent = on ? "云端在线" : "离线模式";
    $("llmRow").title = on ? info.llm : "未配置云端推理（solum-llm.json / SOLUM_LLM_*）";
    $("brandDot").classList.toggle("on", on);
    $("brandDotM").classList.toggle("on", on);
  } catch (e) { toast(safeUserError(e, "同步未完成，本地数据已保留。"), "danger"); }
  await refreshSyncStatus();
}

// 单独一个函数，因为同步健康横幅需要在「手动同步之后」「后台 ticker 报缺口时」
// 各自刷新一次，而不是只在启动时刷。
async function refreshSyncStatus() {
  try {
    const s = await invoke("sync_status");
    if (s.configured) {
      const title = `多设备同步：${s.summary}（设备 ${s.device_id}；每 5 分钟自动，点击立即同步）`;
      const b = $("syncBtn");
      b.style.display = "inline-flex"; b.title = title;
      b.textContent = ""; b.appendChild(icon("refresh", 15)); b.appendChild(el("span", { text: "立即同步" }));
      const m = $("syncBtnM");
      m.style.display = "inline-flex"; m.title = title;
      m.textContent = ""; m.appendChild(icon("refresh", 19));
    }
    renderSyncHealth(s);
  } catch (_) { /* 同步状态拿不到不影响其余 UI */ }
}

// 同步健康横幅。刻意做成**常驻**而非 toast：它描述的是「有一段数据已经没了」，
// 用户必须去做一件事（从另一台设备导出导入）才算解决。一闪而过的提示对这种
// 状态是无效的——按 DESIGN.md，琥珀横幅只留给「需要用户注意/行动」的场景，
// 这条正是。
function renderSyncHealth(s) {
  const host = $("syncHealth");
  if (!host) return;
  host.textContent = "";
  if (!s || !s.configured) { host.hidden = true; return; }

  const rows = [];
  if (s.history_gap) {
    rows.push(el("div", null, [
      el("strong", { text: "这台设备的同步不完整。" }),
      el("span", { text: ` 本机进度落在同步服务器的留存窗口之外（${s.history_gap}），期间的部分远端变更已被清理、无法再取回。请在另一台设备上「导出全部数据」，在本机导入以重新对齐。` }),
      el("button", {
        class: "btn sm", style: "margin-left:8px;",
        text: "我已重新对齐",
        onclick: async () => {
          try { await invoke("sync_gap_acknowledge"); toast("已清除同步缺口提示", "ok"); refreshSyncStatus(); }
          catch (e) { toast(String(e), "danger"); }
        },
      }),
    ]));
  }
  if (s.bad_blobs_dropped > 0) {
    rows.push(el("div", { text:
      `有 ${s.bad_blobs_dropped} 个无法解密的同步批次因超出暂存上限被丢弃（当前暂存 ${s.bad_blobs_held} 个）。` +
      "最常见原因是某台设备的 SOLUM_SYNC_KEY 与本机不一致——请先核对密钥，否则会持续丢弃。" }));
  } else if (s.bad_blobs_held > 0) {
    rows.push(el("div", { text:
      `有 ${s.bad_blobs_held} 个同步批次无法解密，已暂存（密文保留）。请核对各设备的 SOLUM_SYNC_KEY 是否一致。` }));
  }

  // 推送被服务器永久拒绝：拉取照常，但本机的变更上不去——对端收不到本机
  // 的更新这件事必须常驻可见，不能只靠后台日志。与缺口不同它会自愈（下一轮
  // 推送成功即清除），所以措辞是「暂时」而不是「已丢失」。
  if (s.push_block) {
    rows.push(el("div", null, [
      el("strong", { text: "推送暂时未完成。" }),
      el("span", { text: ` ${s.push_block}。本机的变更还没有上传到同步服务器（接收不受影响），解决后点「立即同步」即可恢复上传。` }),
    ]));
  }
  if (s.push_holds > 0) {
    rows.push(el("div", { text:
      `有 ${s.push_holds} 条本地变更因单条体积超过上传上限被滞留：数据仍在本机，但其他设备暂时收不到它们。` }));
  }

  if (rows.length === 0) { host.hidden = true; return; }
  host.hidden = false;
  for (const r of rows) host.appendChild(r);
}
async function doSync(btn) {
  btn.disabled = true;
  try {
    const r = await invoke("sync_now");
    // 缺口不能被「推送 N 条，合并 M 条」这种成功语气盖过去——那正是复审说的
    // 「让桌面用户误以为同步完整」。有缺口时说的第一句话必须是它不完整。
    if (r.history_gap) {
      toast("同步不完整：本机进度已超出服务器留存窗口，部分远端变更无法取回。详见下方提示。", "danger");
    } else if (r.push_block) {
      // 推送被服务器永久拒绝同理：这轮不完整，第一句先说它，成功语气往后放。
      toast(`推送未完成：${r.push_block}。本轮 ${r.push_skipped} 条本地变更未上传（接收不受影响）。`, "danger");
    } else {
      toast(`推送 ${r.pushed} 条，合并 ${r.applied} 条${r.skipped ? `（跳过 ${r.skipped}）` : ""}`, "sync");
    }
    if (r.push_skipped > 0 && !r.push_block) {
      toast(`另有 ${r.push_skipped} 条本地变更因超过单条体积上限被滞留——数据仍在本机，其他设备暂时收不到。`, "danger");
    }
    if (r.push_failed) {
      toast(`推送失败（${r.push_failed}），本轮只完成拉取，下轮自动重试。`, "danger");
    }
    if (r.bad_blobs > 0) {
      toast(`另有 ${r.bad_blobs} 个批次无法解密并已暂存——请核对各设备的同步密钥是否一致。`, "danger");
    }
    await refreshSyncStatus();
    if (r.applied > 0) refreshAll();
  } catch (e) { toast(String(e), "danger"); }
  btn.disabled = false;
}

$("syncBtn").addEventListener("click", () => doSync($("syncBtn")));
$("syncBtnM").addEventListener("click", () => doSync($("syncBtnM")));

"use strict";
/* ============================== 隐私 ============================== */
async function refreshPrivacy() {
  const toggle = $("notifCloud");
  const state = $("notifCloudState");
  if (!accountLoggedIn) {
    $("privacyAccountLock").hidden = false;
    $("privacyAccountControls").hidden = true;
    toggle.checked = false;
    toggle.disabled = true;
    state.textContent = "登录后可配置通知上云、通知智能和账号内的端到端加密同步。";
    refreshPrivacyPolicy();
    return;
  }
  $("privacyAccountLock").hidden = true;
  $("privacyAccountControls").hidden = false;
  toggle.disabled = false;
  try {
    const enabled = await invoke("notif_cloud_get");
    toggle.checked = Boolean(enabled);
    state.textContent = enabled
      ? "当前开启：之后捕获的通知可被 recall 选作云端 AI 的上下文，并进入账号的端到端加密同步。"
      : "当前关闭：之后捕获的通知不发往云端 AI，也不进入 recall；账号的其他可同步数据不受影响。";
  } catch (e) {
    state.textContent = safeUserError(e, "通知处理状态暂时无法读取，请稍后重试。");
  }
  refreshNotificationIntelligence();
  refreshPrivacyPolicy();
}

/* ---------------- 首启隐私门 + 应用内隐私政策（自 Solum Harmony 回移） ----------------
   门与政策页共用同一份 Rust 常量，不在前端另写一份文案——两处措辞漂移过一次就
   再也说不清「用户当时同意的是哪一版」。 */
let privacyDoc = null;

// 政策全文折叠区（设置 → 隐私）。同意状态一并显示，让用户能自查同意时间与记录位置。
async function refreshPrivacyPolicy() {
  const body = $("privacyPolicyBody");
  if (!body) return;
  let s;
  try { s = await invoke("privacy_consent_status"); } catch (_) { return; }
  privacyDoc = s.document;
  body.replaceChildren(renderMarkdown(s.document.full_body));
  $("privacyPolicyMeta").textContent =
    `v${s.document.version} · 更新于 ${s.document.updated_on}`;
  $("privacyConsentState").textContent = s.needs_consent
    ? `本机尚未同意 v${s.document.version}。`
    : `已于 ${formatConsentTime(s.accepted_at)} 在本机同意 v${s.document.version}。`;
}

function formatConsentTime(iso) {
  if (!iso) return "未知时间";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

// 启动时的拦截。needs_consent 为真才显示；显示期间应用其余部分不可交互。
async function initPrivacyGate() {
  let s;
  try {
    s = await invoke("privacy_consent_status");
  } catch (_) {
    // 静态预览（无 IPC）下不该凭空弹一道过不去的门。
    return;
  }
  privacyDoc = s.document;
  if (!s.needs_consent) return;

  const gate = $("privacyGate");
  $("privacyGateTitle").textContent = "";
  $("privacyGateTitle").appendChild(icon("lock", 20));
  $("privacyGateTitle").appendChild(el("span", { text: s.document.gate_title }));
  $("privacyGateTitle").appendChild(
    el("span", { class: "pg-ver", text: `v${s.document.version} · ${s.document.updated_on}` }));
  $("privacyGateBody").replaceChildren(renderMarkdown(s.document.gate_body));
  $("privacyGateStatus").textContent = "";
  gate.classList.add("open");
  // 门盖住视口后，背后的主界面仍能被滚动条拖动（用户看不见却在动）。锁住。
  document.body.style.overflow = "hidden";
  $("privacyGateAcceptBtn").focus();
}

// 「查看完整隐私政策」在门内就地展开，不跳视图——跳走等于让用户离开这道门。
$("privacyGateFullBtn").addEventListener("click", (event) => {
  if (!privacyDoc) return;
  const btn = event.currentTarget;
  const body = $("privacyGateBody");
  const showingFull = btn.dataset.full === "1";
  body.replaceChildren(renderMarkdown(showingFull ? privacyDoc.gate_body : privacyDoc.full_body));
  btn.dataset.full = showingFull ? "" : "1";
  btn.textContent = showingFull ? "查看完整隐私政策" : "返回摘要";
  body.scrollTop = 0;
});

function setPrivacyGateStatus(message, pending) {
  const status = $("privacyGateStatus");
  status.textContent = message;
  status.classList.toggle("pending", Boolean(pending));
}
$("privacyAccountBtn").addEventListener("click", () =>
  requireAccount("登录后配置应用消息接收范围、通知上云和端到端加密同步"));

function invokePrivacyCommand(command, timeoutMs) {
  let timer = null;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("操作超时，请重试")), timeoutMs);
  });
  return Promise.race([invoke(command), timeout]).finally(() => clearTimeout(timer));
}

$("privacyGateAcceptBtn").addEventListener("click", async (event) => {
  const btn = event.currentTarget;
  btn.disabled = true;
  btn.textContent = "正在保存…";
  setPrivacyGateStatus("正在保存你的选择…", true);
  try {
    await invokePrivacyCommand("privacy_consent_accept", 8000);
    $("privacyGate").classList.remove("open");
    document.body.style.overflow = "";
    refreshPrivacyPolicy();
    // Ask for OS notification delivery only after the in-app policy gate is
    // accepted; denial leaves the local UI usable.
    try { await invokePrivacyCommand("notification_permission_request", 8000); } catch (_) { }
    if (!accountLoggedIn) {
      openAccountGate("隐私设置已保存，请登录后开始使用", { dismissible: false, allowGuest: false });
    }
  } catch (e) {
    // 写盘失败必须留在门里：放行等于产生一次没有记录的同意。
    setPrivacyGateStatus(safeUserError(e, "隐私选择没有保存，请重试。"), false);
    btn.disabled = false;
    btn.textContent = "同意并开始使用";
  }
});

$("privacyGateDeclineBtn").addEventListener("click", async (event) => {
  const btn = event.currentTarget;
  btn.disabled = true;
  btn.textContent = "正在退出…";
  setPrivacyGateStatus(privacyDoc ? privacyDoc.decline_message : "应用即将退出", true);
  // Native command is the reliable Android exit path. Keep the window API as
  // a compatibility fallback for older desktop shells.
  try {
    await invokePrivacyCommand("privacy_consent_decline", 1500);
  } catch (_) { /* Android exits before it can return an IPC response. */ }
  try {
    const w = window.__TAURI__ && window.__TAURI__.window;
    const cur = w && (w.getCurrentWindow ? w.getCurrentWindow() : w.getCurrent && w.getCurrent());
    if (cur) {
      await cur.close();
      return;
    }
  } catch (_) { /* show an actionable error below */ }
  if (document.visibilityState !== "hidden") {
    setPrivacyGateStatus("无法自动退出，请使用系统返回键或关闭应用。", false);
    btn.disabled = false;
    btn.textContent = "不同意并退出";
  }
});

/* ---------------- 数据入口（多入口采集，自 Solum Harmony 回移） ---------------- */
// 可用=绿；待接入/需配套端=中性描边；不支持=同样中性但文案写死"这个平台上没有"，
// 不用 danger——入口不存在不是错误状态。
const CAPTURE_AVAIL_KIND = {
  ready: "ok", connector: "plain", companion: "plain", unsupported: "plain",
};
const PLATFORM_SUPPORT = {
  available: "可用", permission_required: "需授权", configuration_required: "需配置",
  planned: "规划中", unsupported: "不支持",
};
let pendingCaptureDraft = null;
let shareDrainRunning = false;

async function drainAndroidShares() {
  if (shareDrainRunning) return;
  shareDrainRunning = true;
  try {
    const received = await invoke("capture_android_share_drain");
    if (received.drafts.length) {
      toast(`收到 ${received.drafts.length} 条采集结果，已放入待确认区`, "ok");
      refreshCaptureInbox();
    }
    for (const notice of received.notices) toast(notice, "danger");
  } catch (error) {
    // Android lifecycle transitions can briefly detach the WebView. Keep the
    // native RAM queue intact and try again on the next focus.
    console.warn("share drain deferred", error);
  } finally {
    shareDrainRunning = false;
  }
}
window.addEventListener("focus", drainAndroidShares);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") drainAndroidShares();
});
setTimeout(drainAndroidShares, 0);

async function startAndroidActiveCapture(command, unavailableMessage) {
  if (!IS_ANDROID) return toast("此入口需要在 Android 设备上使用", "danger");
  try {
    const opened = await invoke(command);
    if (!opened) return toast(unavailableMessage, "danger");
    setTimeout(drainAndroidShares, 800);
    setTimeout(drainAndroidShares, 2500);
  } catch (error) { toast(safeUserError(error, unavailableMessage), "danger"); }
}
$("voiceCaptureBtn").addEventListener("click", () =>
  startAndroidActiveCapture("capture_android_voice_start", "系统没有可用的语音识别服务"));
$("ocrCaptureBtn").addEventListener("click", () =>
  startAndroidActiveCapture("capture_android_ocr_pick", "系统没有可用的图片选择器"));

async function refreshCapture() {
  const box = $("captureConnectors");
  if (!box) return;
  try {
    const list = await invoke("capture_entry_points");
    box.textContent = "";
    for (const c of list) {
      const runtime = c.authorized == null ? "按需检查"
        : c.authorized ? "已授权/可用" : "未授权/未配置";
      const activity = c.last_success
        ? `最近接收 ${fmtDTText(c.last_success)} · 已采用 ${c.accepted_count}`
        : `尚无已采用记录${c.pending_count ? ` · 待确认 ${c.pending_count}` : ""}`;
      box.appendChild(el("div", { class: "capture-connector-row" }, [
        tag(c.status, CAPTURE_AVAIL_KIND[c.availability] || "plain"),
        el("div", { class: "meta-col" }, [
          el("div", { class: "conn-title", text: c.label }),
          el("div", { class: "meta", text: c.description }),
          el("div", { class: "meta", text: `${runtime} · ${activity} · 敏感度：${c.sensitivity}` }),
          el("div", { class: "meta", text: `平台：Android ${PLATFORM_SUPPORT[c.platforms.android]} / iOS ${PLATFORM_SUPPORT[c.platforms.ios]} / HarmonyOS ${PLATFORM_SUPPORT[c.platforms.harmonyos]}` }),
          el("div", { class: "meta", text: `用途：${c.allow_memory ? "可形成记忆" : "不形成记忆"} / ${c.allow_ai ? "可用于 AI" : "不用于 AI"} / ${c.allow_backup ? "可进入加密备份" : "不备份"}` }),
          c.last_error ? el("div", { class: "meta", text: "最近检查失败；请进入对应设置页重试" }) : null,
        ]),
      ]));
    }
  } catch (e) {
    box.textContent = safeUserError(e, "采集内容暂时无法读取，请稍后重试。");
  }
  refreshCaptureInbox();
  refreshCalendarSource();
}

async function refreshCalendarSource() {
  const block = $("calendarSourceBlock");
  if (!block) return;
  if (!IS_ANDROID) {
    block.hidden = true;
    return;
  }
  block.hidden = false;
  try {
    const granted = await invoke("calendar_access_status");
    $("calendarSourceStatus").textContent = granted
      ? "已允许读取。读取操作只生成内存候选，不会直接创建 Solum 日程。"
      : "尚未授权。Solum 只申请读取，不申请直接写入系统日历。";
    $("calendarGrantBtn").hidden = granted;
    $("calendarImportBtn").disabled = !granted;
  } catch (error) {
    $("calendarSourceStatus").textContent = safeUserError(error, "暂时无法检查系统日历权限");
  }
}

$("calendarGrantBtn").addEventListener("click", async () => {
  try {
    await invoke("calendar_access_request");
    toast("请在系统授权框完成选择", "ok");
    setTimeout(refreshCalendarSource, 1200);
    setTimeout(refreshCalendarSource, 3200);
  } catch (error) { toast(safeUserError(error, "无法请求日历权限"), "danger"); }
});
$("calendarImportBtn").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  try {
    const items = await invoke("calendar_candidates_import", { daysAhead: 30 });
    toast(items.length ? `已读取并合并为 ${items.length} 条待确认候选` : "未来 30 天没有可导入日程", "ok");
    refreshCaptureInbox();
  } catch (error) { toast(safeUserError(error, "读取系统日历失败"), "danger"); }
  finally { button.disabled = false; refreshCalendarSource(); }
});

async function refreshCaptureInbox() {
  const box = $("captureInbox");
  if (!box) return;
  let drafts;
  try { drafts = await invoke("capture_inbox_list"); } catch (_) { return; }
  box.textContent = "";
  if (!drafts.length) {
    box.appendChild(el("p", { class: "hint", text: "待确认区是空的。" }));
    return;
  }
  for (const d of drafts) {
    const previewChars = Array.from(String(d.text || "").replace(/\s+/g, " ").trim());
    const preview = previewChars.length > 240
      ? `${previewChars.slice(0, 240).join("")}…`
      : previewChars.join("");
    box.appendChild(el("div", { class: "row" }, [
      tag(d.source_label, "accent"),
      el("div", { class: "grow" }, [
        el("div", { text: d.title }),
        preview ? el("div", { class: "meta capture-preview", text: preview }) : null,
        el("div", { class: "meta", text: d.clue_summary }),
        el("div", { class: "meta", text: `置信度 ${Math.round((d.confidence || 0) / 10)}% · ${d.allow_ai ? "可用于 AI" : "不用于 AI"} · ${d.allow_backup ? "可备份" : "仅本机"}${d.expires_at ? ` · ${new Date(d.expires_at).toLocaleDateString("zh-CN")} 前确认` : ""}` }),
        d.note ? el("div", { class: "meta", text: d.note }) : null,
      ]),
      el("span", { style: "display:flex;gap:8px;" }, [
        el("button", { class: "btn sm", type: "button", text: "放入输入框",
          onclick: () => adoptCaptureDraft(d) }),
        el("button", { class: "btn sm", type: "button", text: "丢弃",
          onclick: () => discardCaptureDraft(d.id) }),
      ]),
    ]));
  }
}

// 「采用」只把正文送进输入框，**不落库**：发送与否仍由用户决定，落库仍走
// 既有 ingest 管道。这是这一层的核心约束，别为了省一步改成自动提交。
function adoptCaptureDraft(draft) {
  const input = $("chatInput");
  if (input) {
    // 输入框是单行 <input>，直接塞多行正文会把换行**静默吃掉**，
    // 「缴纳体检费\n明天…」变成「缴纳体检费明天…」——词边界没了，抽取会解错。
    // 所以换行显式折成空格，而不是交给浏览器去丢（走查发现，2026-07-27）。
    input.value = draft.text.replace(/\s*[\r\n]+\s*/g, " ").trim();
    input.focus();
    saveCurrentChatDraft(true);
  }
  pendingCaptureDraft = { id: draft.id, source: draft.source };
  showView("chat");
  toast("已放入输入框；发送成功后才会保存，并保留来源", "ok");
}

async function discardCaptureDraft(id, quiet) {
  try {
    await invoke("capture_inbox_discard", { id });
    if (pendingCaptureDraft?.id === id) pendingCaptureDraft = null;
    if (!quiet) toast("已丢弃（未写入数据库）", "ok");
  } catch (e) {
    toast(String(e), "danger");
  }
  refreshCaptureInbox();
}

$("captureAddBtn").addEventListener("click", async (event) => {
  const area = $("captureText");
  const text = area.value.trim();
  if (!text) { toast("先粘贴要采集的文字", "danger"); return; }
  const btn = event.currentTarget;
  btn.disabled = true;
  try {
    await invoke("capture_inbox_add", { source: "share", title: "", text, externalId: null });
    area.value = "";
    toast("已放入待确认区（尚未写入数据库）", "ok");
    refreshCaptureInbox();
  } catch (e) {
    toast(String(e), "danger");
  } finally {
    btn.disabled = false;
  }
});

$("notifCloud").addEventListener("change", async (event) => {
  const toggle = event.currentTarget;
  const enabled = toggle.checked;
  toggle.disabled = true;
  try {
    await invoke("notif_cloud_set", { enabled });
    toast(enabled ? "通知上云已开启（仅影响之后的新捕获）" : "通知上云已关闭（仅影响之后的新捕获）", "ok");
  } catch (e) {
    toast(String(e), "danger");
  } finally {
    toggle.disabled = false;
    refreshPrivacy();
  }
});

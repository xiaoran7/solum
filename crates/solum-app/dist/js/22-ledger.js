"use strict";
/* ============================== 记忆台账 ============================== */
// F12 台账分页状态：每层的下一页游标（null = 该层已取完）；
// rows 是已加载条目的合并缓存，"加载更多"在其上追加并整体重排。
const ledgerPaging = { cursors: {}, loading: false, routines: [] };
let ledgerRowsCache = [];
const LEDGER_PAGE_SIZE = 50;
const LEDGER_LAYERS = ["raw_input", "event", "notification", "behavior", "suggestion",
  "wearable", "fact", "routine", "notification_capture"];

function ledgerMoreAvailable() {
  return LEDGER_LAYERS.some((layer) => ledgerPaging.cursors[layer] !== null);
}

async function loadLedgerFirstPage() {
  for (const layer of LEDGER_LAYERS) ledgerPaging.cursors[layer] = "";
  // 每层各拉一页并行返回；空串游标表示从第一页开始。
  const active = LEDGER_LAYERS.filter((layer) => ledgerPaging.cursors[layer] !== null);
  const pages = await Promise.all(active.map(async (layer) => {
    const [entries, next] = await invoke("ledger_page", {
      layer, limit: LEDGER_PAGE_SIZE, cursor: null,
    });
    return { entries, next };
  }));
  let rows = [];
  active.forEach((layer, i) => {
    const { entries, next } = pages[i];
    rows.push(...entries);
    ledgerPaging.cursors[layer] = typeof next === "string" ? next : null;
  });
  return sortLedgerRows(rows);
}

async function loadLedgerNextPages() {
  // 只对还有余量的层各再拉一页。
  const active = LEDGER_LAYERS.filter((layer) => ledgerPaging.cursors[layer]);
  const pages = await Promise.all(active.map(async (layer) => {
    const [entries, next] = await invoke("ledger_page", {
      layer, limit: LEDGER_PAGE_SIZE, cursor: ledgerPaging.cursors[layer],
    });
    return { entries, next };
  }));
  let rows = [];
  active.forEach((layer, i) => {
    const { entries, next } = pages[i];
    rows.push(...entries);
    ledgerPaging.cursors[layer] = typeof next === "string" ? next : null;
  });
  return sortLedgerRows(rows);
}

function sortLedgerRows(rows) {
  return rows.sort((a, b) =>
    (a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : a.id - b.id));
}

let selectedLedgerRow = null;

function filteredLedgerRows() {
  const source = $("ledgerSourceFilter")?.value || "";
  return source ? ledgerRowsCache.filter((row) => humanSourceLabel(row.source) === source) : ledgerRowsCache;
}

function memoryTimeLabel(value) {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return fmtDTText(value);
  return parsed.toLocaleString("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
}

function renderLedgerRows(box, rows) {
  const visibleRows = rows || filteredLedgerRows();
  $("ledgerCount").textContent = `${visibleRows.length} 条记忆`;
  box.textContent = "";
  if (!visibleRows.length) {
    box.appendChild(emptyBox("memory", "这里还没有记忆", "对话、事件、提醒和行为会逐渐沉淀在这里。"));
    return;
  }
  for (const memory of visibleRows) {
    box.appendChild(el("button", {
      class: "memory-row",
      type: "button",
      "aria-label": `查看记忆：${memory.summary || "无标题"}`,
      onclick: () => openMemoryDetail(memory),
    }, [
      el("span", { class: "memory-row-copy" }, [
        el("span", { class: "memory-row-summary", text: memory.summary || "（无内容）" }),
        el("span", { class: "memory-row-meta" }, [
          el("span", { class: "memory-row-source", text: humanSourceLabel(memory.source) }),
          el("time", { class: "memory-row-time", text: memoryTimeLabel(memory.created_at), datetime: memory.created_at || "" }),
        ]),
      ]),
      el("span", { class: "memory-row-arrow", "aria-hidden": "true" }, icon("chev", 17)),
    ]));
  }
}

function appendLedgerMoreButton(box) {
  if (!ledgerMoreAvailable()) return;
  const wrap = el("div", { class: "memory-more" });
  wrap.appendChild(el("button", {
    class: "btn", text: "加载更多", "aria-label": "加载更多台账条目",
    onclick: () => loadMoreLedger(),
  }));
  box.appendChild(wrap);
}

async function loadMoreLedger() {
  if (ledgerPaging.loading || !ledgerMoreAvailable()) return;
  ledgerPaging.loading = true;
  const box = $("ledgerBox");
  try {
    const more = await loadLedgerNextPages();
    ledgerRowsCache.push(...more);
    sortLedgerRows(ledgerRowsCache);
    updateLedgerSourceFilter();
    renderLedgerRows(box);
    appendLedgerMoreButton(box);
  } catch (e) {
    toast(safeUserError(e, "加载更多失败，请稍后重试。"), "danger");
  } finally {
    ledgerPaging.loading = false;
  }
}

function updateLedgerSourceFilter() {
  const select = $("ledgerSourceFilter");
  const previous = select.value;
  const sources = [...new Set(ledgerRowsCache.map((row) => humanSourceLabel(row.source)))].sort((a, b) => a.localeCompare(b, "zh-CN"));
  select.replaceChildren(el("option", { value: "", text: "全部来源" }));
  for (const source of sources) select.appendChild(el("option", { value: source, text: source }));
  if (sources.includes(previous)) select.value = previous;
}

function openMemoryDetail(memory) {
  selectedLedgerRow = memory;
  const type = LAYER[memory.layer] || [memory.layer || "未知类型", ""];
  $("memoryDetailContent").textContent = memory.summary || "（无内容）";
  $("memoryDetailSource").textContent = humanSourceLabel(memory.source);
  $("memoryDetailTime").textContent = fmtDTText(memory.created_at) || "—";
  $("memoryDetailType").textContent = type[0];
  const routine = memory.layer === "routine" ? ledgerPaging.routines.find((item) => item.id === memory.id) : null;
  $("memoryDetailEdit").hidden = memory.layer !== "fact" && !routine;
  $("memoryDetailEdit").textContent = routine ? "编辑提醒" : "编辑内容";
  showView("ledger-detail");
}

async function refreshLedger() {
  const box = $("ledgerBox");
  try {
    ledgerPaging.routines = accountLoggedIn ? await invoke("routines") : [];
    $("ledgerCount").textContent = "正在读取…";

    // 首屏：每层各一页合并显示；之后"加载更多"逐层续拉。
    ledgerRowsCache = await loadLedgerFirstPage();
    updateLedgerSourceFilter();
    renderLedgerRows(box);
    appendLedgerMoreButton(box);
  } catch (e) {
    box.textContent = "";
    $("ledgerCount").textContent = "读取失败";
    box.appendChild(emptyBox("alert", "记忆暂时无法读取", safeUserError(e, "请稍后重试。")));
  }
}

async function refreshCaptures() {
  const box = $("notificationLedger");
  if (!accountLoggedIn) {
    box.replaceChildren(emptyBox("lock", "登录后查看通知回看", "访客的本地记忆仍可正常查看。"));
    return;
  }
  try {
    renderNotificationLedger(box, await invoke("notif_intelligence_status"));
  } catch (error) {
    box.replaceChildren(emptyBox("alert", "通知回看暂时无法读取", safeUserError(error, "请稍后重试。")));
  }
}
$("ledgerSourceFilter").addEventListener("change", () => {
  renderLedgerRows($("ledgerBox"));
  appendLedgerMoreButton($("ledgerBox"));
});
$("memoryDetailBackIcon").appendChild(icon("back", 16));
$("memoryDetailBack").addEventListener("click", () => showView("ledger", { refresh: false }));
$("memoryDetailEdit").addEventListener("click", () => {
  if (!selectedLedgerRow) return;
  if (selectedLedgerRow.layer === "fact") {
    askFactEdit(selectedLedgerRow.id, selectedLedgerRow.summary);
    return;
  }
  const routine = ledgerPaging.routines.find((item) => item.id === selectedLedgerRow.id);
  if (routine) askRoutineEdit(routine);
});
$("memoryDetailDelete").addEventListener("click", () => {
  if (selectedLedgerRow) askForget(selectedLedgerRow.layer, selectedLedgerRow.id);
});
async function askSoulousEventPush(id) {
  try {
    const p = await invoke("guard_request", {
      tool: "soulous_push_event", args: JSON.stringify({ event_id: id }), now: nowParam(),
    });
    openGuardConfirm(p, true);
  } catch (e) { toast(String(e), "danger"); }
}
async function setRoutineActive(id, active, button) {
  button.disabled = true;
  try {
    await invoke("routine_set_active", { id, active, now: nowParam() });
    toast(active ? "固定提醒已启用" : "固定提醒已暂停", "ok");
    refreshLedger();
  } catch (e) {
    button.disabled = false;
    toast(String(e), "danger");
  }
}
function askRoutineEdit(routine) {
  const titleId = `routine-title-${routine.id}`;
  const timeId = `routine-time-${routine.id}`;
  const title = el("input", { id: titleId, type: "text", value: routine.title, autocomplete: "off" });
  const time = el("input", { id: timeId, type: "time", value: routine.time_of_day });
  openModal("编辑固定提醒",
    el("div", null, [
      el("div", { class: "fine", text: "修改会立即重排尚未触发的提醒；已触发记录会保留在台账中。" }),
      el("div", { class: "field", style: "margin-top:12px;" }, [
        el("label", { for: titleId, text: "提醒内容" }), title,
      ]),
      el("div", { class: "field" }, [
        el("label", { for: timeId, text: "每天时间" }), time,
      ]),
    ]),
    "保存设置",
    async () => {
      try {
        await invoke("routine_update", {
          id: routine.id, title: title.value, time_of_day: time.value, now: nowParam(),
        });
        toast("固定提醒已更新", "ok");
        if (selectedLedgerRow?.layer === "routine" && selectedLedgerRow.id === routine.id) {
          selectedLedgerRow.summary = title.value;
          $("memoryDetailContent").textContent = title.value;
        }
        refreshLedger();
      } catch (e) { toast(String(e), "danger"); }
    },
    { okStyle: "primary" });
  title.focus(); title.select();
}
function askFactEdit(id, current) {
  const input = el("input", { type: "text", value: current, style: "width:100%;margin:8px 0;" });
  openModal("改写这条记忆",
    el("div", null, [
      "新的措辞会立即生效（recall 直接读权威存储）：",
      input,
      el("div", { class: "fine", text: "留痕在台账本身；与另一条记忆完全重复的措辞会被拒绝。" }),
    ]),
    "保存改写",
    async () => {
      try {
        await invoke("fact_update", { id, content: input.value });
        toast("已改写这条记忆", "ok");
        if (selectedLedgerRow?.layer === "fact" && selectedLedgerRow.id === id) {
          selectedLedgerRow.summary = input.value;
          $("memoryDetailContent").textContent = input.value;
        }
        refreshLedger();
      } catch (e) { toast(String(e), "danger"); }
    },
    { okStyle: "primary" });
  input.focus(); input.select();
}
$("exportBtn").addEventListener("click", async () => {
  try {
    const path = await invoke("export_data", { now: nowParam() });
    toast(`已导出全部数据：${path}`, "ok");
  } catch (e) { toast(String(e), "danger"); }
});
// 导入是导出的反向。它跨所有层写入，所以和删除组件一样走 Guard：
// 先看清楚来自哪台设备、什么时候、多少条，再决定。
$("importBtn").addEventListener("click", () => $("importFile").click());
$("importFile").addEventListener("change", async (event) => {
  const file = event.currentTarget.files[0];
  if (!file) return;
  event.currentTarget.value = ""; // 允许连续选同一个文件
  try {
    // 先看大小再读内容：file.text() + JSON.parse 会把整份文件读进 WebView
    // 内存，而这一步发生在 Guard 预览之前——用户还没决定要不要导入，进程
    // 就可能已经被撑爆了。64 MiB 远大于任何真实的 Solum 备份。
    const MAX_IMPORT_BYTES = 64 * 1024 * 1024;
    if (file.size > MAX_IMPORT_BYTES) {
      toast(`备份文件超过 ${Math.round(MAX_IMPORT_BYTES / 1024 / 1024)} MiB 上限，已拒绝读取`, "danger");
      return;
    }
    const document_ = JSON.parse(await file.text());
    const args = JSON.stringify({ document: document_ });
    const pending = await invoke("guard_request", { tool: "data_import", args, now: nowParam() });
    const body = el("div", null, [
      el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(pending.tool) }), " ", tag("高危", "danger")]),
      el("div", { class: "preview", text: "后果预览：" + pending.preview }),
      el("div", { class: "fine", text: "确认后签发一次性令牌，绑定此次导入；令牌用后即焚。导入不会删除本机数据。" }),
    ]);
    openModal("人工确认：导入备份", body, "合并导入", async () => {
      try {
        const result = await invoke("guard_confirm", { pendingId: pending.pending_id, tool: pending.tool, args: pending.args, now: nowParam() });
        toast(result.output, result.ok ? "ok" : "danger");
        refreshAll(); refreshAudit();
      } catch (error) { toast(String(error), "danger"); refreshAudit(); }
    }, { icon: "shield" });
  } catch (error) { toast(String(error), "danger"); }
});
$("exportDataIcon").appendChild(icon("download", 19));
$("importDataIcon").appendChild(icon("upload", 19));
$("backupDataIcon").appendChild(icon("lock", 19));
$("exportDataChev").appendChild(icon("chev", 16));
$("importDataChev").appendChild(icon("chev", 16));
$("backupDataChev").appendChild(icon("chev", 16));
$("openBackupCenterBtn").addEventListener("click", () => {
  if (requireAccount("登录后使用加密云备份")) showView("backup");
});
function askForget(layer, id) {
  // 后果预览改由后端生成：它能说出级联真正会带走多少条日程/提醒，
  // 而前端此前只能给一句笼统的「会级联删除」。
  guardedDestructive({
    tool: "memory_forget",
    args: { layer, id },
    title: "人工确认：删除这条记忆",
    note: "确认后签发一次性令牌，绑定这一条记忆；删除后不可通过对话恢复。",
    onDone: async () => {
      selectedLedgerRow = null;
      showView("ledger", { refresh: false });
      await refreshLedger();
    },
  });
}

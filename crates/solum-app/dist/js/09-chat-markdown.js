"use strict";
/* ============================== 对话 Markdown（自研无依赖；块级语义逐条对齐鸿蒙 core/markdown.ets） ==============================
   HTML 永远只按普通文字处理（全走 textContent，绝不 innerHTML）；链接/图片语法
   只显示安全文本（`文字（URL）` / `图片：alt`），不在回复区打开外部地址或加载远程资源。 */
function mdReplacePaired(input, marker) {
  let out = input;
  let start = out.indexOf(marker);
  while (start >= 0) {
    const end = out.indexOf(marker, start + marker.length);
    if (end < 0) break;
    out = out.substring(0, start) + out.substring(start + marker.length, end) + out.substring(end + marker.length);
    start = out.indexOf(marker, start);
  }
  return out;
}
const MD_PLAIN_TEXT_CACHE_MAX = 256;
const mdPlainTextCache = new Map();
// 去掉行内标记，保留人真正要读的文字与链接目标。支持 LRU 缓存。
function mdPlainText(input) {
  const str = String(input);
  if (!str) return "";
  const cached = mdPlainTextCache.get(str);
  if (cached !== undefined) return cached;
  let out = str;
  out = out.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, "图片：$1");
  out = out.replace(/\[([^\]]+)\]\(([^)]+)\)/g, "$1（$2）");
  out = mdReplacePaired(out, "**");
  out = mdReplacePaired(out, "__");
  out = mdReplacePaired(out, "~~");
  out = mdReplacePaired(out, "`");
  out = out.replace(/\\([\\`*_[\]{}()#+\-.!>])/g, "$1");
  const res = out.trim();
  if (mdPlainTextCache.size >= MD_PLAIN_TEXT_CACHE_MAX) {
    const oldest = mdPlainTextCache.keys().next().value;
    if (oldest !== undefined) mdPlainTextCache.delete(oldest);
  }
  mdPlainTextCache.set(str, res);
  return res;
}
function mdIsRule(line) { return /^\s*(?:---+|\*\*\*+|___+)\s*$/.test(line); }
function mdTableCells(line) {
  let t = line.trim();
  if (t.startsWith("|")) t = t.substring(1);
  if (t.endsWith("|")) t = t.substring(0, t.length - 1);
  if (t.indexOf("|") < 0) return [];
  return t.split("|").map((cell) => mdPlainText(cell));
}
function mdIsTableSeparator(line) {
  const cells = mdTableCells(line);
  return cells.length > 0 && cells.every((c) => /^:?-{3,}:?$/.test(c.trim()));
}
function mdStartsBlock(lines, i) {
  const line = lines[i];
  if (line.trim().length === 0 || line.trim().startsWith("```") ||
    /^(#{1,4})\s+/.test(line) || /^\s*>\s?/.test(line) ||
    /^\s*[-*+]\s+/.test(line) || /^\s*\d+[.)]\s+/.test(line) || mdIsRule(line)) return true;
  return i + 1 < lines.length && mdTableCells(line).length > 0 && mdIsTableSeparator(lines[i + 1]);
}
const MD_PARSE_CACHE_MAX = 128;
const mdParseCache = new Map();
// 常用 GFM 块级语法 → 展示模型；格式损坏时退回段落，绝不吞正文。支持 LRU 缓存。
function mdParse(input) {
  const rawInput = String(input);
  if (!rawInput) return [];
  const cached = mdParseCache.get(rawInput);
  if (cached !== undefined) return cached;
  const lines = rawInput.replace(/\r\n?/g, "\n").split("\n");
  const blocks = [];
  let i = 0;
  while (i < lines.length) {
    const raw = lines[i];
    const trimmed = raw.trim();
    if (trimmed.length === 0) { i += 1; continue; }
    if (trimmed.startsWith("```")) {
      const language = mdPlainText(trimmed.substring(3));
      const code = [];
      i += 1;
      while (i < lines.length && !lines[i].trim().startsWith("```")) { code.push(lines[i]); i += 1; }
      if (i < lines.length) i += 1;
      blocks.push({ kind: "code", text: code.join("\n"), language });
      continue;
    }
    const heading = raw.match(/^(#{1,4})\s+(.+)$/);
    if (heading) { blocks.push({ kind: "heading", text: mdPlainText(heading[2]), level: heading[1].length }); i += 1; continue; }
    if (i + 1 < lines.length && mdTableCells(raw).length > 0 && mdIsTableSeparator(lines[i + 1])) {
      const rows = [{ cells: mdTableCells(raw), header: true }];
      i += 2;
      while (i < lines.length) {
        const cells = mdTableCells(lines[i]);
        if (cells.length === 0) break;
        rows.push({ cells, header: false });
        i += 1;
      }
      blocks.push({ kind: "table", rows });
      continue;
    }
    if (mdIsRule(raw)) { blocks.push({ kind: "rule" }); i += 1; continue; }
    const quote = raw.match(/^\s*>\s?(.*)$/);
    if (quote) { blocks.push({ kind: "quote", text: mdPlainText(quote[1]) }); i += 1; continue; }
    const bullet = raw.match(/^\s*[-*+]\s+(.*)$/);
    if (bullet) {
      let text = bullet[1];
      const task = text.match(/^\[([ xX])\]\s+(.*)$/);
      if (task) text = `${task[1].toLowerCase() === "x" ? "☑" : "☐"} ${task[2]}`;
      blocks.push({ kind: "bullet", text: mdPlainText(text) });
      i += 1;
      continue;
    }
    const ordered = raw.match(/^\s*(\d+)[.)]\s+(.*)$/);
    if (ordered) { blocks.push({ kind: "ordered", text: mdPlainText(ordered[2]), level: parseInt(ordered[1], 10) }); i += 1; continue; }
    const paragraph = [trimmed];
    i += 1;
    while (i < lines.length && !mdStartsBlock(lines, i)) { paragraph.push(lines[i].trim()); i += 1; }
    blocks.push({ kind: "paragraph", text: mdPlainText(paragraph.join(" ")) });
  }
  if (mdParseCache.size >= MD_PARSE_CACHE_MAX) {
    const oldest = mdParseCache.keys().next().value;
    if (oldest !== undefined) mdParseCache.delete(oldest);
  }
  mdParseCache.set(rawInput, blocks);
  return blocks;
}
// 助手散文 → DOM。纯段落沿用轻量 span（与旧渲染同形），出现结构才升级为块容器。
function renderMarkdown(text) {
  const raw = String(text || "");
  const blocks = mdParse(raw);
  if (blocks.length === 0) return el("span", { text: raw });
  if (blocks.length === 1 && blocks[0].kind === "paragraph") return el("span", { text: blocks[0].text });
  const box = el("div", { class: "mdbody" });
  for (const b of blocks) {
    if (b.kind === "heading") box.appendChild(el("div", { class: "md-h md-h" + b.level, text: b.text }));
    else if (b.kind === "paragraph") box.appendChild(el("p", { class: "md-p", text: b.text }));
    else if (b.kind === "bullet") box.appendChild(el("div", { class: "md-li" }, [el("span", { class: "md-marker", text: "•" }), el("span", { text: b.text })]));
    else if (b.kind === "ordered") box.appendChild(el("div", { class: "md-li" }, [el("span", { class: "md-marker", text: b.level + "." }), el("span", { text: b.text })]));
    else if (b.kind === "quote") box.appendChild(el("div", { class: "md-quote", text: b.text }));
    else if (b.kind === "code") {
      if (b.language) box.appendChild(el("div", { class: "md-lang", text: b.language }));
      box.appendChild(el("pre", { class: "md-code" }, el("code", { text: b.text })));
    } else if (b.kind === "rule") box.appendChild(el("div", { class: "md-rule" }));
    else if (b.kind === "table") box.appendChild(el("div", { class: "tscroll" },
      dataTable(b.rows[0].cells, b.rows.slice(1).map((r) => r.cells))));
  }
  return box;
}

/* 流式（§3.6 第 7 条）：闲聊纯文本回复边到边渲染。每次发送生成 streamId，
   后端把可见 token 通过 solum-chat-delta 事件按 streamId 推回；信封仍整包（后端
   sniff-router 抑制其可见流式），非闲聊意图无 delta，走原整包路径不变。 */
let chatStreamSeq = 0;
const activeChatStreams = {};

function onChatDelta(streamId, delta) {
  const s = activeChatStreams[streamId];
  if (!s || delta == null) return;
  if (!s.bubble) {
    // 首个 token：把「正在输入」气泡就地转成一个会增长的真实气泡（不闪）。
    s.textSpan = el("span", { text: "" });
    s.typing.className = "msg";
    s.typing.textContent = "";
    s.typing.appendChild(el("div", { class: "intent" }, tag(INTENT.chat, "accent")));
    s.typing.appendChild(s.textSpan);
    s.bubble = s.typing;
  }
  s.text += delta;
  s.textSpan.textContent = s.text;
  requestAnimationFrame(() => { $("chatScroll").scrollTop = $("chatScroll").scrollHeight; });
}

async function sendChat(text, captureId = null) {
  text = String(text || "").trim();
  if (!text) return;
  // 「今日简报」是只读投影，不该落进闲聊意图（欢迎屏 chip 之外的唯一入口）。
  if (/^(刷新)?(今日|每日)简报$/.test(text)) {
    if (!accountLoggedIn) {
      requireAccount("登录后生成包含建议与跨设备信息的今日简报");
      return;
    }
    addMsg("user", [text]);
    recordChatMessage("user", text);
    await refreshDailyBrief();
    return;
  }
  try { await syncChatContext(); }
  catch (e) { toast(`无法载入当前会话上下文：${String(e)}`, "danger"); return; }
  addMsg("user", [text]);
  recordChatMessage("user", text);
  const streamId = "chat-" + (++chatStreamSeq);
  const typing = addMsg("agent typing", [el("i"), el("i"), el("i")]);
  const stream = { typing, bubble: null, textSpan: null, text: "" };
  activeChatStreams[streamId] = stream;
  try {
    const r = await invoke("ingest", { text, now: nowParam(), streamId, captureId });
    delete activeChatStreams[streamId];
    if (stream.bubble) {
      // 已流式的闲聊：用权威终值定稿已在增长的气泡（信封路径不会走到这里，
      // 因为其输出以 { 开头被后端抑制，不产生可见 delta）。流式期间是纯文本，
      // 定稿时才换成 Markdown 渲染（与鸿蒙 B4 相同的分工）。
      const finalized = renderMarkdown(r.message);
      stream.textSpan.replaceWith(finalized);
      stream.textSpan = finalized;
      if (r.ui) stream.bubble.appendChild(renderEnvelope(r.ui, { skipLeadingText: true }));
      if (r.intent === "status_answer") $("checkinBar").classList.remove("show");
      recordChatMessage("assistant", r.message, r.intent);
      recordChatTurn(text, r.message, r.intent);
      refreshAfterIngest();
      return true;
    }
    // 无可见流式（非闲聊意图，或被抑制的信封）：整包构建，与流式前完全一致。
    (typing._msgRow || typing).remove();
    const parts = [];
    parts.push(el("div", { class: "intent" },
      tag(INTENT[r.intent] || r.intent, r.intent === "dangerous_command" ? "danger" : "accent")));
    parts.push(renderMarkdown(r.message));
    if (r.event) {
      parts.push(el("div", { class: "evmeta" }, [
        `事件 #${r.event.id} · ${KIND[r.event.kind] || r.event.kind} · `, fmtDT(r.event.start),
      ]));
      if (IS_ANDROID) parts.push(el("button", {
        class: "btn sm", type: "button", text: "在系统日历中核对并保存",
        onclick: async () => {
          try {
            const opened = await invoke("calendar_open_insert", { eventId: r.event.id });
            toast(opened ? "已打开系统日历；保存前请再次核对" : "没有可处理日历写入的系统应用", opened ? "ok" : "danger");
          } catch (error) { toast(safeUserError(error, "暂时无法打开系统日历"), "danger"); }
        },
      }));
    }
    const bubble = addMsg("agent", parts);
    // F18：信封只当数据渲染（目录内组件 → DOM），绝不当 HTML。
    if (r.ui) bubble.appendChild(renderEnvelope(r.ui, { skipLeadingText: true }));
    // F19 deliberately has its own data-bound renderer. Confirming this card
    // is the only UI path that can make the validated definition persistent.
    if (r.widget_preview) bubble.appendChild(renderWidgetPreview(r.widget_preview));
    // 高危拦截不能是死胡同：没有专属确认入口（如 ledger_purge 的发起按钮）
    // 时，至少给一条去护栏页的路（2026-07-17 走查发现 3）。
    if (r.intent === "dangerous_command" && !r.ui) {
      bubble.appendChild(el("button", {
        class: "btn sm", style: "margin-top:8px;", text: "前往护栏查看确认流程",
        onclick: () => showView("guard"),
      }));
    }
    if (r.intent === "status_answer") $("checkinBar").classList.remove("show");
    recordChatMessage("assistant", r.message, r.intent);
    recordChatTurn(text, r.message, r.intent);
    refreshAfterIngest();
    return true;
  } catch (err) {
    delete activeChatStreams[streamId];
    if (stream.bubble) {
      // 已经流出一段又报错：在同一气泡后补一条错误行，不丢已显示的内容。
      stream.bubble.appendChild(el("div", { class: "intent" }, tag("错误", "danger")));
      stream.bubble.appendChild(el("span", { text: safeUserError(err, "这次请求没有完成，请稍后重试。") }));
    } else {
      typing.remove();
      addMsg("agent", [el("div", { class: "intent" }, tag("错误", "danger")), safeUserError(err, "这次请求没有完成，请稍后重试。")] );
    }
    recordChatMessage("assistant", safeUserError(err, "这次请求没有完成，请稍后重试。"), "error");
    return false;
  }
}
$("chatForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = $("chatInput").value.trim();
  if (!text) return;
  $("chatInput").value = "";
  saveCurrentChatDraft(true);
  const captureId = pendingCaptureDraft?.id || null;
  const sent = await sendChat(text, captureId);
  if (sent && captureId) pendingCaptureDraft = null;
});
$("chatInput").addEventListener("input", () => saveCurrentChatDraft(false));
$("sendBtn").appendChild(icon("send", 17));
$("newSessionBtn").appendChild(icon("plus", 17));
$("newSessionBtnMobile").appendChild(icon("plus", 17));
$("sessionPickerMobileChevron").appendChild(icon("chev", 16));
$("newSessionBtn").addEventListener("click", createChatSession);
$("newSessionBtnMobile").addEventListener("click", createChatSession);
function toggleSessionMenu(force) {
  const menu = $("sessionMenuMobile");
  const show = force != null ? force : menu.hidden;
  if (show === !menu.hidden) return;
  if (show) toggleComposerMenu(false);
  menu.hidden = !show;
  $("sessionPickerMobile").setAttribute("aria-expanded", String(show));
  syncOverlayHistory();
  if (show) requestAnimationFrame(() => menu.querySelector('[aria-selected="true"]')?.focus());
}
$("sessionPickerMobile").addEventListener("click", (event) => {
  event.stopPropagation();
  toggleSessionMenu();
});
document.addEventListener("click", (event) => {
  if (!$("sessionMenuMobile").hidden && !event.target.closest(".session-picker-mobile")) toggleSessionMenu(false);
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("sessionMenuMobile").hidden) toggleSessionMenu(false);
});
// 资料上传入口当前由工作台的原生文件选择器承接；旧的独立图标节点已移除。
$("documentUploadIcon")?.appendChild(icon("folder", 23));

const EXAMPLES = [
  ["安排一个日程", "明天下午 3 点，在会议室和张伟开会"],
  ["记住一个截止时间", "下周五之前交实验报告"],
  ["整理最近的状态", "我最近总忘记喝水，帮我想个办法"],
];
for (const [label, text] of EXAMPLES) {
  $("examples").appendChild(el("button", {
    class: "example", type: "button",
    onclick: () => { $("chatInput").value = text; $("chatInput").focus(); },
  }, [
    el("span", { class: "example-seed", "aria-hidden": "true" }),
    el("span", { class: "example-copy" }, [
      el("strong", { text: label }),
      el("small", { text }),
    ]),
    el("span", { class: "example-arrow", text: "→", "aria-hidden": "true" }),
  ]));
}

function dailyBriefText(brief) {
  const events = (brief.events_today || []).map((e) => `${fmtDTText(e.start)} ${e.title}`);
  const due = (brief.due_reminders || []).map((n) => `${fmtDTText(n.fire_at)} 提醒`);
  const upcoming = (brief.upcoming_reminders || []).map((n) => `${fmtDTText(n.fire_at)} 提醒`);
  const suggestions = (brief.top_suggestions || []).map((s) => s.text);
  const section = (label, rows) => `${label}${rows.length ? "：" + rows.join("；") : "：无"}`;
  return [
    `今日聚焦（${brief.date}）`,
    section("今日日程", events),
    section("到点提醒", due),
    section("即将提醒", upcoming),
    section("待处理建议", suggestions),
  ].join("\n");
}
async function refreshDailyBrief() {
  try {
    const brief = await invoke("daily_brief", { now: nowParam() });
    const bubble = addMsg("agent", [el("div", { class: "intent" }, tag("今日简报", "accent"))]);
    // 手动入口复用协议渲染器显示 Tauri command 返回的只读摘要。
    bubble.appendChild(renderEnvelope({
      version: 1,
      components: [{ type: "text", content: dailyBriefText(brief) }],
    }));
    recordChatMessage("assistant", dailyBriefText(brief), "daily_brief");
    refreshToday(); refreshNotifs(); refreshSuggest();
  } catch (e) { toast(String(e), "danger"); }
}

/* ---------------- 生成式 UI 渲染器（F18 §3.9） ----------------
   JSON → DOM，只认目录内 8 种组件；动作经下方白名单分发，
   solum-core 已校验过一遍，这里是壳层二次校验（§3.9 原则 2）。 */
const GENUI_DISPATCH = {
  ingest: (a) => sendChat(a.text),
  checkin_answer: (a) => sendChat(a.text),
  reminder_dismiss: async (a) => {
    await invoke("dismiss", { id: a.id });
    toast(`已取消提醒 #${a.id}（事件保留）`, "ok");
    refreshAll();
  },
  event_reschedule: async (a) => {
    const msg = await invoke("event_reschedule", { id: a.id, start: a.start, now: nowParam() });
    toast(msg, "ok");
    refreshAgenda(); refreshNotifs(); refreshToday();
  },
  event_cancel: (a) => guardedDestructive({
    tool: "event_cancel",
    args: { event_id: a.id },
    title: "人工确认：取消日程",
    confirmLabel: "确认取消",
    note: "确认后签发一次性令牌，并绑定当前日程标题、时间和提醒数量。",
    onDone: async (ok) => { if (ok) await refreshAll(); },
  }),
  routine_set_active: async (a) => {
    await invoke("routine_set_active", { id: a.id, active: a.active, now: nowParam() });
    toast(a.active ? "固定提醒已启用" : "固定提醒已暂停", "ok");
    refreshAll();
  },
  reminder_fire: async () => {
    const fired = await invoke("fire_due", { now: nowParam() });
    toast(fired.length ? `已触发 ${fired.length} 条提醒` : "没有到点的提醒", fired.length ? "amber" : undefined);
    refreshAll();
  },
  suggestion_set: async (a) => {
    const followUp = await invoke("suggest_set", { id: a.id, status: a.status, now: nowParam() });
    // D4：采纳习惯建议会自动创建固定提醒（或暂停一个），后端会说明。
    toast(followUp || (a.status === "accepted" ? "已采纳" : "已忽略"),
          a.status === "accepted" ? "ok" : undefined);
    refreshSuggest(); refreshToday();
  },
  proactivity_set: async (a) => {
    await invoke("proactivity_set", { dimension: a.dimension, level: a.level });
    toast(`已设置 ${DIM[a.dimension] || a.dimension} = ${LEVEL[a.level] || a.level}`, "ok");
    refreshProactivity();
  },
  persona_import_save: async (a) => {
    const p = await invoke("persona_import_save", {
      nickname: (a.nickname || "").trim() || null,
      tone: a.tone || "",
      catchphrases: String(a.catchphrases || "").split(/[,，、]/).map((s) => s.trim()).filter(Boolean),
      styleNotes: (a.style_notes || "").trim() || null,
      note: (a.note || "").trim() || null,
      now: nowParam(),
    });
    toast(`已保存导入人格 v${p.version}（可随时回滚）`, "ok");
    refreshPersona();
  },
  // 只是入口：§3.3 的完整确认对话框 + 一次性令牌照走。
  guard_request: async (a) => {
    const p = await invoke("guard_request", { tool: a.tool, args: a.args, now: nowParam() });
    openGuardConfirm(p, false);
  },
};
async function runGenuiAction(action, extraArgs, groupEl) {
  if (!action || !GENUI_DISPATCH[action.command]) {
    toast(`动作不在白名单：${action && action.command}`, "danger");
    return;
  }
  // 一次性：点过的组整体失效——信封是当下的一次性渲染，
  // 对已推进的状态重复执行动作正是要避免的。
  if (groupEl) groupEl.querySelectorAll("button").forEach((b) => { b.disabled = true; });
  try {
    await GENUI_DISPATCH[action.command]({ ...(action.args || {}), ...(extraArgs || {}) });
  } catch (e) {
    toast(String(e), "danger");
    if (groupEl) groupEl.querySelectorAll("button").forEach((b) => { b.disabled = false; });
  }
}
function genuiButton(b, groupEl, extra) {
  return el("button", {
    type: "button",
    class: "btn sm" + (b.style === "primary" ? " primary" : b.style === "danger" ? " danger" : ""),
    text: b.label,
    onclick: () => runGenuiAction(b.action, extra && extra(), groupEl),
  });
}
function renderEnvelope(env, opts) {
  opts = opts || {};
  const root = el("div", { class: "gc" });
  if (!env || env.version !== 1 || !Array.isArray(env.components)) return root;
  let skippedText = false;
  for (const c of env.components) {
    // message 字段已展示首个 text 组件，跳过一次避免重复。
    if (opts.skipLeadingText && !skippedText && c.type === "text") { skippedText = true; continue; }
    const node = renderComponent(c);
    if (node) root.appendChild(node);
  }
  return root;
}
let genuiFieldSeq = 0;
function renderComponent(c) {
  switch (c.type) {
    case "text":
      return el("div", { class: "gc", text: c.content });
    case "event_card":
      return el("div", { class: "gc gc-card" }, [
        tag(KIND[c.kind] || c.kind, "accent"),
        el("span", { class: "title", text: c.title }),
        el("span", { class: "meta" }, [fmtDT(c.start), c.location ? " · " + c.location : ""]),
      ]);
    case "reminder_card": {
      const s = STATUS[c.status] || [c.status, ""];
      return el("div", { class: "gc gc-card" }, [
        icon("bell", 15),
        el("span", { class: "title", text: c.event_title }),
        el("span", { class: "meta" }, fmtDT(c.fire_at)),
        st(s[0], s[1]),
      ]);
    }
    case "routine_card":
      return el("div", { class: "gc gc-card" }, [
        icon("clock", 15),
        tag("固定提醒", "accent"),
        el("span", { class: "title", text: c.title }),
        el("span", { class: "meta", text: `每天 ${c.time_of_day}` }),
        st(c.active ? "已启用" : "已暂停", c.active ? "ok" : ""),
      ]);
    case "suggestion_card":
      return el("div", { class: "gc gc-card" }, [
        tag(SKIND[c.kind] || c.kind, "accent"),
        el("span", { class: "title", text: c.content }),
      ]);
    case "button_group":
    case "choice": {
      const div = el("div", { class: "gc gc-btns" });
      for (const b of (c.buttons || c.options || [])) div.appendChild(genuiButton(b, div));
      return div;
    }
    case "form": {
      const div = el("div", { class: "gc gc-form" });
      const inputs = {};
      for (const f of c.fields || []) {
        let input;
        if (f.type === "select") {
          input = el("select", null, (f.options || []).map((o) => el("option", { value: o, text: o })));
          if (f.value != null) input.value = f.value;
        } else if (f.type === "toggle") {
          input = el("input", { type: "checkbox", checked: f.value === "true" });
        } else {
          input = el("input", { type: f.type === "datetime" ? "datetime-local" : "text" });
          if (f.value != null) input.value = f.value;
        }
        const inputId = "genui-field-" + (++genuiFieldSeq);
        input.id = inputId;
        inputs[f.name] = input;
        div.appendChild(el("div", { class: "gf" }, [el("label", { for: inputId, text: f.label }), input]));
      }
      const values = () => {
        const v = {};
        for (const [name, input] of Object.entries(inputs)) {
          v[name] = input.type === "checkbox" ? String(input.checked) : input.value;
        }
        return v;
      };
      div.appendChild(el("div", { class: "submit" }, genuiButton(c.submit, div, values)));
      return div;
    }
    default:
      return null; // 目录外组件：什么都不渲染，绝不猜
  }
}

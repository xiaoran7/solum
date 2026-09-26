"use strict";
/* ============================== 基础工具 ============================== */
const $ = (id) => document.getElementById(id);

/* UI 状态只留在本机 WebView：导航位置、滚动位置和对话草稿。
   不写邮件字段、搜索关键词、通知正文或任何账号/密钥。 */
let localProfileId = "guest";
let UI_STATE_KEY = "solum.ui-state.v1";
const UI_STATE_VERSION = 1;
function safeLocalGet(key) {
  try { return localStorage.getItem(key); } catch (_) { return null; }
}
function safeLocalSet(key, value) {
  try { localStorage.setItem(key, value); return true; } catch (_) { return false; }
}
function loadUiState() {
  try {
    const parsed = JSON.parse(safeLocalGet(UI_STATE_KEY) || "null");
    if (!parsed || parsed.version !== UI_STATE_VERSION) throw new Error("unsupported UI state");
    return {
      version: UI_STATE_VERSION,
      lastView: typeof parsed.lastView === "string" ? parsed.lastView : "chat",
      navLastView: parsed.navLastView && typeof parsed.navLastView === "object" ? parsed.navLastView : {},
      scroll: parsed.scroll && typeof parsed.scroll === "object" ? parsed.scroll : {},
      chatDrafts: parsed.chatDrafts && typeof parsed.chatDrafts === "object" ? parsed.chatDrafts : {},
    };
  } catch (_) {
    return { version: UI_STATE_VERSION, lastView: "chat", navLastView: {}, scroll: {}, chatDrafts: {} };
  }
}
let uiState = loadUiState();
let uiStateSaveTimer = 0;
function flushUiState() {
  clearTimeout(uiStateSaveTimer);
  uiStateSaveTimer = 0;
  const drafts = Object.entries(uiState.chatDrafts || {})
    .filter(([id, text]) => id && typeof text === "string" && text.length)
    .slice(-12);
  uiState.chatDrafts = Object.fromEntries(drafts);
  const logical = JSON.stringify(uiState);
  try {
    const existing = JSON.parse(safeLocalGet(UI_STATE_KEY) || "null");
    if (existing && typeof existing === "object") {
      delete existing.updatedAt;
      if (JSON.stringify(existing) === logical) return true;
    }
  } catch (_) { /* replace malformed state below */ }
  return safeLocalSet(UI_STATE_KEY, JSON.stringify({ ...uiState, updatedAt: new Date().toISOString() }));
}
function saveUiState(immediate) {
  clearTimeout(uiStateSaveTimer);
  if (immediate) { flushUiState(); return; }
  uiStateSaveTimer = setTimeout(flushUiState, 280);
}

function migrateMemoryListUiState() {
  const key = `solum.memory-list-layout.v2.${localProfileId}`;
  if (safeLocalGet(key) === "done") return;
  const movedMemoryViews = new Set(["journal", "wearable", "review", "captures"]);
  viewScrollPositions.set("ledger", 0);
  uiState.scroll.ledger = 0;
  navLastView.memory = "ledger";
  uiState.navLastView.memory = "ledger";
  if (movedMemoryViews.has(uiState.lastView)) uiState.lastView = "ledger";
  saveUiState(true);
  safeLocalSet(key, "done");
}

// el("div", { class: "x", text: "y", onclick: fn }, [children]) → DOM 节点。
// 全部走 textContent / createElement，杜绝字符串拼 HTML 的注入面。
function el(tag, attrs, children) {
  const n = document.createElement(tag);
  if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v == null) continue;
    if (k === "class") n.className = v;
    else if (k === "text") n.textContent = v;
    else if (k === "html") n.innerHTML = v;            // 仅限内部静态字符串（图标）
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (k === "value") n.value = v;
    else if (k === "checked") n.checked = !!v;
    else if (k === "disabled") n.disabled = !!v;
    else n.setAttribute(k, v);
  }
  if (children) for (const c of [].concat(children)) {
    if (c == null) continue;
    n.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return n;
}

/* 手绘 24 网格线条图标（stroke=currentColor），全站唯一图标词汇。 */
const ICONS = {
  home:     '<path d="m3.5 11 8.5-7 8.5 7"/><path d="M5.5 10v9.5h13V10M9.5 19.5v-6h5v6"/>',
  search:   '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m15.5 15.5 5 5"/>',
  chat:     '<path d="M21 11.5a8.5 8.5 0 0 1-8.5 8.5c-1.35 0-2.64-.31-3.78-.88L3 21l1.88-5.72A8.5 8.5 0 1 1 21 11.5z"/>',
  calendar: '<rect x="3.5" y="5" width="17" height="16" rx="2.5"/><path d="M8 3v4M16 3v4M3.5 11h17"/>',
  journal:  '<rect x="5" y="3" width="15" height="18" rx="2"/><path d="M9.5 3v18M13.5 8.5h3M13.5 12.5h3"/>',
  memory:   '<rect x="3" y="4" width="18" height="4.5" rx="1.5"/><path d="M5 8.5V18a2.5 2.5 0 0 0 2.5 2.5h9A2.5 2.5 0 0 0 19 18V8.5M10 12.5h4"/>',
  settings: '<path d="M5 8h7.5M17.5 8H19M5 16h1.5M11 16h8"/><circle cx="15" cy="8" r="2.3"/><circle cx="8.5" cy="16" r="2.3"/>',
  bell:     '<path d="M18 10a6 6 0 1 0-12 0c0 5.5-2 6.8-2 6.8h16s-2-1.3-2-6.8"/><path d="M10.4 20.5a1.9 1.9 0 0 0 3.2 0"/>',
  bulb:     '<path d="M12 3a6 6 0 0 1 3.6 10.8c-.7.53-1.1 1.3-1.1 2.2h-5c0-.9-.4-1.67-1.1-2.2A6 6 0 0 1 12 3z"/><path d="M9.8 19.2h4.4M10.8 21.5h2.4"/>',
  heart:    '<path d="M12 20.2 4.9 13a4.8 4.8 0 1 1 7.1-6.4A4.8 4.8 0 1 1 19.1 13z"/>',
  doc:      '<path d="M13.5 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5z"/><path d="M13.5 3v5.5H19M9 13.5h6M9 17h6"/>',
  folder:   '<path d="M3.5 7.5A2.5 2.5 0 0 1 6 5h4l2 2.5h6A2.5 2.5 0 0 1 20.5 10v7.5A2.5 2.5 0 0 1 18 20H6a2.5 2.5 0 0 1-2.5-2.5z"/>',
  image:    '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><circle cx="9" cy="9" r="1.6"/><path d="m5.5 17 4.2-4.2 3.1 3.1 2.2-2.2 3.5 3.3"/>',
  download: '<path d="M12 3v12M7.5 10.5 12 15l4.5-4.5"/><path d="M4 20h16"/>',
  upload:   '<path d="M12 16V4M7.5 8.5 12 4l4.5 4.5"/><path d="M4 20h16"/>',
  plus:     '<path d="M12 5v14M5 12h14"/>',
  user:     '<circle cx="12" cy="8" r="3.8"/><path d="M5.5 20.5a6.5 6.5 0 0 1 13 0"/>',
  gauge:    '<path d="M4.5 16.5a8.5 8.5 0 1 1 15 0"/><path d="m12 14 3.2-3.2"/>',
  rules:    '<path d="M8.5 6h11.5M8.5 12h11.5M8.5 18h11.5M4 6h.01M4 12h.01M4 18h.01"/>',
  shield:   '<path d="M12 2.8 19.5 5.6v5.9c0 4.6-3.2 7.7-7.5 9.2-4.3-1.5-7.5-4.6-7.5-9.2V5.6z"/>',
  refresh:  '<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8"/><path d="M21 3v5h-5"/>',
  cloud:    '<path d="M17.5 18.5a4 4 0 0 0 .5-7.97A6.5 6.5 0 0 0 5.3 12 3.8 3.8 0 0 0 6.5 18.5z"/>',
  clock:    '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 1.8"/>',
  trash:    '<path d="M4.5 6.5h15M9.5 6.5v-2a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v2M6.5 6.5l.8 12.1a2 2 0 0 0 2 1.9h5.4a2 2 0 0 0 2-1.9l.8-12.1"/>',
  pencil:   '<path d="M4 20h4L19.5 8.5a2.12 2.12 0 0 0-3-3L5 17zM13.5 6.5l3 3"/>',
  x:        '<path d="M6 6l12 12M18 6 6 18"/>',
  check:    '<path d="M4.5 12.5 10 18 19.5 7"/>',
  send:     '<path d="M21.5 2.5 11 13M21.5 2.5l-6.7 19-3.8-8.5-8.5-3.8z"/>',
  mail:     '<rect x="3.5" y="5" width="17" height="14" rx="2"/><path d="m4 7 8 6 8-6"/>',
  alert:    '<path d="M10.3 4.2 2.7 17.6A2 2 0 0 0 4.4 20.6h15.2a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0z"/><path d="M12 9.5v4.2M12 16.8h.01"/>',
  watch:    '<circle cx="12" cy="12" r="5.5"/><path d="M12 9.7V12l1.7 1M9.7 3.5h4.6l.4 3.1H9.3zM9.7 20.5h4.6l.4-3.1H9.3z"/>',
  help:     '<circle cx="12" cy="12" r="8.5"/><path d="M9.6 9.6a2.4 2.4 0 0 1 4.6.9c0 1.6-2.4 2-2.4 3.3"/><path d="M12 17.3v.01"/>',
  back:     '<path d="M19 12H5M11.5 5.5 5 12l6.5 6.5"/>',
  chev:     '<path d="m9 5 7 7-7 7"/>',
  lock:     '<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8.5 10.5V7.5a3.5 3.5 0 0 1 7 0v3"/>',
  eye:      '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><circle cx="12" cy="12" r="3"/>',
  eye_off:  '<path d="M9.88 9.88a3 3 0 1 0 4.24 4.24"/><path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68"/><path d="M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61"/><line x1="2" y1="2" x2="22" y2="22"/>',
  inbox:    '<path d="M3.5 13.5h4l1.5 3h6l1.5-3h4"/><path d="M5.8 4.5h12.4l2.3 9v4a2 2 0 0 1-2 2H5.5a2 2 0 0 1-2-2v-4z"/>',
};
function icon(name, size) {
  // SVG 必须走 SVG 命名空间：借 innerHTML 的 HTML 解析器切换 foreign content。
  const s = size || 18;
  const wrap = document.createElement("span");
  wrap.innerHTML = `<svg class="ic" viewBox="0 0 24 24" width="${s}" height="${s}"`
    + ` fill="none" stroke="currentColor" stroke-width="1.75"`
    + ` stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">`
    + (ICONS[name] || "") + `</svg>`;
  return wrap.firstChild;
}

// 品牌印记的唯一实体在 index.html 的 <defs>（#solumMark）；这里经 <use> 复用，
// 主描边是 currentColor，跟随所在上下文的 color。
function solumMark(size) {
  const wrap = document.createElement("span");
  wrap.innerHTML = `<svg class="solum-logo-svg" viewBox="0 0 256 256" width="${size}" height="${size}" aria-hidden="true"><use href="#solumMark"/></svg>`;
  return wrap.firstChild;
}

function tag(label, kind) { return el("span", { class: "tag" + (kind ? " " + kind : ""), text: label }); }
function st(label, dotKind) {
  return el("span", { class: "st" }, [
    el("span", { class: "sdot " + (dotKind || "hollow") }), label,
  ]);
}
// "2026-07-15T15:00" → 日期弱化 + 时间突出的等宽片段
function fmtDT(s) {
  const [d, t] = String(s || "").split("T");
  return el("span", { class: "dtme" }, [el("span", { class: "d", text: d + " " }), t || ""]);
}
// Text-only counterpart for prose. `fmtDT` intentionally returns a DOM node,
// so it must never be interpolated into a template string.
function fmtDTText(s) {
  const [d, t] = String(s || "").split("T");
  return d + (t ? " " + t : "");
}
function emptyBox(iconName, title, hint) {
  return el("div", { class: "empty" }, [
    icon(iconName, 22),
    el("span", { class: "et", text: title }),
    hint ? el("span", { class: "eh", text: hint }) : null,
  ]);
}
function dataTable(headers, rows) {
  return el("table", { class: "dt" }, [
    el("thead", null, el("tr", null, headers.map((h) => el("th", { text: h })))),
    el("tbody", null, rows.map((cells) => el("tr", null,
      cells.map((c) => (c && c.tagName === "TD") ? c : el("td", null, c))))),
  ]);
}
function td(children, cls) { return el("td", { class: cls }, children); }

// Keep transport/IPC details in the console for diagnosis, but expose only a
// recovery action in product UI. The backend remains free to return rich
// errors; this is the presentation boundary for desktop and Android.
function safeUserError(error, fallback) {
  const raw = String(error == null ? "" : error);
  try { console.warn("[solum-ui]", raw); } catch (_) { /* logging is best effort */ }
  if (/state not managed|app_info/i.test(raw)) return "应用状态正在初始化，请重新打开应用。";
  if (/账号或密码|账号名已经存在|请填写账号和密码|注册密码|登录已过期|重新登录|服务器未开放注册|尝试太频繁|验证码错误|验证码已失效|图形验证码错误|已被注册/.test(raw)) {
    return raw.replace(/^.*?(账号或密码不正确|该账号名已经存在|请填写账号和密码|注册密码至少需要 8 个字符。|登录已过期，请重新登录|账号服务器版本过旧：登录结果缺少不可变 user_id，请先升级 solum-cloud|服务器未开放注册或当前操作无权限|尝试太频繁，请稍后再试|图形验证码错误或已过期，请点击刷新重试|邮箱验证码错误或已失效|该邮箱已被注册，请直接登录).*$/s, "$1");
  }
  if (/同步|sync|无法解密|留存窗口/i.test(raw)) return "同步暂时失败，已保留本地数据；请稍后重试。";
  if (/网络|network|transport|tls|ssl|errno|os error|连接|超时|timeout|dns|url|https?:\/\//i.test(raw)) {
    return "网络连接失败，请检查网络后重试。";
  }
  if (/guard|token|确认|dangerous|高危/i.test(raw)) return "这次操作没有执行，请重新确认。";
  return fallback || "操作未完成，请重试。";
}

function toast(msg, kind, ms) {
  const t = $("toast");
  t.textContent = "";
  const ic = { ok: "check", amber: "bell", danger: "alert", bulb: "bulb", cloud: "cloud",
               sync: "refresh", heart: "heart", clock: "clock" }[kind];
  if (ic) {
    const cls = { ok: "ok", bulb: "amber", amber: "amber", danger: "danger" }[kind] || "";
    const i = icon(ic, 16); i.classList.add(cls || "plainic"); if (cls) i.classList.add(cls);
    t.appendChild(i);
  }
  t.appendChild(el("span", { text: kind === "danger" ? safeUserError(msg) : msg }));
  t.classList.add("show");
  clearTimeout(t._h);
  t._h = setTimeout(() => t.classList.remove("show"), ms || 2800);
}

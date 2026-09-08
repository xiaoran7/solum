
"use strict";

// Android WebView 下 env(safe-area-inset-*) 可能恒为 0，CSS 里 html.android
// 分支提供状态栏/手势条兜底内边距（见移动端媒体查询注释）。
const IS_ANDROID = /Android/i.test(navigator.userAgent);
if (IS_ANDROID) document.documentElement.classList.add("android");
const rawInvoke = window.__TAURI__ && window.__TAURI__.core
  ? window.__TAURI__.core.invoke.bind(window.__TAURI__.core)
  : () => Promise.reject(new Error("Tauri API 不可用（请从桌面应用打开）"));

// 只缓存幂等、权威数据的短时读取：目的不是复制一份数据库，而是合并同一轮
// refreshToday / refreshAll 发出的重复 IPC。邮件内容、隐私同意状态和配置密钥
// 不进入这里；任何写命令成功后整代失效，删除/改写会立即回到权威存储。
const IPC_CACHE_TTL_MS = new Map([
  ["sync_status", 1200], ["sync_config_get", 1200],
  ["agenda", 900], ["due", 900], ["all_notifications", 900], ["all_events", 900],
  ["suggestions", 900], ["behavior_log", 900], ["health_status", 1200],
  ["ledger", 900], ["routines", 900], ["persona_get", 1200], ["rules", 1200],
  ["proactivity_get", 1200], ["notif_cloud_get", 900],
  ["capture_entry_points", 1200], ["capture_inbox_list", 700],
  ["notif_intelligence_status", 700], ["tools", 1500], ["audit", 900],
  ["llm_config_get", 1200], ["account_status_get", 1200],
  ["soulous_config_get", 1200], ["widget_defs", 900], ["widget_records", 900],
]);
const IPC_READ_ONLY_UNCACHED = new Set([
  "app_info", "email_config_get", "email_folders", "email_message", "email_messages",
  "email_search", "health_samples", "health_trends", "notif_access_status", "notif_intelligence_apps",
  "notif_intelligence_auto_event_counts", "privacy_consent_status", "review",
]);
const ipcCache = new Map();
const ipcInFlight = new Map();
let ipcCacheGeneration = 0;
function ipcCacheKey(command, args) {
  try { return `${command}:${JSON.stringify(args || {})}`; }
  catch (_) { return command; }
}
function invalidateIpcCache() {
  ipcCacheGeneration += 1;
  ipcCache.clear();
}
const ACCOUNT_EXPIRY_PATTERN = /登录已过期|认证已过期|当前账号会话已退出|旧账号会话缺少[^。]*重新登录|(?:^|\s)(?:401|unauthorized)(?:\s|$)/i;
const ACCOUNT_EXPIRY_BYPASS = new Set([
  "account_login", "account_register", "account_logout", "account_status_get",
  "account_oauth_config_get", "account_oauth_login", "account_recover_login",
]);
function isAccountExpiryError(error) {
  return ACCOUNT_EXPIRY_PATTERN.test(String(error || ""));
}
function observeAccountExpiry(command, promise) {
  return promise.catch((error) => {
    if (!ACCOUNT_EXPIRY_BYPASS.has(command) && isAccountExpiryError(error)) {
      void handleAccountExpiry(error, command);
    }
    throw error;
  });
}
function invoke(command, args) {
  const ttl = IPC_CACHE_TTL_MS.get(command);
  if (!ttl) {
    return observeAccountExpiry(command, rawInvoke(command, args).then((value) => {
      if (!IPC_READ_ONLY_UNCACHED.has(command)) invalidateIpcCache();
      return value;
    }));
  }
  const key = ipcCacheKey(command, args);
  const now = performance.now();
  const cached = ipcCache.get(key);
  if (cached && now - cached.at < ttl) return Promise.resolve(cached.value);
  const pending = ipcInFlight.get(key);
  if (pending) return observeAccountExpiry(command, pending);
  const generation = ipcCacheGeneration;
  const request = rawInvoke(command, args).then((value) => {
    if (generation === ipcCacheGeneration) ipcCache.set(key, { at: performance.now(), value });
    return value;
  }).finally(() => ipcInFlight.delete(key));
  ipcInFlight.set(key, request);
  return observeAccountExpiry(command, request);
}

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
  if (/账号或密码|账号名已经存在|请填写账号和密码|注册密码|登录已过期|重新登录|服务器未开放注册|尝试太频繁/.test(raw)) {
    return raw.replace(/^.*?(账号或密码不正确|该账号名已经存在|请填写账号和密码|注册密码至少需要 8 个字符。|登录已过期，请重新登录|账号服务器版本过旧：登录结果缺少不可变 user_id，请先升级 solum-cloud|服务器未开放注册或当前操作无权限|尝试太频繁，请稍后再试).*$/s, "$1");
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

/* ============================== 领域词汇 ============================== */
const KIND = { exam: "考试", meeting: "会议", class: "课程", deadline: "截止", reminder: "提醒", other: "事件" };
const RISK = { safe: ["安全", "ok"], sensitive: ["敏感", "accent"], dangerous: ["高危", "danger"] };
const TOOL_LABELS = {
  memory_forget: "删除一条记忆",
  event_cancel: "取消日程",
  ledger_purge: "清理旧记录",
  widget_delete: "删除组件",
  data_import: "合并导入数据",
  email_send: "发送邮件",
  soulous_push_event: "发送日程到息壤",
  demo_delete: "删除示例记录",
};
function humanToolLabel(name) { return TOOL_LABELS[name] || "受保护操作"; }
function humanSourceLabel(source) {
  const raw = String(source || "");
  if (!raw) return "本地记录";
  if (raw === "ingest_event") return "聊天记录";
  if (/^event#\d+$/.test(raw)) return "日程";
  if (/^routine#\d+$/.test(raw)) return "固定提醒";
  if (raw.startsWith("habit:")) return "习惯建议";
  if (raw === "system_share") return "系统分享";
  if (raw === "manual") return "手动记录";
  if (/soulous/i.test(raw)) return "Soulous";
  return "本地记录";
}
const STATUS = { pending: ["待触发", "hollow"], fired: ["已触发", "ok"], dismissed: ["已取消", ""] };
const INTENT = { chat: "闲聊", ingest_event: "事件登记", status_answer: "状态记录", memory_write: "记忆写入",
                 create_widget: "创建组件", reschedule_event: "改期", cancel_event: "取消日程", dangerous_command: "高危指令" };
const DIM = { schedule_reminders: "日程提醒", life_suggestions: "生活建议", status_checkins: "状态问询", weekly_review: "周报复盘" };
const DIMDESC = {
  schedule_reminders: "到点与分级提前提醒的触达",
  life_suggestions: "是否主动生成并提示生活建议",
  status_checkins: "主动问「现在在干嘛」的频率（秘书 4h、管家 2h）",
  weekly_review: "周期性复盘简报的主动程度",
};
const LEVEL = { passive: "被动", secretary: "秘书", butler: "管家" };
const LEVELS = ["passive", "secretary", "butler"];
const BKIND = { status: ["状态", "ok"], checkin_asked: ["问询", "accent"], reminder_fired: ["提醒触发", ""] };
const SKIND = { exam_prep: "考试备考", deadline_crunch: "截止赶工", early_event: "早会早休", schedule_conflict: "日程冲突", habit_reminder: "习惯提醒" };
const SSTATUS = { pending: ["待处理", "hollow"], accepted: ["已采纳", "ok"], dismissed: ["已忽略", ""] };
const LAYER = { raw_input: ["原始输入", ""], event: ["事件", "accent"], notification: ["提醒", ""],
                behavior: ["行为", "ok"], suggestion: ["建议", ""], wearable: ["健康", "ok"],
                fact: ["记忆", "accent"], routine: ["固定提醒", "accent"],
                notification_capture: ["通知", "accent"] };
const HKIND = { heart_rate: ["心率", "danger"], steps: ["步数", "ok"], sleep: ["睡眠", ""] };
const HUNIT = { heart_rate: "bpm", steps: "步", sleep: "分钟" };

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
$("simClock").addEventListener("change", refreshAll);

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

/* ============================== 同步账户（设置 → 同步） ============================== */
// 账号访问令牌只存在 solum-account.json；这里仅配置中继和端到端加密密码。
// 密码由 Rust 派生为 key 后丢弃，留空保存表示沿用已经派生的 key。
let syncFormSeeded = false;

function renderSyncConfigStatus(s) {
  const box = $("syncConfigStatus");
  box.textContent = "";
  if (s.format === "account") {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}（本地、gitignore，不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `已连接：${s.username} · ${s.url}（设备 ${s.device_id}）` }),
    ]));
  } else if (s.format === "credentials") {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}` }, [
      el("span", { class: "sdot amber" }),
      el("span", { class: "t", text: `检测到旧版用户名密码配置（设备 ${s.device_id}）——登录 Solum 账号并保存即可迁移。` }),
    ]));
  } else if (s.format === "raw") {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}` }, [
      el("span", { class: "sdot amber" }),
      el("span", { class: "t", text: `正在使用 legacy 静态令牌（设备 ${s.device_id}）——登录 Solum 账号并保存即可迁移。` }),
    ]));
  } else {
    box.appendChild(el("div", { class: "banner" }, [
      icon("alert", 18),
      el("div", { class: "grow" }, [
        el("div", { text: "同步未配置——这台设备的数据不会同步到其他设备。" }),
        el("div", { class: "hint", text: `保存后写入：${s.path}` }),
      ]),
    ]));
  }
}

async function refreshSyncConfig() {
  let s;
  try { s = await invoke("sync_config_get"); } catch (_) { return; }
  renderSyncConfigStatus(s);
  const identity = $("syncAccountIdentity");
  identity.textContent = "";
  identity.appendChild(el("span", { class: `sdot${s.account_logged_in ? " ok" : ""}` }));
  identity.appendChild(el("span", { class: "t", text: s.account_logged_in ? `已登录 ${s.username}` : "尚未登录，请先到“云端接入”登录" }));
  $("syncSaveBtn").disabled = !s.account_logged_in;
  if (syncFormSeeded) return;
  syncFormSeeded = true;
  $("syncUrl").value = s.url;
}

$("syncSaveBtn").addEventListener("click", async () => {
  const cfg = {
    url: $("syncUrl").value.trim(),
    password: "",
  };
  if (!/^https?:\/\//.test(cfg.url)) return toast("同步服务器地址需要以 http(s):// 开头", "danger");
  $("syncSaveBtn").disabled = true;
  try {
    const summary = await invoke("sync_config_save", { cfg });
    toast(summary, "ok");
    syncFormSeeded = false;
    await refreshSyncConfig();
    await refreshSyncStatus();
  } catch (e) { toast(String(e), "danger"); }
  finally { $("syncSaveBtn").disabled = false; }
});

/* ============================== 说明问号 ============================== */
// 静态 HTML 里的 .qmark 只留一个空按钮，图标统一在这里补上。
// 说明是浮窗（不推动布局）：点问号开/关，点浮窗外任意处或 Esc 关闭。
document.querySelectorAll(".qmark").forEach((b) => b.appendChild(icon("help", 12)));
function closeDescPopovers(except) {
  document.querySelectorAll(".vhead .desc:not([hidden])").forEach((d) => {
    if (d === except) return;
    d.hidden = true;
    const b = document.querySelector(`.qmark[aria-controls="${d.id}"]`);
    if (b) b.setAttribute("aria-expanded", "false");
  });
}
document.addEventListener("click", (e) => {
  const btn = e.target.closest(".qmark");
  if (!btn) {
    if (!e.target.closest(".vhead .desc")) closeDescPopovers();
    return;
  }
  const target = document.getElementById(btn.getAttribute("aria-controls"));
  if (!target) return;
  target.hidden = !target.hidden;
  btn.setAttribute("aria-expanded", String(!target.hidden));
  if (!target.hidden) closeDescPopovers(target);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeDescPopovers();
});

/* ============================== 浮层与 Android 返回键 ============================== */
// 宿主 WryActivity 的返回策略是 `webView.canGoBack() ? goBack() : 退出 Activity`。
// history 栈从栈底到栈顶依次是：初始视图 → 视图回退栈（见「导航」一节）→ 浮层条目。
// 浮层（确认框 / 说明与资料弹窗 / 抽屉 / 移动工具 / 可关闭账户门 / 输入框菜单）
// 打开时压一条 history，返回键就先关浮层而不是退出
// 整个应用——2026-07-21 真机走查：抽屉开着按返回直接回到桌面。
// 只维护「当前有没有浮层」这一条 history，不按浮层个数计数：菜单项点开抽屉时是
// 「关菜单 + 开抽屉」两步，若各自压/弹，`history.back()` 的 popstate 是异步的，会在
// 抽屉开好之后才到，把刚打开的抽屉又关掉（2026-07-21 真机实测到这个竞态）。
// 因此状态变更后只登记一次同步，放到微任务里等这一轮 DOM 改动全部落定再比对。
// 视图条目只在无浮层时压入，所以 overlayPushed 成立时浮层条目一定在栈顶；若用户
// 从浮层里的入口直接导航，showView 会把这个栈顶条目原位替换成目标视图。
let overlayPushed = false;
let overlaySyncScheduled = false;
let closingFromPopstate = false;
// 程序化 history.back()（浮层在 UI 上先关、再补弹栈）触发的 popstate 不是用户返回，
// 吞掉它，避免被下面的视图回退分支再导航一次。
let swallowNextPopstate = false;
function anyOverlayOpen() {
  return !$("composerMenu").hidden
    || !$("sessionMenuMobile").hidden
    || $("modalMask").classList.contains("open")
    || $("drawer").classList.contains("open")
    || !$("profileEditorModal").hidden
    || !$("infoModal").hidden
    || !$("mobileTools").hidden
    || (!$("accountGate").hidden && !$("accountGateClose").hidden);
}
function syncOverlayHistory() {
  if (closingFromPopstate || overlaySyncScheduled) return;
  overlaySyncScheduled = true;
  queueMicrotask(() => {
    overlaySyncScheduled = false;
    if (closingFromPopstate) return;
    const open = anyOverlayOpen();
    if (open && !overlayPushed) {
      overlayPushed = true;
      history.pushState({ solumOverlay: 1 }, "");
    } else if (!open && overlayPushed) {
      overlayPushed = false;
      swallowNextPopstate = true;
      history.back();
    }
  });
}
window.addEventListener("popstate", (event) => {
  if (swallowNextPopstate) { swallowNextPopstate = false; return; }
  if (overlayPushed) {
    overlayPushed = false;
    closingFromPopstate = true;
    try {
      if (!$("composerMenu").hidden) toggleComposerMenu(false);
      if (!$("sessionMenuMobile").hidden) toggleSessionMenu(false);
      if ($("modalMask").classList.contains("open")) closeModal();
      if ($("drawer").classList.contains("open")) closeDrawer();
      if (!$("profileEditorModal").hidden) closeProfileEditor();
      if (!$("infoModal").hidden) closeInfoModal();
      if (!$("mobileTools").hidden) closeMobileTools();
      if (!$("accountGate").hidden && !$("accountGateClose").hidden) closeAccountGate();
    } finally {
      closingFromPopstate = false;
    }
    return;
  }
  // 视图回退栈：返回到上一个视图；栈底没有更多条目时交还宿主（= 退出 Activity）。
  const target = event.state && event.state.solumView;
  if (target && target !== currentView && $("view-" + target)) {
    showView(target, { push: false });
  }
});

/* ============================== 通用确认模态 ============================== */
let modalAction = null;
let modalDismiss = null;
let modalOpener = null;
function modalFocusable() {
  return Array.from($("modal").querySelectorAll(
    'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
  )).filter((node) => !node.hidden);
}
function openModal(title, bodyNode, okLabel, action, opts) {
  modalOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  $("modalTitle").textContent = "";
  if (opts && opts.icon) { const i = icon(opts.icon, 19); i.style.color = "var(--danger)"; $("modalTitle").appendChild(i); }
  $("modalTitle").appendChild(el("span", { text: title }));
  $("modalBody").textContent = "";
  $("modalBody").appendChild(bodyNode);
  $("modalOk").textContent = okLabel;
  // 默认危险红（历史上模态只用于删除类确认）；非破坏性动作传 okStyle:
  // "primary" 用琥珀主按钮，别把「保存」渲染成要出事的样子。
  $("modalOk").className = "btn " + ((opts && opts.okStyle === "primary") ? "primary" : "danger-fill");
  modalAction = action;
  modalDismiss = (opts && opts.onDismiss) || null;
  $("modalMask").classList.add("open");
  syncOverlayHistory();
  $("modalCancel").focus();
}
function closeModal() {
  if (!$("modalMask").classList.contains("open")) return;
  const opener = modalOpener;
  const dismiss = modalDismiss;
  $("modalMask").classList.remove("open");
  modalAction = null;
  modalDismiss = null;
  modalOpener = null;
  syncOverlayHistory();
  if (opener && document.contains(opener)) requestAnimationFrame(() => opener.focus());
  if (dismiss) {
    try { dismiss(); } catch (_) {}
  }
}
$("modalCancel").addEventListener("click", closeModal);
$("modalMask").addEventListener("click", (e) => { if (e.target === $("modalMask")) closeModal(); });
document.addEventListener("keydown", (e) => {
  if (!$("modalMask").classList.contains("open")) return;
  if (e.key === "Escape") { closeModal(); return; }
  if (e.key !== "Tab") return;
  const focusable = modalFocusable();
  if (!focusable.length) { e.preventDefault(); return; }
  const current = focusable.indexOf(document.activeElement);
  const next = e.shiftKey
    ? (current <= 0 ? focusable.length - 1 : current - 1)
    : (current === focusable.length - 1 ? 0 : current + 1);
  e.preventDefault();
  focusable[next].focus();
});
$("modalOk").addEventListener("click", async () => {
  const act = modalAction;
  modalDismiss = null;
  closeModal();
  if (act) await act();
});

/* ============================== 对话 ============================== */
// 会话完整记录只放 WebView 的本地存储：不进 SQLite、不参与同步或导出。
// 后端每次只接收当前会话最后四个已完成的闲聊回合，继续遵守最小上下文边界。
// 名字停留在改名前的 pa.*：这是 localStorage 的键，改了等于把用户已有的
// 会话记录全部孤立掉，而它对用户完全不可见。别当作改名遗漏"修"掉。
let CHAT_SESSIONS_KEY = "pa.chat-sessions.v1";
const CHAT_MAX_SESSIONS = 40;
const CHAT_MAX_MESSAGES = 240;
const CHAT_MAX_MESSAGE_CHARS = 12_000;
// localStorage 常见配额只有数 MiB；给聊天留出明确预算，避免理论上的
// 40 × 240 × 12k 把整个 UI 状态一起顶出配额。优先保留最近会话与最近消息。
const CHAT_STORAGE_BUDGET_CHARS = 2_400_000;
let chatSessions = loadChatSessions();

function profiledStorageName(base, profileId) {
  return profileId === "guest" ? base : `${base}.${profileId}`;
}

let localStorageIdentity = "guest-legacy";
function selectLocalProfile(value, suppliedStorage) {
  const candidate = String(value || "guest").toLowerCase();
  const profileId = candidate === "guest" || /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(candidate)
    ? candidate : "guest";
  const fallback = {
    ui_state_key: profiledStorageName("solum.ui-state.v1", profileId),
    chat_sessions_key: profiledStorageName("pa.chat-sessions.v1", profileId),
    document_db_name: profiledStorageName("pa-local-documents-v1", profileId),
  };
  const storage = suppliedStorage && typeof suppliedStorage === "object" ? suppliedStorage : fallback;
  const validStorage = typeof storage.ui_state_key === "string" && storage.ui_state_key.startsWith("solum.ui-state.v1") &&
    typeof storage.chat_sessions_key === "string" && storage.chat_sessions_key.startsWith("pa.chat-sessions.v1") &&
    typeof storage.document_db_name === "string" && storage.document_db_name.startsWith("pa-local-documents-v1");
  const selected = validStorage ? storage : fallback;
  const identity = `${profileId}\0${selected.ui_state_key}\0${selected.chat_sessions_key}\0${selected.document_db_name}`;
  if (identity === localStorageIdentity) return;
  localProfileId = profileId;
  localStorageIdentity = identity;
  UI_STATE_KEY = selected.ui_state_key;
  CHAT_SESSIONS_KEY = selected.chat_sessions_key;
  DOCUMENT_DB_NAME = selected.document_db_name;
  uiState = loadUiState();
  chatSessions = loadChatSessions();
  navLastView = { ...uiState.navLastView };
  viewScrollPositions = new Map(Object.entries(uiState.scroll || {})
    .map(([view, top]) => [view, Number(top) || 0]));
}

function newChatSession() {
  return {
    id: `session-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    title: "新对话",
    updatedAt: new Date().toISOString(),
    messages: [],
    contextTurns: [],
  };
}
function isUnusedChatSession(session) {
  return Boolean(session)
    && (!Array.isArray(session.messages) || session.messages.length === 0)
    && (!Array.isArray(session.contextTurns) || session.contextTurns.length === 0)
    && !String(uiState.chatDrafts?.[session.id] || "").trim();
}
function collapseUnusedChatSessions(sessions, preferredId) {
  const unused = sessions.filter(isUnusedChatSession);
  if (unused.length <= 1) return sessions;
  const preferred = unused.find((session) => session.id === preferredId);
  const keeper = preferred || [...unused].sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))[0];
  return sessions.filter((session) => !isUnusedChatSession(session) || session.id === keeper.id);
}
function loadChatSessions() {
  try {
    const raw = JSON.parse(safeLocalGet(CHAT_SESSIONS_KEY) || "null");
    const sessions = Array.isArray(raw?.sessions) ? raw.sessions.map((item) => ({
      id: typeof item.id === "string" ? item.id : "",
      title: typeof item.title === "string" ? item.title.slice(0, 80) : "新对话",
      updatedAt: typeof item.updatedAt === "string" ? item.updatedAt : new Date().toISOString(),
      messages: Array.isArray(item.messages) ? item.messages.slice(-CHAT_MAX_MESSAGES)
        .filter((m) => m && (m.role === "user" || m.role === "assistant") && typeof m.content === "string")
        .map((m) => ({ role: m.role, content: m.content.slice(0, CHAT_MAX_MESSAGE_CHARS), intent: typeof m.intent === "string" ? m.intent : "" })) : [],
      contextTurns: Array.isArray(item.contextTurns) ? item.contextTurns.slice(-4)
        .filter((turn) => turn && typeof turn.user === "string" && typeof turn.assistant === "string")
        .map((turn) => ({ user: turn.user.slice(0, 4_000), assistant: turn.assistant.slice(0, 4_000) })) : [],
    })).filter((item) => item.id) : [];
    const requestedActiveId = sessions.some((item) => item.id === raw?.activeId) ? raw.activeId : sessions[0]?.id;
    const compacted = collapseUnusedChatSessions(sessions, requestedActiveId);
    const activeId = compacted.some((item) => item.id === requestedActiveId) ? requestedActiveId : compacted[0]?.id;
    return { sessions: compacted.length ? compacted : [newChatSession()], activeId: activeId || "" };
  } catch (_) {
    const session = newChatSession();
    return { sessions: [session], activeId: session.id };
  }
}
let chatSessionSaveTimer = 0;
function serializeChatSessions(budget) {
  const full = JSON.stringify(chatSessions);
  if (full.length <= budget) return full;
  let remaining = budget - 256;
  const snapshot = { activeId: chatSessions.activeId, sessions: [] };
  const sessions = [...chatSessions.sessions]
    .sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))
    .slice(0, CHAT_MAX_SESSIONS);
  for (const session of sessions) {
    const base = {
      id: session.id,
      title: session.title,
      updatedAt: session.updatedAt,
      messages: [],
      contextTurns: session.contextTurns || [],
    };
    remaining -= JSON.stringify({ ...base, contextTurns: [] }).length;
    const turns = [];
    for (const turn of [...base.contextTurns].reverse()) {
      const cost = JSON.stringify(turn).length + 1;
      if (cost > remaining) break;
      turns.unshift(turn);
      remaining -= cost;
    }
    base.contextTurns = turns;
    const messages = [];
    for (const message of [...(session.messages || [])].reverse()) {
      const cost = JSON.stringify(message).length + 1;
      if (cost > remaining) break;
      messages.unshift(message);
      remaining -= cost;
    }
    base.messages = messages;
    snapshot.sessions.push(base);
    if (remaining < 1024) break;
  }
  if (!snapshot.sessions.some((session) => session.id === snapshot.activeId)) {
    const active = activeChatSession();
    snapshot.sessions.unshift({
      id: active.id, title: active.title, updatedAt: active.updatedAt,
      messages: (active.messages || []).slice(-12),
      contextTurns: (active.contextTurns || []).slice(-4),
    });
  }
  return JSON.stringify(snapshot);
}
function flushChatSessions() {
  clearTimeout(chatSessionSaveTimer);
  chatSessionSaveTimer = 0;
  if (safeLocalSet(CHAT_SESSIONS_KEY, serializeChatSessions(CHAT_STORAGE_BUDGET_CHARS))) return true;
  // 同源里若已有其他 UI 数据，退到更小预算再试一次；IndexedDB 文档不受影响。
  return safeLocalSet(CHAT_SESSIONS_KEY, serializeChatSessions(1_400_000));
}
function saveChatSessions(immediate) {
  clearTimeout(chatSessionSaveTimer);
  if (immediate) return flushChatSessions();
  // localStorage 序列化是同步任务；让当前点击/消息渲染先完成，退出时再强制落盘。
  chatSessionSaveTimer = setTimeout(flushChatSessions, 80);
  return true;
}
function activeChatSession() {
  let session = chatSessions.sessions.find((item) => item.id === chatSessions.activeId);
  if (!session) {
    session = chatSessions.sessions[0] || newChatSession();
    if (!chatSessions.sessions.length) chatSessions.sessions.push(session);
    chatSessions.activeId = session.id;
  }
  return session;
}
function saveCurrentChatDraft(immediate) {
  const session = activeChatSession();
  const value = String($("chatInput")?.value || "").slice(0, CHAT_MAX_MESSAGE_CHARS);
  if (value) {
    // 重新插入使最近编辑的草稿排在对象末尾；达到 12 份上限时淘汰真正最旧的。
    delete uiState.chatDrafts[session.id];
    uiState.chatDrafts[session.id] = value;
  }
  else delete uiState.chatDrafts[session.id];
  saveUiState(immediate);
}
function snapshotPortableChat() {
  // Include what is currently in the input, then freeze the same bounded shape
  // that localStorage can actually retain. contextTurns are rebuildable and
  // intentionally excluded by the v3 contract.
  saveCurrentChatDraft(true);
  saveChatSessions(true);
  const stored = JSON.parse(serializeChatSessions(CHAT_STORAGE_BUDGET_CHARS));
  const sessions = (stored.sessions || []).map((session) => ({
    id: String(session.id || ""),
    title: String(session.title || ""),
    updated_at: String(session.updatedAt || ""),
    messages: (session.messages || []).map((message) => ({
      role: message.role,
      content: String(message.content || ""),
      intent: String(message.intent || ""),
    })),
  }));
  const sessionIds = new Set(sessions.map((session) => session.id));
  const drafts = Object.entries(uiState.chatDrafts || {})
    .filter(([sessionId, content]) => sessionIds.has(sessionId) && typeof content === "string" && content.length)
    .map(([sessionId, content]) => ({ session_id: sessionId, content }));
  return { sessions, drafts };
}
function restoreCurrentChatDraft() {
  const input = $("chatInput");
  if (input) input.value = String(uiState.chatDrafts[activeChatSession().id] || "").slice(0, CHAT_MAX_MESSAGE_CHARS);
}
function sessionTime(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "刚刚";
  return new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(d);
}
function sessionPreview(session) {
  const last = session.messages[session.messages.length - 1];
  return last ? last.content.replace(/\s+/g, " ").slice(0, 26) : "还没有消息";
}
function renderChatSessions() {
  const active = activeChatSession();
  const list = $("sessionList");
  list.textContent = "";
  const sorted = [...chatSessions.sessions].sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)));
  for (const session of sorted) {
    const row = el("div", { class: "session-row" });
    row.appendChild(el("button", {
      class: `session-item ${session.id === active.id ? "active" : ""}`, type: "button",
      "aria-current": session.id === active.id ? "page" : null,
      onclick: () => switchChatSession(session.id),
    }, [
      el("span", { class: "session-title", text: session.title || "新对话" }),
      el("span", { class: "session-meta", text: `${sessionTime(session.updatedAt)} · ${sessionPreview(session)}` }),
    ]));
    row.appendChild(el("button", {
      class: "btn icon session-remove", type: "button", title: "删除此对话", "aria-label": `删除对话：${session.title || "新对话"}`,
      onclick: () => confirmDeleteChatSession(session.id),
    }, icon("trash", 15)));
    list.appendChild(row);
  }
  $("sessionPickerMobileLabel").textContent = active.title || "新对话";
  const mobileMenu = $("sessionMenuMobile");
  mobileMenu.textContent = "";
  for (const session of sorted) {
    const selected = session.id === active.id;
    mobileMenu.appendChild(el("button", {
      class: "session-picker-option", type: "button", role: "option",
      "aria-selected": String(selected),
      onclick: () => { toggleSessionMenu(false); switchChatSession(session.id); },
    }, [
      el("span", { class: "session-picker-check", "aria-hidden": "true" }, icon("check", 15)),
      el("span", { class: "session-picker-option-copy" }, [
        el("span", { class: "session-picker-option-title", text: session.title || "新对话" }),
        el("span", { class: "session-picker-option-meta", text: `${sessionTime(session.updatedAt)} · ${sessionPreview(session)}` }),
      ]),
    ]));
  }
}
function renderActiveChatSession() {
  const session = activeChatSession();
  $("chatLog").textContent = "";
  $("chatEmpty").style.display = session.messages.length ? "none" : "block";
  for (const message of session.messages) {
    if (message.role === "user") addMsg("user", [message.content], { restore: true });
    else {
      const nodes = [];
      if (message.intent) nodes.push(el("div", { class: "intent" }, tag(INTENT[message.intent] || message.intent, message.intent === "dangerous_command" ? "danger" : "accent")));
      nodes.push(renderMarkdown(message.content));
      addMsg("agent", nodes, { restore: true });
    }
  }
  // 有消息才贴底看最新；空会话贴底会把欢迎屏（灯 + 问候 + 顶部横幅）顶出视口——
  // 手机窄屏上一进来就只剩半截示例，桌面视口高看不出来（2026-07-21 真机走查发现）。
  requestAnimationFrame(() => {
    const scroller = $("chatScroll");
    scroller.scrollTop = session.messages.length ? scroller.scrollHeight : 0;
  });
}
function recordChatMessage(role, content, intent) {
  const session = activeChatSession();
  const text = String(content || "").slice(0, CHAT_MAX_MESSAGE_CHARS);
  session.messages.push({ role, content: text, intent: intent || "" });
  if (session.messages.length > CHAT_MAX_MESSAGES) session.messages.splice(0, session.messages.length - CHAT_MAX_MESSAGES);
  if (role === "user" && session.title === "新对话") session.title = text.replace(/\s+/g, " ").slice(0, 26) || "新对话";
  session.updatedAt = new Date().toISOString();
  saveChatSessions();
  renderChatSessions();
}
function recordChatTurn(user, assistant, intent) {
  if (intent !== "chat") return;
  const session = activeChatSession();
  session.contextTurns.push({ user: String(user).slice(0, 4_000), assistant: String(assistant).slice(0, 4_000) });
  if (session.contextTurns.length > 4) session.contextTurns.splice(0, session.contextTurns.length - 4);
  saveChatSessions();
}
async function syncChatContext() {
  const turns = activeChatSession().contextTurns || [];
  await invoke("chat_context_set", { turns });
}
async function switchChatSession(id) {
  if (!chatSessions.sessions.some((session) => session.id === id)) return;
  toggleSessionMenu(false);
  saveCurrentChatDraft(true);
  chatSessions.activeId = id;
  saveChatSessions(true);
  renderChatSessions();
  renderActiveChatSession();
  restoreCurrentChatDraft();
  try { await syncChatContext(); } catch (e) { toast(`无法切换会话上下文：${String(e)}`, "danger"); }
}
async function createChatSession() {
  const current = activeChatSession();
  if (isUnusedChatSession(current) && !String($("chatInput")?.value || "").trim()) {
    toggleSessionMenu(false);
    $("chatInput").focus();
    return;
  }
  saveCurrentChatDraft(true);
  toggleSessionMenu(false);
  chatSessions.sessions = collapseUnusedChatSessions(chatSessions.sessions, chatSessions.activeId);
  let session = chatSessions.sessions.find(isUnusedChatSession);
  if (!session) {
    session = newChatSession();
    chatSessions.sessions.unshift(session);
  }
  session.updatedAt = new Date().toISOString();
  chatSessions.sessions.sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)));
  if (chatSessions.sessions.length > CHAT_MAX_SESSIONS) chatSessions.sessions.length = CHAT_MAX_SESSIONS;
  chatSessions.activeId = session.id;
  saveChatSessions(true);
  renderChatSessions();
  renderActiveChatSession();
  restoreCurrentChatDraft();
  try { await syncChatContext(); } catch (e) { toast(`无法初始化新会话：${String(e)}`, "danger"); }
  $("chatInput").focus();
}
function confirmDeleteChatSession(id) {
  const session = chatSessions.sessions.find((item) => item.id === id);
  if (!session) return;
  openModal("删除本地对话？", el("div", null, [
    `将删除「${session.title || "新对话"}」的本地消息记录。`,
    el("div", { class: "fine", text: "不会影响已创建的日程、提醒、组件或记忆；删除后无法恢复。" }),
  ]), "删除对话", async () => {
    chatSessions.sessions = chatSessions.sessions.filter((item) => item.id !== id);
    delete uiState.chatDrafts[id];
    if (!chatSessions.sessions.length) chatSessions.sessions.push(newChatSession());
    if (chatSessions.activeId === id) chatSessions.activeId = chatSessions.sessions[0].id;
    saveChatSessions(true);
    saveUiState(true);
    renderChatSessions();
    renderActiveChatSession();
    restoreCurrentChatDraft();
    try { await syncChatContext(); } catch (_) { /* visual deletion still succeeds locally */ }
  }, { icon: "trash" });
}

// 消息行结构：.msg-row（角色 + 头像）承载布局，.msg 气泡承载内容。
// 返回值是气泡本体——调用方会继续往里追加信封；气泡的 _msgRow 指向整行，
// 需要移除整条消息（如打字指示器）时经它删除。
function addMsg(cls, nodes, options) {
  const role = cls.indexOf("user") === 0 ? "user" : "assistant";
  const bubble = el("div", { class: cls.indexOf("typing") >= 0 ? "msg typing" : "msg" }, nodes);
  const avatar = el("span", { class: "msg-avatar", "aria-hidden": "true" });
  if (role === "user") {
    renderAvatarElement(avatar, loadUserProfile());
  } else {
    avatar.appendChild(solumMark(18));
  }
  const row = el("div", { class: "msg-row " + role }, [avatar, bubble]);
  bubble._msgRow = row;
  $("chatLog").appendChild(row);
  $("chatEmpty").style.display = "none";
  if (!options || !options.restore) requestAnimationFrame(() => { $("chatScroll").scrollTop = $("chatScroll").scrollHeight; });
  return bubble;
}
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

/* ============================== 持久化组件（F19） ============================== */
// This is intentionally independent from renderEnvelope(): F18 is an
// ephemeral snapshot protocol, while widgets re-fetch records and rebuild the
// DOM whenever the local data changes. All schema and record strings flow
// through `text`, never innerHTML.
const widgetState = { definitions: [], selectedId: null, sortBy: {}, descending: {} };

function fieldByName(definition, name) {
  return (definition.schema.fields || []).find((field) => field.name === name);
}
function listViewFor(definition) {
  return (definition.schema.views || []).find((view) => view.type === "list");
}
function viewOf(definition, type) {
  return (definition.schema.views || []).find((view) => view.type === type);
}
// stat 的算子由字段类型定死（设计稿 ②），这里必须和 Rust 侧 StatOp 一致：
// number → 合计，bool → 计为真的条数，其余 → 计已填写的条数。
function statTiles(definition, records) {
  const view = viewOf(definition, "stat");
  if (!view) return [];
  return view.fields.map((name) => {
    const field = fieldByName(definition, name);
    if (!field) return null;
    const values = records.map((r) => r.data && r.data[name]).filter((v) => v != null);
    let value, opLabel;
    if (field.type === "number") { value = values.reduce((a, b) => a + Number(b || 0), 0); opLabel = "合计"; }
    else if (field.type === "bool") { value = values.filter((v) => v === true).length; opLabel = "为是"; }
    else { value = values.length; opLabel = "已填"; }
    return { label: field.label, opLabel, value };
  }).filter(Boolean);
}
function renderWidgetStats(definition, records) {
  const tiles = statTiles(definition, records);
  if (!tiles.length) return null;
  const box = el("div", { class: "block" }, [el("h3", { text: "概览" })]);
  const row = el("div", { class: "statrow" });
  for (const tile of tiles) {
    row.appendChild(el("div", { class: "stattile" }, [
      el("div", { class: "statnum", text: String(Number(tile.value.toFixed(2))) }),
      el("div", { class: "statlabel", text: `${tile.label} · ${tile.opLabel}` }),
    ]));
  }
  box.appendChild(row);
  return box;
}
// table 与 list 共用数据绑定，只是排布成列（设计稿 ②：增量成本低）。
function renderWidgetTable(definition, records) {
  const view = viewOf(definition, "table");
  if (!view) return null;
  const box = el("div", { class: "block" }, [el("h3", { text: "表格" })]);
  const scroll = el("div", { class: "tscroll" });
  const table = el("table", { class: "wtable" });
  const head = el("tr");
  for (const name of view.fields) {
    const field = fieldByName(definition, name);
    head.appendChild(el("th", { text: field ? field.label : name }));
  }
  table.appendChild(el("thead", null, [head]));
  const body = el("tbody");
  const sorted = sortWidgetRecords(records, definition, view.sort_by);
  for (const record of sorted) {
    const tr = el("tr");
    for (const name of view.fields) {
      const field = fieldByName(definition, name);
      tr.appendChild(el("td", { text: formatWidgetValue(record.data && record.data[name], field) }));
    }
    body.appendChild(tr);
  }
  table.appendChild(body);
  scroll.appendChild(table);
  box.appendChild(scroll);
  return box;
}
function formViewFor(definition) {
  return (definition.schema.views || []).find((view) => view.type === "form");
}
function widgetFieldTypeLabel(field) {
  const labels = { text: "文本", number: "数字", date: "日期", datetime: "日期时间", time: "时刻", bool: "开关", enum: "单选" };
  return labels[field.type] || field.type;
}
function formatWidgetValue(value, field) {
  if (value == null || value === "") return "—";
  if (field && field.type === "bool") return value ? "是" : "否";
  return String(value);
}
function widgetInput(field, value, inputId) {
  if (field.type === "enum") {
    const input = el("select", { id: inputId }, (field.options || []).map((option) => el("option", { value: option, text: option })));
    if (value != null) input.value = String(value);
    return input;
  }
  if (field.type === "bool") return el("input", { id: inputId, type: "checkbox", checked: value === true });
  const type = { number: "number", date: "date", datetime: "datetime-local", time: "time" }[field.type] || "text";
  const attrs = { id: inputId, type };
  if (type === "number") attrs.step = "any";
  const input = el("input", attrs);
  if (value != null) input.value = String(value);
  return input;
}
function widgetFormData(inputs) {
  const data = {};
  for (const [field, input] of inputs) {
    if (field.type === "bool") { data[field.name] = input.checked; continue; }
    const value = String(input.value || "").trim();
    // Optional blanks are omitted. Required blanks deliberately reach the
    // strict Rust validator and return an actionable error instead of a guess.
    if (!value) continue;
    data[field.name] = field.type === "number" ? Number(value) : value;
  }
  return data;
}
// forcedSort 供 table 用：表格按 schema 声明的列排序，不跟着 list 的
// 用户选择走——两个视图各有各的序，共用数据不等于共用排序。
function sortWidgetRecords(records, definition, forcedSort) {
  const listView = listViewFor(definition);
  const available = (listView && listView.fields) || [];
  const fallback = (listView && listView.sort_by) || available[0] || "";
  const key = forcedSort
    || (available.includes(widgetState.sortBy[definition.id]) ? widgetState.sortBy[definition.id] : fallback);
  const descending = forcedSort ? false : !!widgetState.descending[definition.id];
  return records.slice().sort((left, right) => {
    const field = fieldByName(definition, key);
    const a = left.data && left.data[key];
    const b = right.data && right.data[key];
    if (a == null && b == null) return 0;
    if (a == null) return 1;
    if (b == null) return -1;
    let order;
    if (field && field.type === "number") order = Number(a) - Number(b);
    else if (field && field.type === "bool") order = Number(a) - Number(b);
    else order = String(a).localeCompare(String(b), "zh-Hans-CN", { numeric: true });
    return descending ? -order : order;
  });
}
function renderWidgetPreview(preview) {
  const definition = preview.definition || {};
  const root = el("div", { class: "widget-preview" });
  root.appendChild(el("h4", null, [icon(definition.icon, 17), el("span", { text: definition.name || "未命名组件" })]));
  const fields = el("ul", { class: "wfields" });
  for (const field of (definition.fields || [])) {
    const extra = field.type === "enum" ? `（${(field.options || []).join(" / ")}）` : "";
    fields.appendChild(el("li", null, [
      el("span", { text: `${field.label} · ${widgetFieldTypeLabel(field)}${field.required ? " · 必填" : ""}${extra} ` }),
      el("span", { class: "mono", text: field.name }),
    ]));
  }
  root.appendChild(fields);
  root.appendChild(el("div", { class: "meta", text: `视图：${(definition.views || []).map((view) => view.type).join("、") || "—"}` }));
  const actions = el("div", { class: "btnrow", style: "margin-top:12px;" });
  const confirm = el("button", { class: "btn primary", type: "button", text: "确认创建" });
  const discard = el("button", { class: "btn", type: "button", text: "放弃" });
  confirm.addEventListener("click", async () => {
    confirm.disabled = true; discard.disabled = true;
    try {
      const definition = await invoke("widget_confirm_preview", { previewId: preview.preview_id, now: nowParam() });
      widgetState.selectedId = definition.id;
      root.textContent = "";
      root.appendChild(el("div", { class: "llm-test ok", text: `已创建组件「${definition.name}」。` }));
      // 组件列表在抽屉里，抽屉关着时刷新等于渲染进看不见的地方——创建完必须给出去处，
      // 否则就是死胡同（工作台并入对话后的直接副作用，2026-07-20 自查发现）。
      root.appendChild(el("div", { class: "btnrow", style: "margin-top:10px;" }, [
        el("button", { class: "btn sm primary", type: "button", text: "打开组件", onclick: () => openDrawer("widgets") }),
      ]));
      toast("组件已创建", "ok");
      openDrawer("widgets");
    } catch (error) {
      confirm.disabled = false; discard.disabled = false;
      root.appendChild(el("div", { class: "llm-test danger", text: safeUserError(error, "组件操作暂时不可用，请稍后重试。") }));
    }
  });
  discard.addEventListener("click", async () => {
    try { await invoke("widget_discard_preview", { previewId: preview.preview_id }); } catch (_) { /* already consumed */ }
    root.textContent = "";
    root.appendChild(el("span", { class: "meta", text: "已放弃此组件预览，未写入任何定义。" }));
  });
  actions.append(confirm, discard);
  root.appendChild(actions);
  return root;
}
// Add-only schema evolution (设计稿 ⑧). No "required" control on purpose:
// existing records could never satisfy it, and the core rejects it anyway.
function renderWidgetAddField(definition, afterSave) {
  const root = el("div", { class: "wform" });
  root.appendChild(el("div", { class: "meta", text: "只能新增可空字段。已有字段不能删除或改类型——那会丢数据；要改就新建一个组件再导入。" }));
  const nameIn = el("input", { id: "wAddName", type: "text", placeholder: "字段键（小写英文，如 notes）" });
  const labelIn = el("input", { id: "wAddLabel", type: "text", placeholder: "显示名（如 备注）" });
  const typeIn = el("select", { id: "wAddType" }, [
    ["text", "文本"], ["number", "数字"], ["date", "日期"], ["datetime", "日期时间"],
    ["time", "时刻"], ["bool", "开关"], ["enum", "单选"],
  ].map(([value, text]) => el("option", { value, text })));
  const optionsIn = el("input", { id: "wAddOptions", type: "text", placeholder: "选项，用逗号分隔" });
  const optionsRow = el("div", { class: "frow" }, [el("label", { text: "选项" }), optionsIn]);
  optionsRow.hidden = true;
  typeIn.addEventListener("change", () => { optionsRow.hidden = typeIn.value !== "enum"; });

  root.append(
    el("div", { class: "frow" }, [el("label", { text: "字段键" }), nameIn]),
    el("div", { class: "frow" }, [el("label", { text: "显示名" }), labelIn]),
    el("div", { class: "frow" }, [el("label", { text: "类型" }), typeIn]),
    optionsRow,
  );
  const save = el("button", { class: "btn primary", type: "button", text: "加入组件" });
  const cancel = el("button", { class: "btn", type: "button", text: "取消", onclick: afterSave });
  save.addEventListener("click", async () => {
    save.disabled = true;
    try {
      await invoke("widget_add_field", {
        widgetId: definition.id,
        field: {
          name: nameIn.value.trim(),
          label: labelIn.value.trim(),
          type: typeIn.value,
          required: false,
          options: typeIn.value === "enum"
            ? optionsIn.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean)
            : [],
        },
        now: nowParam(),
      });
      toast("字段已加入", "ok");
      await afterSave();
    } catch (error) {
      save.disabled = false;
      root.appendChild(el("div", { class: "llm-test danger", text: safeUserError(error, "组件操作暂时不可用，请稍后重试。") }));
    }
  });
  root.appendChild(el("div", { class: "btnrow", style: "margin-top:12px;" }, [save, cancel]));
  return root;
}

function renderWidgetCatalog() {
  const box = $("widgetsBox");
  box.textContent = "";
  if (!widgetState.definitions.length) {
    box.appendChild(emptyBox("doc", "还没有组件", "在对话里说「帮我弄一个收支记录组件」，我会先给你预览，确认后才保存。"));
    return;
  }
  const list = el("div", { class: "rows" });
  for (const definition of widgetState.definitions) {
    const fields = (definition.schema.fields || []).map((field) => field.label).join("、");
    list.appendChild(el("button", { class: "widget-def", type: "button", onclick: () => showWidget(definition.id) }, [
      icon(definition.icon, 19),
      el("div", { class: "grow" }, [
        el("div", { class: "wname", text: definition.name }),
        el("div", { class: "wmeta", text: `${(definition.schema.fields || []).length} 个字段 · ${fields}` }),
      ]),
      icon("doc", 16),
    ]));
  }
  box.appendChild(list);
}
function renderWidgetEditor(definition, record, afterSave) {
  const formView = formViewFor(definition);
  const data = record ? record.data || {} : {};
  const form = el("form", { class: "formcol" });
  const inputs = [];
  for (const name of (formView && formView.fields) || []) {
    const field = fieldByName(definition, name);
    if (!field) continue;
    const inputId = `widget-${definition.id}-${record ? record.id : "new"}-${field.name}`;
    const input = widgetInput(field, data[field.name], inputId);
    inputs.push([field, input]);
    form.appendChild(el("div", { class: "field" }, [
      el("label", { for: inputId, text: `${field.label}${field.required ? "（必填）" : ""}` }), input,
    ]));
  }
  const actions = el("div", { class: "btnrow" });
  const save = el("button", { class: "btn primary", type: "submit", text: record ? "保存修改" : "添加记录" });
  actions.appendChild(save);
  if (record) actions.appendChild(el("button", { class: "btn", type: "button", text: "取消", onclick: afterSave }));
  form.appendChild(actions);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    save.disabled = true;
    try {
      const data = widgetFormData(inputs);
      if (record) await invoke("widget_record_update", { widgetId: definition.id, recordId: record.id, data });
      else await invoke("widget_record_create", { widgetId: definition.id, data, now: nowParam() });
      toast(record ? "记录已更新" : "记录已添加", "ok");
      await showWidget(definition.id);
    } catch (error) {
      save.disabled = false;
      form.appendChild(el("div", { class: "llm-test danger", text: safeUserError(error, "组件记录暂时无法保存，请稍后重试。") }));
    }
  });
  return form;
}
// 破坏性操作统一的 Guard 面：预览 → 人工确认 → 一次性令牌 → 审计。
// 三处删除（记忆台账 / 人格清空 / 组件记录）此前是「前端 modal + 直调 IPC」，
// 而 modal 是渲染不是授权边界——命令本身可以被直接调用。收口到这里之后，
// 没有 guard.rs 签发的令牌就执行不了，且预览文案由后端生成、执行前会重新
// 比对（效果摘要绑定），预览与实际后果不一致时自动作废。
async function guardedDestructive(opts) {
  try {
    const args = JSON.stringify(opts.args || {});
    const pending = await invoke("guard_request", { tool: opts.tool, args, now: nowParam() });
    const body = el("div", null, [
      el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(pending.tool) }), " ", tag("高危", "danger")]),
      el("div", { class: "preview", text: "后果预览：" + pending.preview }),
      el("div", { class: "fine", text: opts.note || "确认后签发一次性令牌，绑定这一次操作；令牌用后即焚。" }),
    ]);
    openModal(opts.title, body, opts.confirmLabel || "永久删除", async () => {
      try {
        const result = await invoke("guard_confirm", { pendingId: pending.pending_id, tool: pending.tool, args: pending.args, now: nowParam() });
        toast(result.output, result.ok ? "ok" : "danger");
        if (opts.onDone) { await opts.onDone(result.ok); }
        refreshAudit();
      } catch (error) {
        toast(String(error), "danger");
        if (opts.onError) { await opts.onError(error); }
        refreshAudit();
      }
    }, { icon: "shield" });
  } catch (error) { toast(String(error), "danger"); }
}
async function requestWidgetDelete(definition) {
  try {
    const args = JSON.stringify({ widget_id: definition.id });
    const pending = await invoke("guard_request", { tool: "widget_delete", args, now: nowParam() });
    const body = el("div", null, [
      el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(pending.tool) }), " ", tag("高危", "danger")]),
      el("div", { class: "preview", text: "后果预览：" + pending.preview }),
      el("div", { class: "fine", text: "确认后签发一次性令牌，绑定此组件删除；令牌用后即焚。" }),
    ]);
    openModal("人工确认：删除组件", body, "永久删除", async () => {
      try {
        const result = await invoke("guard_confirm", { pendingId: pending.pending_id, tool: pending.tool, args: pending.args, now: nowParam() });
        toast(result.output, result.ok ? "ok" : "danger");
        if (result.ok) widgetState.selectedId = null;
        await refreshWidgets();
        refreshAudit();
      } catch (error) { toast(String(error), "danger"); refreshAudit(); }
    }, { icon: "shield" });
  } catch (error) { toast(String(error), "danger"); }
}
async function showWidget(widgetId) {
  const definition = widgetState.definitions.find((item) => item.id === widgetId);
  if (!definition) { widgetState.selectedId = null; renderWidgetCatalog(); return; }
  widgetState.selectedId = widgetId;
  const box = $("widgetsBox");
  box.textContent = "";
  try {
    const records = await invoke("widget_records", { widgetId });
    // 抽屉是窄容器：返回 + 组件名占标题行，三个次要操作收进「⋯」浮窗。
    const moreWrap = el("div", { class: "dh-more" });
    let moreMenu = null;
    let moreOutsideHandler = null;
    const closeMore = () => {
      if (moreOutsideHandler) {
        document.removeEventListener("click", moreOutsideHandler);
        moreOutsideHandler = null;
      }
      if (moreMenu) { moreMenu.remove(); moreMenu = null; moreBtn.setAttribute("aria-expanded", "false"); }
    };
    const moreBtn = el("button", { class: "btn icon", type: "button", title: "更多操作", "aria-label": "更多操作", "aria-expanded": "false" }, icon("settings", 17));
    const head = el("div", { class: "drawer-detail-head" }, [
      el("button", { class: "btn icon", type: "button", title: "返回组件列表", "aria-label": "返回组件列表",
        onclick: () => { widgetState.selectedId = null; renderWidgetCatalog(); } }, icon("back", 17)),
      el("div", { class: "dh-title" }, [icon(definition.icon, 17), el("span", { text: definition.name })]),
      moreWrap,
    ]);
    const menuActions = [
      el("button", { type: "button", text: "从日程导入", title: "把已有日程拷贝成记录。是快照不是链接：之后改日程不会影响这些记录。", onclick: async (event) => {
          const button = event.currentTarget; button.disabled = true;
          try {
            const r = await invoke("widget_import_events", { widgetId: definition.id, limit: 20, now: nowParam() });
            // 「导入 0 条」单独出现是无从下手的——把跳过的条数和头几条理由一起说，
            // 用户才分得清「没有日程」和「日程都对不上字段」。
            const parts = [];
            if (r.imported) parts.push(`已导入 ${r.imported} 条日程快照`);
            if (r.skipped) parts.push(`跳过 ${r.skipped} 条`);
            if (!r.imported && !r.skipped) parts.push("没有可导入的日程");
            let text = parts.join("，");
            if (r.reasons && r.reasons.length) {
              const shown = r.reasons.map((x) => `「${x.title}」${x.reason}`).join("；");
              text += `：${shown}${r.skipped > r.reasons.length ? " 等" : ""}`;
            }
            toast(text, r.imported ? "ok" : "");
            await showWidget(definition.id);
          } catch (error) { toast(String(error), "danger"); }
          finally { button.disabled = false; }
        } }),
      el("button", { type: "button", text: "加字段", onclick: () => {
        const editor = $("widgetEditor"); editor.textContent = "";
        editor.appendChild(renderWidgetAddField(definition, () => showWidget(definition.id)));
        editor.scrollIntoView({ block: "nearest" });
      } }),
      el("button", { type: "button", class: "danger", text: "删除组件", onclick: () => requestWidgetDelete(definition) }),
    ];
    moreBtn.addEventListener("click", (event) => {
      event.stopPropagation();
      if (moreMenu) { closeMore(); return; }
      moreMenu = el("div", { class: "dh-menu" }, menuActions);
      // 点菜单项后收起菜单（动作本身各自负责刷新/开模态）
      moreMenu.addEventListener("click", (e) => { if (e.target.closest("button")) setTimeout(closeMore, 0); });
      moreWrap.appendChild(moreMenu);
      moreBtn.setAttribute("aria-expanded", "true");
      moreOutsideHandler = (e) => { if (!e.target.closest(".dh-more")) closeMore(); };
      document.addEventListener("click", moreOutsideHandler);
    });
    moreWrap.appendChild(moreBtn);
    box.appendChild(head);
    const statBlock = renderWidgetStats(definition, records);
    if (statBlock) box.appendChild(statBlock);
    const listView = listViewFor(definition);
    const fields = (listView && listView.fields) || [];
    const defaultSort = (listView && listView.sort_by) || fields[0] || "";
    const selectedSort = fields.includes(widgetState.sortBy[definition.id]) ? widgetState.sortBy[definition.id] : defaultSort;
    widgetState.sortBy[definition.id] = selectedSort;
    const sort = el("select", { "aria-label": "记录排序字段" }, fields.map((name) => {
      const field = fieldByName(definition, name);
      return el("option", { value: name, text: field ? field.label : name });
    }));
    sort.value = selectedSort;
    sort.addEventListener("change", (event) => { widgetState.sortBy[definition.id] = event.currentTarget.value; showWidget(definition.id); });
    const direction = el("select", { "aria-label": "记录排序方向" }, [el("option", { value: "asc", text: "升序" }), el("option", { value: "desc", text: "降序" })]);
    direction.value = widgetState.descending[definition.id] ? "desc" : "asc";
    direction.addEventListener("change", (event) => { widgetState.descending[definition.id] = event.currentTarget.value === "desc"; showWidget(definition.id); });
    const listBlock = el("div", { class: "block" }, [
      el("h3", { text: "记录" }),
      el("div", { class: "vactions", style: "margin-bottom:10px;" }, [el("span", { class: "inline-num", text: "按" }), sort, direction]),
    ]);
    const rows = el("div", { class: "rows" });
    const sorted = sortWidgetRecords(records, definition);
    if (!sorted.length) rows.appendChild(emptyBox("doc", "还没有记录", "用下方表单添加第一条；所有记录只保存在本机。"));
    for (const record of sorted) {
      const primary = fieldByName(definition, fields[0]);
      const details = fields.slice(1).map((name) => {
        const field = fieldByName(definition, name);
        return `${field ? field.label : name}：${formatWidgetValue(record.data && record.data[name], field)}`;
      }).join(" · ");
      const row = el("div", { class: "row widget-record" }, [
        el("div", { class: "grow" }, [
          el("div", { class: "record-title", text: formatWidgetValue(record.data && record.data[fields[0]], primary) }),
          el("div", { class: "record-meta", text: details || `创建于 ${fmtDTText(record.created_at)}` }),
        ]),
        el("div", { class: "record-actions" }, [
          el("button", { class: "btn sm", type: "button", text: "编辑", onclick: () => {
            const editor = $("widgetEditor"); editor.textContent = ""; editor.appendChild(renderWidgetEditor(definition, record, () => showWidget(definition.id)));
          } }),
          // 提升是快照拷贝（设计稿 ⑦-C）：生成一条独立日程，此后与本记录无关。
          el("button", { class: "btn sm", type: "button", text: "提为日程", onclick: async (event) => {
            const button = event.currentTarget; button.disabled = true;
            try {
              const created = await invoke("widget_promote_record", { widgetId: definition.id, recordId: record.id, now: nowParam() });
              toast(`已生成日程「${created.title}」（快照，改日程不影响本记录）`, "ok");
              refreshAll();
            } catch (error) { toast(String(error), "danger"); }
            finally { button.disabled = false; }
          } }),
          el("button", { class: "btn sm danger", type: "button", text: "删除", onclick: async () => {
            await guardedDestructive({
              tool: "widget_record_delete",
              args: { widget_id: definition.id, record_id: record.id },
              title: "人工确认：删除记录",
              onDone: async () => { await showWidget(definition.id); },
            });
          } }),
        ]),
      ]);
      rows.appendChild(row);
    }
    listBlock.appendChild(rows);
    box.appendChild(listBlock);
    const tableBlock = renderWidgetTable(definition, records);
    if (tableBlock) box.appendChild(tableBlock);
    const editorBlock = el("div", { class: "block" }, [el("h3", { text: "添加记录" })]);
    const editor = el("div", { id: "widgetEditor" });
    editor.appendChild(renderWidgetEditor(definition, null, () => showWidget(definition.id)));
    editorBlock.appendChild(editor);
    box.appendChild(editorBlock);
  } catch (error) { box.appendChild(emptyBox("alert", "无法读取组件", safeUserError(error, "组件暂时无法读取，请稍后重试。"))); }
}
async function refreshWidgets() {
  // 当前工作台壳层未挂载组件面板时，后台刷新不能阻断其他页面初始化。
  if (!$("widgetsBox")) return;
  try {
    widgetState.definitions = await invoke("widget_defs");
    if (widgetState.selectedId && widgetState.definitions.some((item) => item.id === widgetState.selectedId)) {
      await showWidget(widgetState.selectedId);
    } else {
      widgetState.selectedId = null;
      renderWidgetCatalog();
    }
  } catch (error) {
    const box = $("widgetsBox");
    box.textContent = "";
    box.appendChild(emptyBox("alert", "无法读取组件", safeUserError(error, "组件暂时无法读取，请稍后重试。")));
  }
}

/* ============================== 本地资料工作台 ============================== */
// 浏览器的 IndexedDB 保存文件二进制，避免把 PDF/文档误塞进同步数据库或云端。
// 同上：IndexedDB 库名，改名会让已上传的 PDF/文档整批读不回来。保持不动。
let DOCUMENT_DB_NAME = "pa-local-documents-v1";
const DOCUMENT_STORE = "documents";
const DOCUMENT_MAX_BYTES = 20 * 1024 * 1024;
const DOCUMENT_MAX_COUNT = 30;
function openDocumentDb(databaseName = DOCUMENT_DB_NAME) {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(DOCUMENT_STORE)) request.result.createObjectStore(DOCUMENT_STORE, { keyPath: "id" });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("无法打开本地资料库"));
  });
}
async function withDocuments(mode, action, databaseName = DOCUMENT_DB_NAME) {
  const db = await openDocumentDb(databaseName);
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(DOCUMENT_STORE, mode);
      const store = tx.objectStore(DOCUMENT_STORE);
      let result;
      action(store, (value) => { result = value; });
      tx.oncomplete = () => resolve(result);
      tx.onerror = () => reject(tx.error || new Error("本地资料操作失败"));
      tx.onabort = () => reject(tx.error || new Error("本地资料操作已取消"));
    });
  } finally { db.close(); }
}
async function listDocuments() {
  return withDocuments("readonly", (store, set) => {
    const request = store.getAll();
    request.onsuccess = () => set(request.result || []);
  });
}
// F17 Android v3 adapter. One readonly transaction freezes the IndexedDB row
// set; hashes are over Blob bytes, never base64 and never metadata alone.
async function snapshotPortableDocumentManifests() {
  if (!globalThis.crypto?.subtle) throw new Error("当前 WebView 不支持资料完整性摘要");
  const docs = await listDocuments();
  if (docs.length > DOCUMENT_MAX_COUNT) throw new Error("本地资料数量超过快照上限");
  const manifests = [];
  for (const doc of docs) {
    if (!(doc?.blob instanceof Blob)) throw new Error("本地资料记录缺少文件内容");
    if (!Number.isSafeInteger(doc.size) || doc.size !== doc.blob.size) {
      throw new Error("本地资料记录尺寸不一致");
    }
    if (doc.blob.size > DOCUMENT_MAX_BYTES) throw new Error("本地资料超过 20 MiB 快照上限");
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", await doc.blob.arrayBuffer()));
    manifests.push({
      id: String(doc.id || ""),
      name: String(doc.name || ""),
      media_type: String(doc.type || doc.blob.type || "application/octet-stream"),
      plaintext_size: doc.blob.size,
      sha256: Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join(""),
    });
  }
  return manifests;
}
async function buildPortableWebViewSnapshot() {
  const profileId = localProfileId;
  const chat = snapshotPortableChat();
  const documents = await snapshotPortableDocumentManifests();
  if (profileId !== localProfileId) throw new Error("账号在快照期间发生变化");
  return { profile_id: profileId, chat, documents };
}
// Reserved for the F17 backup coordinator. Do not bind this to the current
// manual export button until v3 has a platform-wide atomic restore path; the
// existing v2 export/import pair must remain recoverable in the meantime.
async function exportPortableDataV3(now) {
  const snapshot = await buildPortableWebViewSnapshot();
  return invoke("export_portable_data", { snapshot, now: now ?? null });
}

// F17 manual backup path: freeze the same v3 snapshot, then stream each manifest-
// bound Blob to Rust in bounded chunks. Rust performs all compression,
// encryption, encrypted-only staging, generation checks, retries, and commit.
// The backup center is the only visible caller; there is deliberately no timer
// or background schedule.
const BACKUP_UPLOAD_IPC_CHUNK_BYTES = 512 * 1024;
function encodeCanonicalBase64(bytes) {
  let binary = "";
  const stride = 32 * 1024;
  for (let offset = 0; offset < bytes.length; offset += stride) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + stride, bytes.length)));
  }
  return btoa(binary);
}
async function createAndUploadPortableBackup() {
  const profileId = localProfileId;
  const snapshot = await buildPortableWebViewSnapshot();
  if (profileId !== localProfileId) throw new Error("账号在备份准备期间发生变化");
  const begin = await invoke("backup_upload_begin", { snapshot });
  const sessionId = String(begin.session_id || "");
  try {
    const rows = await listDocuments();
    const byId = new Map(rows.map((row) => [String(row.id || ""), row]));
    if (byId.size !== rows.length || rows.length !== (begin.attachments || []).length) {
      throw new Error("本地资料集合在备份期间发生变化");
    }
    for (const expected of begin.attachments || []) {
      if (profileId !== localProfileId) throw new Error("账号在备份期间发生变化");
      const row = byId.get(String(expected.id || ""));
      if (!(row?.blob instanceof Blob) || row.blob.size !== expected.plaintext_size) {
        throw new Error("本地资料与冻结清单不一致");
      }
      for (let offset = 0; offset < row.blob.size; offset += BACKUP_UPLOAD_IPC_CHUNK_BYTES) {
        const end = Math.min(offset + BACKUP_UPLOAD_IPC_CHUNK_BYTES, row.blob.size);
        const chunk = new Uint8Array(await row.blob.slice(offset, end).arrayBuffer());
        await invoke("backup_upload_attachment_chunk", {
          sessionId,
          attachmentId: expected.id,
          offset,
          dataBase64: encodeCanonicalBase64(chunk),
        });
      }
      await invoke("backup_upload_attachment_finish", {
        sessionId,
        attachmentId: expected.id,
      });
    }
    if (profileId !== localProfileId) throw new Error("账号在备份提交前发生变化");
    return await invoke("backup_upload_commit", { sessionId });
  } catch (error) {
    if (sessionId) await invoke("backup_upload_cancel", { sessionId }).catch(() => {});
    throw error;
  }
}

function decodeBase64Bytes(value) {
  const binary = atob(String(value || ""));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}
async function sha256Hex(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
function portableChatStorage(chat) {
  const sessions = (chat.sessions || []).map((session) => ({
    id: session.id,
    title: session.title,
    updatedAt: session.updated_at,
    messages: (session.messages || []).map((message) => ({
      role: message.role,
      content: message.content,
      intent: message.intent || "",
    })),
    contextTurns: [],
  }));
  return JSON.stringify({ sessions, activeId: sessions[0]?.id || "" });
}
function portableUiStateStorage(chat) {
  const chatDrafts = Object.fromEntries((chat.drafts || []).map((draft) => [draft.session_id, draft.content]));
  return JSON.stringify({ version: UI_STATE_VERSION, lastView: "chat", navLastView: {}, scroll: {}, chatDrafts });
}
// The restore coordinator calls this before pointer publication; it writes
// only the generation-specific inactive namespace.
async function hydratePortableGenerationWebView(generationId) {
  if (!globalThis.crypto?.subtle) throw new Error("当前 WebView 不支持恢复完整性摘要");
  const plan = await invoke("restore_webview_begin", { generationId });
  if (plan.generation !== generationId || plan.bootstrap?.generation !== generationId) {
    throw new Error("恢复 generation 在 hydrate 前发生变化");
  }
  const storage = plan.web_storage;
  const chatJson = portableChatStorage(plan.bootstrap.chat || { sessions: [], drafts: [] });
  const uiStateJson = portableUiStateStorage(plan.bootstrap.chat || { sessions: [], drafts: [] });
  if (!safeLocalSet(storage.chat_sessions_key, chatJson) || !safeLocalSet(storage.ui_state_key, uiStateJson)) {
    throw new Error("无法写入 inactive WebView 会话命名空间");
  }

  await withDocuments("readwrite", (store) => { store.clear(); }, storage.document_db_name);
  for (const document of plan.bootstrap.documents || []) {
    const parts = [];
    let offset = 0;
    while (offset < document.plaintext_size) {
      const chunk = await invoke("restore_webview_document_chunk", {
        generationId,
        documentId: document.id,
        offset,
      });
      if (chunk.generation !== generationId || chunk.document_id !== document.id ||
          chunk.offset !== offset || chunk.total_size !== document.plaintext_size) {
        throw new Error("恢复资料分块与 sealed generation 不一致");
      }
      const bytes = decodeBase64Bytes(chunk.data_base64);
      if (!bytes.length || offset + bytes.length > document.plaintext_size) {
        throw new Error("恢复资料分块长度无效");
      }
      parts.push(bytes);
      offset += bytes.length;
      if (chunk.done !== (offset === document.plaintext_size)) throw new Error("恢复资料分块结束标记无效");
    }
    const blob = new Blob(parts, { type: document.media_type || "application/octet-stream" });
    if (blob.size !== document.plaintext_size || await sha256Hex(await blob.arrayBuffer()) !== document.sha256) {
      throw new Error("恢复资料完整性校验失败");
    }
    await withDocuments("readwrite", (store) => {
      store.put({
        id: document.id,
        name: document.name,
        type: document.media_type,
        size: document.plaintext_size,
        addedAt: new Date().toISOString(),
        blob,
      });
    }, storage.document_db_name);
  }

  const restored = await withDocuments("readonly", (store, set) => {
    const request = store.getAll();
    request.onsuccess = () => set(request.result || []);
  }, storage.document_db_name);
  if (restored.length !== (plan.bootstrap.documents || []).length) throw new Error("恢复资料集合不完整");
  const restoredById = new Map(restored.map((document) => [document.id, document]));
  const documents = [];
  for (const expected of plan.bootstrap.documents || []) {
    const actual = restoredById.get(expected.id);
    if (!actual || !(actual.blob instanceof Blob) || actual.name !== expected.name ||
        actual.type !== expected.media_type || actual.size !== expected.plaintext_size ||
        actual.blob.size !== expected.plaintext_size ||
        await sha256Hex(await actual.blob.arrayBuffer()) !== expected.sha256) {
      throw new Error("恢复资料回读结果不一致");
    }
    documents.push({ id: expected.id, plaintext_size: expected.plaintext_size, sha256: expected.sha256 });
  }
  if (safeLocalGet(storage.chat_sessions_key) !== chatJson || safeLocalGet(storage.ui_state_key) !== uiStateJson) {
    throw new Error("恢复会话回读结果不一致");
  }
  const status = await invoke("restore_webview_finish", {
    generationId,
    receipt: { profile_id: plan.profile_id, generation: generationId, documents },
  });
  if (!status.ready || status.generation !== generationId) throw new Error("WebView hydrate 未获得 ready ack");
  return status;
}

async function snapshotLiveWebViewSummary() {
  if (!globalThis.crypto?.subtle) throw new Error("当前 WebView 不支持恢复发布摘要");
  const profileId = localProfileId;
  saveCurrentChatDraft(true);
  if (!flushChatSessions()) throw new Error("无法固化当前会话，恢复发布已取消");
  const chatJson = safeLocalGet(CHAT_SESSIONS_KEY);
  const uiStateJson = safeLocalGet(UI_STATE_KEY);
  if (chatJson === null || uiStateJson === null) throw new Error("无法回读当前 WebView 状态");
  const chat = JSON.parse(chatJson);
  const ui = JSON.parse(uiStateJson);
  const rows = await listDocuments();
  if (rows.length > DOCUMENT_MAX_COUNT) throw new Error("本地资料数量超过恢复发布上限");
  const documents = [];
  for (const row of rows) {
    if (!(row?.blob instanceof Blob) || !Number.isSafeInteger(row.size) || row.size !== row.blob.size ||
        row.blob.size > DOCUMENT_MAX_BYTES) {
      throw new Error("本地资料无法纳入恢复发布摘要");
    }
    documents.push({
      id: String(row.id || ""),
      name: String(row.name || ""),
      type: String(row.type || row.blob.type || ""),
      size: row.blob.size,
      added_at: String(row.addedAt || ""),
      sha256: await sha256Hex(await row.blob.arrayBuffer()),
    });
  }
  documents.sort((left, right) => left.id.localeCompare(right.id));
  if (profileId !== localProfileId) throw new Error("账号在恢复发布摘要期间发生变化");
  const fingerprintInput = JSON.stringify({
    profile_id: profileId,
    chat_json: chatJson,
    ui_state_json: uiStateJson,
    documents,
  });
  const fingerprint = await sha256Hex(new TextEncoder().encode(fingerprintInput));
  const sessions = Array.isArray(chat?.sessions) ? chat.sessions : [];
  const chatMessages = sessions.reduce((count, session) =>
    count + (Array.isArray(session?.messages) ? session.messages.length : 0), 0);
  const drafts = ui?.chatDrafts && typeof ui.chatDrafts === "object"
    ? Object.values(ui.chatDrafts).filter((value) => typeof value === "string" && value.length).length
    : 0;
  return {
    profile_id: profileId,
    chat_sessions: sessions.length,
    chat_messages: chatMessages,
    drafts,
    documents: documents.length,
    fingerprint,
  };
}

// Preview is mandatory and its opaque token can be spent once only against a
// fresh snapshot of both stores.
async function prepareRestorePublication(generationId) {
  const webview = await snapshotLiveWebViewSummary();
  return invoke("restore_publish_preview", { generationId, webview });
}
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
async function getDocument(id) {
  return withDocuments("readonly", (store, set) => {
    const request = store.get(id);
    request.onsuccess = () => set(request.result || null);
  });
}
async function putDocument(file) {
  return withDocuments("readwrite", (store) => {
    store.put({
      id: `doc-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      name: file.name || "未命名资料",
      type: file.type || "",
      size: file.size || 0,
      addedAt: new Date().toISOString(),
      blob: file,
    });
  });
}
async function removeDocument(id) {
  return withDocuments("readwrite", (store) => { store.delete(id); });
}
function readableDocumentSize(size) {
  if (size < 1024 * 1024) return `${Math.max(1, Math.round(size / 1024))} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}
function isTextDocument(doc) {
  return /^text\//.test(doc.type || "") || /\.(txt|md|markdown)$/i.test(doc.name || "");
}
async function addDocumentToChat(id) {
  try {
    const doc = await getDocument(id);
    if (!doc || !isTextDocument(doc)) return;
    const text = (await doc.blob.text()).trim();
    if (!text) return toast("这份文本资料没有可加入的内容", "danger");
    const excerpt = text.slice(0, 12_000);
    closeDrawer();
    showView("chat");
    $("chatInput").value = `请基于我手动附上的资料《${doc.name}》回答：\n\n${excerpt}${text.length > excerpt.length ? "\n\n[资料过长，已截取前 12000 字符]" : ""}`;
    $("chatInput").focus();
    toast("资料已加入输入框；发送前可继续修改", "ok");
  } catch (e) { toast(`无法读取资料：${String(e)}`, "danger"); }
}
async function refreshDocuments() {
  const box = $("documentsBox");
  // 资料面板没有挂载在当前壳层时，保留本地资料实现但不让轮询链路报错。
  if (!box) return;
  try {
    const docs = (await listDocuments()).sort((a, b) => String(b.addedAt).localeCompare(String(a.addedAt)));
    box.textContent = "";
    if (!docs.length) {
      box.appendChild(emptyBox("folder", "还没有本地资料", "选择一份 PDF 或文档后，它只保存在这台设备，可随时删除。"));
      return;
    }
    const list = el("div", { class: "rows" });
    for (const doc of docs) {
      const isText = isTextDocument(doc);
      list.appendChild(el("div", { class: "doc-source" }, [
        icon(doc.type === "application/pdf" || /\.pdf$/i.test(doc.name) ? "doc" : "folder", 17),
        el("div", { class: "name", text: doc.name }),
        el("span", { class: "meta", text: `${readableDocumentSize(doc.size)} · ${sessionTime(doc.addedAt)}` }),
        isText ? el("button", { class: "btn sm", type: "button", text: "加入对话", onclick: () => addDocumentToChat(doc.id) }) : null,
        el("button", { class: "btn icon danger", type: "button", title: "删除本地资料", "aria-label": `删除资料：${doc.name}`, onclick: async () => {
          try { await removeDocument(doc.id); await refreshDocuments(); toast("已从本机资料库删除", "ok"); }
          catch (e) { toast(String(e), "danger"); }
        } }, icon("trash", 16)),
      ]));
    }
    box.appendChild(list);
  } catch (e) {
    box.textContent = "";
    box.appendChild(emptyBox("alert", "无法读取本地资料", safeUserError(e, "本地资料暂时无法读取，请稍后重试。")));
  }
}
$("documentPickBtn")?.addEventListener("click", () => $("documentInput")?.click());
$("documentInput")?.addEventListener("change", async () => {
  const files = Array.from($("documentInput").files || []);
  $("documentInput").value = "";
  if (!files.length) return;
  try {
    const existing = await listDocuments();
    let room = Math.max(0, DOCUMENT_MAX_COUNT - existing.length);
    let saved = 0;
    const rejected = [];
    for (const file of files) {
      if (!room) { rejected.push("资料库最多保留 30 份文件"); break; }
      if (file.size > DOCUMENT_MAX_BYTES) { rejected.push(`《${file.name}》超过 20 MB`); continue; }
      await putDocument(file);
      room -= 1; saved += 1;
    }
    await refreshDocuments();
    if (saved) { toast(`已在本机保存 ${saved} 份资料`, "ok"); openDrawer("documents"); }
    if (rejected.length) toast(rejected.join("；"), "danger", 4200);
  } catch (e) { toast(`保存资料失败：${String(e)}`, "danger"); }
});

const WIDGET_QUICK_STARTS = [
  ["收支记录", "记录收入、支出和分类", "帮我制作一个收支记录组件，包含金额、分类、日期和备注"],
  ["任务清单", "跟进状态和截止日期", "帮我制作一个任务清单组件，包含任务、状态、截止日期和备注"],
  ["习惯打卡", "记录每天的完成情况", "帮我制作一个习惯打卡组件，包含习惯名称、日期、是否完成和备注"],
  ["自定义组件", "用自己的字段描述开始", "帮我制作一个组件："],
];
function renderWidgetQuickStarts() {
  const box = $("widgetQuickStarts");
  if (!box) return;
  box.textContent = "";
  for (const [title, hint, prompt] of WIDGET_QUICK_STARTS) {
    box.appendChild(el("button", { class: "workbench-choice", type: "button", onclick: () => {
      closeDrawer();
      showView("chat");
      $("chatInput").value = prompt;
      $("chatInput").focus();
    } }, [el("strong", { text: title }), el("span", { text: hint })]));
  }
}
renderWidgetQuickStarts();

/* ---------------- 工作台抽屉与输入框「+」菜单 ---------------- */
// 原顶层「工作台」组并入对话：资料/组件以右侧抽屉呈现，入口在输入框「+」浮窗。
const DRAWER_PANES = { documents: "drawerTabDocuments", widgets: "drawerTabWidgets" };
let drawerOpener = null;
function drawerShowPane(pane) {
  for (const [p, tabId] of Object.entries(DRAWER_PANES)) {
    $("pane-" + p).classList.toggle("active", p === pane);
    $(tabId).classList.toggle("active", p === pane);
    $(tabId).setAttribute("aria-selected", String(p === pane));
  }
  if (pane === "documents") refreshDocuments(); else refreshWidgets();
}
function openDrawer(pane) {
  if (!accountLoggedIn) {
    requireAccount(pane === "documents" ? "登录后使用本地资料工作台" : "登录后创建和管理个人组件");
    return false;
  }
  if (!$("drawer").classList.contains("open")) {
    drawerOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    $("drawerMask").classList.add("open");
    $("drawer").classList.add("open");
    syncOverlayHistory();
    $("drawer").focus({ preventScroll: true });
  }
  drawerShowPane(pane);
  return true;
}
function closeDrawer() {
  if (!$("drawer").classList.contains("open")) return;
  $("drawerMask").classList.remove("open");
  $("drawer").classList.remove("open");
  const opener = drawerOpener;
  drawerOpener = null;
  syncOverlayHistory();
  if (opener && document.contains(opener)) requestAnimationFrame(() => opener.focus());
}
$("drawerClose").appendChild(icon("x", 17));
$("drawerClose").addEventListener("click", closeDrawer);
$("drawerMask").addEventListener("click", closeDrawer);
$("drawerTabDocuments").addEventListener("click", () => drawerShowPane("documents"));
$("drawerTabWidgets").addEventListener("click", () => drawerShowPane("widgets"));

function toggleComposerMenu(force) {
  const menu = $("composerMenu");
  const show = force != null ? force : menu.hidden;
  if (show === !menu.hidden) return; // 状态没变就别动 history
  if (show) toggleSessionMenu(false);
  menu.hidden = !show;
  $("composerPlus").setAttribute("aria-expanded", String(show));
  syncOverlayHistory();
}
$("composerPlus").appendChild(icon("plus", 18));
{
  const menuItem = (ic, label, hint, onclick) => el("button", { type: "button", onclick: () => { toggleComposerMenu(false); onclick(); } }, [
    icon(ic, 18),
    el("span", null, [el("span", { text: label }), el("span", { class: "mhint", text: hint })]),
  ]);
  $("composerMenu").append(
    menuItem("plus", "上传资料", "PDF / 文档，仅保存在本机", () => accountLoggedIn ? $("documentInput").click() : requireAccount("登录后使用本地资料工作台")),
    menuItem("folder", "资料库", "已上传的本地资料", () => openDrawer("documents")),
    menuItem("doc", "组件库", "自定义组件与制作起点", () => openDrawer("widgets")),
  );
}
$("composerPlus").addEventListener("click", (e) => { e.stopPropagation(); toggleComposerMenu(); });
document.addEventListener("click", (e) => {
  if (!$("composerMenu").hidden && !e.target.closest("#composerMenu")) toggleComposerMenu(false);
});
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if ($("modalMask").classList.contains("open")) return; // 模态优先，由模态自己处理
  if (!$("composerMenu").hidden) { toggleComposerMenu(false); return; }
  closeDrawer();
});

/* ---------------- 主动问询条 ---------------- */
$("checkinIc").appendChild(icon("clock", 18));
$("checkinClose").appendChild(icon("x", 15));
$("checkinClose").addEventListener("click", () => $("checkinBar").classList.remove("show"));
// r 为 string 或 { question, ui }（F18）：条内渲染快捷答选项，自由输入照常。
function showCheckin(r) {
  const q = typeof r === "string" ? r : r.question;
  $("checkinText").textContent = q;
  const old = $("checkinBar").querySelector(".gc");
  if (old) old.remove();
  if (r && r.ui) $("checkinBar").querySelector(".grow").appendChild(renderEnvelope(r.ui, { skipLeadingText: true }));
  $("checkinBar").classList.add("show");
  showView("chat"); // 跳到对话页方便立刻回答
}

/* ============================== 日程 ============================== */
function dayLabel(dateStr) {
  const d = new Date(dateStr + "T00:00");
  if (isNaN(d)) return dateStr;
  const n = effectiveNow();
  const today = new Date(n.getFullYear(), n.getMonth(), n.getDate());
  const diff = Math.round((d - today) / 86400000);
  const wd = "日一二三四五六"[d.getDay()];
  const md = `${d.getMonth() + 1}月${d.getDate()}日`;
  const y = d.getFullYear() !== n.getFullYear() ? `${d.getFullYear()}年` : "";
  const pre = diff === 0 ? "今天 · " : diff === 1 ? "明天 · " : diff === 2 ? "后天 · " : "";
  return `${pre}${y}${md} 周${wd}`;
}
async function refreshAgenda() {
  const box = $("agendaBox");
  try {
    const [evs, soulous] = await Promise.all([
      invoke("agenda", { now: nowParam() }),
      accountLoggedIn
        ? invoke("soulous_config_get", { now: nowParam() })
        : Promise.resolve({ push_schedule_events: false }),
    ]);
    const outboundEnabled = Boolean(soulous.push_schedule_events);
    box.textContent = "";
    // 陈述性状态用低调状态行；琥珀横幅只留给需要用户动作的场景（语义规约见 DESIGN.md）。
    box.appendChild(el("div", { class: "statusline", style: "margin-bottom:16px;", title: outboundEnabled
        ? "每次发送都会展示最小数据预览并要求确认。"
        : "可在 设置 → 云端 单独开启；关闭时不会有任何 息壤 数据出站。" }, [
      el("span", { class: "sdot" + (outboundEnabled ? " ok" : "") }),
      el("span", { class: "t", text: outboundEnabled ? "Soulous 日程事件推送已授权（逐次确认）" : "Soulous 日程事件推送已关闭" }),
    ]));
    if (!evs.length) {
      box.appendChild(emptyBox("calendar", "没有即将到来的日程", "在对话里随口说一件事，我会自动抽取并排进来。"));
      return;
    }
    let curDay = null;
    for (const ev of evs) {
      const [d, t] = String(ev.start || "").split("T");
      if (d !== curDay) {
        curDay = d;
        box.appendChild(el("div", { class: "day-h", text: dayLabel(d) }));
      }
      box.appendChild(el("div", { class: "ev-row" }, [
        el("span", { class: "time", text: (t || "").slice(0, 5) }),
        el("div", { class: "grow" }, [
          el("span", { class: "title", text: ev.title }),
          tag(KIND[ev.kind] || ev.kind, "accent"),
          el("div", { class: "meta", text: [ev.location, (ev.people || []).join("、")].filter(Boolean).join(" · ") || "" }),
        ]),
        outboundEnabled
          ? el("button", {
              class: "btn sm", text: "推送", title: "发送最小化日程事实到你自己的 Soulous（需确认）",
              onclick: () => askSoulousEventPush(ev.id),
            })
          : "",
        el("button", {
          class: "btn icon danger", title: "遗忘此事件", "aria-label": "遗忘此事件",
          onclick: () => askForget("event", ev.id),
        }, icon("trash", 16)),
      ]));
    }
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}

/* ============================== 提醒 ============================== */
function setDuePills(n) {
  for (const id of ["dueNav", "dueTab", "duePillM"]) {
    const p = $(id);
    if (!p) continue;
    p.textContent = n || "";
    p.classList.toggle("show", n > 0);
  }
}
async function refreshNotifs() {
  const dueBox = $("dueBox"), notifBox = $("notifBox");
  try {
    const [due, all, events] = await Promise.all([
      invoke("due", { now: nowParam() }),
      invoke("all_notifications"),
      invoke("all_events"),
    ]);
    const evTitle = {}, evRoutine = {};
    events.forEach((ev) => {
      evTitle[ev.id] = ev.title;
      // routine 物化出的事件以 "routine#<id>" 为 raw_input（见 core
      // materialize_routines）——识别出来给「已完成」一键确认（D4）。
      const m = /^routine#(\d+)$/.exec(ev.raw_input || "");
      if (m) evRoutine[ev.id] = parseInt(m[1], 10);
    });
    setDuePills(due.length);

    dueBox.textContent = "";
    if (due.length) {
      const list = el("div", { class: "due-list" });
      for (const n of due) {
        list.appendChild(el("div", { class: "due-item" }, [
          icon("bell", 16),
          el("span", { class: "title", style: "font-weight:600;", text: evTitle[n.event_id] || "日程" }),
          el("span", { style: "color:var(--muted);font-size:.8125rem;" }, [
            `提前${n.lead_label}，应于 `, fmtDT(n.fire_at), " 提醒",
          ]),
        ]));
      }
      dueBox.appendChild(list);
    } else {
      dueBox.appendChild(el("div", { class: "empty", style: "padding:10px 2px;" },
        el("span", { class: "eh", text: "当前时钟下没有到点的提醒。" })));
    }

    notifBox.textContent = "";
    if (all.length) {
      // 扫读型数据用行不用表（DESIGN.md）：表格版在窄屏横滚会把操作列藏出屏幕，
      // 行布局允许折行，「取消/稍后再响」任何宽度下都可达（2026-07-20 重构）。
      const list = el("div", { class: "rows" });
      for (const n of all) {
        const s = STATUS[n.status] || [n.status, ""];
        const CH = { push: "系统推送", banner: "应用横幅" };
        const acts = [
          n.status === "pending"
            ? el("button", { class: "btn sm", text: "+10分钟", title: "推迟：10 分钟后再响", onclick: () => doSnooze(n.id, 10) })
            : "",
          n.status === "pending"
            ? el("button", { class: "btn sm", text: "取消", onclick: () => doDismiss(n.id) })
            : "",
          n.status === "fired"
            ? el("button", { class: "btn sm", text: "稍后再响", title: "10 分钟后重新拉响这条提醒", onclick: () => doSnooze(n.id, 10) })
            : "",
          evRoutine[n.event_id] != null
            ? el("button", { class: "btn sm primary", text: "已完成", title: "记一笔完成（计入行为日志，连续 7 天未确认会建议暂停）",
                onclick: () => doRoutineDone(evRoutine[n.event_id]) })
            : "",
        ].filter(Boolean);
        list.appendChild(el("div", { class: "notif-row" }, [
          el("span", { class: "when" }, fmtDT(n.fire_at)),
          el("div", { class: "grow" }, [
          el("span", { class: "title", text: evTitle[n.event_id] || "日程" }),
            st(s[0], s[1]),
            el("div", { class: "meta", text: [`提前${n.lead_label || "—"}`, (n.channels || []).map((c) => CH[c] || c).join(" + "), `#${n.id}`].filter(Boolean).join(" · ") }),
          ]),
          acts.length ? el("span", { class: "acts" }, acts) : "",
        ]));
      }
      notifBox.appendChild(list);
    } else {
      notifBox.appendChild(emptyBox("bell", "还没有任何提醒", "录入带时间的事件后，按重要度规则自动排程。"));
    }
  } catch (e) { notifBox.textContent = ""; notifBox.appendChild(emptyBox("alert", safeUserError(e, "提醒暂时无法读取，请稍后重试。"))); }
}
$("fireBtn").addEventListener("click", async () => {
  try {
    const fired = await invoke("fire_due", { now: nowParam() });
    toast(fired.length ? `已触发 ${fired.length} 条提醒` : "当前时钟下没有需要触发的提醒", fired.length ? "amber" : undefined);
    refreshAll();
  } catch (e) { toast(String(e), "danger"); }
});
async function doDismiss(id) {
  try { await invoke("dismiss", { id }); toast(`已取消提醒 #${id}（事件保留）`, "ok"); refreshAll(); }
  catch (e) { toast(String(e), "danger"); }
}
async function doRoutineDone(id) {
  try {
    const msg = await invoke("routine_done", { id, now: nowParam() });
    toast(msg, "ok");
    refreshNotifs(); refreshJournal();
  } catch (e) { toast(String(e), "danger"); }
}
async function doSnooze(id, minutes) {
  try {
    const until = await invoke("snooze", { id, minutes, now: nowParam() });
    toast(`提醒 #${id} 将于 ${fmtDTText(until)} 再响`, "amber");
    refreshAll();
  } catch (e) { toast(String(e), "danger"); }
}

/* ============================== 建议 ============================== */
async function refreshSuggest() {
  const box = $("suggestBox");
  try {
    const rows = await invoke("suggestions");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("bulb", "还没有建议", "录几条日程，或点右上「立即生成」。"));
      return;
    }
    const list = el("div", { class: "rows" });
    for (const s of rows) {
      const stx = SSTATUS[s.status] || [s.status, ""];
      list.appendChild(el("div", { class: "row" }, [
        tag(SKIND[s.kind] || s.kind, "accent"),
        el("div", { class: "grow", text: s.text }),
        s.status === "pending"
          ? el("span", { style: "display:flex;gap:8px;" }, [
              el("button", { class: "btn sm", text: "采纳", onclick: () => setSuggest(s.id, "accepted") }),
              el("button", { class: "btn sm", text: "忽略", onclick: () => setSuggest(s.id, "dismissed") }),
            ])
          : st(stx[0], stx[1]),
      ]));
    }
    box.appendChild(list);
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
$("suggestBtn").addEventListener("click", async () => {
  try {
    const fresh = await invoke("suggest_generate", {
      days: parseInt($("suggestDays").value || "3", 10), now: nowParam(),
    });
    toast(fresh.length ? `新增 ${fresh.length} 条建议` : "没有新的建议", fresh.length ? "bulb" : undefined);
    refreshSuggest(); refreshToday();
  } catch (e) { toast(String(e), "danger"); }
});
async function setSuggest(id, status) {
  try {
    const followUp = await invoke("suggest_set", { id, status, now: nowParam() });
    if (followUp) toast(followUp, "ok");
    refreshSuggest(); refreshToday();
  }
  catch (e) { toast(String(e), "danger"); }
}

/* ============================== 行为日志 ============================== */
async function refreshJournal() {
  const box = $("journalBox");
  try {
    const rows = await invoke("behavior_log");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("journal", "行为日志为空", "在对话里回一句「我在……」试试。"));
      return;
    }
    box.appendChild(dataTable(["#", "时间", "类型", "内容", "来源"],
      rows.map((b) => {
        const k = BKIND[b.kind] || [b.kind, ""];
        return [
          td(String(b.id), "num"),
          [fmtDT(b.ts)],
          [tag(k[0], k[1])],
          [b.content],
          td(b.source || "—", "mut"),
        ];
      })));
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
$("checkinBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("checkin_now", { now: nowParam() });
    if (r) { showCheckin(r); toast("问询已发出（也会计入行为日志）", "clock"); }
    else toast("当前不需要问询：档位被动 / 非清醒时段 / 距上次太近");
    refreshJournal();
  } catch (e) { toast(String(e), "danger"); }
});

/* ============================== 穿戴 ============================== */
async function refreshWearable() {
  const banner = $("healthBanner");
  try {
    const stt = await invoke("health_status");
    banner.textContent = "";
    if (!stt.available) {
      // 桌面没有 Health Connect 是正常稳态，不值得一条琥珀横幅。
      banner.appendChild(el("div", { class: "statusline", style: "margin-bottom:16px;" }, [
        el("span", { class: "sdot" }),
        el("span", { text: "此设备没有 Health Connect（桌面壳本就没有），数据靠多设备同步从手机带过来。" }),
      ]));
    } else if (!stt.granted) {
      banner.appendChild(el("div", { class: "banner", style: "margin-bottom:18px;" }, [
        icon("watch", 18),
        el("div", { class: "grow", text: "尚未授权读取心率 / 步数 / 睡眠。" }),
        el("button", {
          class: "btn primary sm", text: "去授权",
          onclick: async () => {
            try {
              const ok = await invoke("health_request");
              toast(ok ? "已授权" : "未完成授权", ok ? "ok" : undefined);
              refreshWearable();
            } catch (e) { toast(String(e), "danger"); }
          },
        }),
      ]));
    }
  } catch (_) { banner.textContent = ""; /* 老桌面构建没有该 command 也不影响其余 UI */ }

  const box = $("wearableBox");
  try {
    const rows = await invoke("health_trends");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("heart", "还没有健康趋势", "手机端授权 Health Connect 后，有数据的指标会在本机生成近 7 天摘要。"));
      return;
    }
    const stats = el("div", { class: "hstats" });
    for (const h of rows) {
      const comparison = h.changePercent === null
        ? `最近 7 个完整自然日有 ${h.currentDays} 天数据，数据不足，暂不比较`
        : `较前 7 天 ${h.changePercent > 0 ? "+" : ""}${h.changePercent}% · ${h.currentDays}/${h.previousDays} 天`;
      stats.appendChild(el("div", { class: "hstat" }, [
        el("div", { class: "v" }, [String(h.current), el("small", { text: h.unit })]),
        el("div", { class: "l", text: `${h.label} · ${comparison}` }),
      ]));
    }
    box.appendChild(stats);
    box.appendChild(el("p", { class: "hint", text: "所有计算均在本机完成，仅表示记录变化，不作健康诊断。" }));
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}

/* ============================== 复盘 ============================== */
$("reviewBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("review", {
      days: parseInt($("reviewDays").value || "7", 10),
      now: nowParam(),
      styled: $("reviewStyled").checked,
    });
    $("reviewOut").style.display = "block";
    $("reviewOut").textContent = r.text;
    const m = $("reviewMode");
    m.style.display = "inline-flex";
    m.textContent = "";
    if (r.styled) m.appendChild(tag("已按人格改写", "ok"));
    else if ($("reviewStyled").checked) m.appendChild(tag("离线原文（云端不可用或校验未过）"));
    else m.appendChild(tag("离线原文"));
  } catch (e) { toast(String(e), "danger"); }
});

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

/* ============================== 人格 ============================== */
function personaSummary(p) {
  const parts = [];
  if (p.nickname) parts.push(`称呼「${p.nickname}」`);
  if (p.tone) parts.push(`语气：${p.tone}`);
  if ((p.catchphrases || []).length) parts.push(`口头禅：${p.catchphrases.join("、")}`);
  if (p.style_notes) parts.push(`备注：${p.style_notes}`);
  return parts.join(" · ") || "—";
}
async function refreshPersona() {
  const act = $("personaActive"), vers = $("personaVersions");
  try {
    const d = await invoke("persona_get");
    act.textContent = "";
    if (d.active) {
      act.appendChild(el("div", { class: "row", style: "border:none;padding:0;" }, [
        tag("v" + d.active.version, "ok"),
        el("div", { class: "grow", text: personaSummary(d.active) }),
        el("span", { class: "meta" }, fmtDT(d.active.created_at)),
      ]));
      $("pNickname").value = d.active.nickname || "";
      $("pTone").value = d.active.tone || "";
      $("pCatch").value = (d.active.catchphrases || []).join(", ");
      $("pNotes").value = d.active.style_notes || "";
    } else {
      act.appendChild(el("div", { class: "empty", style: "padding:4px 2px;" },
        el("span", { class: "eh", text: "还没设置人格。展开下方「编辑并保存为新版本」填写风格设定即可。" })));
      $("personaEditFold").open = true; // 没设过人格时别把唯一的设置入口折起来
    }
    vers.textContent = "";
    if (d.versions.length) {
      vers.appendChild(dataTable(["版本", "时间", "设定", "说明", ""],
        d.versions.map((v) => {
          const isActive = d.active && v.version === d.active.version;
          return [
            [el("span", { class: "mono", text: "v" + v.version }), isActive ? el("span", null, [" ", tag("当前", "ok")]) : ""],
            [fmtDT(v.created_at)],
            [personaSummary(v)],
            td(v.note || "—", "mut"),
            [isActive ? "" : el("button", { class: "btn sm", text: "回滚到此版", onclick: () => rollbackPersona(v.version) })],
          ];
        })));
    } else {
      vers.appendChild(el("div", { class: "empty", style: "padding:4px 2px;" },
        el("span", { class: "eh", text: "还没有版本。" })));
    }
  } catch (e) { act.textContent = ""; act.appendChild(emptyBox("alert", safeUserError(e, "受保护操作暂时无法读取，请稍后重试。"))); }
}
$("personaSaveBtn").addEventListener("click", async () => {
  try {
    const p = await invoke("persona_set", {
      nickname: $("pNickname").value.trim() || null,
      tone: $("pTone").value,
      catchphrases: $("pCatch").value.split(/[,，、]/).map((s) => s.trim()).filter(Boolean),
      styleNotes: $("pNotes").value.trim() || null,
      note: $("pNote").value.trim() || null,
      now: nowParam(),
    });
    $("pNote").value = "";
    toast(`已保存人格 v${p.version}`, "ok");
    refreshPersona();
  } catch (e) { toast(String(e), "danger"); }
});
async function rollbackPersona(version) {
  try {
    const p = await invoke("persona_rollback", { version });
    toast(`已回滚到人格 v${p.version}（历史保留）`, "ok");
    refreshPersona();
  } catch (e) { toast(String(e), "danger"); }
}
$("personaClearBtn").addEventListener("click", () => {
  guardedDestructive({
    tool: "persona_clear",
    title: "人工确认：删除全部人格版本",
    note: "确认后签发一次性令牌。删除后闲聊与复盘退回默认助理口吻，版本历史不可恢复。",
    onDone: async () => { refreshPersona(); },
  });
});

/* ---------------- 聊天记录导入（F9 §3.4，纯本地） ---------------- */
$("pImportPick").addEventListener("click", () => $("pImportFile").click());
$("pImportFile").addEventListener("change", () => {
  const f = $("pImportFile").files[0];
  $("pImportName").textContent = f ? f.name : "未选择";
});
$("personaImportPreviewBtn").addEventListener("click", async () => {
  const file = $("pImportFile").files[0];
  const me = $("pImportMe").value.trim();
  if (!file) { toast("先选择聊天记录文件"); return; }
  if (!me) { toast("填一下你在记录里的昵称"); return; }
  const report = $("personaImportReport");
  try {
    const raw = await file.text();
    const resp = await invoke("persona_import_preview", { raw, me });
    const r = resp.report;
    const pair = (a) => a.map(([w, n]) => `${w}×${n}`).join("、");
    report.textContent = "";
    const lines = el("div", { style: "line-height:1.9;font-size:.875rem;" });
    lines.appendChild(el("div", null, [
      "共解析 ", el("strong", { text: String(r.total_messages) }), " 条消息，其中本人 ",
      el("strong", { text: String(r.my_messages) }), " 条",
      el("span", { style: "color:var(--muted);", text: `（发言人：${r.speakers.join("、")}）` }),
    ]));
    lines.appendChild(el("div", { text: `平均每条 ${r.avg_chars.toFixed(1)} 字` }));
    if (r.top_particles.length) lines.appendChild(el("div", { text: `常用语气词：${pair(r.top_particles)}` }));
    if (r.top_phrases.length) lines.appendChild(el("div", { text: `高频短语：${pair(r.top_phrases)}` }));
    lines.appendChild(el("div", { text: `推断语气：${r.suggested.tone}` }));
    for (const n of r.notes) lines.appendChild(el("div", null, [tag("注意", "accent"), " ", n]));
    report.appendChild(lines);
    // F18：初稿以就地编辑表单渲染（信封来自 solum-core），改完提交即保存为导入版本。
    if (resp.ui) report.appendChild(renderEnvelope(resp.ui));
    // 初稿同时填入上方编辑表单（昵称保留用户已有设定——聊天记录推断不了这个）。
    // 表单在折叠块里，必须展开，否则「已填入」是用户看不见的（2026-07-20 自查）。
    $("personaEditFold").open = true;
    $("pTone").value = r.suggested.tone || "";
    $("pCatch").value = (r.suggested.catchphrases || []).join(", ");
    $("pNotes").value = r.suggested.style_notes || "";
    $("pNote").value = `从聊天记录导入（${file.name}）`;
    $("personaImportSaveBtn").disabled = false;
    toast("提取完成（纯本地，未保存）", "ok");
  } catch (e) {
    report.textContent = "";
    report.appendChild(emptyBox("alert", safeUserError(e, "今日简报暂时无法生成，请稍后重试。")));
    $("personaImportSaveBtn").disabled = true;
  }
});
$("personaImportSaveBtn").addEventListener("click", async () => {
  try {
    const p = await invoke("persona_import_save", {
      nickname: $("pNickname").value.trim() || null,
      tone: $("pTone").value,
      catchphrases: $("pCatch").value.split(/[,，、]/).map((s) => s.trim()).filter(Boolean),
      styleNotes: $("pNotes").value.trim() || null,
      note: $("pNote").value.trim() || null,
      now: nowParam(),
    });
    $("pNote").value = "";
    $("personaImportSaveBtn").disabled = true;
    toast(`已保存导入人格 v${p.version}（可随时回滚）`, "ok");
    refreshPersona();
  } catch (e) { toast(String(e), "danger"); }
});

/* ============================== 规则 ============================== */
async function refreshRules() {
  const box = $("rulesBox");
  try {
    const rs = await invoke("rules");
    box.textContent = "";
    const rows = el("div", { class: "rows" });
    for (const rule of rs) rows.appendChild(el("div", { class: "row" }, [
      el("div", { class: "grow" }, [
        el("div", null, [tag(KIND[rule.kind] || rule.kind, "accent"), " ", el("span", { style: "font-weight:600;", text: KIND[rule.kind] || rule.kind })]),
        el("div", { class: "meta", text: `提前 ${rule.leads.join("、")} · ${rule.channels.map((c) => c === "push" ? "系统推送" : "应用横幅").join(" + ")}` }),
      ]),
      el("button", { class: "btn sm", type: "button", text: "编辑", onclick: () => renderRuleEditor(rule) }),
    ]));
    box.appendChild(rows);
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
// 规则编辑走浮窗（2026-07-20 重构）：列表保持在原地，不再被编辑表单整个替换。
function renderRuleEditor(rule) {
  const leadInput = el("input", { type: "text", value: (rule.leads || []).join(", "), placeholder: "如 3d, 1h 或 30m" });
  const push = el("input", { type: "checkbox", checked: (rule.channels || []).includes("push") });
  const banner = el("input", { type: "checkbox", checked: (rule.channels || []).includes("banner") });
  const body = el("div", null, [
    el("div", { class: "field", style: "margin-top:4px;" }, [
      el("label", { text: "提前提醒" }), leadInput,
      el("span", { class: "hint", text: "用逗号分隔 1–4 条，例如 3d, 1h；支持 m / h / d / w，0 表示准点提醒。" }),
    ]),
    el("div", { class: "field" }, [
      el("label", { text: "通知渠道" }),
      el("div", { class: "btnrow" }, [
        el("label", { class: "switchrow" }, [push, "系统推送"]),
        el("label", { class: "switchrow" }, [banner, "应用横幅"]),
      ]),
    ]),
    el("div", { class: "fine", text: "保存后立即重排未来日程尚未触发的提醒；历史记录不会被改写。" }),
  ]);
  openModal(`编辑规则：${KIND[rule.kind] || rule.kind}`, body, "保存并重排", async () => {
    const leads = leadInput.value.split(/[,，、]/).map((item) => item.trim()).filter(Boolean);
    const channels = [];
    if (push.checked) channels.push("push");
    if (banner.checked) channels.push("banner");
    try {
      const result = await invoke("rules_save", { rule: { kind: rule.kind, leads, channels }, now: nowParam() });
      toast(`规则已保存；已重排 ${result.replannedEvents || 0} 个未来日程的提醒`, "ok");
      refreshRules(); refreshNotifs(); refreshToday();
    } catch (e) { toast(String(e), "danger"); }
  }, { okStyle: "primary" });
  leadInput.focus();
}

/* ============================== 主动度 ============================== */
async function refreshProactivity() {
  const box = $("proactivityBox");
  try {
    const dims = await invoke("proactivity_get");
    box.textContent = "";
    for (const d of dims) {
      const seg = el("div", { class: "seg", role: "group", "aria-label": DIM[d.dimension] || d.dimension });
      for (const l of LEVELS) {
        seg.appendChild(el("button", {
          type: "button", class: l === d.level ? "active" : "", text: LEVEL[l],
          onclick: async () => {
            try {
              await invoke("proactivity_set", { dimension: d.dimension, level: l });
              toast(`已设置 ${DIM[d.dimension]} = ${LEVEL[l]}`, "ok");
              refreshProactivity();
            } catch (e) { toast(String(e), "danger"); refreshProactivity(); }
          },
        }));
      }
      box.appendChild(el("div", { class: "solum-dim" }, [
        el("div", { class: "grow" }, [
          el("div", { class: "name", text: DIM[d.dimension] || d.dimension }),
          el("div", { class: "desc", text: DIMDESC[d.dimension] || "" }),
        ]),
        seg,
      ]));
    }
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}

/* ============================== 通知智能管线（F20） ============================== */
let notificationAppCatalog = [];
let notificationConfigForPicker = null;
function appNameForPackage(packageName) {
  return notificationAppCatalog.find((app) => app.packageName === packageName)?.name || "已授权应用";
}
function renderNotificationAppPicker(config) {
  const picker = $("notifAppPicker");
  picker.textContent = "";
  if (!config || config.capture_mode !== "allowlist") return;
  const allowed = new Set((config && config.allowed_packages) || []);
  const query = $("notifAppSearch").value.trim().toLocaleLowerCase();
  const apps = notificationAppCatalog.filter((app) => !allowed.has(app.packageName)
    && (!query || app.name.toLocaleLowerCase().includes(query)));
  if (!notificationAppCatalog.length) {
    $("notifAppHint").textContent = "应用列表只在 Android 设备上提供；桌面端不读取第三方应用通知。";
    picker.appendChild(el("div", { class: "hint", text: "请在 Android 设备上打开此页，息壤 会列出可选择的已安装应用。" }));
    return;
  }
  $("notifAppHint").textContent = "只会显示应用名称；点选后才允许 息壤 读取该应用通知。";
  if (!apps.length) {
    picker.appendChild(el("div", { class: "hint", text: query ? "没有匹配的应用" : "所有可见应用都已选择，或系统没有返回可选应用。" }));
    return;
  }
  const list = el("div", { class: "rows" });
  for (const app of apps.slice(0, 60)) list.appendChild(el("div", { class: "row" }, [
    el("span", { class: "grow", text: app.name }),
    el("button", { class: "btn sm", type: "button", text: "选择", onclick: () => setNotificationApp(app.packageName, true) }),
  ]));
  if (apps.length > 60) list.appendChild(el("div", { class: "hint", text: `仅显示前 60 个匹配项，请继续输入应用名称缩小范围。` }));
  picker.appendChild(list);
}
function captureState(state) {
  return {
    queued: ["待处理", "hollow"], event_created: ["已建事件", "ok"],
    filtered: ["已过滤", ""], deduplicated: ["已判重", ""], needs_review: ["待回看", "accent"],
    resolved: ["已处理", "ok"],
  }[state] || [state, ""];
}
function renderNotificationLedger(box, s) {
  box.textContent = "";
  const captures = s.captures || [];
  const pending = (s.filter_proposals || []).filter((p) => p.state === "pending");
  const actionPending = (s.action_proposals || []).filter((p) => p.state === "pending");
  // 已是独立「通知回看」视图（vhead 就是标题），这里不再重复 h3。
  const heading = el("div", { class: "block" }, [
    el("p", { class: "hint", style: "margin:0 0 10px;", text: captures.length
      ? "每条已捕获通知都在这里留下去向；过滤、判重和待处理均可回看。"
      : "还没有已捕获通知。授予 Android 通知访问后，息壤 会按设置的接收范围在后台静默整理。" }),
  ]);
  if (captures.length) {
    const rows = el("div", { class: "rows" });
    for (const capture of captures.slice(0, 20)) {
      const status = captureState(capture.state);
      const actions = [];
      if (["filtered", "deduplicated", "needs_review"].includes(capture.state)) {
        actions.push(el("button", { class: "btn sm", text: "恢复处理", onclick: (event) => restoreNotificationCapture(capture.id, event.currentTarget) }));
      }
      if (!["event_created", "resolved"].includes(capture.state)) {
        actions.push(el("button", { class: "btn sm", text: "提升为日程", title: "仅在本地规则能确定时间时创建日程", onclick: (event) => promoteNotificationCapture(capture.id, event.currentTarget) }));
      }
      const metadata = [
        appNameForPackage(capture.package_name), " · ", capture.lane === "urgent" ? "重要即时" : "普通批量", " · ", fmtDT(capture.received_at),
      ];
      if (capture.local_only) metadata.push(" · 仅本机");
      if (capture.reason) metadata.push(" · " + capture.reason);
      rows.appendChild(el("div", { class: "row", style: "align-items:flex-start;" }, [
        el("div", { class: "grow" }, [
          el("div", { text: capture.title || "（无标题通知）" }),
          capture.body ? el("div", { class: "hint", style: "margin-top:2px;display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;overflow:hidden;", text: capture.body }) : null,
          el("div", { class: "hint" }, metadata),
        ]),
        tag(status[0], status[1]),
        actions.length ? el("span", { style: "display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end;" }, actions) : null,
      ]));
    }
    heading.appendChild(rows);
  }
  if (pending.length) {
    const rows = el("div", { class: "rows", style: "margin-top:12px;" });
    rows.appendChild(el("div", { class: "hint", text: "待确认的 LLM 过滤规则（确认前不会生效）：" }));
    for (const proposal of pending) {
      rows.appendChild(el("div", { class: "row" }, [
        el("div", { class: "grow" }, [
          el("div", { text: `过滤「${proposal.pattern}」` }),
          el("div", { class: "hint", text: `${proposal.package_name ? appNameForPackage(proposal.package_name) : "所有已选择应用"}${proposal.reason ? " · " + proposal.reason : ""}` }),
        ]),
        el("button", { class: "btn sm primary", text: "确认规则", onclick: (event) => decideNotificationProposal(proposal.id, true, event.currentTarget) }),
        el("button", { class: "btn sm", text: "忽略", onclick: (event) => decideNotificationProposal(proposal.id, false, event.currentTarget) }),
      ]));
    }
    heading.appendChild(rows);
  }
  if (actionPending.length) {
    const rows = el("div", { class: "rows", style: "margin-top:12px;" });
    rows.appendChild(el("div", { class: "hint", text: "待确认的通知动作提议（目标由本机匹配，点按前不会改动日程）：" }));
    for (const proposal of actionPending) {
      const isCancel = proposal.kind === "cancel_event";
      const event = proposal.event || null;
      const title = event?.title || proposal.event_title || "（原日程已不存在）";
      const originalStart = event?.start ? fmtDTText(event.start) : "原日程已不存在";
      const newStart = proposal.new_start ? fmtDTText(proposal.new_start) : "新时间未提供";
      const summary = isCancel
        ? `建议取消「${title}」（原时间：${originalStart}）`
        : `建议把「${title}」从 ${originalStart} 改到 ${newStart}`;
      rows.appendChild(el("div", { class: "row", style: "align-items:flex-start;" }, [
        el("div", { class: "grow" }, [
          el("div", { text: summary }),
          el("div", { class: "hint", text: proposal.reason || "来自通知分诊；仍需你确认" }),
        ]),
        event ? el("button", { class: `btn sm ${isCancel ? "danger" : "primary"}`, text: isCancel ? `确认取消「${title}」` : "确认改期", onclick: (event) => decideNotificationActionProposal(proposal.id, true, event.currentTarget) }) : null,
        el("button", { class: "btn sm", text: "忽略", onclick: (event) => decideNotificationActionProposal(proposal.id, false, event.currentTarget) }),
      ]));
    }
    heading.appendChild(rows);
  }
  box.appendChild(heading);
}
async function renderNotificationIntelligenceSettings(s) {
  const apps = $("notifIntelApps"), rules = $("notifIntelRules"), pipeline = $("notifPipelineState"), scope = $("notifPriorityPackage");
  // 事后发现机制：某个应用写得比你预期的多，这里能看出来。它不构成授权。
  let autoEventCounts = {};
  try {
    for (const [pkg, n] of await invoke("notif_intelligence_auto_event_counts", { now: nowParam() })) {
      autoEventCounts[pkg] = n;
    }
  } catch (_) { /* 统计拿不到不影响授权界面本身 */ }
  const config = s.config || { capture_mode: "all", allowed_packages: [], auto_event_packages: [], filter_rules: [] };
  const captureMode = config.capture_mode === "allowlist" ? "allowlist" : "all";
  notificationConfigForPicker = config;
  $("notifCaptureMode").value = captureMode;
  $("notifAllowlistControls").hidden = captureMode !== "allowlist";
  $("notifCaptureModeHint").textContent = captureMode === "allowlist"
    ? "只有白名单里的应用消息会进入本机处理管线。"
    : "接收所有第三方应用消息；后台静默整理，需确认或明确开启的提醒才会提示。";
  $("notifBatchInterval").value = String(config.batch_interval_minutes || 15);
  apps.textContent = "";
  const allowed = config.allowed_packages || [];
  const selectedScope = scope.value;
  scope.textContent = "";
  scope.appendChild(el("option", { value: "", text: "全局（所有已接收的应用）" }));
  for (const pkg of allowed) scope.appendChild(el("option", { value: pkg, text: appNameForPackage(pkg) }));
  scope.value = allowed.includes(selectedScope) ? selectedScope : "";
  // 采集丢失必须说出来。这两类都是「告诉过用户会捕获、实际没捕获」的通知，
  // 只写日志等于没说。undeliverable 的原始内容还留在 notif-spool/failed/，
  // 所以文案要区分「彻底丢了」和「留着但没进来」。
  const lostBox = $("notifIntakeLoss");
  if (lostBox) {
    lostBox.textContent = "";
    const overflow = s.intake_overflow || 0, undeliverable = s.intake_undeliverable || 0;
    if (overflow || undeliverable) {
      lostBox.hidden = false;
      const parts = [];
      // marker 本身也有上限（原生侧 2000）。到顶之后就是欠计数了，
      // 这时说「N 条」是在给一个我们拿不到的精度——只能说「至少」。
      if (overflow) {
        const atLeast = overflow >= 2000 ? "至少 " : "";
        parts.push(`${atLeast}${overflow} 条因待处理队列已满被丢弃（内容未保留）`);
      }
      // 这一条是「此刻盘上还留着几份」，不是累计事件数——所以用现在时说。
      if (undeliverable) parts.push(`当前有 ${undeliverable} 条多次尝试后仍未能入库，原始内容留存在 notif-spool/failed/（未删除）`);
      lostBox.appendChild(el("div", { text: `有通知没能进入 息壤：${parts.join("；")}。` }));
      lostBox.appendChild(el("div", { class: "fine", style: "margin-top:4px;",
        text: "常见原因是应用长期未运行或存储空间不足。恢复后新通知会正常捕获；" +
              "被丢弃的无法找回，留存在 failed/ 的仍可人工查看。计数只增不虚报，" +
              "但存储异常时可能连丢弃本身都没能记下，因此是下限。" }));
      lostBox.appendChild(el("button", { class: "btn sm", style: "margin-top:8px;", text: "我知道了",
        onclick: async () => {
          try { await invoke("notif_intelligence_acknowledge_losses"); refreshNotificationIntelligence(); }
          catch (e) { toast(String(e), "danger"); }
        } }));
    } else {
      lostBox.hidden = true;
    }
  }

  // 两种授权分开呈现：读取通知 ≠ 允许它往日历里写东西。
  // 上面那句「选择后才允许读取该应用通知」授的是前者；自动建日程必须在这里
  // 逐个应用单独打开，默认关闭。
  const autoPkgs = config.auto_event_packages || [];
  const managedPackages = captureMode === "allowlist" ? allowed : autoPkgs;
  apps.appendChild(el("div", { class: "rows" }, managedPackages.length
    ? managedPackages.map((pkg) => {
        const on = autoPkgs.includes(pkg);
        const made = (autoEventCounts[pkg] || 0);
        return el("div", { class: "row", style: "flex-wrap:wrap;gap:8px;" }, [
          el("span", { class: "grow", text: appNameForPackage(pkg) }),
          el("span", { class: "hint", style: "flex-basis:100%;order:3;margin:0;" }, [
            el("span", { text: on
              ? `已允许自动建日程${made ? `（近 7 天创建 ${made} 条）` : "（近 7 天未创建）"}`
              : "仅读取通知，不会自动建日程；识别到的内容会进「通知回看」等你确认。" }),
          ]),
          el("button", {
            class: on ? "btn sm danger" : "btn sm",
            text: on ? "撤销自动建日程" : "允许自动建日程",
            title: on
              ? "撤销后，该应用的通知仍会被读取，但不再自动写入日历"
              : "单独授权：允许这个应用的通知在本地解析出确定时间时，直接创建日程和提醒",
            onclick: () => setNotificationAppAutoEvent(pkg, !on),
          }),
          captureMode === "allowlist"
            ? el("button", { class: "btn sm", text: "移出白名单", onclick: () => setNotificationApp(pkg, false) })
            : null,
        ]);
      })
    : [el("div", { class: "hint", text: captureMode === "all"
      ? "当前接收所有第三方应用消息；自动建日程仍保持逐应用、默认关闭。切换到白名单后可管理逐应用授权。"
      : "白名单为空：息壤 不会读取第三方应用消息。" })]));
  rules.textContent = "";
  const allRules = s.priority_rules || [];
  const filterRules = config.filter_rules || [];
  const list = el("div", { class: "rows" });
  list.appendChild(el("div", { class: "hint", text: "重要路由规则（命中只影响分诊时机，不授权任何动作）：" }));
  if (!allRules.length) list.appendChild(el("div", { class: "hint", text: "未配置；允许应用时会加入可编辑的预设。" }));
  for (const rule of allRules) list.appendChild(el("div", { class: "row" }, [
    el("span", { class: "grow", text: `${rule.pattern} · ${rule.package_name ? appNameForPackage(rule.package_name) : "所有已接收应用"} · ${rule.matcher === "regex" ? "正则" : "包含"}` }),
    el("button", { class: "btn icon danger", title: "删除即时规则", "aria-label": "删除即时规则", onclick: () => removeNotificationPriorityRule(rule.id) }, icon("trash", 16)),
  ]));
  if (filterRules.length) {
    list.appendChild(el("div", { class: "hint", style: "margin-top:8px;", text: "已确认过滤规则（可撤销，已过滤通知仍在台账）：" }));
    for (const rule of filterRules) list.appendChild(el("div", { class: "row" }, [
      el("span", { class: "grow", text: `${rule.pattern} · ${rule.package_name ? appNameForPackage(rule.package_name) : "所有已接收应用"}` }),
      el("button", { class: "btn sm", text: "撤销", onclick: () => removeNotificationFilterRule(rule.id) }),
    ]));
  }
  rules.appendChild(list);
  renderNotificationAppPicker(config);
  pipeline.textContent = "";
  const p = s.pipeline || {};
  $("notifPipelineAndroidControls").hidden = !p.supported;
  if (p.supported && !p.running) {
    // Android 上服务没跑才是需要用户行动的事，用横幅；其余是稳态陈述。
    pipeline.appendChild(el("div", { class: "banner", style: "margin-top:12px;" }, [
      icon("alert", 18),
      el("div", { class: "grow" }, [
        el("div", { text: "Android 前台处理服务未运行" }),
        el("div", { class: "hint", text: p.ignoring_battery_optimizations
          ? "已获电池优化豁免；国产 ROM 仍可能需要在系统的后台/自启动设置中手动放行。"
          : "建议允许忽略电池优化；国产 ROM 仍可能需要在系统的后台/自启动设置中手动放行。" }),
      ]),
    ]));
  } else {
    pipeline.appendChild(el("div", { class: "statusline", style: "margin-top:12px;", title: p.supported
        ? "国产 ROM 仍可能需要在系统的后台/自启动设置中手动放行。"
        : "电池优化与 ROM 后台设置只适用于 Android，桌面端无需操作。" }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: p.supported ? "Android 前台处理服务正在运行" : "前台处理服务未运行（桌面端为正常状态）" }),
    ]));
  }
}
async function refreshNotificationIntelligence() {
  try {
    const status = await invoke("notif_intelligence_status");
    try { notificationAppCatalog = await invoke("notif_intelligence_apps"); }
    catch (_) { notificationAppCatalog = []; }
    await renderNotificationIntelligenceSettings(status);
  }
  catch (e) { $("notifPipelineState").textContent = safeUserError(e, "通知处理状态暂时无法读取，请稍后重试。"); }
}
async function setNotificationApp(packageName, enabled) {
  try {
    await invoke("notif_intelligence_set_app", { packageName, enabled });
    toast(enabled ? `已允许接收「${appNameForPackage(packageName)}」的通知` : `已停止接收「${appNameForPackage(packageName)}」的通知`, "ok");
    refreshNotificationIntelligence(); refreshLedger();
  } catch (e) { toast(String(e), "danger"); }
}
async function setNotificationCaptureMode(mode) {
  try {
    await invoke("notif_intelligence_set_capture_mode", { mode });
    toast(mode === "all" ? "已改为接收全应用消息" : "已改为仅接收白名单应用消息", "ok");
    refreshNotificationIntelligence(); refreshLedger();
  } catch (e) { toast(String(e), "danger"); refreshNotificationIntelligence(); }
}
// 第二种授权：允许该应用的通知自动创建日程。与「读取通知」分开，
// 因为它是另一件事——读取是让 息壤 看见，这一步是让这个应用往你的日历里写。
async function setNotificationAppAutoEvent(packageName, enabled) {
  const name = appNameForPackage(packageName);
  if (enabled) {
    const body = el("div", null, [
      el("div", { text: `允许「${name}」的通知在本地解析出确定时间时，直接创建日程和提醒——不再逐条问你。` }),
      el("div", { class: "fine", style: "margin-top:8px;", text:
        "这与「读取该应用通知」是两回事：读取只是让 息壤 看见，这一步是让它据此写入你的日历。" +
        `为防止意外，单个应用每天最多自动创建 ${AUTO_EVENT_DAILY_CAP} 条，超出的转为待你确认；` +
        "创建数量可在本页随时查看，也可随时撤销。" }),
    ]);
    openModal(`允许「${name}」自动建日程`, body, "允许", async () => {
      try {
        await invoke("notif_intelligence_set_app_auto_event", { packageName, enabled: true });
        toast(`已允许「${name}」自动建日程`, "ok");
        refreshNotificationIntelligence();
      } catch (e) { toast(String(e), "danger"); }
    });
    return;
  }
  try {
    await invoke("notif_intelligence_set_app_auto_event", { packageName, enabled: false });
    toast(`已撤销「${name}」的自动建日程（仍会读取其通知）`, "ok");
    refreshNotificationIntelligence();
  } catch (e) { toast(String(e), "danger"); }
}
// 与核心 MAX_AUTO_EVENTS_PER_APP_PER_DAY 保持一致；只用于文案。
const AUTO_EVENT_DAILY_CAP = 20;

function disableNotificationSubmit(button) {
  const buttons = button && button.parentElement
    ? Array.from(button.parentElement.querySelectorAll("button"))
    : button ? [button] : [];
  buttons.forEach((item) => { item.disabled = true; });
  return buttons;
}
function restoreNotificationSubmit(buttons) {
  buttons.forEach((item) => { item.disabled = false; });
}
async function restoreNotificationCapture(id, button) {
  const buttons = disableNotificationSubmit(button);
  try { await invoke("notif_intelligence_restore_capture", { id }); toast("已恢复到处理队列", "ok"); refreshLedger(); }
  catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function promoteNotificationCapture(id, button) {
  const buttons = disableNotificationSubmit(button);
  try { toast(await invoke("notif_intelligence_promote_capture", { id, now: nowParam() }), "ok"); refreshAll(); }
  catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function decideNotificationProposal(id, accepted, button) {
  const buttons = disableNotificationSubmit(button);
  try { await invoke("notif_intelligence_set_filter_proposal", { id, accepted }); toast(accepted ? "过滤规则已确认，可随时撤销" : "已忽略该过滤提议", "ok"); refreshAll(); }
  catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function decideNotificationActionProposal(id, accepted, button) {
  const buttons = disableNotificationSubmit(button);
  try {
    const message = await invoke("notif_intelligence_set_action_proposal", { id, accepted, now: nowParam() });
    toast(message || (accepted ? "已确认通知动作提议" : "已忽略通知动作提议"), "ok");
    refreshAll();
  } catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function removeNotificationPriorityRule(id) {
  try { await invoke("notif_intelligence_remove_priority_rule", { id }); toast("已删除即时规则", "ok"); refreshNotificationIntelligence(); }
  catch (e) { toast(String(e), "danger"); }
}
async function removeNotificationFilterRule(id) {
  try { await invoke("notif_intelligence_remove_filter_rule", { id }); toast("已撤销过滤规则；历史通知仍可在台账恢复", "ok"); refreshAll(); }
  catch (e) { toast(String(e), "danger"); }
}

$("notifAppSearch").addEventListener("input", () => renderNotificationAppPicker(notificationConfigForPicker));
$("notifCaptureMode").addEventListener("change", (event) => setNotificationCaptureMode(event.target.value));
$("notifBatchInterval").addEventListener("change", async (event) => {
  try { await invoke("notif_intelligence_set_batch_interval", { minutes: Number(event.currentTarget.value) }); toast("普通通知批量间隔已更新", "ok"); }
  catch (e) { toast(String(e), "danger"); refreshNotificationIntelligence(); }
});
$("notifPriorityAddBtn").addEventListener("click", async () => {
  const pattern = $("notifPriorityPattern").value.trim();
  if (!pattern) return toast("请填写重要通知模式", "danger");
  try {
    await invoke("notif_intelligence_add_priority_rule", {
      pattern, packageName: $("notifPriorityPackage").value || null, matcher: $("notifPriorityMatcher").value,
    });
    $("notifPriorityPattern").value = "";
    toast("已添加即时分诊规则", "ok"); refreshNotificationIntelligence();
  } catch (e) { toast(String(e), "danger"); }
});
$("notifBatteryRequestBtn").addEventListener("click", async () => {
  try { await invoke("notif_pipeline_request_battery_optimization"); toast("已打开系统电池优化授权", "ok"); }
  catch (e) { toast(String(e), "danger"); }
});
$("notifBatterySettingsBtn").addEventListener("click", async () => {
  try { await invoke("notif_pipeline_open_battery_settings"); } catch (e) { toast(String(e), "danger"); }
});
$("notifBackgroundSettingsBtn").addEventListener("click", async () => {
  try { await invoke("notif_pipeline_open_background_settings"); } catch (e) { toast(String(e), "danger"); }
});
$("notifBatchNowBtn").addEventListener("click", async () => {
  try { const count = await invoke("notif_intelligence_process_now", { now: nowParam() }); toast(count ? `已处理 ${count} 条普通通知` : "普通队列为空", "ok"); refreshAll(); }
  catch (e) { toast(String(e), "danger"); }
});

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
    if (!accountLoggedIn && !guestChoiceMade()) {
      openAccountGate("隐私设置已保存，现在选择使用方式", { dismissible: false, allowGuest: true });
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
      box.appendChild(el("div", { class: "row" }, [
        tag(c.status, CAPTURE_AVAIL_KIND[c.availability] || "plain"),
        el("div", { class: "grow" }, [
          el("div", { text: c.label }),
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

/* ============================== 护栏 ============================== */
$("guardNoteIc").appendChild(icon("shield", 18));
async function refreshGuard() {
  const box = $("toolsBox");
  try {
    const ts = await invoke("tools");
    box.textContent = "";
    const list = el("div", { class: "rows" });
    for (const t of ts) {
      const r = RISK[t.risk] || [t.risk, ""];
      list.appendChild(el("div", { class: "row" }, [
        el("span", { class: "title", text: humanToolLabel(t.name) }),
        el("div", { class: "grow" }),
        tag(r[0], r[1]),
      ]));
    }
    box.appendChild(list);
    const sel = $("guardTool");
    const cur = sel.value;
    sel.textContent = "";
    for (const t of ts) sel.appendChild(el("option", { value: t.name, text: humanToolLabel(t.name) }));
    if ([...sel.options].some((o) => o.value === cur)) sel.value = cur;
    else if ([...sel.options].some((o) => o.value === "demo_delete")) sel.value = "demo_delete";
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
  refreshAudit();
}
async function refreshAudit() {
  const box = $("auditBox");
  try {
    const rows = await invoke("audit");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("shield", "暂无安全操作记录", "所有涉及敏感权限的操作与拦截结果都会安全记录在此。"));
      return;
    }
    box.appendChild(dataTable(["#", "时间", "风险", "操作", "结果", "详情"],
      rows.map((r) => {
        const rk = RISK[r.risk] || [r.risk, ""];
        return [
          td(String(r.id), "num"),
          [fmtDT(r.ts)],
          [tag(rk[0], rk[1])],
          [el("span", { class: "title", text: humanToolLabel(r.tool) })],
          [r.decision === "executed" ? tag("已执行", "ok") : tag("已拦截", "danger")],
          td(r.detail, "mut"),
        ];
      })));
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
function showGuardResult(ok, text) {
  const box = $("guardResult");
  box.className = ok ? "done" : "blocked";
  box.textContent = "";
  box.appendChild(icon(ok ? "check" : "x", 16));
  box.appendChild(el("span", { text: text }));
}
function openGuardConfirm(p, showFine) {
  const rk = RISK[p.risk] || [p.risk, ""];
  const body = el("div", null, [
    el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(p.tool) }), " ", tag(rk[0], rk[1])]),
    el("div", { class: "preview", text: "后果预览：" + p.preview }),
    showFine ? el("div", { class: "fine", text: "确认后签发一次性授权凭证（5 分钟内有效，仅限本次操作）。" }) : null,
  ]);
  openModal("人工确认：将要执行的操作", body, "确认执行", async () => {
    try {
      const r = await invoke("guard_confirm", {
        pendingId: p.pending_id, tool: p.tool, args: p.args, now: nowParam(),
      });
      if (showFine) showGuardResult(r.ok, r.output); else toast(r.output, r.ok ? "ok" : "danger");
      refreshGuard();
    } catch (e) {
      if (showFine) showGuardResult(false, safeUserError(e, "这次操作没有执行，请重新确认。")); else toast(safeUserError(e), "danger");
      refreshAudit();
    }
  }, { icon: "shield" });
}
$("guardRunBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("guard_run", { tool: $("guardTool").value, args: $("guardArgs").value, now: nowParam() });
    showGuardResult(r.ok, r.output);
    refreshAudit();
  } catch (e) { toast(String(e), "danger"); }
});
$("guardReqBtn").addEventListener("click", async () => {
  try {
    const p = await invoke("guard_request", { tool: $("guardTool").value, args: $("guardArgs").value, now: nowParam() });
    openGuardConfirm(p, true);
  } catch (e) { toast(String(e), "danger"); }
});

/* ============================== 通知监听横幅（F1 伴随） ============================== */
let notifAccessDismissed = false;
let notifBanner = null;
async function refreshNotifAccess() {
  if (notifAccessDismissed || !accountLoggedIn) return;
  try {
    const enabled = await invoke("notif_access_status");
    if (enabled) { if (notifBanner) { notifBanner.remove(); notifBanner = null; } return; }
    if (notifBanner) return;
    notifBanner = el("div", { class: "banner" }, [
      icon("bell", 18),
      el("div", { class: "grow", text: "通知监听未开启，息壤 收不到手机通知来自动记日程（F1）。" }),
      el("button", {
        class: "btn primary sm", text: "去开启",
        onclick: async () => { try { await invoke("notif_access_open_settings"); } catch (e) { toast(String(e), "danger"); } },
      }),
      el("button", {
        class: "btn sm", text: "本次忽略",
        onclick: () => { notifAccessDismissed = true; notifBanner.remove(); notifBanner = null; },
      }),
    ]);
    $("banners").appendChild(notifBanner);
  } catch (_) { /* 老构建/桌面壳没有该 command——保持隐藏 */ }
}
// 用户刚在系统设置里翻开关的唯一时机：窗口重获焦点时复查。
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") refreshNotifAccess();
});

/* ============================== 今日面板（宽屏对话页侧栏） ============================== */
async function refreshToday() {
  if (!$("todayPanel").offsetParent && window.innerWidth < 1240) return; // 隐藏时不必刷新
  try {
    const [due, evs, sugg, allEvents] = await Promise.all([
      invoke("due", { now: nowParam() }),
      invoke("agenda", { now: nowParam() }),
      accountLoggedIn ? invoke("suggestions") : Promise.resolve([]),
      invoke("all_events"),
    ]);
    const evTitle = {};
    allEvents.forEach((ev) => { evTitle[ev.id] = ev.title; });
    const tpDue = $("tpDue");
    tpDue.textContent = "";
    if (due.length) {
      for (const n of due.slice(0, 4)) {
        tpDue.appendChild(el("div", { class: "tp-item" }, [
          el("span", { class: "sdot amber" }),
          el("div", { class: "grow" }, [
            el("div", { text: evTitle[n.event_id] || "日程" }),
            el("div", { class: "t" }, [fmtDT(n.fire_at), ` · 提前${n.lead_label}`]),
          ]),
        ]));
      }
      tpDue.appendChild(el("button", {
        class: "btn sm primary", style: "margin-top:6px;", text: `触发 ${due.length} 条到点提醒`,
        onclick: () => $("fireBtn").click(),
      }));
    } else {
      tpDue.appendChild(el("div", { class: "tp-empty", text: "暂无。到点我会亮灯。" }));
    }
    const tpNext = $("tpNext");
    tpNext.textContent = "";
    if (evs.length) {
      for (const ev of evs.slice(0, 5)) {
        const [d, t] = String(ev.start || "").split("T");
        tpNext.appendChild(el("div", { class: "tp-item" }, [
          el("span", { class: "t", text: (t || "").slice(0, 5) }),
          el("div", { class: "grow" }, [
            el("div", { text: ev.title }),
            el("div", { style: "font-size:.75rem;color:var(--muted);", text: dayLabel(d) }),
          ]),
        ]));
      }
    } else {
      tpNext.appendChild(el("div", { class: "tp-empty", text: "接下来没有日程。" }));
    }
    const tpS = $("tpSuggest");
    tpS.textContent = "";
    const pend = sugg.filter((s) => s.status === "pending");
    if (!accountLoggedIn) {
      tpS.appendChild(el("button", {
        class: "btn sm", type: "button", text: "登录后开启主动建议",
        onclick: () => requireAccount("登录后让 Solum 根据日程与行为主动给出建议"),
      }));
    } else if (pend.length) {
      for (const s of pend.slice(0, 3)) {
        tpS.appendChild(el("div", { class: "tp-item" }, [
          el("span", { class: "sdot hollow" }),
          el("div", { class: "grow", text: s.text }),
        ]));
      }
      tpS.appendChild(el("button", {
        class: "btn sm", style: "margin-top:6px;", text: `去处理（共 ${pend.length} 条）`,
        onclick: () => showView("suggest"),
      }));
    } else {
      tpS.appendChild(el("div", { class: "tp-empty", text: "没有待处理的建议。" }));
    }
  } catch (_) { /* 面板刷新失败不打扰主流程 */ }
}

/* ============================== 云端接入（设置 → 云端） ============================== */
// 预设服务端点与推理模型
const LLM_PRESETS = [
  { id: "custom", label: "自定义端点…", base: "",
    models: [], timeout: 60, temp: "default",
    note: "支持任何兼容 OpenAI 格式的推理服务端点。" },
];
let solumCenterUrl = "https://solum.tannerlab.cn";
let solumActiveModel = "";
let llmKeyTail = "";   // 已保存密钥的尾巴（空 = 还没存过 key，保存时必填）
let llmFormSeeded = false; // 表单只在首次加载/保存后回填，refreshAll 不许覆盖正在编辑的内容

for (const p of LLM_PRESETS) $("llmPreset").appendChild(el("option", { value: p.id, text: p.label }));

function llmApplyPreset(p) {
  $("llmBase").value = p.base;
  const dl = $("llmModels");
  dl.textContent = "";
  for (const m of p.models) dl.appendChild(el("option", { value: m }));
  $("llmModel").value = p.models[0] || "";
  $("llmTimeout").value = p.timeout;
  $("llmTempMode").value = p.temp;
  llmTempModeSync();
  $("llmPresetNote").hidden = !p.note;
  $("llmPresetNote").textContent = p.note || "";
}
function llmTempModeSync() { $("llmTempVal").hidden = $("llmTempMode").value !== "custom"; }
$("llmPreset").addEventListener("change", () => {
  const p = LLM_PRESETS.find((x) => x.id === $("llmPreset").value);
  if (p) llmApplyPreset(p);
});
$("llmTempMode").addEventListener("change", llmTempModeSync);

// 表单 → 后端参数。temperature 三态：默认 0.3 / 自定义 / null（不发送）。
function llmCollect() {
  const mode = $("llmTempMode").value;
  const temperature = mode === "omit" ? null
    : mode === "custom" ? Math.min(2, Math.max(0, Number($("llmTempVal").value) || 0.3))
    : 0.3;
  const mt = parseInt($("llmMaxTok").value, 10);
  return { cfg: {
    base_url: $("llmBase").value.trim(),
    api_key: $("llmKey").value.trim(),
    model: $("llmModel").value.trim(),
    temperature,
    max_tokens: mt > 0 ? mt : null,
    timeout_secs: Math.min(600, Math.max(5, parseInt($("llmTimeout").value, 10) || 30)),
  } };
}
function llmValidate(cfg) {
  if (!/^https?:\/\//.test(cfg.base_url)) return "Base URL 需要以 http(s):// 开头";
  if (!cfg.model) return "请填写模型名";
  if (!cfg.api_key && !llmKeyTail) return "请填写 API Key";
  return null;
}

function renderCloudStatus(s) {
  const box = $("cloudStatus");
  box.textContent = "";
  if (s.configured) {
    // 已配置是稳态陈述：低调状态行，细节进 title；琥珀横幅留给「未配置」这类要行动的事。
    box.appendChild(el("div", { class: "statusline", title: s.source === "env"
        ? "凭据来自环境变量 SOLUM_LLM_*：这里保存只对本次运行生效，下次启动仍以环境变量为准。"
        : `配置文件：${s.path}（已 gitignore，不入库、不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `当前在用：${s.base_url} · ${s.model} · key:${s.key_tail}` }),
    ]));
    if (s.source === "env") box.appendChild(el("div", { class: "hint", style: "margin:4px 0 0 16px;",
      text: "凭据来自环境变量 SOLUM_LLM_*，这里保存只对本次运行生效。" }));
  } else {
    box.appendChild(el("div", { class: "banner" }, [
      icon("alert", 18),
      el("div", { class: "grow" }, [
        el("div", { text: "未配置云端，当前为离线模式——规则路径照常工作，闲聊与兜底抽取不可用。" }),
        el("div", { class: "hint", text: `保存后写入：${s.path}` }),
      ]),
    ]));
  }
}

// 状态横幅每次都刷；表单只在首次 seed（或保存后重新 seed），避免覆盖正在编辑的值。
async function refreshCloud() {
  let s;
  try { s = await invoke("llm_config_get"); } catch (_) { return; }
  llmKeyTail = s.key_tail || "";
  renderCloudStatus(s);
  $("llmKey").placeholder = llmKeyTail ? `已保存（key:${llmKeyTail}），留空则沿用` : "必填";
  if (llmFormSeeded) return;
  llmFormSeeded = true;
  const p = LLM_PRESETS.find((x) => x.base && s.base_url === x.base)
    || LLM_PRESETS[s.configured ? LLM_PRESETS.length - 1 : 0];
  $("llmPreset").value = p.id;
  llmApplyPreset(p);
  if (s.configured) {
    $("llmBase").value = s.base_url;
    $("llmModel").value = s.model;
    $("llmTimeout").value = s.timeout_secs;
    $("llmMaxTok").value = s.max_tokens == null ? "" : s.max_tokens;
    if (s.temperature == null) $("llmTempMode").value = "omit";
    else if (s.temperature === 0.3) $("llmTempMode").value = "default";
    else { $("llmTempMode").value = "custom"; $("llmTempVal").value = s.temperature; }
    llmTempModeSync();
  }
}

$("llmSaveBtn").addEventListener("click", async () => {
  const args = llmCollect();
  const bad = llmValidate(args.cfg);
  if (bad) return toast(bad, "danger");
  $("llmSaveBtn").disabled = true;
  try {
    const summary = await invoke("llm_config_save", args);
    $("llmKey").value = "";
    toast("已保存并启用云端配置", "ok");
    // 页脚状态就地点亮（不动模拟时钟，所以不走 initClock）。
    $("llmDot").className = "sdot ok";
    $("llmText").textContent = "云端在线";
    $("llmRow").title = summary;
    llmFormSeeded = false;
    refreshCloud();
  } catch (e) { toast(safeUserError(e, "云端配置未保存，请稍后重试。"), "danger"); }
  finally { $("llmSaveBtn").disabled = false; }
});

$("llmTestBtn").addEventListener("click", async () => {
  const args = llmCollect();
  const bad = llmValidate(args.cfg);
  if (bad) return toast(bad, "danger");
  const out = $("llmTestOut");
  out.textContent = "";
  out.appendChild(el("div", { class: "llm-test", text: "正在请求端点，最长等待 " + args.cfg.timeout_secs + " 秒…" }));
  $("llmTestBtn").disabled = true;
  try {
    const r = await invoke("llm_config_test", args);
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test ok" }, [
      icon("check", 15),
      el("span", null, [
        "连通正常 · ", el("span", { class: "mono", text: r.latency_ms + " ms" }),
        " · 模型回复：" + String(r.reply).slice(0, 80),
      ]),
    ]));
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [
      icon("alert", 15), el("span", { text: "测试失败：" + safeUserError(e, "云端暂时不可用，请稍后重试。") }),
    ]));
  } finally { $("llmTestBtn").disabled = false; }
});

/* ============================== Android 提醒可靠性 ============================== */
async function refreshReminderDeliveryHealth() {
  const block = $("reminderReliabilityBlock");
  const warning = $("reminderDeliveryWarning");
  const warningText = $("reminderDeliveryWarningText");
  const status = $("reminderDeliveryStatus");
  const qa = $("reminderProcessDeathQa");
  try {
    const health = await invoke("reminder_delivery_health");
    if (!health.platformSupported) {
      block.hidden = true;
      return;
    }
    block.hidden = false;
    const failures = [];
    if (!health.receiverRegistered) failures.push("当前安装包缺少后台提醒接收器，应用进程退出后提醒不会触达；请安装通过发布核对的新版本");
    if (!health.bootReceiverRegistered) failures.push("当前安装包缺少开机恢复组件，设备重启后提醒不会自动重新挂载");
    if (!health.notificationsEnabled) failures.push("系统通知已关闭，即使提醒到点也无法显示；请在系统设置中允许 Solum 通知");
    warning.hidden = failures.length === 0;
    warningText.textContent = failures.join("；");
    const degraded = !health.exact || failures.length > 0;
    status.textContent = "";
    status.appendChild(el("span", { class: "sdot " + (failures.length ? "danger" : degraded ? "amber" : "ok") }));
    status.appendChild(el("span", { class: "t", text: failures.length
      ? "后台提醒交付未就绪"
      : health.exact
        ? "后台接收、重启恢复与精确提醒均已就绪"
        : "后台接收与重启恢复已就绪；当前使用系统弹性调度，提醒可能延迟" }));
    qa.hidden = !health.debugProcessDeathAvailable;
  } catch (error) {
    qa.hidden = true;
    if (!IS_ANDROID) { block.hidden = true; return; }
    block.hidden = false;
    warning.hidden = false;
    warningText.textContent = "无法确认后台提醒组件是否完整，请重新打开应用；若仍出现，请安装通过发布核对的新版本。";
    status.textContent = "";
    status.appendChild(el("span", { class: "sdot danger" }));
    status.appendChild(el("span", { class: "t", text: safeUserError(error, "后台提醒自检失败") }));
  }
}

function requestDebugProcessDeath() {
  openModal("Debug：模拟后台回收", el("div", null, [
    el("div", { class: "preview", text: "将结束 Solum 当前进程，但不清除数据、不修改设置，也不取消系统已经挂载的提醒。" }),
    el("div", { class: "fine", text: "请先创建一条几分钟后到点的提醒。应用关闭后等待系统通知；不要从系统设置点“强行停止”。" }),
  ]), "结束当前进程", async () => {
    try {
      const scheduled = await invoke("debug_simulate_process_death");
      if (!scheduled) throw new Error("当前构建不允许进程回收模拟");
      toast("当前进程即将结束；请等待已挂载提醒到点", "amber");
    } catch (error) {
      toast(safeUserError(error, "无法启动进程回收模拟"), "danger");
    }
  }, { icon: "alert" });
}
$("reminderProcessDeathBtn").addEventListener("click", requestDebugProcessDeath);

/* ============================== 账号登录（设置 → 云端，solum-cloud 代理） ============================== */
let accountLoggedIn = false;
let accountFormSeeded = false; // 同 llm 表单：只首次 seed，refreshAll 不覆盖正在编辑的值
let accountGateMode = "login";
let accountAuthSubmitting = false;
const GUEST_CHOICE_KEY = "solum.guest-choice.v1";
const AUTH_EXPIRED_REDIRECT_KEY = "solum.auth-expired-redirect.v1";
const GUEST_ALLOWED_VIEWS = new Set(["chat", "agenda", "notifs", "ledger", "ledger-detail", "journal", "search", "settings", "privacy", "capture", "data"]);
const GUEST_ALLOWED_REFRESHERS = new Set([
  refreshAgenda, refreshNotifs, refreshJournal, refreshLedger, refreshPrivacy, refreshToday,
  refreshReminderDeliveryHealth,
]);

function guestChoiceMade() {
  try { return localStorage.getItem(GUEST_CHOICE_KEY) === "guest"; } catch (_) { return false; }
}

function takeAuthExpiredRedirect() {
  try {
    const pending = localStorage.getItem(AUTH_EXPIRED_REDIRECT_KEY) === "1";
    localStorage.removeItem(AUTH_EXPIRED_REDIRECT_KEY);
    return pending;
  } catch (_) { return false; }
}

let accountExpiryFlight = null;
function handleAccountExpiry(error, command) {
  if (accountExpiryFlight || !accountLoggedIn) return accountExpiryFlight || Promise.resolve();
  accountLoggedIn = false;
  invalidateIpcCache();
  safeLocalSet(AUTH_EXPIRED_REDIRECT_KEY, "1");
  try { localStorage.removeItem(GUEST_CHOICE_KEY); } catch (_) { /* login gate still opens immediately */ }
  renderShellIdentity({ logged_in: false });
  setAccountGateMode("login");
  closeMobileTools();
  console.info("Solum account session expired", command, String(error || ""));
  toast("登录凭据已失效，正在为您重置安全会话…", "danger", 3500);
  accountExpiryFlight = rawInvoke("account_logout")
    .catch(() => null)
    .finally(() => {
      accountExpiryFlight = null;
      // 桌面端或无需重启进程环境下的平滑兜底
      openAccountGate("登录状态已过期，请重新登录", { dismissible: false, allowGuest: true });
      $("accountGateStatus").textContent = "登录状态已失效，请重新登录。";
    });
  return accountExpiryFlight;
}

function setAccountGateMode(mode) {
  accountGateMode = mode === "register" ? "register" : "login";
  const registering = accountGateMode === "register";
  $("accountGateLoginTab").setAttribute("aria-selected", registering ? "false" : "true");
  $("accountGateRegisterTab").setAttribute("aria-selected", registering ? "true" : "false");
  $("accountGateModeTitle").textContent = registering ? "创建你的空间" : "欢迎回来";
  $("accountGateSubmit").textContent = registering ? "注册并进入" : "登录";
  $("accountGatePass").autocomplete = registering ? "new-password" : "current-password";
  $("accountGateConfirmField").hidden = !registering;
  $("accountGateConfirm").required = registering;
  $("accountGateConfirm").value = "";
  $("accountGateStatus").textContent = "";
}

function openAccountGate(reason, options) {
  if (accountLoggedIn) return;
  options = options || {};
  $("accountGateReason").textContent = reason || "登录完整使用 Solum";
  $("accountGateClose").hidden = !options.dismissible;
  $("accountGuestBtn").hidden = !options.allowGuest;
  $("accountGate").hidden = false;
  document.body.classList.add("account-gate-open");
  syncOverlayHistory();
  requestAnimationFrame(() => {
    const shell = $("accountGate").querySelector(".account-gate-shell");
    if (!motionPreference.matches && shell && typeof shell.animate === "function") {
      shell.animate([
        { opacity: 0, transform: "translateY(18px) scale(.992)" },
        { opacity: 1, transform: "translateY(0) scale(1)" },
      ], { duration: 420, easing: "cubic-bezier(.22,1,.36,1)" });
    }
    $("accountGateUser").focus();
  });
}

function closeAccountGate() {
  if ($("accountGate").hidden) return;
  $("accountGate").hidden = true;
  document.body.classList.remove("account-gate-open");
  $("accountGateStatus").textContent = "";
  syncOverlayHistory();
}

function requireAccount(reason) {
  if (accountLoggedIn) return true;
  closeMobileTools();
  openAccountGate(reason || "这项能力需要 Solum 账号", { dismissible: true, allowGuest: false });
  return false;
}


/* =========================================================================
   Solum Profile Customization & Feature Info System
   ========================================================================= */
const PROFILE_STORAGE_KEY = "solum.user-profile.v1";
const DEFAULT_AVATAR_SRC = "assets/avatars/default-sprout.png";
const MAX_AVATAR_INPUT_BYTES = 12 * 1024 * 1024;
const MAX_AVATAR_STORED_BYTES = 220 * 1024;
const DEFAULT_USER_PROFILE = {
  nickname: "",
  role: "",
  bio: "",
  avatarData: "",
};

function normalizeAvatarData(value) {
  if (typeof value !== "string" || !/^data:image\/(?:png|jpeg|webp);base64,/i.test(value)) return "";
  const encoded = value.slice(value.indexOf(",") + 1);
  const estimatedBytes = Math.ceil(encoded.length * 3 / 4);
  return estimatedBytes <= MAX_AVATAR_STORED_BYTES ? value : "";
}

function normalizeUserProfile(value) {
  const source = value && typeof value === "object" ? value : {};
  return {
    nickname: String(source.nickname || "").trim().slice(0, 24),
    role: String(source.role || "").trim().slice(0, 36),
    bio: String(source.bio || "").trim().slice(0, 100),
    avatarData: normalizeAvatarData(source.avatarData),
  };
}

function loadUserProfile() {
  try {
    const raw = localStorage.getItem(PROFILE_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return normalizeUserProfile(parsed);
    }
  } catch (_) { }
  return { ...DEFAULT_USER_PROFILE };
}

function saveUserProfile(profile) {
  const normalized = normalizeUserProfile(profile);
  try {
    localStorage.setItem(PROFILE_STORAGE_KEY, JSON.stringify(normalized));
  } catch (_) {
    toast("头像保存失败，请换一张更小的图片重试", "danger");
    return false;
  }
  applyUserProfile(normalized);
  return true;
}

function renderAvatarElement(target, profile) {
  if (!target) return;
  const p = normalizeUserProfile(profile);
  const img = el("img", {
    src: p.avatarData || DEFAULT_AVATAR_SRC,
    alt: "",
    draggable: "false",
  });
  if (p.avatarData) img.addEventListener("error", () => { img.src = DEFAULT_AVATAR_SRC; }, { once: true });
  target.replaceChildren(img);
  target.style.removeProperty("background-color");
  target.style.removeProperty("color");
  target.style.removeProperty("border-color");
}

function applyUserProfile(profile) {
  const p = normalizeUserProfile(profile || loadUserProfile());

  const avatars = [
    $("accountShellAvatar"),
    $("accountShellAvatarM"),
    $("profileSheetAvatar"),
  ].filter(Boolean);

  avatars.forEach((avatar) => renderAvatarElement(avatar, p));

  const fallbackRole = (typeof t === "function" ? t("profile.default_role") : "本地空间");
  const fallbackBio = (typeof t === "function" ? t("profile.default_bio") : "记录当下，专注生活");
  const fallbackName = (typeof t === "function" ? t("common.guest") : "访客");

  if ($("profileSheetRole")) $("profileSheetRole").textContent = p.role || fallbackRole;
  if ($("profileSheetBio")) $("profileSheetBio").textContent = p.bio || fallbackBio;
  
  if (!accountLoggedIn) {
    const displayNick = p.nickname || fallbackName;
    if ($("accountShellLabel")) $("accountShellLabel").textContent = displayNick;
    if ($("mobileAccountLabel")) $("mobileAccountLabel").textContent = displayNick;
  }

  document.querySelectorAll(".msg-row.user .msg-avatar").forEach((el) => {
    renderAvatarElement(el, p);
  });
}

let editingProfileDraft = null;

function openProfileEditor() {
  closeMobileTools();
  const current = loadUserProfile();
  editingProfileDraft = { ...current };

  const modal = $("profileEditorModal");
  if (!modal) return;

  if ($("profileInputName")) $("profileInputName").value = current.nickname || "";
  if ($("profileInputRole")) $("profileInputRole").value = current.role || "";
  if ($("profileInputBio")) $("profileInputBio").value = current.bio || "";

  renderProfilePreview();

  modal.hidden = false;
  syncOverlayHistory();
  $("profileInputName")?.focus();
}

function closeProfileEditor() {
  const modal = $("profileEditorModal");
  if (!modal || modal.hidden) return;
  modal.hidden = true;
  syncOverlayHistory();
}

function renderProfilePreview() {
  if (!editingProfileDraft) return;
  const p = editingProfileDraft;
  const fallbackRole = (typeof t === "function" ? t("profile.default_role") : "本地空间");
  const fallbackBio = (typeof t === "function" ? t("profile.default_bio") : "记录当下，专注生活");
  const fallbackName = (typeof t === "function" ? t("common.guest") : "访客");

  renderAvatarElement($("profilePreviewAvatar"), p);
  if ($("profilePreviewName")) $("profilePreviewName").textContent = p.nickname || fallbackName;
  if ($("profilePreviewRole")) $("profilePreviewRole").textContent = p.role || fallbackRole;
  if ($("profilePreviewBio")) $("profilePreviewBio").textContent = p.bio || fallbackBio;
}

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(new Error("图片读取失败"));
    reader.readAsDataURL(blob);
  });
}

function canvasToBlob(canvas, quality) {
  return new Promise((resolve) => canvas.toBlob(resolve, "image/webp", quality));
}

async function decodeAvatarFile(file) {
  if (typeof createImageBitmap === "function") return createImageBitmap(file);
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    image.decoding = "async";
    image.src = url;
    await image.decode();
    return image;
  } finally {
    URL.revokeObjectURL(url);
  }
}

async function compressAvatarFile(file) {
  if (!file || !/^image\/(?:png|jpeg|webp)$/i.test(file.type)) throw new Error("请选择 PNG、JPG 或 WebP 图片");
  if (file.size > MAX_AVATAR_INPUT_BYTES) throw new Error("原图不能超过 12 MB");
  const source = await decodeAvatarFile(file);
  const side = Math.min(source.width, source.height);
  if (!side) throw new Error("无法识别这张图片");
  let outputSize = 320;
  let quality = 0.84;
  let blob = null;
  while (outputSize >= 224) {
    const canvas = document.createElement("canvas");
    canvas.width = outputSize;
    canvas.height = outputSize;
    const context = canvas.getContext("2d", { alpha: false });
    context.fillStyle = "#ffffff";
    context.fillRect(0, 0, outputSize, outputSize);
    const sx = Math.max(0, (source.width - side) / 2);
    const sy = Math.max(0, (source.height - side) / 2);
    context.drawImage(source, sx, sy, side, side, 0, 0, outputSize, outputSize);
    blob = await canvasToBlob(canvas, quality);
    if (blob && blob.size <= MAX_AVATAR_STORED_BYTES) break;
    quality -= 0.12;
    if (quality < 0.46) { outputSize -= 48; quality = 0.76; }
  }
  if (typeof source.close === "function") source.close();
  if (!blob || blob.size > MAX_AVATAR_STORED_BYTES) throw new Error("图片压缩后仍过大，请换一张图片");
  return blobToDataUrl(blob);
}

function setupProfileEditorEvents() {
  $("profileCustomBtn")?.addEventListener("click", openProfileEditor);
  $("profileEditorClose")?.addEventListener("click", closeProfileEditor);
  $("profileEditorCancel")?.addEventListener("click", closeProfileEditor);

  $("profileInputName")?.addEventListener("input", (e) => {
    if (!editingProfileDraft) return;
    editingProfileDraft.nickname = e.target.value.trim();
    renderProfilePreview();
  });
  $("profileInputRole")?.addEventListener("input", (e) => {
    if (!editingProfileDraft) return;
    editingProfileDraft.role = e.target.value.trim();
    renderProfilePreview();
  });
  $("profileInputBio")?.addEventListener("input", (e) => {
    if (!editingProfileDraft) return;
    editingProfileDraft.bio = e.target.value.trim();
    renderProfilePreview();
  });

  $("profileChooseAvatarBtn")?.addEventListener("click", () => $("profileAvatarFile")?.click());
  $("profileAvatarFile")?.addEventListener("change", async (event) => {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (!file || !editingProfileDraft) return;
    const button = $("profileChooseAvatarBtn");
    button.disabled = true;
    try {
      editingProfileDraft.avatarData = await compressAvatarFile(file);
      renderProfilePreview();
      toast("头像已裁切并压缩，保存后生效", "ok");
    } catch (error) {
      toast(String(error), "danger");
    } finally {
      button.disabled = false;
    }
  });
  $("profileDeleteAvatarBtn")?.addEventListener("click", () => {
    if (!editingProfileDraft) return;
    editingProfileDraft.avatarData = "";
    renderProfilePreview();
    toast("已恢复嫩芽默认头像", "ok");
  });

  $("profileEditorSave")?.addEventListener("click", () => {
    if (!editingProfileDraft) return;
    if (!saveUserProfile(editingProfileDraft)) return;
    closeProfileEditor();
    toast("个人资料已更新", "ok");
  });
}

const VIEW_FEATURE_DESCRIPTIONS = {
  descAgenda: {
    title: "日程安排",
    desc: "通过自然语言对话智能抽取您的工作、会议与生活安排，按当前时间清晰呈现。\n\n• 支持按今天、明天与未来时间段分层归集\n• 到期后自动联动系统提醒与日历\n• 全程在本地安全离线存储",
  },
  descNotifs: {
    title: "提醒与通知",
    desc: "智能分级提醒与通知生命周期管理。\n\n• 待触发：到点前按设定规则发送弹窗与振动提醒\n• 已触发：历史通知归档可随时回看\n• 离线保障：即使未连接网络，系统底层闹钟仍准时送达",
  },
  descSuggest: {
    title: "生活建议",
    desc: "息壤结合您的作息规律与近期日程，在合适时刻给出的贴心建议。\n\n• 考试备考：提前规划复习备考节奏\n• 截止赶工：临近截止事项重点置顶\n• 早会早休：根据次日首个日程提示休息时间",
  },
  descJournal: {
    title: "行为足迹",
    desc: "自动汇总您的日常互动、打卡、问询与状态记录，形成温润的生活轨迹。\n\n• 记录每一步专注与生活足迹\n• 支持随时回顾与导出\n• 数据仅在本地保存",
  },
  descWearable: {
    title: "健康趋势",
    desc: "本地只读接入系统健康传感器，汇总心率、步数与睡眠 7 日趋势。\n\n• 纯本地计算，健康数据绝不上传云端\n• 辅助调整生活建议与作息提醒",
  },
  descReview: {
    title: "复盘简报",
    desc: "阶段性智能总结，助您轻松看清过去一段时间的节奏与成效。\n\n• 自动梳理已完成事件与待跟进事项\n• 契合您设定的助手风格口吻输出",
  },
  descLedger: {
    title: "记忆库",
    desc: "息壤为您沉淀的专属记忆网络。\n\n• 事实记忆：记住您的偏好、重要人物与习惯\n• 来源透明：每条记忆均清晰标注来源出处\n• 随时掌控：支持随时修改纠正或一键抹除",
  },
  descCaptures: {
    title: "通知回看",
    desc: "静默捕获的手机通知集中在此，智能分级过滤垃圾信息，提取出行、订单与待办。\n\n• 避免打扰：无需频繁亮屏看手机\n• 智能提炼：关键信息一键提取为日程",
  },
  descPersona: {
    title: "数字人格",
    desc: "定义息壤的对话口吻、称呼与表达性格。\n\n• 支持从日常聊天记录中安全导入表达习惯\n• 历史版本随时可一键回退",
  },
  descProactivity: {
    title: "智能主动度",
    desc: "调节息壤在不同维度的关怀与提醒频率。\n\n• 被动模式：仅在您询问时回答\n• 秘书模式：在关键节点适度提醒\n• 管家模式：主动跟进生活与工作事项",
  },
  descPrivacy: {
    title: "隐私与数据安全",
    desc: "息壤坚持「本地优先、端到端加密」原则。\n\n• 数据驻留：聊天原文、记忆与行为数据默认仅存在本机\n• 加密同步：开启多端同步时在本地高强度加密后传输\n• 权限透明：敏感权限操作必须人工确认",
  },
  descCapture: {
    title: "灵感与数据入口",
    desc: "多渠道汇集外部信息。\n\n• 文本速记、语音输入、截图 OCR、系统日历\n• 先进入待确认暂存区，确认后再入库",
  },
  descSync: {
    title: "多端加密同步",
    desc: "连接您的多台电脑与手机，数据无缝漫游。\n\n• 采用端到端高强度密钥加密\n• 只有您本人的设备拥有解密钥匙",
  },
  descBackup: {
    title: "备份与恢复",
    desc: "为您的所有数据创建安全快照副本。\n\n• 一键备份本地全部日程、记忆与资料\n• 支持随时随地完整导入恢复",
  },
  descEmail: {
    title: "邮箱关键事件",
    desc: "在本地直接读取邮件头，提取机票、高铁、酒店、账单等重要事件。\n\n• 邮件正文不上传，不提供给第三方\n• 提取的事件先入待确认区供您审阅",
  },
  descCloud: {
    title: "云端服务接入",
    desc: "连接高性能云端大模型，获得更强大的自然语言理解与智能推理能力。\n\n• 离线自动降级：无网络时本地规则库无缝承接\n• 流量与调用额度实时透明呈现",
  },
  descRules: {
    title: "重要度与提醒规则",
    desc: "自定义不同类型日程的提醒策略。\n\n• 会议提前 15 分钟、考试提前 1 天等\n• 自动匹配所有新建日程",
  },
  descGuard: {
    title: "安全防护与护栏",
    desc: "守护您的设备与个人数据安全的终极防线。\n\n• 所有高危指令与敏感操作（如删除、外发）均被自动拦截\n• 必须由您在弹窗中确认后签发一次性授权凭证方可执行",
  },
};

function showInfoModal(title, text) {
  const modal = $("infoModal");
  if (!modal) return;
  $("infoModalTitle").textContent = title || "功能说明";
  $("infoModalBody").replaceChildren(
    ...text.split("\n\n").map((para) => el("p", { style: "margin:0 0 10px;line-height:1.7;", text: para }))
  );
  modal.hidden = false;
  syncOverlayHistory();
}

function closeInfoModal() {
  const modal = $("infoModal");
  if (!modal || modal.hidden) return;
  modal.hidden = true;
  syncOverlayHistory();
}

function setupInfoModalTriggers() {
  $("infoModalClose")?.addEventListener("click", closeInfoModal);
  $("infoModalConfirm")?.addEventListener("click", closeInfoModal);

  document.querySelectorAll(".qmark").forEach((btn) => {
    const descId = btn.getAttribute("aria-controls");
    const info = VIEW_FEATURE_DESCRIPTIONS[descId];
    if (info) {
      btn.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        showInfoModal(info.title, info.desc);
      });
    }
  });
}

function setupThemeToggle() {
  const toggleBtn = $("themeToggleBtn");
  const iconSpan = $("themeToggleIcon");
  const labelSpan = $("themeToggleLabel");
  if (!toggleBtn) return;

  const currentTheme = localStorage.getItem("solum.theme") || "system";

  function applyTheme(theme) {
    if (theme === "dark") {
      document.documentElement.setAttribute("data-theme", "dark");
      document.documentElement.style.colorScheme = "dark";
      if (iconSpan) iconSpan.replaceChildren(icon("clock", 16));
      if (labelSpan) labelSpan.textContent = typeof t === "function" ? t("theme.dark", null, "深色模式") : "深色模式";
    } else if (theme === "light") {
      document.documentElement.setAttribute("data-theme", "light");
      document.documentElement.style.colorScheme = "light";
      if (iconSpan) iconSpan.replaceChildren(icon("bulb", 16));
      if (labelSpan) labelSpan.textContent = typeof t === "function" ? t("theme.light", null, "浅色模式") : "浅色模式";
    } else {
      document.documentElement.removeAttribute("data-theme");
      document.documentElement.style.colorScheme = "light dark";
      if (iconSpan) iconSpan.replaceChildren(icon("refresh", 16));
      if (labelSpan) labelSpan.textContent = typeof t === "function" ? t("theme.system", null, "跟随系统") : "跟随系统";
    }
    localStorage.setItem("solum.theme", theme);
  }

  applyTheme(currentTheme);

  toggleBtn.addEventListener("click", () => {
    const cur = localStorage.getItem("solum.theme") || "system";
    const next = cur === "system" ? "dark" : cur === "dark" ? "light" : "system";
    applyTheme(next);
    const themeName = next === "dark"
      ? (typeof t === "function" ? t("theme.dark") : "深色")
      : next === "light"
      ? (typeof t === "function" ? t("theme.light") : "浅色")
      : (typeof t === "function" ? t("theme.system") : "跟随系统");
    const msg = typeof t === "function" ? t("theme.switched", { theme: themeName }) : "外观已切换为：" + themeName;
    toast(msg, "ok");
  });
}

function renderShellIdentity(s) {
  const loggedIn = !!s.logged_in;
  const profile = loadUserProfile();
  const name = loggedIn ? (s.username || profile.nickname) : profile.nickname;
  const meta = loggedIn ? "已连接 · 自动同步" : "仅保存在本机";
  $("accountShellLabel").textContent = name;
  $("accountShellMeta").textContent = meta;
  $("accountShellAction").textContent = loggedIn ? "管理" : "登录";
  $("mobileAccountLabel").textContent = name;
  $("mobileAccountMeta").textContent = meta;
  $("mobileAccountBtn").textContent = loggedIn ? "管理" : "登录";
  if ($("profileSheetRole")) $("profileSheetRole").textContent = profile.role || (loggedIn ? "已认证成员" : "本地空间");
  if ($("profileSheetBio")) $("profileSheetBio").textContent = profile.bio || "专注当下，记录生活";
  if ($("profileSheetPlanTitle")) $("profileSheetPlanTitle").textContent = loggedIn ? "标准版权益" : "本地访客空间";
  if ($("profileSheetPlanDesc")) $("profileSheetPlanDesc").textContent = loggedIn ? "多端加密同步已就绪" : "数据仅保存在这台设备";
  
  applyUserProfile(profile);
  
  document.body.dataset.account = loggedIn ? "member" : "guest";
  document.querySelectorAll(".utility-btn[data-requires-account='true']").forEach((button) => {
    button.classList.toggle("locked", !loggedIn);
  });
}

function renderAccountStatus(s) {
  const box = $("accountStatus");
  box.textContent = "";
  if (s.logged_in) {
    box.appendChild(el("div", { class: "statusline", title: `会话文件：${s.path}（已 gitignore，不入库、不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `已登录：${s.username} @ ${s.server_url} · ${s.model}` }),
    ]));
    box.appendChild(el("div", { class: "hint", style: "margin:4px 0 0 16px;",
      text: "登录期间云端走账号代理；退出后回到纯本地访客空间。" }));
    if (s.usage) {
      const usage = s.usage;
      const percent = usage.monthly_request_limit > 0
        ? Math.min(100, Math.round(usage.requests_used * 100 / usage.monthly_request_limit)) : 0;
      const reset = usage.reset_at ? new Date(usage.reset_at).toLocaleDateString("zh-CN", {
        month: "long", day: "numeric",
      }) : "下个周期";
      const latest = usage.latest_request;
      let latestText = "还没有云端调用记录";
      if (latest) {
        latestText = `最近实际使用 ${latest.effective_model}`;
        if (latest.fallback_occurred) latestText += ` · 已从 ${latest.requested_model} 回退`;
      }
      box.appendChild(el("div", { class: "account-ai-status" }, [
        el("div", { class: "account-ai-head" }, [
          el("strong", { text: `${usage.plan} 权益` }),
          el("span", { text: `${usage.requests_remaining} 次可用` }),
        ]),
        el("div", { class: "account-quota-track", role: "progressbar", "aria-label": "本月云端调用额度",
          "aria-valuemin": "0", "aria-valuemax": String(usage.monthly_request_limit),
          "aria-valuenow": String(usage.requests_used) }, [
          el("div", { class: "account-quota-fill", style: `--quota-percent:${percent}%` }),
        ]),
        el("div", { class: "account-ai-meta" }, [
          el("span", { text: `已用 ${usage.requests_used} / ${usage.monthly_request_limit}` }),
          el("span", { text: `${reset} 重置` }),
        ]),
        el("div", { class: "hint", text: latestText }),
        latest && latest.fallback_occurred && latest.fallback_reason
          ? el("div", { class: "hint", text: `回退原因：${fallbackReasonLabel(latest.fallback_reason)}` }) : null,
      ].filter(Boolean)));
    } else if (s.state_error) {
      box.appendChild(el("div", { class: "statusline", title: s.state_error }, [
        el("span", { class: "sdot amber" }),
        el("span", { class: "t", text: "模型目录和额度暂时无法刷新；默认模型仍可继续使用。" }),
      ]));
    }
  }
}

function fallbackReasonLabel(reason) {
  if (reason === "model_not_allowed") return "所选模型不在中心目录";
  if (reason === "custom_models_disabled") return "当前权益只允许默认模型";
  if (reason === "upstream_unavailable") return "所选模型服务暂不可用";
  if (reason.startsWith("upstream_http_")) return `所选模型服务返回 ${reason.slice(14)}`;
  return reason;
}

async function refreshAccount() {
  let s;
  try { s = await invoke("account_status_get"); } catch (_) { return; }
  accountLoggedIn = !!s.logged_in;
  if (accountLoggedIn && isAccountExpiryError(s.state_error)) {
    await handleAccountExpiry(s.state_error, "account_status_get");
    return;
  }
  renderShellIdentity(s);
  renderAccountStatus(s);
  $("accountLoginForm").hidden = accountLoggedIn;
  $("accountLoginBtn").hidden = accountLoggedIn;
  $("accountRegisterBtn").hidden = accountLoggedIn;
  $("accountLogoutBtn").hidden = !accountLoggedIn;
  $("accountModelBtn").hidden = !accountLoggedIn;
  $("accountSecurityArea").hidden = !accountLoggedIn;
  if (accountLoggedIn) {
    setRecoverMode(false);
    refreshRecoveryCodeStatus();
  } else {
    $("accountRecoverToggle").hidden = false;
  }
  if (accountLoggedIn) $("accountFold").open = true;
  if (s.default_server_url) solumCenterUrl = s.default_server_url;
  const modelSelect = $("accountModel");
  const currentValue = modelSelect.value;
  modelSelect.textContent = "";
  const defaultModel = s.default_model || solumActiveModel;
  solumActiveModel = defaultModel;
  const options = Array.from(new Set([defaultModel, ...(s.model_options || [])]));
  for (const model of options) modelSelect.appendChild(el("option", {
    value: model, text: model === defaultModel ? `${model}（默认）` : model,
  }));
  if (accountFormSeeded && options.includes(currentValue)) modelSelect.value = currentValue;
  $("accountModelNote").replaceChildren(
    icon("shield", 15),
    el("span", { text: `默认模型 ${defaultModel} 始终保留。所选模型在回复开始前失败时，中心会自动回退默认模型，并在这里说明实际使用的模型与原因。` }),
  );
  if (accountFormSeeded) return;
  accountFormSeeded = true;
  if (s.logged_in) {
    $("accountServer").value = s.server_url;
    $("accountUser").value = s.username;
  } else {
    $("accountServer").value = s.server_url || s.default_server_url || solumCenterUrl;
  }
  $("accountModel").value = options.includes(s.model) ? s.model : defaultModel;
}

async function submitSettingsLogin() {
  if (accountAuthSubmitting || $("accountLoginBtn").disabled) return;
  const serverUrl = $("accountServer").value.trim() || solumCenterUrl;
  const username = $("accountUser").value.trim();
  const password = $("accountPass").value;
  if (!username || !password) return toast("请填写账号和密码", "danger");
  accountAuthSubmitting = true;
  $("accountLoginBtn").disabled = true;
  $("accountRegisterBtn").disabled = true;
  try {
    const summary = await invoke("account_login", {
      serverUrl, username, password, model: $("accountModel").value.trim(),
    });
    $("accountPass").value = ""; // 密码只用于本次登录与本地解封，界面也不留
    toast("已登录，云端改走账号代理", "ok");
    $("llmDot").className = "sdot ok";
    $("llmText").textContent = "云端在线";
    $("llmRow").title = summary;
    accountFormSeeded = false;
    refreshAccount(); refreshCloud(); refreshSyncConfig(); refreshSyncStatus();
  } catch (e) { toast(safeUserError(e, "云端配置未保存，请稍后重试。"), "danger"); }
  finally {
    accountAuthSubmitting = false;
    $("accountLoginBtn").disabled = false;
    $("accountRegisterBtn").disabled = false;
  }
}

async function submitAccountGate() {
  if (accountAuthSubmitting || $("accountGateSubmit").disabled) return;
  const username = $("accountGateUser").value.trim();
  const password = $("accountGatePass").value;
  const registering = accountGateMode === "register";
  if (!username || !password) {
    $("accountGateStatus").textContent = "请填写账号和密码。";
    return;
  }
  if (registering && Array.from(password).length < 8) {
    $("accountGateStatus").textContent = "注册密码至少需要 8 个字符。";
    return;
  }
  if (registering && password !== $("accountGateConfirm").value) {
    $("accountGateStatus").textContent = "两次输入的密码不一致。";
    return;
  }
  accountAuthSubmitting = true;
  $("accountGateSubmit").disabled = true;
  $("accountGateSubmit").textContent = registering ? "正在创建…" : "正在登录…";
  $("accountGateStatus").textContent = "正在建立你的加密空间…";
  try {
    await invoke(registering ? "account_register" : "account_login", {
      serverUrl: solumCenterUrl,
      username,
      password,
      model: solumActiveModel,
    });
    $("accountGatePass").value = "";
    $("accountGateConfirm").value = "";
    $("accountGateStatus").textContent = "身份已确认，正在进入账号空间…";
  } catch (e) {
    const rawMessage = String(e);
    const message = safeUserError(e, registering ? "注册未完成，请检查网络后重试。" : "登录未完成，请检查网络后重试。");
    if (registering && rawMessage.startsWith("账号已创建，但")) {
      setAccountGateMode("login");
      $("accountGatePass").value = "";
    }
    $("accountGateStatus").textContent = message;
    $("accountGateSubmit").disabled = false;
    $("accountGateSubmit").textContent = registering ? "注册并进入" : "登录";
  } finally {
    accountAuthSubmitting = false;
  }
}

function setupPasswordToggles() {
  document.querySelectorAll(".password-wrap").forEach((wrap) => {
    const input = wrap.querySelector("input");
    const toggleBtn = wrap.querySelector(".btn-eye");
    if (!input || !toggleBtn) return;
    if (toggleBtn.dataset.bound === "true") return;
    toggleBtn.dataset.bound = "true";
    toggleBtn.replaceChildren(icon("eye", 16));
    toggleBtn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const isPass = input.type === "password";
      input.type = isPass ? "text" : "password";
      toggleBtn.replaceChildren(icon(isPass ? "eye_off" : "eye", 16));
      toggleBtn.setAttribute("aria-label", isPass ? "隐藏密码" : "显示密码");
      toggleBtn.setAttribute("title", isPass ? "隐藏密码" : "显示密码");
    });
  });
}

setupPasswordToggles();
applyUserProfile();
setupProfileEditorEvents();
setupInfoModalTriggers();
setupThemeToggle();
if ($("profileSheetSyncBtn")) {
  $("profileSheetSyncBtn").appendChild(icon("refresh", 16));
  $("profileSheetSyncBtn").addEventListener("click", () => {
    syncManual();
    toast("正在同步最新数据…", "sync");
  });
}
if ($("profileSheetStatusIcon")) {
  $("profileSheetStatusIcon").appendChild(icon("cloud", 20));
}


$("accountGatePrivacyIcon").appendChild(icon("lock", 20));
$("accountGateClose").appendChild(icon("x", 18));
$("accountGateLoginTab").addEventListener("click", () => setAccountGateMode("login"));
$("accountGateRegisterTab").addEventListener("click", () => setAccountGateMode("register"));
$("accountGateSubmit").addEventListener("click", submitAccountGate);
$("accountGatePass").addEventListener("keydown", (event) => {
  if (event.key === "Enter") { event.preventDefault(); submitAccountGate(); }
});
$("accountGateConfirm").addEventListener("keydown", (event) => {
  if (event.key === "Enter") { event.preventDefault(); submitAccountGate(); }
});
$("accountGateClose").addEventListener("click", closeAccountGate);
$("accountGateOAuthBtn").addEventListener("click", startOAuthLogin);
$("accountOAuthBtn").addEventListener("click", startOAuthLogin);
$("accountGuestBtn").addEventListener("click", () => {
  try { localStorage.setItem(GUEST_CHOICE_KEY, "guest"); } catch (_) { }
  closeAccountGate();
  toast("已进入访客空间，数据只保存在本机", "ok");
});
$("accountShellBtn").addEventListener("click", () => {
  if (accountLoggedIn) showView("cloud");
  else openAccountGate("登录后连接所有设备与完整能力", { dismissible: true, allowGuest: false });
});
$("mobileAccountBtn").addEventListener("click", () => {
  closeMobileTools();
  if (accountLoggedIn) showView("cloud");
  else openAccountGate("登录后连接所有设备与完整能力", { dismissible: true, allowGuest: false });
});

function promptModal(title, message, placeholder, defaultVal) {
  return new Promise((resolve) => {
    let resolved = false;
    const finish = (val) => {
      if (resolved) return;
      resolved = true;
      resolve(val);
    };
    const input = el("input", {
      type: "text",
      class: "input",
      placeholder: placeholder || "",
      value: defaultVal || "",
      style: "width:100%;margin-top:12px;box-sizing:border-box;",
    });
    const clipBtn = el("button", {
      type: "button",
      class: "btn ghost",
      text: "读取剪贴板",
      style: "margin-top:8px;font-size:12px;",
      click: async () => {
        try {
          const text = await navigator.clipboard.readText();
          if (text) input.value = text.trim();
        } catch (_) {
          toast("请长按输入框粘贴", "info");
        }
      }
    });
    const body = el("div", {}, [
      el("p", { text: message, style: "white-space:pre-wrap;line-height:1.5;color:var(--text-muted);" }),
      input,
      el("div", { style: "display:flex;justify-content:flex-end;" }, [clipBtn])
    ]);
    openModal(
      title,
      body,
      "确认",
      () => finish(input.value.trim()),
      {
        okStyle: "primary",
        onDismiss: () => finish(null),
      }
    );
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        closeModal();
        finish(input.value.trim());
      }
    });
    setTimeout(() => input.focus(), 80);
  });
}

async function startOAuthLogin() {
  if (accountAuthSubmitting) return;
  const serverUrl = solumCenterUrl || "https://solum.tannerlab.cn";
  let autoCheckTimer = null;
  let onVisibilityChange = null;
  try {
    accountAuthSubmitting = true;
    toast("正在连接统一认证中心…", "info");
    const cfg = await invoke("account_oauth_config_get", { serverUrl });
    if (!cfg.enabled) {
      toast("中心服务器未启用 OAuth 登录", "danger");
      return;
    }
    const pkce = await invoke("account_oauth_pkce_generate");
    const redirectUri = `${cfg.issuer.replace(/\/+$/, '')}/callback`;
    const state = Math.random().toString(36).slice(2);
    const authUrl = `${cfg.issuer.replace(/\/+$/, '')}/authorize?response_type=code&client_id=${encodeURIComponent(cfg.client_id)}&redirect_uri=${encodeURIComponent(redirectUri)}&scope=openid%20profile%20email&code_challenge=${encodeURIComponent(pkce.code_challenge)}&code_challenge_method=S256&state=${state}`;

    try {
      await invoke("open_external_url", { url: authUrl });
    } catch (_) {
      window.open(authUrl, "_blank");
    }

    toast("已在浏览器打开授权页面，授权后将自动返回登录…", "info");

    let codePromiseResolve = null;
    const codePromise = new Promise((resolve) => {
      codePromiseResolve = resolve;
    });

    const checkDeepLink = async () => {
      try {
        const items = await invoke("account_oauth_android_drain");
        if (Array.isArray(items) && items.length > 0) {
          const match = items.find((it) => it && (it.code || it.error));
          if (match) {
            if (match.error) {
              toast(`OAuth 授权失败: ${match.error}`, "danger");
              if (codePromiseResolve) codePromiseResolve(null);
            } else if (match.code) {
              if (codePromiseResolve) codePromiseResolve(match.code.trim());
            }
          }
        }
      } catch (_) {}
    };

    autoCheckTimer = setInterval(checkDeepLink, 800);
    onVisibilityChange = () => {
      if (document.visibilityState === "visible") checkDeepLink();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("focus", onVisibilityChange);

    const modalPromptPromise = promptModal(
      "Tg-oauth 统一登录",
      "已在系统外部浏览器打开授权页面。\n完成授权后将自动返回；如未自动跳转，可在此粘贴授权码：",
      "授权后自动登录，或在此粘贴授权码（Code）"
    );

    modalPromptPromise.then((manualCode) => {
      if (manualCode && codePromiseResolve) {
        codePromiseResolve(manualCode.trim());
      }
    });

    const code = await Promise.race([codePromise, modalPromptPromise]);

    if (autoCheckTimer) { clearInterval(autoCheckTimer); autoCheckTimer = null; }
    if (onVisibilityChange) {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("focus", onVisibilityChange);
      onVisibilityChange = null;
    }
    closeModal();

    if (!code || !code.trim()) {
      toast("已取消 OAuth 登录", "info");
      return;
    }

    let recoveryCode = null;
    let attempt = 0;
    while (attempt < 2) {
      attempt++;
      try {
        await invoke("account_oauth_login", {
          args: {
            server_url: serverUrl,
            code: code.trim(),
            code_verifier: pkce.code_verifier,
            redirect_uri: redirectUri,
            model: $("accountModel")?.value?.trim() || solumActiveModel,
            recovery_code: recoveryCode,
          },
        });
        toast("使用 Tg-oauth 统一账号登录成功", "ok");
        if (typeof closeAccountGate === 'function') closeAccountGate();
        accountFormSeeded = false;
        await refreshAccount();
        await refreshCloud();
        await refreshSyncConfig();
        await refreshSyncStatus();
        break;
      } catch (e) {
        const errStr = String(e);
        if (errStr.includes("NEEDS_RECOVERY_CODE") || errStr.includes("恢复码")) {
          recoveryCode = await promptModal(
            "输入数据恢复码",
            "检测到该账号已有加密数据空间。\n请输入您的 20 字符数据恢复码（格式如 XXXXX-XXXXX-XXXXX-XXXXX）以解密：",
            "XXXXX-XXXXX-XXXXX-XXXXX"
          );
          if (!recoveryCode || !recoveryCode.trim()) {
            toast("未输入恢复码，无法解密现有数据空间", "danger");
            break;
          }
          continue;
        } else {
          toast(safeUserError(e, "OAuth 登录失败，请重试。"), "danger");
          break;
        }
      }
    }
  } catch (e) {
    toast(safeUserError(e, "无法获取 OAuth 配置"), "danger");
  } finally {
    if (autoCheckTimer) clearInterval(autoCheckTimer);
    if (onVisibilityChange) {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("focus", onVisibilityChange);
    }
    accountAuthSubmitting = false;
  }
}

$("accountLoginBtn").addEventListener("click", async () => {
  await submitSettingsLogin();
});

$("accountRegisterBtn").addEventListener("click", () => {
  setAccountGateMode("register");
  $("accountGateUser").value = $("accountUser").value.trim();
  openAccountGate("创建账号并进入独立的加密空间", { dismissible: true, allowGuest: false });
});

$("accountLogoutBtn").addEventListener("click", async () => {
  $("accountLogoutBtn").disabled = true;
  try {
    const message = await invoke("account_logout");
    toast(message, "ok");
    // 退出后云端状态取决于是否还有直连配置，页脚交给 refresh 按真实状态刷。
    accountFormSeeded = false;
    llmFormSeeded = false;
    await refreshAccount(); await refreshCloud();
    syncFormSeeded = false;
    await refreshSyncConfig(); await refreshSyncStatus();
    const on = !accountLoggedIn && !!llmKeyTail;
    $("llmDot").className = "sdot" + (on ? " ok" : "");
    $("llmText").textContent = on ? "云端在线" : "离线模式";
  } catch (e) { toast(String(e), "danger"); }
  finally { $("accountLogoutBtn").disabled = false; }
});

$("accountModelBtn").addEventListener("click", async () => {
  $("accountModelBtn").disabled = true;
  try {
    const summary = await invoke("account_model_save", { model: $("accountModel").value.trim() });
    toast("已保存模型", "ok");
    $("llmRow").title = summary;
    accountFormSeeded = false;
    refreshAccount();
  } catch (e) { toast(safeUserError(e, "模型未保存，请稍后重试。"), "danger"); }
  finally { $("accountModelBtn").disabled = false; }
});

/* ============================== 恢复码与改密 ============================== */
let recoveryCodeChecking = false;

function setRecoverMode(on) {
  $("accountLoginForm").hidden = on || accountLoggedIn;
  $("accountRecoverForm").hidden = !on;
  $("accountRecoverToggle").hidden = on || accountLoggedIn;
}

async function refreshRecoveryCodeStatus() {
  if (!accountLoggedIn || recoveryCodeChecking) return;
  recoveryCodeChecking = true;
  const dot = $("recoveryCodeDot");
  const text = $("recoveryCodeStatusText");
  try {
    const state = await invoke("recovery_code_status");
    if (state.configured) {
      dot.className = "sdot ok";
      text.textContent = "已设置恢复码：忘记密码时可用它登录并找回数据。重新生成会使旧码失效。";
      $("recoveryCodeIssueBtn").textContent = "重新生成恢复码";
    } else {
      dot.className = "sdot amber";
      text.textContent = "还没有恢复码：如果忘记密码，将没有任何办法找回加密数据。建议现在生成一个。";
      $("recoveryCodeIssueBtn").textContent = "生成恢复码";
    }
  } catch (_) {
    dot.className = "sdot";
    text.textContent = "恢复码状态暂时无法确认，可稍后再试。";
  } finally {
    recoveryCodeChecking = false;
  }
}

$("recoveryCodeIssueBtn").addEventListener("click", async () => {
  const btn = $("recoveryCodeIssueBtn");
  if (btn.disabled) return;
  const rotating = btn.textContent.includes("重新生成");
  const confirmed = window.confirm(rotating
    ? "重新生成会让旧恢复码立即失效。确定继续吗？"
    : "恢复码只显示这一次，请准备好离线抄写的地方（纸笔或密码管理器）。继续吗？");
  if (!confirmed) return;
  btn.disabled = true;
  try {
    const code = await invoke("recovery_code_issue");
    const out = $("recoveryCodeResult");
    out.hidden = false;
    out.replaceChildren(
      el("div", { class: "hint", style: "margin:8px 0 4px;font-weight:600;",
        text: "请立即离线抄写下面的恢复码，关闭后将无法再次查看：" }),
      el("div", { style: "font-family:var(--mono,monospace);font-size:1.05rem;", text: code }),
      el("div", { class: "hint", style: "margin-top:4px;",
        text: "息壤 与中心都无法找回这个码；丢失它且忘记密码时，加密数据将无法解开。" }),
    );
    toast("恢复码已生成", "ok");
    refreshRecoveryCodeStatus();
  } catch (e) { toast(safeUserError(e, "恢复码生成失败，请稍后重试。"), "danger"); }
  finally { btn.disabled = false; }
});

$("accountChangePassBtn").addEventListener("click", () => {
  const form = $("accountChangePassForm");
  form.hidden = !form.hidden;
  if (!form.hidden) $("accountCurrentPass").focus();
});

$("accountChangePassCancel").addEventListener("click", () => {
  $("accountChangePassForm").hidden = true;
  $("accountCurrentPass").value = "";
  $("accountNewPass").value = "";
});

$("accountChangePassSubmit").addEventListener("click", async () => {
  const btn = $("accountChangePassSubmit");
  if (btn.disabled) return;
  const currentPassword = $("accountCurrentPass").value;
  const newPassword = $("accountNewPass").value;
  if (!currentPassword) return toast("请输入当前密码", "danger");
  if (Array.from(newPassword).length < 8) return toast("新密码至少需要 8 个字符", "danger");
  btn.disabled = true;
  try {
    const message = await invoke("account_change_password", { currentPassword, newPassword });
    $("accountChangePassForm").hidden = true;
    $("accountCurrentPass").value = "";
    $("accountNewPass").value = "";
    toast(message, "ok");
  } catch (e) { toast(safeUserError(e, "密码没有修改，请稍后重试。"), "danger"); }
  finally { btn.disabled = false; }
});

$("accountRecoverToggle").addEventListener("click", () => setRecoverMode(true));
$("accountRecoverCancel").addEventListener("click", () => {
  $("accountRecoverCode").value = "";
  setRecoverMode(false);
});

$("accountRecoverSubmit").addEventListener("click", async () => {
  const btn = $("accountRecoverSubmit");
  if (btn.disabled || accountAuthSubmitting) return;
  const serverUrl = $("accountServer").value.trim() || solumCenterUrl;
  const username = $("accountRecoverUser").value.trim();
  const code = $("accountRecoverCode").value.trim();
  if (!username || !code) return toast("请填写账号和恢复码", "danger");
  btn.disabled = true;
  accountAuthSubmitting = true;
  try {
    const summary = await invoke("recovery_code_login", { serverUrl, username, code });
    $("accountRecoverCode").value = "";
    toast(summary || "已用恢复码登录", "ok");
    accountFormSeeded = false;
    refreshAccount(); refreshCloud(); refreshSyncConfig(); refreshSyncStatus();
  } catch (e) {
    toast(safeUserError(e, "恢复码登录失败，请稍后重试。"), "danger");
  } finally {
    btn.disabled = false;
    accountAuthSubmitting = false;
  }
});

/* ============================== Soulous（设置 → 云端） ============================== */
let soulousAccessTail = "";
let soulousRefreshTail = "";
let soulousFormSeeded = false;

function renderSoulousStatus(s) {
  const box = $("soulousStatus");
  box.textContent = "";
  const c = s.cache || {};
  if (s.configured) {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}（本地、gitignore、不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `已配置：${s.server_url} · access:${s.access_token_tail} · refresh:${s.refresh_token_tail}` }),
    ]));
  } else {
    box.appendChild(el("div", { class: "statusline", title: `首次保存会写入：${s.path}` }, [
      el("span", { class: "sdot" }),
      el("span", { class: "t", text: "未配置：Soulous 数据源保持静默关闭。" }),
    ]));
  }
  box.appendChild(el("div", { class: "statusline" }, [
    el("span", { class: "sdot" + (s.push_schedule_events ? " amber" : "") }),
    el("span", { text: s.push_schedule_events
      ? "出站白名单：日程事件已授权；仍须逐次人工确认。"
      : "出站白名单：全部关闭；不会向 Soulous 推送 息壤 数据。" }),
  ]));
  box.appendChild(el("div", { class: "statusline" }, [
    el("span", { class: "sdot" + (c.total ? " ok" : "") }),
    el("span", { text: c.total
      ? `缓存：课表 ${c.courses} 条、考试 ${c.exams} 场、任务 ${c.tasks} 项、打卡快照 ${c.checkins} 条、专注 ${c.focus_sessions} 段；最近成功 ${fmtDTText(c.last_success_at)}`
      : "尚无成功拉取的本地缓存。" }),
  ]));
}

async function refreshSoulous() {
  let s;
  try { s = await invoke("soulous_config_get", { now: nowParam() }); } catch (_) { return; }
  soulousAccessTail = s.access_token_tail || "";
  soulousRefreshTail = s.refresh_token_tail || "";
  renderSoulousStatus(s);
  $("soulousAccess").placeholder = soulousAccessTail ? `已保存（${soulousAccessTail}），留空则沿用` : "首次保存必填";
  $("soulousRefresh").placeholder = soulousRefreshTail ? `已保存（${soulousRefreshTail}），留空则沿用` : "首次保存必填（自动轮换）";
  $("soulousPushScheduleEvents").checked = Boolean(s.push_schedule_events);
  if (soulousFormSeeded) return;
  soulousFormSeeded = true;
  if (s.configured) {
    $("soulousServer").value = s.server_url;
    $("soulousTimeout").value = s.timeout_secs;
  }
}

function soulousCollect() {
  return {
    server_url: $("soulousServer").value.trim(),
    access_token: $("soulousAccess").value.trim(),
    refresh_token: $("soulousRefresh").value.trim(),
    timeout_secs: Math.min(120, Math.max(3, parseInt($("soulousTimeout").value, 10) || 15)),
    push_schedule_events: $("soulousPushScheduleEvents").checked,
  };
}

function soulousValidate(cfg) {
  if (!/^https?:\/\//.test(cfg.server_url)) return "服务器地址需要以 http(s):// 开头";
  if (!cfg.access_token && !soulousAccessTail) return "请填写 access token";
  if (!cfg.refresh_token && !soulousRefreshTail) return "请填写 refresh token";
  return null;
}

$("soulousSaveBtn").addEventListener("click", async () => {
  const cfg = soulousCollect();
  const bad = soulousValidate(cfg);
  if (bad) return toast(bad, "danger");
  $("soulousSaveBtn").disabled = true;
  try {
    await invoke("soulous_config_save", { args: cfg });
    $("soulousAccess").value = "";
    $("soulousRefresh").value = "";
    soulousFormSeeded = false;
    toast("Soulous 本地配置已保存", "ok");
    refreshSoulous();
  } catch (e) { toast(String(e), "danger"); }
  finally { $("soulousSaveBtn").disabled = false; }
});

$("soulousPullBtn").addEventListener("click", async () => {
  const out = $("soulousPullOut");
  out.textContent = "";
  $("soulousPullBtn").disabled = true;
  out.appendChild(el("div", { class: "llm-test", text: "正在拉取 Soulous 数据；失败会继续使用上次缓存…" }));
  try {
    const r = await invoke("soulous_pull", { now: nowParam() });
    out.textContent = "";
    if (!r.configured) {
      out.appendChild(el("div", { class: "llm-test" }, [
        icon("alert", 15), el("span", { text: "未配置，已保持静默离线。" }),
      ]));
      return;
    }
    const p = r.outcome;
    out.appendChild(el("div", { class: "llm-test ok" }, [
      icon("check", 15), el("span", { text:
        `已拉取：课表 ${p.courses} 条、考试 ${p.exams} 场、任务 ${p.tasks} 项、打卡 ${p.checkins} 条、专注 ${p.focus_sessions} 段${p.refreshed_tokens ? "；JWT 已自动刷新" : ""}` }),
    ]));
    toast("Soulous 缓存已更新", "ok");
    refreshSoulous(); refreshSuggest();
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [
      icon("alert", 15), el("span", { text: safeUserError(e, "拉取未完成，已保留上次缓存，请稍后重试。") }),
    ]));
  } finally { $("soulousPullBtn").disabled = false; }
});

/* ============================== 邮箱（F21，本机连接器） ============================== */
let emailSettings = null;
let emailFormSeeded = false;
let emailOauthTimer = 0;

const EMAIL_PRESETS = {
  qq: { imapHost: "imap.qq.com", imapPort: 993, smtpHost: "smtp.qq.com", smtpPort: 465, smtpTls: "wrapper" },
  gmail: { imapHost: "imap.gmail.com", imapPort: 993, smtpHost: "smtp.gmail.com", smtpPort: 465, smtpTls: "wrapper" },
  microsoft: { imapHost: "outlook.office365.com", imapPort: 993, smtpHost: "smtp.office365.com", smtpPort: 587, smtpTls: "start_tls" },
};

function setEmailSelect(id, accounts, selected) {
  const select = $(id);
  const previous = selected || select.value;
  select.textContent = "";
  if (!accounts.length) {
    select.appendChild(el("option", { value: "", text: "请先连接一个邮箱" }));
    select.disabled = true;
    return;
  }
  select.disabled = false;
  for (const account of accounts) {
    select.appendChild(el("option", { value: account.id, text: `${account.label} · ${account.address}` }));
  }
  select.value = accounts.some((a) => a.id === previous) ? previous : accounts[0].id;
}

function renderEmailStatus(settings) {
  const box = $("emailStatus");
  box.textContent = "";
  if (!settings.configured) {
    box.appendChild(el("div", { class: "statusline", title: `首次保存会写入：${settings.path}` }, [
      el("span", { class: "sdot" }),
      el("span", { class: "t", text: "尚未连接邮箱；连接后只扫描邮件头中的高价值事件。" }),
    ]));
    return;
  }
  box.appendChild(el("div", { class: "statusline", title: `配置文件：${settings.path}（本地、gitignore、不进同步）` }, [
    el("span", { class: "sdot ok" }),
    el("span", { class: "t", text: `已连接 ${settings.accounts.length} 个邮箱账户；扫描只在你点击后进行。` }),
  ]));
}

function emailSyncAuthFields() {
  const provider = $("emailProvider").value;
  let oauth = $("emailAuthKind").value === "oauth2";
  if ((provider === "qq" || provider === "custom") && oauth) {
    $("emailAuthKind").value = "app_password";
    oauth = false;
  }
  $("emailPasswordFields").hidden = oauth;
  $("emailOauthFields").hidden = !oauth;
  $("emailOauthBtn").hidden = !oauth;
  $("emailTenantRow").hidden = provider !== "microsoft";
  const hints = {
    qq: "QQ 邮箱请先在网页端开启 IMAP/SMTP，并生成授权码。",
    gmail: oauth ? "推荐 OAuth2；需先在 Google Cloud 创建桌面 OAuth 客户端。" : "建议改用 OAuth2；若使用应用专用密码，须先满足 Gmail 的安全策略。",
    microsoft: oauth ? "需要在 Microsoft Entra 注册桌面应用并授予 IMAP/SMTP 委托权限。" : "Microsoft 通常要求现代身份验证；推荐改用 OAuth2。",
    custom: "自定义 IMAP/SMTP 当前使用授权码或应用专用密码。",
  };
  $("emailAuthHint").textContent = hints[provider];
}

function emailApplyProviderPreset(force) {
  const preset = EMAIL_PRESETS[$("emailProvider").value];
  if (preset) {
    const fields = [["emailImapHost", preset.imapHost], ["emailImapPort", preset.imapPort],
      ["emailSmtpHost", preset.smtpHost], ["emailSmtpPort", preset.smtpPort], ["emailSmtpTls", preset.smtpTls]];
    for (const [id, value] of fields) if (force || !$(id).value) $(id).value = value;
  }
  $("emailEndpointFold").open = $("emailProvider").value === "custom";
  emailSyncAuthFields();
}

function emailSeedForm(account) {
  emailFormSeeded = true;
  if (!account) {
    $("emailAccountId").value = "";
    $("emailAccountLabel").value = "";
    $("emailProvider").value = "qq";
    $("emailAddress").value = "";
    $("emailAuthKind").value = "app_password";
    $("emailAppPassword").value = "";
    $("emailClientId").value = "";
    $("emailClientSecret").value = "";
    $("emailTenant").value = "";
    $("emailRemoveBtn").hidden = true;
    emailApplyProviderPreset(true);
    return;
  }
  $("emailAccountId").value = account.id || "";
  $("emailAccountLabel").value = account.label || "";
  $("emailProvider").value = account.provider || "custom";
  $("emailAddress").value = account.address || "";
  $("emailAuthKind").value = account.authKind || "app_password";
  $("emailAppPassword").value = "";
  $("emailAppPassword").placeholder = account.secretTail ? `已保存（${account.secretTail}），留空则沿用` : "首次保存必填";
  $("emailClientId").value = account.clientId || "";
  $("emailClientSecret").value = "";
  $("emailClientSecret").placeholder = account.authKind === "oauth2" ? "已保存时留空则沿用" : "可选";
  $("emailTenant").value = account.tenant || "";
  $("emailImapHost").value = account.imapHost || "";
  $("emailImapPort").value = account.imapPort || "";
  $("emailSmtpHost").value = account.smtpHost || "";
  $("emailSmtpPort").value = account.smtpPort || "";
  $("emailSmtpTls").value = account.smtpTls || "wrapper";
  $("emailRemoveBtn").hidden = false;
  $("emailEndpointFold").open = account.provider === "custom";
  emailSyncAuthFields();
}

function renderEmailAccounts(settings) {
  const box = $("emailAccounts");
  box.textContent = "";
  for (const account of settings.accounts) {
    const detail = account.authKind === "oauth2"
      ? (account.oauthReady ? "OAuth 已授权" : "尚未授权")
      : `授权码 ${account.secretTail || "未保存"}`;
    box.appendChild(el("button", { type: "button", class: "mail-account-chip", title: `编辑 ${account.label}`,
      onclick: () => { $("emailAccountFold").open = true; emailSeedForm(account); } }, [
      el("span", { class: "sdot" + ((account.authKind !== "oauth2" || account.oauthReady) ? " ok" : " amber") }),
      el("span", { text: `${account.label} · ${detail}` }),
    ]));
  }
}

async function refreshEmail(seedForm) {
  let settings;
  try { settings = await invoke("email_config_get"); } catch (_) { return; }
  const read = $("emailReadAccount").value;
  const compose = $("emailComposeAccount").value;
  const eventAccount = $("emailEventAccount").value;
  emailSettings = settings;
  renderEmailStatus(settings);
  renderEmailAccounts(settings);
  setEmailSelect("emailReadAccount", settings.accounts, read);
  setEmailSelect("emailComposeAccount", settings.accounts, compose);
  setEmailSelect("emailEventAccount", settings.accounts, eventAccount);
  if (seedForm || !emailFormSeeded) emailSeedForm(settings.accounts.find((a) => a.id === eventAccount || a.id === read) || settings.accounts[0]);
  // 账户配置默认收起，但没配过账户时必须自动展开——否则首次进来只有一个空收件箱，
  // 没有任何可见的连接入口（2026-07-20 自查：折叠低频表单不能把首次流程一起折掉）。
  if (!settings.configured) $("emailAccountFold").open = true;
}

function emailCollectAccount() {
  const port = (id) => {
    const value = parseInt($(id).value, 10);
    return Number.isInteger(value) && value > 0 && value <= 65535 ? value : null;
  };
  return {
    id: $("emailAccountId").value.trim(), label: $("emailAccountLabel").value.trim(),
    provider: $("emailProvider").value, address: $("emailAddress").value.trim(),
    auth_kind: $("emailAuthKind").value, app_password: $("emailAppPassword").value,
    client_id: $("emailClientId").value.trim(), client_secret: $("emailClientSecret").value,
    tenant: $("emailTenant").value.trim(), imap_host: $("emailImapHost").value.trim(),
    imap_port: port("emailImapPort"), smtp_host: $("emailSmtpHost").value.trim(),
    smtp_port: port("emailSmtpPort"), smtp_tls: $("emailSmtpTls").value,
  };
}

function emailClearMailbox() {
  $("emailMessageList").textContent = "";
  $("emailMessageOut").textContent = "";
}

async function refreshEmailFolders(loadMessages) {
  const accountId = $("emailReadAccount").value;
  const out = $("emailMailboxOut");
  if (!accountId) { out.textContent = ""; emailClearMailbox(); return; }
  out.textContent = "";
  out.appendChild(el("div", { class: "hint", text: "正在读取文件夹…" }));
  try {
    const folders = await invoke("email_folders", { accountId });
    const old = $("emailFolder").value;
    $("emailFolder").textContent = "";
    for (const folder of folders) $("emailFolder").appendChild(el("option", { value: folder.name, text: folder.name }));
    if (!folders.length) $("emailFolder").appendChild(el("option", { value: "INBOX", text: "INBOX" }));
    $("emailFolder").value = folders.some((f) => f.name === old) ? old : (folders.find((f) => f.name.toUpperCase() === "INBOX") || folders[0] || { name: "INBOX" }).name;
    out.textContent = "";
    if (loadMessages) await loadEmailMessages();
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮箱设置暂时无法读取，请稍后重试。") })]));
  }
}

function renderEmailMessages(messages) {
  const list = $("emailMessageList");
  list.textContent = "";
  if (!messages.length) { list.appendChild(emptyBox("mail", "没有匹配的邮件", "可调整文件夹或搜索关键词后再试。")); return; }
  for (const message of messages) {
    list.appendChild(el("button", { type: "button", class: "mail-row" + (message.seen ? "" : " unread"), onclick: () => openEmailMessage(message.uid) }, [
      el("span", { class: "mail-from", text: message.from || "（未知发件人）" }),
      el("span", { class: "mail-subject", text: message.subject || "（无主题）" }),
      el("span", { class: "mail-date", text: message.date || "" }),
    ]));
  }
}

async function loadEmailMessages(forceSearch) {
  const accountId = $("emailReadAccount").value;
  const mailbox = $("emailFolder").value || "INBOX";
  const query = $("emailSearchQuery").value.trim();
  const out = $("emailMailboxOut");
  if (!accountId) return;
  $("emailMessageOut").textContent = "";
  out.textContent = "";
  out.appendChild(el("div", { class: "hint", text: query || forceSearch ? "正在从邮箱读取…" : "正在读取最近邮件…" }));
  try {
    const args = { accountId, mailbox, limit: 30 };
    const messages = (query || forceSearch)
      ? await invoke("email_search", { ...args, query })
      : await invoke("email_messages", args);
    out.textContent = "";
    out.appendChild(el("div", { class: "statusline" }, [el("span", { class: "sdot ok" }), el("span", { text: `已加载 ${messages.length} 封邮件；内容只保留在当前界面内存。` })]));
    renderEmailMessages(messages);
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮件暂时无法读取，请稍后重试。") })]));
  }
}

async function openEmailMessage(uid) {
  const accountId = $("emailReadAccount").value;
  const mailbox = $("emailFolder").value || "INBOX";
  const box = $("emailMessageOut");
  box.textContent = "";
  box.appendChild(el("div", { class: "hint", text: "正在读取邮件正文…" }));
  try {
    const message = await invoke("email_message", { accountId, mailbox, uid });
    box.textContent = "";
    box.appendChild(el("div", { class: "block", style: "margin:0;" }, [
      el("div", { class: "statusline" }, [el("span", { class: "t", text: message.subject || "（无主题）" })]),
      el("div", { class: "hint", text: `发件人：${message.from || ""}\n收件人：${message.to || ""}${message.cc ? "\n抄送：" + message.cc : ""}\n日期：${message.date || ""}${message.attachmentCount ? "\n附件：" + message.attachmentCount + " 个（当前版本不下载）" : ""}` }),
      el("pre", { class: "mail-content", text: message.text || "（没有可显示的纯文本正文）" }),
    ]));
  } catch (e) {
    box.textContent = "";
    box.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮件正文暂时无法读取，请稍后重试。") })]));
  }
}

function emailList(value) {
  return value.split(/[;,\n]/).map((item) => item.trim()).filter(Boolean);
}

async function beginEmailOAuth() {
  const account = emailCollectAccount();
  if (account.auth_kind !== "oauth2") return;
  const out = $("emailConfigOut");
  $("emailOauthBtn").disabled = true;
  try {
    const settings = await invoke("email_config_save", { args: account });
    emailSettings = settings;
    await refreshEmail(true);
    const begin = await invoke("email_oauth_begin", { accountId: account.id });
    out.textContent = "";
    const link = el("a", { href: begin.authorizationUrl, target: "_blank", rel: "noopener", text: "若浏览器没有打开，请点这里继续授权" });
    out.appendChild(el("div", { class: "llm-test" }, [
      icon("cloud", 15), el("span", { text: `等待浏览器授权；回调地址：${begin.redirectUri}。` }), link,
    ]));
    window.open(begin.authorizationUrl, "_blank", "noopener");
    clearTimeout(emailOauthTimer);
    const poll = async () => {
      try {
        const result = await invoke("email_oauth_poll", { sessionId: begin.sessionId });
        if (result.status === "pending") { emailOauthTimer = setTimeout(poll, 1200); return; }
        out.textContent = "";
        out.appendChild(el("div", { class: "llm-test ok" }, [icon("check", 15), el("span", { text: "OAuth 授权已完成，刷新令牌仅保存于本机。" })]));
        toast("邮箱 OAuth 授权已完成", "ok");
        await refreshEmail(true);
      } catch (e) {
        out.textContent = "";
        out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮箱授权没有完成，请重新授权。") })]));
      }
    };
    emailOauthTimer = setTimeout(poll, 1200);
  } catch (e) { toast(String(e), "danger"); }
  finally { $("emailOauthBtn").disabled = false; }
}

$("emailProvider").addEventListener("change", () => emailApplyProviderPreset(true));
$("emailAuthKind").addEventListener("change", emailSyncAuthFields);
$("emailSaveBtn").addEventListener("click", async () => {
  const account = emailCollectAccount();
  $("emailSaveBtn").disabled = true;
  try {
    await invoke("email_config_save", { args: account });
    $("emailAppPassword").value = "";
    $("emailClientSecret").value = "";
    toast("邮箱账户已保存到本机", "ok");
    await refreshEmail(true);
  } catch (e) { toast(String(e), "danger"); }
  finally { $("emailSaveBtn").disabled = false; }
});
$("emailOauthBtn").addEventListener("click", beginEmailOAuth);
$("emailEventScanBtn").addEventListener("click", async () => {
  const accountId = $("emailEventAccount").value;
  const button = $("emailEventScanBtn");
  const out = $("emailEventOut");
  if (!accountId) return toast("请先连接邮箱账户", "danger");
  button.disabled = true;
  out.textContent = "";
  out.appendChild(el("div", { class: "hint", text: "正在本机扫描最近 100 封邮件的标题信息…" }));
  try {
    const drafts = await invoke("email_high_value_scan", { accountId });
    const count = drafts.length;
    out.textContent = "";
    out.appendChild(el("div", { class: "statusline" }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: count ? `发现 ${count} 条高价值事件，已放入待确认信息箱（重复项会合并）。` : "未发现新的高价值事件。" }),
    ]));
    await refreshCaptureInbox();
    await refreshCapture();
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮箱事件暂时无法扫描，请稍后重试。") })]));
  } finally { button.disabled = false; }
});
$("emailRemoveBtn").addEventListener("click", () => {
  const accountId = $("emailAccountId").value.trim();
  if (!accountId) return;
  openModal("移除本机邮箱账户", el("div", { text: `将从本机配置移除 “${accountId}” 的凭据和账户信息；不会删除服务商邮箱中的任何邮件。` }), "移除", async () => {
    await invoke("email_config_remove", { accountId });
    emailFormSeeded = false;
    emailClearMailbox();
    await refreshEmail(true);
    toast("已移除本机邮箱账户", "ok");
  }, { icon: "trash" });
});
$("emailReadAccount").addEventListener("change", () => { emailClearMailbox(); refreshEmailFolders(false); });
$("emailFolder").addEventListener("change", () => { emailClearMailbox(); loadEmailMessages(); });
$("emailRefreshBtn").addEventListener("click", () => loadEmailMessages());
$("emailSearchBtn").addEventListener("click", () => loadEmailMessages(true));
$("emailSearchQuery").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); loadEmailMessages(true); } });
$("emailSendBtn").addEventListener("click", async () => {
  const draft = {
    accountId: $("emailComposeAccount").value, to: emailList($("emailTo").value), cc: emailList($("emailCc").value),
    bcc: emailList($("emailBcc").value), subject: $("emailSubject").value, body: $("emailBody").value, html: $("emailHtml").checked,
  };
  if (!draft.accountId) return toast("请先选择发件账户", "danger");
  if (!draft.to.length) return toast("请至少填写一个收件人", "danger");
  try {
    const pending = await invoke("guard_request", { tool: "email_send", args: JSON.stringify(draft), now: nowParam() });
    openGuardConfirm(pending, false);
  } catch (e) { toast(String(e), "danger"); }
});
emailApplyProviderPreset(true);

/* ============================== 本地统一搜索 ============================== */
const SEARCH_SOURCES = [
  { view: "agenda", label: "计划 · 日程", roots: ["agendaBox"] },
  { view: "notifs", label: "计划 · 提醒", roots: ["dueBox", "notifBox"] },
  { view: "suggest", label: "计划 · 建议", roots: ["suggestBox"] },
  { view: "ledger", label: "记忆 · 台账", roots: ["ledgerBox"] },
  { view: "journal", label: "记忆 · 行为", roots: ["journalBox"] },
  { view: "wearable", label: "记忆 · 健康趋势", roots: ["wearableBox", "wearableOut"] },
  { view: "review", label: "记忆 · 复盘", roots: ["reviewOut"] },
  { view: "captures", label: "记忆 · 通知", roots: ["notificationLedger"] },
];
const SEARCH_QUICK = [
  { view: "ledger", label: "记忆台账", desc: "查看 Solum 已经记住的事实、事件与来源。", icon: "memory" },
  { view: "agenda", label: "计划与日程", desc: "回到即将发生的安排和时间节点。", icon: "calendar" },
  { view: "journal", label: "行为记录", desc: "按原始顺序回看最近的状态与活动。", icon: "journal" },
  { view: "captures", label: "通知回看", desc: "找回过滤、判重或仍待处理的通知。", icon: "inbox" },
];
function searchText(value) {
  return String(value || "").replace(/\s+/g, " ").trim();
}
function collectSearchEntries() {
  const entries = [];
  const seen = new Set();
  const selectors = ".ev-row,.notif-row,.row,.mail-row,.gc-card,.tp-item,tbody tr,.empty,.statusline";
  for (const source of SEARCH_SOURCES) {
    for (const rootId of source.roots) {
      const root = $(rootId);
      if (!root) continue;
      const candidates = Array.from(root.querySelectorAll(selectors));
      const nodes = candidates.length ? candidates : [root];
      for (const node of nodes) {
        const text = searchText(node.textContent);
        if (!text || text.length < 2) continue;
        const key = source.view + "\u0000" + text;
        if (seen.has(key)) continue;
        seen.add(key);
        entries.push({ view: source.view, label: source.label, text });
        if (entries.length >= 240) return entries;
      }
    }
  }
  return entries;
}
function searchResultButton(entry) {
  const title = entry.text.length > 76 ? entry.text.slice(0, 76) + "…" : entry.text;
  return el("button", { class: "search-result", type: "button", onclick: () => showView(entry.view) }, [
    el("span", null, [
      el("span", { class: "search-result-type", text: entry.label }),
      el("span", { class: "search-result-title", text: title }),
      el("span", { class: "search-result-copy", text: entry.text }),
    ]),
    icon("chev", 20),
  ]);
}
function renderSearchQuick() {
  const box = $("globalSearchResults");
  box.textContent = "";
  const grid = el("div", { class: "search-quick-grid" });
  for (const item of SEARCH_QUICK) {
    grid.appendChild(el("button", { class: "search-quick", type: "button", onclick: () => showView(item.view) }, [
      el("span", null, [
        el("span", { class: "search-result-type", text: "LOCAL INDEX" }),
        el("span", { class: "search-result-title", text: item.label }),
        el("span", { class: "search-result-copy", text: item.desc }),
      ]),
      icon(item.icon, 24),
    ]));
  }
  box.appendChild(grid);
  $("globalSearchHeading").textContent = "最近入口";
  $("globalSearchCount").textContent = "4 个入口";
}
function runGlobalSearch() {
  const query = searchText($("globalSearchInput").value).toLocaleLowerCase();
  if (!query) { renderSearchQuick(); return; }
  const terms = query.split(" ").filter(Boolean);
  const matches = collectSearchEntries().filter((entry) => {
    const haystack = entry.text.toLocaleLowerCase();
    return terms.every((term) => haystack.includes(term));
  }).slice(0, 40);
  const box = $("globalSearchResults");
  box.textContent = "";
  $("globalSearchHeading").textContent = "搜索结果";
  $("globalSearchCount").textContent = `${matches.length} 条`;
  if (!matches.length) {
    box.appendChild(emptyBox("search", "没有找到匹配内容", "换个更短的关键词，或先在「今天」告诉 Solum 要记住什么。"));
    return;
  }
  for (const entry of matches) box.appendChild(searchResultButton(entry));
}
$("globalSearchIcon").appendChild(icon("search", 22));
$("globalSearchForm").addEventListener("submit", (event) => {
  event.preventDefault();
  runGlobalSearch();
});
let globalSearchTimer = 0;
$("globalSearchInput").addEventListener("input", () => {
  clearTimeout(globalSearchTimer);
  globalSearchTimer = setTimeout(runGlobalSearch, 160);
});
renderSearchQuick();

/* ============================== 导航 ============================== */
// 四个一级入口表达长期稳定的产品心智：今天、记忆、计划、搜索。
// 邮箱、设置和工作台属于低频工具，不与用户每天往返的四个根入口争夺位置。
const PRIMARY_GROUPS = [
  { id: "today",  label: "今天", icon: "home",     views: [["chat", "今天"]] },
  { id: "memory", label: "记忆", icon: "memory",   views: [["ledger", "记忆"], ["ledger-detail", "详情"]], navViews: [["ledger", "记忆"]] },
  { id: "plan",   label: "计划", icon: "calendar", views: [["agenda", "日程"], ["notifs", "提醒"], ["suggest", "建议"]] },
  { id: "search", label: "搜索", icon: "search",   views: [["search", "搜索"]] },
];
const SETTINGS_GROUP = {
  id: "settings", label: "设置", icon: "settings", views: [["settings", "设置"]],
    sub: [
      ["persona",     "数字人格",   "user",   "风格设定影响闲聊与复盘措辞；版本化，可回滚"],
      ["proactivity", "主动度",     "gauge",  "每个能力域独立调档；高危确认不受档位影响"],
      ["privacy",     "隐私",       "lock",   "通知上云开关、通知智能管线与完整隐私政策"],
      ["capture",     "数据入口",   "inbox",  "各采集通道的真实状态与待确认区"],
      ["captures",    "通知回看",   "bell",   "查看通知提炼结果、过滤记录与待确认动作"],
      ["journal",     "行为足迹",   "memory", "回顾互动、状态记录与生活轨迹"],
      ["wearable",    "健康趋势",   "gauge",  "查看本地健康传感器的阶段趋势"],
      ["review",      "复盘简报",   "doc",    "按时间范围生成阶段性回顾"],
      ["data",        "数据与备份", "folder", "本地导出、合并恢复与加密云备份"],
      ["sync",        "同步",       "refresh", "多设备账户与同步状态"],
      ["backup",      "备份与恢复", "lock",   "手动加密封缄、版本历史与恢复接管"],
      ["cloud",       "云端接入",   "cloud",  "LLM 端点配置与 Soulous 互通"],
      ["rules",       "重要度规则", "rules",  "按事件类型设置提前提醒与通知渠道"],
      ["guard",       "高危护栏",   "shield", "可用工具、审计日志与确认流程演示"],
    ],
};
const UTILITY_GROUPS = [
  { id: "mail", label: "邮箱事件", icon: "mail", views: [["email", "邮箱事件"]] },
  SETTINGS_GROUP,
];
const GROUPS = [...PRIMARY_GROUPS, ...UTILITY_GROUPS];
let navLastView = { ...uiState.navLastView };
let viewScrollPositions = new Map(Object.entries(uiState.scroll || {}).map(([view, top]) => [view, Number(top) || 0]));
let currentView = "";
function groupOfView(view) {
  for (const g of GROUPS) {
    if (g.views.some(([v]) => v === view)) return g;
    if (g.sub && g.sub.some(([v]) => v === view)) return g;
  }
  return GROUPS[0];
}
function getGroupLabel(g) {
  if (typeof t === "function") {
    if (g.id === "today") return t("nav.today");
    if (g.id === "memory") return t("nav.memory");
    if (g.id === "plan") return t("agenda.title", null, g.label);
    if (g.id === "search") return t("common.search", null, g.label);
    if (g.id === "settings") return t("nav.settings");
  }
  return g.label;
}

function renderSettingsMenu() {
  const menu = $("settingsMenu");
  if (!menu) return;
  menu.textContent = "";
  for (const [v, label, ic, desc] of SETTINGS_GROUP.sub) {
    if (v === "backup") continue;
    const guestAllowed = GUEST_ALLOWED_VIEWS.has(v);
    menu.appendChild(el("button", {
      class: "nav-menu-item" + (guestAllowed ? "" : " locked-entry"),
      type: "button",
      "data-requires-account": guestAllowed ? "false" : "true",
      onclick: () => showView(v),
    }, [
      icon(ic, 20),
      el("div", null, [el("div", { class: "nm", text: label }), el("div", { class: "nd", text: desc })]),
      guestAllowed ? null : el("span", { class: "menu-lock", title: "登录后可用" }, icon("lock", 15)),
      el("span", { class: "chev" }, icon("chev", 16)),
    ]));
  }
}

function renderPrimaryNav() {
  const nav = $("nav");
  const tabbar = $("tabbar");
  if (!nav || !tabbar) return;
  nav.textContent = "";
  tabbar.textContent = "";
  for (const g of PRIMARY_GROUPS) {
    const label = getGroupLabel(g);
    nav.appendChild(el("button", { class: "nav-btn", "data-group": g.id, onclick: () => gotoGroup(g) }, [
      icon(g.icon, 19),
      el("span", { text: label }),
      el("span", { class: "grow" }),
      g.id === "plan" ? el("span", { class: "due-pill", id: "dueNav" }) : null,
    ]));
    tabbar.appendChild(el("button", { class: "tab-btn", "data-group": g.id, onclick: () => gotoGroup(g) }, [
      icon(g.icon, 21),
      el("span", { text: label }),
      g.id === "plan" ? el("span", { class: "due-pill", id: "dueTab" }) : null,
    ]));
  }
}

renderSettingsMenu();
renderPrimaryNav();

function initLanguageControl() {
  const sel = $("appLanguageSelect");
  if (!sel || typeof window === "undefined" || !window.i18n) return;
  sel.value = window.i18n.getLanguage();
  sel.addEventListener("change", () => {
    window.i18n.setLanguage(sel.value);
    renderPrimaryNav();
    renderSettingsMenu();
    if (typeof refreshNavBadges === "function") refreshNavBadges();
    toast(window.t ? window.t("toast.saved") : "Language updated", "ok");
  });
}
const TOOL_ENTRIES = [
  { label: "邮箱事件", icon: "mail", group: "mail", requiresAccount: true, action: () => showView("email") },
  { label: "设置", icon: "settings", group: "settings", action: () => showView("settings") },
  { label: "资料", icon: "doc", requiresAccount: true, action: () => accountLoggedIn ? openDrawer("documents") : requireAccount("登录后使用本地资料工作台") },
  { label: "组件", icon: "folder", requiresAccount: true, action: () => accountLoggedIn ? openDrawer("widgets") : requireAccount("登录后创建和管理个人组件") },
];
function closeMobileTools() {
  if ($("mobileTools").hidden) return;
  $("mobileTools").hidden = true;
  $("mobileToolsMask").hidden = true;
  $("mobileToolsBtn").setAttribute("aria-expanded", "false");
  syncOverlayHistory();
}
function openMobileTools() {
  $("mobileTools").hidden = false;
  $("mobileToolsMask").hidden = false;
  $("mobileToolsBtn").setAttribute("aria-expanded", "true");
  syncOverlayHistory();
  requestAnimationFrame(() => $("mobileTools").querySelector("button")?.focus());
}
function makeUtilityButton(entry) {
  const button = el("button", {
    class: "utility-btn",
    type: "button",
    "data-group": entry.group,
    "data-label": entry.label,
    "data-requires-account": entry.requiresAccount ? "true" : "false",
    onclick: () => { closeMobileTools(); entry.action(); },
  }, [icon(entry.icon, 18), el("span", { text: entry.label })]);
  if (entry.requiresAccount) button.appendChild(el("span", { class: "utility-lock", title: "登录后可用" }, icon("lock", 14)));
  return button;
}
for (const entry of TOOL_ENTRIES) {
  $("utilityNav").appendChild(makeUtilityButton(entry));
  $("mobileToolsGrid").appendChild(makeUtilityButton(entry));
}
$("mobileToolsClose").appendChild(icon("x", 18));
$("mobileToolsBtn").addEventListener("click", () => {
  if ($("mobileTools").hidden) openMobileTools(); else closeMobileTools();
});
$("mobileToolsClose").addEventListener("click", closeMobileTools);
$("mobileToolsMask").addEventListener("click", closeMobileTools);
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("mobileTools").hidden) {
    closeMobileTools();
    $("mobileToolsBtn").focus();
  }
});
function gotoGroup(g) {
  // 多级组（设置）：已在其二级页时再点组入口 = 返回目录页；否则回到最后停留的视图。
  const curSection = document.querySelector("section.view.active");
  const cur = curSection ? curSection.id.slice(5) : "";
  if (g.sub && cur && groupOfView(cur) === g && cur !== g.views[0][0]) { showView(g.views[0][0]); return; }
  showView(navLastView[g.id] || g.views[0][0]);
}
function handleSubnavKeydown(e) {
  const keys = ["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp", "Home", "End"];
  if (!keys.includes(e.key)) return;
  const tabs = Array.from($("subnav").querySelectorAll('[role="tab"]'));
  const current = tabs.indexOf(e.currentTarget);
  if (current < 0 || !tabs.length) return;
  e.preventDefault();
  let next = current;
  if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (current + 1) % tabs.length;
  else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (current - 1 + tabs.length) % tabs.length;
  else if (e.key === "Home") next = 0;
  else if (e.key === "End") next = tabs.length - 1;
  const nextView = tabs[next].dataset.view;
  showView(nextView);
  requestAnimationFrame(() => $("tab-" + nextView)?.focus());
}

/* ---- Motion system -------------------------------------------------------
   GSAP skill conventions, adapted to this repository's zero-dependency rule:
   timeline-style sequencing, compositor-only transform/opacity, cancellation
   on navigation, and a hard prefers-reduced-motion exit. */
const motionPreference = window.matchMedia("(prefers-reduced-motion: reduce)");
let viewMotion = [];
const motionSeen = new WeakSet();
function cancelViewMotion() {
  for (const animation of viewMotion) {
    try { animation.cancel(); } catch (_) { /* detached target */ }
  }
  viewMotion = [];
}
function motionTween(target, keyframes, options) {
  if (!target || motionPreference.matches || typeof target.animate !== "function") return null;
  const animation = target.animate(keyframes, {
    duration: 420,
    easing: "cubic-bezier(.22,1,.36,1)",
    fill: "both",
    ...options,
  });
  viewMotion.push(animation);
  animation.finished.then(() => {
    animation.cancel();
    viewMotion = viewMotion.filter((item) => item !== animation);
  }).catch(() => { /* navigation intentionally interrupts the timeline */ });
  return animation;
}
function playViewTimeline(view) {
  cancelViewMotion();
  if (motionPreference.matches) return;
  const panel = $("view-" + view);
  if (!panel) return;
  const isChat = view === "chat";
  const title = isChat ? panel.querySelector("#chatEmpty h3") : panel.querySelector(".vhead h2");
  const actions = isChat ? panel.querySelector("#chatEmpty .sub") : panel.querySelector(".vactions");
  const content = isChat
    ? [panel.querySelector("#chatEmpty"), panel.querySelector(".composer")]
    : Array.from(panel.querySelectorAll(".vwrap > .block, .vwrap > details.fold, .vwrap > .banner, .vwrap > .nav-menu, .vwrap > .mail-layout, .vwrap > .search-hero, .vwrap > .search-index-head, .vwrap > .search-results, .vwrap > [id$='Box'], .vwrap > [id$='Out']")).slice(0, 8);

  motionTween(title, [
    { opacity: 0, transform: "translateY(18px)" },
    { opacity: 1, transform: "translateY(0)" },
  ], { duration: 480, delay: 20 });
  motionTween(actions, [
    { opacity: 0, transform: "translateY(12px)" },
    { opacity: 1, transform: "translateY(0)" },
  ], { duration: 420, delay: 90 });
  content.forEach((target, index) => motionTween(target, [
    { opacity: 0, transform: "translateY(20px) scale(.992)" },
    { opacity: 1, transform: "translateY(0) scale(1)" },
  ], { duration: 500, delay: 140 + index * 52 }));

  document.querySelectorAll(".nav-btn.active, .tab-btn.active").forEach((target) => {
    motionTween(target, [
      { transform: "scale(.96)" },
      { transform: "scale(1)" },
    ], { duration: 300, delay: 0 });
  });
}

// Async IPC refreshes populate cards after the main view timeline. Animate only
// newly inserted user-facing rows/cards, cap each batch, and never touch layout.
const dataMotionObserver = new MutationObserver((records) => {
  if (motionPreference.matches) return;
  const targets = [];
  for (const record of records) {
    for (const node of record.addedNodes) {
      if (!(node instanceof HTMLElement)) continue;
      if (node.closest("#chatLog") || node.matches("#chatLog, .msg, .msg *")) continue;
      const candidates = node.matches(".row,.ev-row,.notif-row,.mail-row,.gc-card,.search-result,.search-quick,.memory-row,.empty,.statusline")
        ? [node]
        : Array.from(node.querySelectorAll(".row,.ev-row,.notif-row,.mail-row,.gc-card,.search-result,.search-quick,.memory-row,.empty,.statusline"));
      for (const candidate of candidates) {
        if (motionSeen.has(candidate) || !candidate.closest("section.view.active")) continue;
        motionSeen.add(candidate);
        targets.push(candidate);
        if (targets.length >= 14) break;
      }
      if (targets.length >= 14) break;
    }
    if (targets.length >= 14) break;
  }
  targets.forEach((target, index) => {
    const animation = target.animate([
      { opacity: 0, transform: "translateY(10px)" },
      { opacity: 1, transform: "translateY(0)" },
    ], {
      duration: 340,
      delay: index * 28,
      easing: "cubic-bezier(.22,1,.36,1)",
    });
    animation.finished.catch(() => {});
  });
});
dataMotionObserver.observe($("main"), { childList: true, subtree: true });

/* ---- 视图切换管线 -------------------------------------------------------
   showView 只做编排：记住滚动 → 记账 → 同步导航 chrome → 激活面板 → 压历史 →
   恢复滚动/动画 → 派发数据刷新。每一步是可独立阅读的函数。 */
function rememberScroll() {
  if (!currentView) return;
  const currentPanel = $("view-" + currentView);
  if (currentPanel) {
    viewScrollPositions.set(currentView, currentPanel.scrollTop);
    uiState.scroll[currentView] = currentPanel.scrollTop;
  }
}

function renderSubnav(group, view) {
  const sn = $("subnav");
  sn.textContent = "";
  const subEntry = group.sub && group.sub.find(([v]) => v === view);
  if (subEntry) {
    // 多级导航二级页：分段控件位置改为面包屑返回（← 设置 / 当前页名）
    sn.classList.remove("seg"); sn.classList.add("crumb");
    $("subnavWrap").classList.remove("hide");
    sn.appendChild(el("button", { class: "crumb-back", type: "button", onclick: () => showView(group.views[0][0]) }, [
      icon("back", 16), el("span", { text: group.label }),
    ]));
    sn.appendChild(el("span", { class: "crumb-sep", text: "/" }));
    sn.appendChild(el("span", { class: "crumb-cur", text: subEntry[1] }));
  } else if ((group.navViews || group.views).length > 1) {
    sn.classList.add("seg"); sn.classList.remove("crumb");
    $("subnavWrap").classList.remove("hide");
    for (const [v, label] of (group.navViews || group.views)) {
      const panel = $("view-" + v);
      const lockedForGuest = !accountLoggedIn && !GUEST_ALLOWED_VIEWS.has(v);
      if (panel) {
        panel.setAttribute("role", "tabpanel");
        panel.setAttribute("aria-labelledby", "tab-" + v);
      }
      sn.appendChild(el("button", {
        id: "tab-" + v, "data-view": v, type: "button", class: (v === view ? "active" : "") + (lockedForGuest ? " locked-tab" : ""), role: "tab",
        "aria-selected": v === view ? "true" : "false",
        "aria-controls": "view-" + v,
        tabindex: v === view ? "0" : "-1",
        onclick: () => showView(v),
        onkeydown: handleSubnavKeydown,
      }, [el("span", { text: label }), lockedForGuest ? icon("lock", 13) : null]));
    }
  } else {
    $("subnavWrap").classList.add("hide");
  }
}

// 侧栏 / 底栏 / 工具入口的高亮与 aria-current，外加子导航重建。
function syncNavChrome(group, view) {
  document.querySelectorAll(".nav-btn, .tab-btn").forEach((b) => {
    const on = b.dataset.group === group.id;
    b.classList.toggle("active", on);
    if (on) b.setAttribute("aria-current", "page"); else b.removeAttribute("aria-current");
  });
  document.querySelectorAll(".utility-btn[data-group]").forEach((button) => {
    const on = button.dataset.group === group.id;
    button.classList.toggle("active", on);
    if (on) button.setAttribute("aria-current", "page"); else button.removeAttribute("aria-current");
  });
  $("mobileToolsBtn").classList.toggle("active", UTILITY_GROUPS.includes(group));
  renderSubnav(group, view);
}

// 视图回退栈：启动时 replaceState 落锚（见启动一节），之后每次真实切换压一条。
// 浮层打开期间不压视图条目，保证浮层条目始终是栈顶。
let viewHistoryArmed = false;

function showView(view, options) {
  options = options || {};
  if (!$("view-" + view)) view = "chat";
  if (!accountLoggedIn && !GUEST_ALLOWED_VIEWS.has(view)) {
    requireAccount(`“${groupOfView(view).label}”中的完整能力需要登录`);
    return false;
  }
  if (currentView !== view) rememberScroll();
  const group = groupOfView(view);
  const rememberedView = view === "ledger-detail" ? "ledger" : view;
  navLastView[group.id] = rememberedView;
  uiState.lastView = rememberedView;
  uiState.navLastView = { ...navLastView };
  saveUiState(false);
  closeMobileTools();
  syncNavChrome(group, view);
  document.querySelectorAll("section.view").forEach((x) =>
    x.classList.toggle("active", x.id === "view-" + view));
  document.body.dataset.view = view;
  document.body.dataset.group = group.id;
  const changed = currentView !== view;
  currentView = view;
  if (changed && options.push !== false && viewHistoryArmed) {
    try {
      if (overlayPushed && !anyOverlayOpen()) {
        // 典型路径：移动工具面板里的“设置”。面板已在 DOM 中关闭，但关闭动作登记的
        // history.back() 还没进入微任务。把浮层条目直接改成目标视图，避免 DOM 已切换、
        // history.state 却退回旧视图；随后那次 overlay 同步会看到 overlayPushed=false。
        overlayPushed = false;
        history.replaceState({ solumView: view }, "");
      } else if (!overlayPushed) {
        history.pushState({ solumView: view }, "");
      }
    } catch (_) { /* file:// 等场景无 history 能力 */ }
  }
  requestAnimationFrame(() => {
    const panel = $("view-" + view);
    if (panel && changed && options.restoreScroll !== false) panel.scrollTop = viewScrollPositions.get(view) || 0;
    if (options.animate !== false) playViewTimeline(view);
  });
  if (options.refresh !== false) refreshView(view);
  if (view === "search") {
    clearTimeout(globalSearchTimer);
    // 正常进入时由 refreshView 在依赖数据完成后搜索一次；只有明确跳过视图刷新
    // 的启动态才需要补一次延迟搜索，避免同一查询先后渲染两遍。
    if (options.refresh === false) globalSearchTimer = setTimeout(runGlobalSearch, 120);
  }
  return true;
}

const VIEW_REFRESHERS = {
  chat: [refreshToday],
  agenda: [refreshAgenda],
  notifs: [refreshNotifs],
  suggest: [refreshSuggest],
  journal: [refreshJournal],
  wearable: [refreshWearable],
  review: [],
  ledger: [refreshLedger],
  "ledger-detail": [],
  captures: [refreshCaptures],
  data: [],
  search: [refreshAgenda, refreshLedger, refreshJournal],
  settings: [refreshReminderDeliveryHealth],
  persona: [refreshPersona],
  proactivity: [refreshProactivity],
  privacy: [refreshPrivacy],
  capture: [refreshCapture],
  sync: [refreshSyncConfig, refreshSyncStatus],
  backup: [refreshBackupCenter],
  email: [refreshEmail],
  cloud: [refreshCloud, refreshAccount, refreshSoulous],
  rules: [refreshRules],
  guard: [refreshGuard],
};
let routeRefreshDepth = 0;
let routeProgressTimer = 0;
const viewRefreshFlights = new Map();
let refreshAllFlight = null;
let ingestRefreshFlight = null;
function beginRouteRefresh(view) {
  routeRefreshDepth += 1;
  const panel = $("view-" + view);
  if (panel) panel.setAttribute("aria-busy", "true");
  clearTimeout(routeProgressTimer);
  routeProgressTimer = setTimeout(() => $("routeProgress").classList.add("active"), 120);
}
function endRouteRefresh(view) {
  routeRefreshDepth = Math.max(0, routeRefreshDepth - 1);
  const panel = $("view-" + view);
  if (panel) panel.removeAttribute("aria-busy");
  if (routeRefreshDepth) return;
  clearTimeout(routeProgressTimer);
  $("routeProgress").classList.remove("active");
}
function settleRefreshers(refreshers) {
  return Promise.allSettled([...new Set(refreshers)].map((refresh) => Promise.resolve().then(refresh)));
}

function refreshView(view) {
  const pending = viewRefreshFlights.get(view);
  if (pending) return pending;
  const refreshers = VIEW_REFRESHERS[view] || [];
  if (!refreshers.length) return Promise.resolve();
  const request = (async () => {
    beginRouteRefresh(view);
    try {
      await settleRefreshers(refreshers);
      if (view === "search") runGlobalSearch();
    } finally {
      endRouteRefresh(view);
    }
  })().finally(() => viewRefreshFlights.delete(view));
  viewRefreshFlights.set(view, request);
  return request;
}

// 对话结果已经在当前气泡中落地；隐藏的日程、行为和台账会在真正进入视图时刷新。
// 这里只更新全局提醒角标，以及宽屏当前可见的今日面板。过去 refreshAll 会连账号、
// 邮箱、备份、隐私和设置一起重载，台账还会额外并发读取九个数据层。
function refreshAfterIngest() {
  if (ingestRefreshFlight) return ingestRefreshFlight;
  const refreshers = [refreshNotifs, refreshToday];
  ingestRefreshFlight = settleRefreshers(refreshers).finally(() => { ingestRefreshFlight = null; });
  return ingestRefreshFlight;
}

function refreshAll(options) {
  if (refreshAllFlight) return refreshAllFlight;
  const refreshers = [
    refreshAgenda, refreshNotifs, refreshJournal, refreshWearable, refreshSuggest,
    refreshLedger, refreshPersona, refreshRules, refreshProactivity, refreshPrivacy,
    refreshCapture, refreshGuard, refreshCloud, refreshAccount, refreshSoulous,
    refreshSyncConfig, refreshBackupCenter, refreshEmail, refreshToday, refreshWidgets, refreshDocuments,
    refreshReminderDeliveryHealth,
  ];
  const view = currentView || "chat";
  refreshAllFlight = (async () => {
    if (options && options.progress) beginRouteRefresh(view);
    try {
      const activeRefreshers = accountLoggedIn
        ? refreshers
        : refreshers.filter((refresh) => GUEST_ALLOWED_REFRESHERS.has(refresh));
      await settleRefreshers(activeRefreshers);
      if (currentView === "search") runGlobalSearch();
    } finally {
      if (options && options.progress) endRouteRefresh(view);
    }
  })().finally(() => { refreshAllFlight = null; });
  return refreshAllFlight;
}

// 四个一级入口对应 Alt+1…4；“/”随时进入本地搜索。表单输入中不抢键。
document.addEventListener("keydown", (event) => {
  if ($("privacyGate").classList.contains("open")) return;
  const target = event.target;
  const typing = target && (target.matches("input, textarea, select") || target.isContentEditable);
  if (!typing && event.altKey && !event.ctrlKey && !event.metaKey && /^[1-4]$/.test(event.key)) {
    event.preventDefault();
    gotoGroup(PRIMARY_GROUPS[Number(event.key) - 1]);
    return;
  }
  if (!typing && !event.altKey && !event.ctrlKey && !event.metaKey && event.key === "/") {
    event.preventDefault();
    showView("search");
    requestAnimationFrame(() => $("globalSearchInput").focus());
  }
});

/* ============================== 常驻宿主事件（Rust ticker → window） ============================== */
if (window.__TAURI__ && window.__TAURI__.event) {
  const { listen } = window.__TAURI__.event;
  // 流式聊天 token（§3.6 第 7 条）：按 streamId 路由到正在增长的气泡。
  listen("solum-chat-delta", (e) => {
    const p = e.payload || {};
    onChatDelta(p.streamId, p.delta);
  });
  listen("solum-fired", (e) => {
    toast((e.payload || []).join("；"), "amber");
    refreshNotifs(); refreshJournal(); refreshToday();
  });
  listen("solum-checkin", (e) => { showCheckin(e.payload); refreshJournal(); });
  listen("solum-suggestions", (e) => {
    const p = e.payload || {};
    const texts = Array.isArray(p) ? p : (p.texts || []);
    toast(`新增 ${texts.length} 条建议`, "bulb");
    // F18：采纳/忽略就地发生在对话流里。
    if (p.ui) {
      const bubble = addMsg("agent", [el("div", { class: "intent" }, tag("建议", "accent"))]);
      bubble.appendChild(renderEnvelope(p.ui));
    }
    refreshSuggest(); refreshToday();
  });
  listen("solum-daily-brief", (e) => {
    const env = e.payload;
    if (!env) return;
    const bubble = addMsg("agent", [el("div", { class: "intent" }, tag("今日简报", "accent"))]);
    // F18：推送与其他信封一样由既有目录渲染器负责，不另起渲染路径。
    bubble.appendChild(renderEnvelope(env));
    refreshToday(); refreshNotifs(); refreshSuggest();
  });
  listen("solum-captured", (e) => {
    toast((e.payload || []).join("；"), "clock");
    refreshAgenda(); refreshNotifs(); refreshToday(); refreshLedger();
  });
  // 后台 ticker 发现同步缺口时同样要落到常驻横幅上，而不是只写 stderr。
  listen("solum-sync-gap", () => { refreshSyncStatus(); });
  listen("solum-notification-processed", (e) => {
    toast(`已完成 ${e.payload || 0} 条普通通知分诊`, "clock");
    refreshAgenda(); refreshNotifs(); refreshToday(); refreshLedger();
  });
  listen("solum-health", (e) => {
    toast(`新增 ${e.payload} 条穿戴数据`, "heart");
    refreshWearable();
  });
  listen("solum-tick", refreshClockDrivenSurfaces);
  listen("solum-synced", (e) => {
    toast(`已从其他设备同步 ${e.payload} 条变更`, "sync");
    refreshAll();
  });
}

/* ============================== 启动 ============================== */
// 隐私门判定必须先完成；身份选择只能出现在同意之后。
const privacyGateReady = initPrivacyGate();
// 先从 Rust 取得本地 profile id，再读取 Web Storage/IndexedDB。否则账号用户
// 启动时会短暂渲染 guest 的聊天记录，而且 pagehide 还可能把错误分区写回。
initClock().then(async () => {
  if (typeof window !== "undefined" && window.i18n) {
    window.i18n.translateDom();
    initLanguageControl();
  }
  await privacyGateReady;
  migrateMemoryListUiState();
  await refreshAccount();
  const authExpiredRedirect = takeAuthExpiredRedirect();
  if (authExpiredRedirect && !accountLoggedIn && !$("privacyGate").classList.contains("open")) {
    setAccountGateMode("login");
    openAccountGate("登录状态已过期，请重新登录", { dismissible: false, allowGuest: true });
    $("accountGateStatus").textContent = "登录状态已过期失效，请重新登录以恢复云端服务。";
  }
  renderChatSessions();
  renderActiveChatSession();
  restoreCurrentChatDraft();
  syncChatContext().catch(() => { /* static preview has no IPC; sending stays guarded */ });
  const requestedView = $("view-" + uiState.lastView) ? uiState.lastView : "chat";
  const initialView = accountLoggedIn || GUEST_ALLOWED_VIEWS.has(requestedView) ? requestedView : "chat";
  showView(initialView, { refresh: false, animate: false });
  // 视图回退栈落锚：此刻起 showView 才开始压历史（武装前压栈会把启动态重复入栈）。
  try { history.replaceState({ solumView: initialView }, ""); } catch (_) { /* 静态预览无 history 能力 */ }
  viewHistoryArmed = true;
  refreshNotifAccess();
  // 首屏只读取导航角标与当前视图需要的数据。旧实现无差别启动二十多个
  // refresher，连未打开的邮箱、备份、隐私与设置页也会渲染一遍。
  beginRouteRefresh(initialView);
  try {
    await settleRefreshers([refreshNotifs, ...(VIEW_REFRESHERS[initialView] || [])]);
  } finally {
    endRouteRefresh(initialView);
  }
  if (!accountLoggedIn && !$("privacyGate").classList.contains("open")) {
    if (!authExpiredRedirect && !guestChoiceMade()) {
      openAccountGate("登录完整使用 Solum，或先从本地访客空间开始", { dismissible: false, allowGuest: true });
    }
  }
});
let clockRefreshFlight = null;
function refreshClockDrivenSurfaces() {
  if (document.visibilityState === "hidden") return Promise.resolve();
  if (clockRefreshFlight) return clockRefreshFlight;
  clockRefreshFlight = settleRefreshers([refreshNotifs, refreshToday])
    .finally(() => { clockRefreshFlight = null; });
  return clockRefreshFlight;
}
setInterval(refreshClockDrivenSurfaces, 30000); // 到点角标定期刷新；后台页不做 DOM/IPC 工作
window.addEventListener("pagehide", () => {
  if (currentView) {
    const panel = $("view-" + currentView);
    if (panel) uiState.scroll[currentView] = panel.scrollTop;
  }
  saveCurrentChatDraft(true);
  flushChatSessions();
  flushUiState();
});
// 宽屏今日面板在窗口跨过 1200px 断点时才首次可见——不挂 resize 就会一直是
// 空壳直到下一个 30s 周期（2026-07-17 走查发现 4）。
let todayResizeTimer = 0;
let todayPanelWide = window.innerWidth >= 1240;
window.addEventListener("resize", () => {
  clearTimeout(todayResizeTimer);
  todayResizeTimer = setTimeout(() => {
    const nextWide = window.innerWidth >= 1240;
    const becameVisible = nextWide && !todayPanelWide;
    todayPanelWide = nextWide;
    if (becameVisible) refreshToday();
  }, 200);
});

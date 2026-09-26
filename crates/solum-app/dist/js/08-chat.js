"use strict";
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
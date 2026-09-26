"use strict";
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
      ["data",        "数据与备份", "folder", "仅限本地手动文件导入与导出，丢失自负"],
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
  if (!accountLoggedIn) {
    openAccountGate(`请先登录 Solum 账号以使用完整功能`, { dismissible: false, allowGuest: false });
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

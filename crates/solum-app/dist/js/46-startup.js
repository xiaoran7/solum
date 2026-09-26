"use strict";
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
  // 首登签发的数据恢复码若尚未确认保存，登录态就绪后立即弹窗提醒。
  if (accountLoggedIn) { void maybeShowPendingRecoveryCode(); }
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
    openAccountGate("欢迎使用 Solum，请登录后开启完整功能", { dismissible: false, allowGuest: false });
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

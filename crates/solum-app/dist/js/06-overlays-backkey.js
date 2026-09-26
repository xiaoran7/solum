"use strict";
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

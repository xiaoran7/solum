"use strict";
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

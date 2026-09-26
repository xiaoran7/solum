"use strict";
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

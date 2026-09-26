"use strict";
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

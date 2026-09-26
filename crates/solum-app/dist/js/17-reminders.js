"use strict";
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

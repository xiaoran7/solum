"use strict";
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

"use strict";
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

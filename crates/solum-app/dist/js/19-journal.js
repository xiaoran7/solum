"use strict";
/* ============================== 行为日志 ============================== */
async function refreshJournal() {
  const box = $("journalBox");
  try {
    const rows = await invoke("behavior_log");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("journal", "行为日志为空", "在对话里回一句「我在……」试试。"));
      return;
    }
    box.appendChild(dataTable(["#", "时间", "类型", "内容", "来源"],
      rows.map((b) => {
        const k = BKIND[b.kind] || [b.kind, ""];
        return [
          td(String(b.id), "num"),
          [fmtDT(b.ts)],
          [tag(k[0], k[1])],
          [b.content],
          td(b.source || "—", "mut"),
        ];
      })));
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
$("checkinBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("checkin_now", { now: nowParam() });
    if (r) { showCheckin(r); toast("问询已发出（也会计入行为日志）", "clock"); }
    else toast("当前不需要问询：档位被动 / 非清醒时段 / 距上次太近");
    refreshJournal();
  } catch (e) { toast(String(e), "danger"); }
});

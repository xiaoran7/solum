"use strict";
/* ============================== 护栏 ============================== */
$("guardNoteIc").appendChild(icon("shield", 18));
async function refreshGuard() {
  const box = $("toolsBox");
  try {
    const ts = await invoke("tools");
    box.textContent = "";
    const list = el("div", { class: "rows" });
    for (const t of ts) {
      const r = RISK[t.risk] || [t.risk, ""];
      list.appendChild(el("div", { class: "row" }, [
        el("span", { class: "title", text: humanToolLabel(t.name) }),
        el("div", { class: "grow" }),
        tag(r[0], r[1]),
      ]));
    }
    box.appendChild(list);
    const sel = $("guardTool");
    const cur = sel.value;
    sel.textContent = "";
    for (const t of ts) sel.appendChild(el("option", { value: t.name, text: humanToolLabel(t.name) }));
    if ([...sel.options].some((o) => o.value === cur)) sel.value = cur;
    else if ([...sel.options].some((o) => o.value === "demo_delete")) sel.value = "demo_delete";
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
  refreshAudit();
}
async function refreshAudit() {
  const box = $("auditBox");
  try {
    const rows = await invoke("audit");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("shield", "暂无安全操作记录", "所有涉及敏感权限的操作与拦截结果都会安全记录在此。"));
      return;
    }
    box.appendChild(dataTable(["#", "时间", "风险", "操作", "结果", "详情"],
      rows.map((r) => {
        const rk = RISK[r.risk] || [r.risk, ""];
        return [
          td(String(r.id), "num"),
          [fmtDT(r.ts)],
          [tag(rk[0], rk[1])],
          [el("span", { class: "title", text: humanToolLabel(r.tool) })],
          [r.decision === "executed" ? tag("已执行", "ok") : tag("已拦截", "danger")],
          td(r.detail, "mut"),
        ];
      })));
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
function showGuardResult(ok, text) {
  const box = $("guardResult");
  box.className = ok ? "done" : "blocked";
  box.textContent = "";
  box.appendChild(icon(ok ? "check" : "x", 16));
  box.appendChild(el("span", { text: text }));
}
function openGuardConfirm(p, showFine) {
  const rk = RISK[p.risk] || [p.risk, ""];
  const body = el("div", null, [
    el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(p.tool) }), " ", tag(rk[0], rk[1])]),
    el("div", { class: "preview", text: "后果预览：" + p.preview }),
    showFine ? el("div", { class: "fine", text: "确认后签发一次性授权凭证（5 分钟内有效，仅限本次操作）。" }) : null,
  ]);
  openModal("人工确认：将要执行的操作", body, "确认执行", async () => {
    try {
      const r = await invoke("guard_confirm", {
        pendingId: p.pending_id, tool: p.tool, args: p.args, now: nowParam(),
      });
      if (showFine) showGuardResult(r.ok, r.output); else toast(r.output, r.ok ? "ok" : "danger");
      refreshGuard();
    } catch (e) {
      if (showFine) showGuardResult(false, safeUserError(e, "这次操作没有执行，请重新确认。")); else toast(safeUserError(e), "danger");
      refreshAudit();
    }
  }, { icon: "shield" });
}
$("guardRunBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("guard_run", { tool: $("guardTool").value, args: $("guardArgs").value, now: nowParam() });
    showGuardResult(r.ok, r.output);
    refreshAudit();
  } catch (e) { toast(String(e), "danger"); }
});
$("guardReqBtn").addEventListener("click", async () => {
  try {
    const p = await invoke("guard_request", { tool: $("guardTool").value, args: $("guardArgs").value, now: nowParam() });
    openGuardConfirm(p, true);
  } catch (e) { toast(String(e), "danger"); }
});

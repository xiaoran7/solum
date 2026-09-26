"use strict";
/* ============================== 规则 ============================== */
async function refreshRules() {
  const box = $("rulesBox");
  try {
    const rs = await invoke("rules");
    box.textContent = "";
    const rows = el("div", { class: "rows" });
    for (const rule of rs) rows.appendChild(el("div", { class: "row" }, [
      el("div", { class: "grow" }, [
        el("div", null, [tag(KIND[rule.kind] || rule.kind, "accent"), " ", el("span", { style: "font-weight:600;", text: KIND[rule.kind] || rule.kind })]),
        el("div", { class: "meta", text: `提前 ${rule.leads.join("、")} · ${rule.channels.map((c) => c === "push" ? "系统推送" : "应用横幅").join(" + ")}` }),
      ]),
      el("button", { class: "btn sm", type: "button", text: "编辑", onclick: () => renderRuleEditor(rule) }),
    ]));
    box.appendChild(rows);
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
// 规则编辑走浮窗（2026-07-20 重构）：列表保持在原地，不再被编辑表单整个替换。
function renderRuleEditor(rule) {
  const leadInput = el("input", { type: "text", value: (rule.leads || []).join(", "), placeholder: "如 3d, 1h 或 30m" });
  const push = el("input", { type: "checkbox", checked: (rule.channels || []).includes("push") });
  const banner = el("input", { type: "checkbox", checked: (rule.channels || []).includes("banner") });
  const body = el("div", null, [
    el("div", { class: "field", style: "margin-top:4px;" }, [
      el("label", { text: "提前提醒" }), leadInput,
      el("span", { class: "hint", text: "用逗号分隔 1–4 条，例如 3d, 1h；支持 m / h / d / w，0 表示准点提醒。" }),
    ]),
    el("div", { class: "field" }, [
      el("label", { text: "通知渠道" }),
      el("div", { class: "btnrow" }, [
        el("label", { class: "switchrow" }, [push, "系统推送"]),
        el("label", { class: "switchrow" }, [banner, "应用横幅"]),
      ]),
    ]),
    el("div", { class: "fine", text: "保存后立即重排未来日程尚未触发的提醒；历史记录不会被改写。" }),
  ]);
  openModal(`编辑规则：${KIND[rule.kind] || rule.kind}`, body, "保存并重排", async () => {
    const leads = leadInput.value.split(/[,，、]/).map((item) => item.trim()).filter(Boolean);
    const channels = [];
    if (push.checked) channels.push("push");
    if (banner.checked) channels.push("banner");
    try {
      const result = await invoke("rules_save", { rule: { kind: rule.kind, leads, channels }, now: nowParam() });
      toast(`规则已保存；已重排 ${result.replannedEvents || 0} 个未来日程的提醒`, "ok");
      refreshRules(); refreshNotifs(); refreshToday();
    } catch (e) { toast(String(e), "danger"); }
  }, { okStyle: "primary" });
  leadInput.focus();
}

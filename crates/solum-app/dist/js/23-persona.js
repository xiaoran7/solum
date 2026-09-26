"use strict";
/* ============================== 人格 ============================== */
function personaSummary(p) {
  const parts = [];
  if (p.nickname) parts.push(`称呼「${p.nickname}」`);
  if (p.tone) parts.push(`语气：${p.tone}`);
  if ((p.catchphrases || []).length) parts.push(`口头禅：${p.catchphrases.join("、")}`);
  if (p.style_notes) parts.push(`备注：${p.style_notes}`);
  return parts.join(" · ") || "—";
}
async function refreshPersona() {
  const act = $("personaActive"), vers = $("personaVersions");
  try {
    const d = await invoke("persona_get");
    act.textContent = "";
    if (d.active) {
      act.appendChild(el("div", { class: "row", style: "border:none;padding:0;" }, [
        tag("v" + d.active.version, "ok"),
        el("div", { class: "grow", text: personaSummary(d.active) }),
        el("span", { class: "meta" }, fmtDT(d.active.created_at)),
      ]));
      $("pNickname").value = d.active.nickname || "";
      $("pTone").value = d.active.tone || "";
      $("pCatch").value = (d.active.catchphrases || []).join(", ");
      $("pNotes").value = d.active.style_notes || "";
    } else {
      act.appendChild(el("div", { class: "empty", style: "padding:4px 2px;" },
        el("span", { class: "eh", text: "还没设置人格。展开下方「编辑并保存为新版本」填写风格设定即可。" })));
      $("personaEditFold").open = true; // 没设过人格时别把唯一的设置入口折起来
    }
    vers.textContent = "";
    if (d.versions.length) {
      vers.appendChild(dataTable(["版本", "时间", "设定", "说明", ""],
        d.versions.map((v) => {
          const isActive = d.active && v.version === d.active.version;
          return [
            [el("span", { class: "mono", text: "v" + v.version }), isActive ? el("span", null, [" ", tag("当前", "ok")]) : ""],
            [fmtDT(v.created_at)],
            [personaSummary(v)],
            td(v.note || "—", "mut"),
            [isActive ? "" : el("button", { class: "btn sm", text: "回滚到此版", onclick: () => rollbackPersona(v.version) })],
          ];
        })));
    } else {
      vers.appendChild(el("div", { class: "empty", style: "padding:4px 2px;" },
        el("span", { class: "eh", text: "还没有版本。" })));
    }
  } catch (e) { act.textContent = ""; act.appendChild(emptyBox("alert", safeUserError(e, "受保护操作暂时无法读取，请稍后重试。"))); }
}
$("personaSaveBtn").addEventListener("click", async () => {
  try {
    const p = await invoke("persona_set", {
      nickname: $("pNickname").value.trim() || null,
      tone: $("pTone").value,
      catchphrases: $("pCatch").value.split(/[,，、]/).map((s) => s.trim()).filter(Boolean),
      styleNotes: $("pNotes").value.trim() || null,
      note: $("pNote").value.trim() || null,
      now: nowParam(),
    });
    $("pNote").value = "";
    toast(`已保存人格 v${p.version}`, "ok");
    refreshPersona();
  } catch (e) { toast(String(e), "danger"); }
});
async function rollbackPersona(version) {
  try {
    const p = await invoke("persona_rollback", { version });
    toast(`已回滚到人格 v${p.version}（历史保留）`, "ok");
    refreshPersona();
  } catch (e) { toast(String(e), "danger"); }
}
$("personaClearBtn").addEventListener("click", () => {
  guardedDestructive({
    tool: "persona_clear",
    title: "人工确认：删除全部人格版本",
    note: "确认后签发一次性令牌。删除后闲聊与复盘退回默认助理口吻，版本历史不可恢复。",
    onDone: async () => { refreshPersona(); },
  });
});

/* ---------------- 聊天记录导入（F9 §3.4，纯本地） ---------------- */
$("pImportPick").addEventListener("click", () => $("pImportFile").click());
$("pImportFile").addEventListener("change", () => {
  const f = $("pImportFile").files[0];
  $("pImportName").textContent = f ? f.name : "未选择";
});
$("personaImportPreviewBtn").addEventListener("click", async () => {
  const file = $("pImportFile").files[0];
  const me = $("pImportMe").value.trim();
  if (!file) { toast("先选择聊天记录文件"); return; }
  if (!me) { toast("填一下你在记录里的昵称"); return; }
  const report = $("personaImportReport");
  try {
    const raw = await file.text();
    const resp = await invoke("persona_import_preview", { raw, me });
    const r = resp.report;
    const pair = (a) => a.map(([w, n]) => `${w}×${n}`).join("、");
    report.textContent = "";
    const lines = el("div", { style: "line-height:1.9;font-size:.875rem;" });
    lines.appendChild(el("div", null, [
      "共解析 ", el("strong", { text: String(r.total_messages) }), " 条消息，其中本人 ",
      el("strong", { text: String(r.my_messages) }), " 条",
      el("span", { style: "color:var(--muted);", text: `（发言人：${r.speakers.join("、")}）` }),
    ]));
    lines.appendChild(el("div", { text: `平均每条 ${r.avg_chars.toFixed(1)} 字` }));
    if (r.top_particles.length) lines.appendChild(el("div", { text: `常用语气词：${pair(r.top_particles)}` }));
    if (r.top_phrases.length) lines.appendChild(el("div", { text: `高频短语：${pair(r.top_phrases)}` }));
    lines.appendChild(el("div", { text: `推断语气：${r.suggested.tone}` }));
    for (const n of r.notes) lines.appendChild(el("div", null, [tag("注意", "accent"), " ", n]));
    report.appendChild(lines);
    // F18：初稿以就地编辑表单渲染（信封来自 solum-core），改完提交即保存为导入版本。
    if (resp.ui) report.appendChild(renderEnvelope(resp.ui));
    // 初稿同时填入上方编辑表单（昵称保留用户已有设定——聊天记录推断不了这个）。
    // 表单在折叠块里，必须展开，否则「已填入」是用户看不见的（2026-07-20 自查）。
    $("personaEditFold").open = true;
    $("pTone").value = r.suggested.tone || "";
    $("pCatch").value = (r.suggested.catchphrases || []).join(", ");
    $("pNotes").value = r.suggested.style_notes || "";
    $("pNote").value = `从聊天记录导入（${file.name}）`;
    $("personaImportSaveBtn").disabled = false;
    toast("提取完成（纯本地，未保存）", "ok");
  } catch (e) {
    report.textContent = "";
    report.appendChild(emptyBox("alert", safeUserError(e, "今日简报暂时无法生成，请稍后重试。")));
    $("personaImportSaveBtn").disabled = true;
  }
});
$("personaImportSaveBtn").addEventListener("click", async () => {
  try {
    const p = await invoke("persona_import_save", {
      nickname: $("pNickname").value.trim() || null,
      tone: $("pTone").value,
      catchphrases: $("pCatch").value.split(/[,，、]/).map((s) => s.trim()).filter(Boolean),
      styleNotes: $("pNotes").value.trim() || null,
      note: $("pNote").value.trim() || null,
      now: nowParam(),
    });
    $("pNote").value = "";
    $("personaImportSaveBtn").disabled = true;
    toast(`已保存导入人格 v${p.version}（可随时回滚）`, "ok");
    refreshPersona();
  } catch (e) { toast(String(e), "danger"); }
});

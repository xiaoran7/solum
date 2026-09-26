"use strict";
/* ============================== 持久化组件（F19） ============================== */
// This is intentionally independent from renderEnvelope(): F18 is an
// ephemeral snapshot protocol, while widgets re-fetch records and rebuild the
// DOM whenever the local data changes. All schema and record strings flow
// through `text`, never innerHTML.
const widgetState = { definitions: [], selectedId: null, sortBy: {}, descending: {} };

function fieldByName(definition, name) {
  return (definition.schema.fields || []).find((field) => field.name === name);
}
function listViewFor(definition) {
  return (definition.schema.views || []).find((view) => view.type === "list");
}
function viewOf(definition, type) {
  return (definition.schema.views || []).find((view) => view.type === type);
}
// stat 的算子由字段类型定死（设计稿 ②），这里必须和 Rust 侧 StatOp 一致：
// number → 合计，bool → 计为真的条数，其余 → 计已填写的条数。
function statTiles(definition, records) {
  const view = viewOf(definition, "stat");
  if (!view) return [];
  return view.fields.map((name) => {
    const field = fieldByName(definition, name);
    if (!field) return null;
    const values = records.map((r) => r.data && r.data[name]).filter((v) => v != null);
    let value, opLabel;
    if (field.type === "number") { value = values.reduce((a, b) => a + Number(b || 0), 0); opLabel = "合计"; }
    else if (field.type === "bool") { value = values.filter((v) => v === true).length; opLabel = "为是"; }
    else { value = values.length; opLabel = "已填"; }
    return { label: field.label, opLabel, value };
  }).filter(Boolean);
}
function renderWidgetStats(definition, records) {
  const tiles = statTiles(definition, records);
  if (!tiles.length) return null;
  const box = el("div", { class: "block" }, [el("h3", { text: "概览" })]);
  const row = el("div", { class: "statrow" });
  for (const tile of tiles) {
    row.appendChild(el("div", { class: "stattile" }, [
      el("div", { class: "statnum", text: String(Number(tile.value.toFixed(2))) }),
      el("div", { class: "statlabel", text: `${tile.label} · ${tile.opLabel}` }),
    ]));
  }
  box.appendChild(row);
  return box;
}
// table 与 list 共用数据绑定，只是排布成列（设计稿 ②：增量成本低）。
function renderWidgetTable(definition, records) {
  const view = viewOf(definition, "table");
  if (!view) return null;
  const box = el("div", { class: "block" }, [el("h3", { text: "表格" })]);
  const scroll = el("div", { class: "tscroll" });
  const table = el("table", { class: "wtable" });
  const head = el("tr");
  for (const name of view.fields) {
    const field = fieldByName(definition, name);
    head.appendChild(el("th", { text: field ? field.label : name }));
  }
  table.appendChild(el("thead", null, [head]));
  const body = el("tbody");
  const sorted = sortWidgetRecords(records, definition, view.sort_by);
  for (const record of sorted) {
    const tr = el("tr");
    for (const name of view.fields) {
      const field = fieldByName(definition, name);
      tr.appendChild(el("td", { text: formatWidgetValue(record.data && record.data[name], field) }));
    }
    body.appendChild(tr);
  }
  table.appendChild(body);
  scroll.appendChild(table);
  box.appendChild(scroll);
  return box;
}
function formViewFor(definition) {
  return (definition.schema.views || []).find((view) => view.type === "form");
}
function widgetFieldTypeLabel(field) {
  const labels = { text: "文本", number: "数字", date: "日期", datetime: "日期时间", time: "时刻", bool: "开关", enum: "单选" };
  return labels[field.type] || field.type;
}
function formatWidgetValue(value, field) {
  if (value == null || value === "") return "—";
  if (field && field.type === "bool") return value ? "是" : "否";
  return String(value);
}
function widgetInput(field, value, inputId) {
  if (field.type === "enum") {
    const input = el("select", { id: inputId }, (field.options || []).map((option) => el("option", { value: option, text: option })));
    if (value != null) input.value = String(value);
    return input;
  }
  if (field.type === "bool") return el("input", { id: inputId, type: "checkbox", checked: value === true });
  const type = { number: "number", date: "date", datetime: "datetime-local", time: "time" }[field.type] || "text";
  const attrs = { id: inputId, type };
  if (type === "number") attrs.step = "any";
  const input = el("input", attrs);
  if (value != null) input.value = String(value);
  return input;
}
function widgetFormData(inputs) {
  const data = {};
  for (const [field, input] of inputs) {
    if (field.type === "bool") { data[field.name] = input.checked; continue; }
    const value = String(input.value || "").trim();
    // Optional blanks are omitted. Required blanks deliberately reach the
    // strict Rust validator and return an actionable error instead of a guess.
    if (!value) continue;
    data[field.name] = field.type === "number" ? Number(value) : value;
  }
  return data;
}
// forcedSort 供 table 用：表格按 schema 声明的列排序，不跟着 list 的
// 用户选择走——两个视图各有各的序，共用数据不等于共用排序。
function sortWidgetRecords(records, definition, forcedSort) {
  const listView = listViewFor(definition);
  const available = (listView && listView.fields) || [];
  const fallback = (listView && listView.sort_by) || available[0] || "";
  const key = forcedSort
    || (available.includes(widgetState.sortBy[definition.id]) ? widgetState.sortBy[definition.id] : fallback);
  const descending = forcedSort ? false : !!widgetState.descending[definition.id];
  return records.slice().sort((left, right) => {
    const field = fieldByName(definition, key);
    const a = left.data && left.data[key];
    const b = right.data && right.data[key];
    if (a == null && b == null) return 0;
    if (a == null) return 1;
    if (b == null) return -1;
    let order;
    if (field && field.type === "number") order = Number(a) - Number(b);
    else if (field && field.type === "bool") order = Number(a) - Number(b);
    else order = String(a).localeCompare(String(b), "zh-Hans-CN", { numeric: true });
    return descending ? -order : order;
  });
}
function renderWidgetPreview(preview) {
  const definition = preview.definition || {};
  const root = el("div", { class: "widget-preview" });
  root.appendChild(el("h4", null, [icon(definition.icon, 17), el("span", { text: definition.name || "未命名组件" })]));
  const fields = el("ul", { class: "wfields" });
  for (const field of (definition.fields || [])) {
    const extra = field.type === "enum" ? `（${(field.options || []).join(" / ")}）` : "";
    fields.appendChild(el("li", null, [
      el("span", { text: `${field.label} · ${widgetFieldTypeLabel(field)}${field.required ? " · 必填" : ""}${extra} ` }),
      el("span", { class: "mono", text: field.name }),
    ]));
  }
  root.appendChild(fields);
  root.appendChild(el("div", { class: "meta", text: `视图：${(definition.views || []).map((view) => view.type).join("、") || "—"}` }));
  const actions = el("div", { class: "btnrow", style: "margin-top:12px;" });
  const confirm = el("button", { class: "btn primary", type: "button", text: "确认创建" });
  const discard = el("button", { class: "btn", type: "button", text: "放弃" });
  confirm.addEventListener("click", async () => {
    confirm.disabled = true; discard.disabled = true;
    try {
      const definition = await invoke("widget_confirm_preview", { previewId: preview.preview_id, now: nowParam() });
      widgetState.selectedId = definition.id;
      root.textContent = "";
      root.appendChild(el("div", { class: "llm-test ok", text: `已创建组件「${definition.name}」。` }));
      // 组件列表在抽屉里，抽屉关着时刷新等于渲染进看不见的地方——创建完必须给出去处，
      // 否则就是死胡同（工作台并入对话后的直接副作用，2026-07-20 自查发现）。
      root.appendChild(el("div", { class: "btnrow", style: "margin-top:10px;" }, [
        el("button", { class: "btn sm primary", type: "button", text: "打开组件", onclick: () => openDrawer("widgets") }),
      ]));
      toast("组件已创建", "ok");
      openDrawer("widgets");
    } catch (error) {
      confirm.disabled = false; discard.disabled = false;
      root.appendChild(el("div", { class: "llm-test danger", text: safeUserError(error, "组件操作暂时不可用，请稍后重试。") }));
    }
  });
  discard.addEventListener("click", async () => {
    try { await invoke("widget_discard_preview", { previewId: preview.preview_id }); } catch (_) { /* already consumed */ }
    root.textContent = "";
    root.appendChild(el("span", { class: "meta", text: "已放弃此组件预览，未写入任何定义。" }));
  });
  actions.append(confirm, discard);
  root.appendChild(actions);
  return root;
}
// Add-only schema evolution (设计稿 ⑧). No "required" control on purpose:
// existing records could never satisfy it, and the core rejects it anyway.
function renderWidgetAddField(definition, afterSave) {
  const root = el("div", { class: "wform" });
  root.appendChild(el("div", { class: "meta", text: "只能新增可空字段。已有字段不能删除或改类型——那会丢数据；要改就新建一个组件再导入。" }));
  const nameIn = el("input", { id: "wAddName", type: "text", placeholder: "字段键（小写英文，如 notes）" });
  const labelIn = el("input", { id: "wAddLabel", type: "text", placeholder: "显示名（如 备注）" });
  const typeIn = el("select", { id: "wAddType" }, [
    ["text", "文本"], ["number", "数字"], ["date", "日期"], ["datetime", "日期时间"],
    ["time", "时刻"], ["bool", "开关"], ["enum", "单选"],
  ].map(([value, text]) => el("option", { value, text })));
  const optionsIn = el("input", { id: "wAddOptions", type: "text", placeholder: "选项，用逗号分隔" });
  const optionsRow = el("div", { class: "frow" }, [el("label", { text: "选项" }), optionsIn]);
  optionsRow.hidden = true;
  typeIn.addEventListener("change", () => { optionsRow.hidden = typeIn.value !== "enum"; });

  root.append(
    el("div", { class: "frow" }, [el("label", { text: "字段键" }), nameIn]),
    el("div", { class: "frow" }, [el("label", { text: "显示名" }), labelIn]),
    el("div", { class: "frow" }, [el("label", { text: "类型" }), typeIn]),
    optionsRow,
  );
  const save = el("button", { class: "btn primary", type: "button", text: "加入组件" });
  const cancel = el("button", { class: "btn", type: "button", text: "取消", onclick: afterSave });
  save.addEventListener("click", async () => {
    save.disabled = true;
    try {
      await invoke("widget_add_field", {
        widgetId: definition.id,
        field: {
          name: nameIn.value.trim(),
          label: labelIn.value.trim(),
          type: typeIn.value,
          required: false,
          options: typeIn.value === "enum"
            ? optionsIn.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean)
            : [],
        },
        now: nowParam(),
      });
      toast("字段已加入", "ok");
      await afterSave();
    } catch (error) {
      save.disabled = false;
      root.appendChild(el("div", { class: "llm-test danger", text: safeUserError(error, "组件操作暂时不可用，请稍后重试。") }));
    }
  });
  root.appendChild(el("div", { class: "btnrow", style: "margin-top:12px;" }, [save, cancel]));
  return root;
}

function renderWidgetCatalog() {
  const box = $("widgetsBox");
  box.textContent = "";
  if (!widgetState.definitions.length) {
    box.appendChild(emptyBox("doc", "还没有组件", "在对话里说「帮我弄一个收支记录组件」，我会先给你预览，确认后才保存。"));
    return;
  }
  const list = el("div", { class: "rows" });
  for (const definition of widgetState.definitions) {
    const fields = (definition.schema.fields || []).map((field) => field.label).join("、");
    list.appendChild(el("button", { class: "widget-def", type: "button", onclick: () => showWidget(definition.id) }, [
      icon(definition.icon, 19),
      el("div", { class: "grow" }, [
        el("div", { class: "wname", text: definition.name }),
        el("div", { class: "wmeta", text: `${(definition.schema.fields || []).length} 个字段 · ${fields}` }),
      ]),
      icon("doc", 16),
    ]));
  }
  box.appendChild(list);
}
function renderWidgetEditor(definition, record, afterSave) {
  const formView = formViewFor(definition);
  const data = record ? record.data || {} : {};
  const form = el("form", { class: "formcol" });
  const inputs = [];
  for (const name of (formView && formView.fields) || []) {
    const field = fieldByName(definition, name);
    if (!field) continue;
    const inputId = `widget-${definition.id}-${record ? record.id : "new"}-${field.name}`;
    const input = widgetInput(field, data[field.name], inputId);
    inputs.push([field, input]);
    form.appendChild(el("div", { class: "field" }, [
      el("label", { for: inputId, text: `${field.label}${field.required ? "（必填）" : ""}` }), input,
    ]));
  }
  const actions = el("div", { class: "btnrow" });
  const save = el("button", { class: "btn primary", type: "submit", text: record ? "保存修改" : "添加记录" });
  actions.appendChild(save);
  if (record) actions.appendChild(el("button", { class: "btn", type: "button", text: "取消", onclick: afterSave }));
  form.appendChild(actions);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    save.disabled = true;
    try {
      const data = widgetFormData(inputs);
      if (record) await invoke("widget_record_update", { widgetId: definition.id, recordId: record.id, data });
      else await invoke("widget_record_create", { widgetId: definition.id, data, now: nowParam() });
      toast(record ? "记录已更新" : "记录已添加", "ok");
      await showWidget(definition.id);
    } catch (error) {
      save.disabled = false;
      form.appendChild(el("div", { class: "llm-test danger", text: safeUserError(error, "组件记录暂时无法保存，请稍后重试。") }));
    }
  });
  return form;
}
// 破坏性操作统一的 Guard 面：预览 → 人工确认 → 一次性令牌 → 审计。
// 三处删除（记忆台账 / 人格清空 / 组件记录）此前是「前端 modal + 直调 IPC」，
// 而 modal 是渲染不是授权边界——命令本身可以被直接调用。收口到这里之后，
// 没有 guard.rs 签发的令牌就执行不了，且预览文案由后端生成、执行前会重新
// 比对（效果摘要绑定），预览与实际后果不一致时自动作废。
async function guardedDestructive(opts) {
  try {
    const args = JSON.stringify(opts.args || {});
    const pending = await invoke("guard_request", { tool: opts.tool, args, now: nowParam() });
    const body = el("div", null, [
      el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(pending.tool) }), " ", tag("高危", "danger")]),
      el("div", { class: "preview", text: "后果预览：" + pending.preview }),
      el("div", { class: "fine", text: opts.note || "确认后签发一次性令牌，绑定这一次操作；令牌用后即焚。" }),
    ]);
    openModal(opts.title, body, opts.confirmLabel || "永久删除", async () => {
      try {
        const result = await invoke("guard_confirm", { pendingId: pending.pending_id, tool: pending.tool, args: pending.args, now: nowParam() });
        toast(result.output, result.ok ? "ok" : "danger");
        if (opts.onDone) { await opts.onDone(result.ok); }
        refreshAudit();
      } catch (error) {
        toast(String(error), "danger");
        if (opts.onError) { await opts.onError(error); }
        refreshAudit();
      }
    }, { icon: "shield" });
  } catch (error) { toast(String(error), "danger"); }
}
async function requestWidgetDelete(definition) {
  try {
    const args = JSON.stringify({ widget_id: definition.id });
    const pending = await invoke("guard_request", { tool: "widget_delete", args, now: nowParam() });
    const body = el("div", null, [
      el("div", null, ["操作：", el("span", { class: "title", text: humanToolLabel(pending.tool) }), " ", tag("高危", "danger")]),
      el("div", { class: "preview", text: "后果预览：" + pending.preview }),
      el("div", { class: "fine", text: "确认后签发一次性令牌，绑定此组件删除；令牌用后即焚。" }),
    ]);
    openModal("人工确认：删除组件", body, "永久删除", async () => {
      try {
        const result = await invoke("guard_confirm", { pendingId: pending.pending_id, tool: pending.tool, args: pending.args, now: nowParam() });
        toast(result.output, result.ok ? "ok" : "danger");
        if (result.ok) widgetState.selectedId = null;
        await refreshWidgets();
        refreshAudit();
      } catch (error) { toast(String(error), "danger"); refreshAudit(); }
    }, { icon: "shield" });
  } catch (error) { toast(String(error), "danger"); }
}
async function showWidget(widgetId) {
  const definition = widgetState.definitions.find((item) => item.id === widgetId);
  if (!definition) { widgetState.selectedId = null; renderWidgetCatalog(); return; }
  widgetState.selectedId = widgetId;
  const box = $("widgetsBox");
  box.textContent = "";
  try {
    const records = await invoke("widget_records", { widgetId });
    // 抽屉是窄容器：返回 + 组件名占标题行，三个次要操作收进「⋯」浮窗。
    const moreWrap = el("div", { class: "dh-more" });
    let moreMenu = null;
    let moreOutsideHandler = null;
    const closeMore = () => {
      if (moreOutsideHandler) {
        document.removeEventListener("click", moreOutsideHandler);
        moreOutsideHandler = null;
      }
      if (moreMenu) { moreMenu.remove(); moreMenu = null; moreBtn.setAttribute("aria-expanded", "false"); }
    };
    const moreBtn = el("button", { class: "btn icon", type: "button", title: "更多操作", "aria-label": "更多操作", "aria-expanded": "false" }, icon("settings", 17));
    const head = el("div", { class: "drawer-detail-head" }, [
      el("button", { class: "btn icon", type: "button", title: "返回组件列表", "aria-label": "返回组件列表",
        onclick: () => { widgetState.selectedId = null; renderWidgetCatalog(); } }, icon("back", 17)),
      el("div", { class: "dh-title" }, [icon(definition.icon, 17), el("span", { text: definition.name })]),
      moreWrap,
    ]);
    const menuActions = [
      el("button", { type: "button", text: "从日程导入", title: "把已有日程拷贝成记录。是快照不是链接：之后改日程不会影响这些记录。", onclick: async (event) => {
          const button = event.currentTarget; button.disabled = true;
          try {
            const r = await invoke("widget_import_events", { widgetId: definition.id, limit: 20, now: nowParam() });
            // 「导入 0 条」单独出现是无从下手的——把跳过的条数和头几条理由一起说，
            // 用户才分得清「没有日程」和「日程都对不上字段」。
            const parts = [];
            if (r.imported) parts.push(`已导入 ${r.imported} 条日程快照`);
            if (r.skipped) parts.push(`跳过 ${r.skipped} 条`);
            if (!r.imported && !r.skipped) parts.push("没有可导入的日程");
            let text = parts.join("，");
            if (r.reasons && r.reasons.length) {
              const shown = r.reasons.map((x) => `「${x.title}」${x.reason}`).join("；");
              text += `：${shown}${r.skipped > r.reasons.length ? " 等" : ""}`;
            }
            toast(text, r.imported ? "ok" : "");
            await showWidget(definition.id);
          } catch (error) { toast(String(error), "danger"); }
          finally { button.disabled = false; }
        } }),
      el("button", { type: "button", text: "加字段", onclick: () => {
        const editor = $("widgetEditor"); editor.textContent = "";
        editor.appendChild(renderWidgetAddField(definition, () => showWidget(definition.id)));
        editor.scrollIntoView({ block: "nearest" });
      } }),
      el("button", { type: "button", class: "danger", text: "删除组件", onclick: () => requestWidgetDelete(definition) }),
    ];
    moreBtn.addEventListener("click", (event) => {
      event.stopPropagation();
      if (moreMenu) { closeMore(); return; }
      moreMenu = el("div", { class: "dh-menu" }, menuActions);
      // 点菜单项后收起菜单（动作本身各自负责刷新/开模态）
      moreMenu.addEventListener("click", (e) => { if (e.target.closest("button")) setTimeout(closeMore, 0); });
      moreWrap.appendChild(moreMenu);
      moreBtn.setAttribute("aria-expanded", "true");
      moreOutsideHandler = (e) => { if (!e.target.closest(".dh-more")) closeMore(); };
      document.addEventListener("click", moreOutsideHandler);
    });
    moreWrap.appendChild(moreBtn);
    box.appendChild(head);
    const statBlock = renderWidgetStats(definition, records);
    if (statBlock) box.appendChild(statBlock);
    const listView = listViewFor(definition);
    const fields = (listView && listView.fields) || [];
    const defaultSort = (listView && listView.sort_by) || fields[0] || "";
    const selectedSort = fields.includes(widgetState.sortBy[definition.id]) ? widgetState.sortBy[definition.id] : defaultSort;
    widgetState.sortBy[definition.id] = selectedSort;
    const sort = el("select", { "aria-label": "记录排序字段" }, fields.map((name) => {
      const field = fieldByName(definition, name);
      return el("option", { value: name, text: field ? field.label : name });
    }));
    sort.value = selectedSort;
    sort.addEventListener("change", (event) => { widgetState.sortBy[definition.id] = event.currentTarget.value; showWidget(definition.id); });
    const direction = el("select", { "aria-label": "记录排序方向" }, [el("option", { value: "asc", text: "升序" }), el("option", { value: "desc", text: "降序" })]);
    direction.value = widgetState.descending[definition.id] ? "desc" : "asc";
    direction.addEventListener("change", (event) => { widgetState.descending[definition.id] = event.currentTarget.value === "desc"; showWidget(definition.id); });
    const listBlock = el("div", { class: "block" }, [
      el("h3", { text: "记录" }),
      el("div", { class: "vactions", style: "margin-bottom:10px;" }, [el("span", { class: "inline-num", text: "按" }), sort, direction]),
    ]);
    const rows = el("div", { class: "rows" });
    const sorted = sortWidgetRecords(records, definition);
    if (!sorted.length) rows.appendChild(emptyBox("doc", "还没有记录", "用下方表单添加第一条；所有记录只保存在本机。"));
    for (const record of sorted) {
      const primary = fieldByName(definition, fields[0]);
      const details = fields.slice(1).map((name) => {
        const field = fieldByName(definition, name);
        return `${field ? field.label : name}：${formatWidgetValue(record.data && record.data[name], field)}`;
      }).join(" · ");
      const row = el("div", { class: "row widget-record" }, [
        el("div", { class: "grow" }, [
          el("div", { class: "record-title", text: formatWidgetValue(record.data && record.data[fields[0]], primary) }),
          el("div", { class: "record-meta", text: details || `创建于 ${fmtDTText(record.created_at)}` }),
        ]),
        el("div", { class: "record-actions" }, [
          el("button", { class: "btn sm", type: "button", text: "编辑", onclick: () => {
            const editor = $("widgetEditor"); editor.textContent = ""; editor.appendChild(renderWidgetEditor(definition, record, () => showWidget(definition.id)));
          } }),
          // 提升是快照拷贝（设计稿 ⑦-C）：生成一条独立日程，此后与本记录无关。
          el("button", { class: "btn sm", type: "button", text: "提为日程", onclick: async (event) => {
            const button = event.currentTarget; button.disabled = true;
            try {
              const created = await invoke("widget_promote_record", { widgetId: definition.id, recordId: record.id, now: nowParam() });
              toast(`已生成日程「${created.title}」（快照，改日程不影响本记录）`, "ok");
              refreshAll();
            } catch (error) { toast(String(error), "danger"); }
            finally { button.disabled = false; }
          } }),
          el("button", { class: "btn sm danger", type: "button", text: "删除", onclick: async () => {
            await guardedDestructive({
              tool: "widget_record_delete",
              args: { widget_id: definition.id, record_id: record.id },
              title: "人工确认：删除记录",
              onDone: async () => { await showWidget(definition.id); },
            });
          } }),
        ]),
      ]);
      rows.appendChild(row);
    }
    listBlock.appendChild(rows);
    box.appendChild(listBlock);
    const tableBlock = renderWidgetTable(definition, records);
    if (tableBlock) box.appendChild(tableBlock);
    const editorBlock = el("div", { class: "block" }, [el("h3", { text: "添加记录" })]);
    const editor = el("div", { id: "widgetEditor" });
    editor.appendChild(renderWidgetEditor(definition, null, () => showWidget(definition.id)));
    editorBlock.appendChild(editor);
    box.appendChild(editorBlock);
  } catch (error) { box.appendChild(emptyBox("alert", "无法读取组件", safeUserError(error, "组件暂时无法读取，请稍后重试。"))); }
}
async function refreshWidgets() {
  // 当前工作台壳层未挂载组件面板时，后台刷新不能阻断其他页面初始化。
  if (!$("widgetsBox")) return;
  try {
    widgetState.definitions = await invoke("widget_defs");
    if (widgetState.selectedId && widgetState.definitions.some((item) => item.id === widgetState.selectedId)) {
      await showWidget(widgetState.selectedId);
    } else {
      widgetState.selectedId = null;
      renderWidgetCatalog();
    }
  } catch (error) {
    const box = $("widgetsBox");
    box.textContent = "";
    box.appendChild(emptyBox("alert", "无法读取组件", safeUserError(error, "组件暂时无法读取，请稍后重试。")));
  }
}

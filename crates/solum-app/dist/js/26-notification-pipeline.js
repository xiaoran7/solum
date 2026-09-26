"use strict";
/* ============================== 通知智能管线（F20） ============================== */
let notificationAppCatalog = [];
let notificationConfigForPicker = null;
function appNameForPackage(packageName) {
  return notificationAppCatalog.find((app) => app.packageName === packageName)?.name || "已授权应用";
}
function renderNotificationAppPicker(config) {
  const picker = $("notifAppPicker");
  picker.textContent = "";
  if (!config || config.capture_mode !== "allowlist") return;
  const allowed = new Set((config && config.allowed_packages) || []);
  const query = $("notifAppSearch").value.trim().toLocaleLowerCase();
  const apps = notificationAppCatalog.filter((app) => !allowed.has(app.packageName)
    && (!query || app.name.toLocaleLowerCase().includes(query)));
  if (!notificationAppCatalog.length) {
    $("notifAppHint").textContent = "应用列表只在 Android 设备上提供；桌面端不读取第三方应用通知。";
    picker.appendChild(el("div", { class: "hint", text: "请在 Android 设备上打开此页，息壤 会列出可选择的已安装应用。" }));
    return;
  }
  $("notifAppHint").textContent = "只会显示应用名称；点选后才允许 息壤 读取该应用通知。";
  if (!apps.length) {
    picker.appendChild(el("div", { class: "hint", text: query ? "没有匹配的应用" : "所有可见应用都已选择，或系统没有返回可选应用。" }));
    return;
  }
  const list = el("div", { class: "rows" });
  for (const app of apps.slice(0, 60)) list.appendChild(el("div", { class: "row" }, [
    el("span", { class: "grow", text: app.name }),
    el("button", { class: "btn sm", type: "button", text: "选择", onclick: () => setNotificationApp(app.packageName, true) }),
  ]));
  if (apps.length > 60) list.appendChild(el("div", { class: "hint", text: `仅显示前 60 个匹配项，请继续输入应用名称缩小范围。` }));
  picker.appendChild(list);
}
function captureState(state) {
  return {
    queued: ["待处理", "hollow"], event_created: ["已建事件", "ok"],
    filtered: ["已过滤", ""], deduplicated: ["已判重", ""], needs_review: ["待回看", "accent"],
    resolved: ["已处理", "ok"],
  }[state] || [state, ""];
}
function renderNotificationLedger(box, s) {
  box.textContent = "";
  const captures = s.captures || [];
  const pending = (s.filter_proposals || []).filter((p) => p.state === "pending");
  const actionPending = (s.action_proposals || []).filter((p) => p.state === "pending");
  // 已是独立「通知回看」视图（vhead 就是标题），这里不再重复 h3。
  const heading = el("div", { class: "block" }, [
    el("p", { class: "hint", style: "margin:0 0 10px;", text: captures.length
      ? "每条已捕获通知都在这里留下去向；过滤、判重和待处理均可回看。"
      : "还没有已捕获通知。授予 Android 通知访问后，息壤 会按设置的接收范围在后台静默整理。" }),
  ]);
  if (captures.length) {
    const rows = el("div", { class: "rows" });
    for (const capture of captures.slice(0, 20)) {
      const status = captureState(capture.state);
      const actions = [];
      if (["filtered", "deduplicated", "needs_review"].includes(capture.state)) {
        actions.push(el("button", { class: "btn sm", text: "恢复处理", onclick: (event) => restoreNotificationCapture(capture.id, event.currentTarget) }));
      }
      if (!["event_created", "resolved"].includes(capture.state)) {
        actions.push(el("button", { class: "btn sm", text: "提升为日程", title: "仅在本地规则能确定时间时创建日程", onclick: (event) => promoteNotificationCapture(capture.id, event.currentTarget) }));
      }
      const metadata = [
        appNameForPackage(capture.package_name), " · ", capture.lane === "urgent" ? "重要即时" : "普通批量", " · ", fmtDT(capture.received_at),
      ];
      if (capture.local_only) metadata.push(" · 仅本机");
      if (capture.reason) metadata.push(" · " + capture.reason);
      rows.appendChild(el("div", { class: "row", style: "align-items:flex-start;" }, [
        el("div", { class: "grow" }, [
          el("div", { text: capture.title || "（无标题通知）" }),
          capture.body ? el("div", { class: "hint", style: "margin-top:2px;display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;overflow:hidden;", text: capture.body }) : null,
          el("div", { class: "hint" }, metadata),
        ]),
        tag(status[0], status[1]),
        actions.length ? el("span", { style: "display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end;" }, actions) : null,
      ]));
    }
    heading.appendChild(rows);
  }
  if (pending.length) {
    const rows = el("div", { class: "rows", style: "margin-top:12px;" });
    rows.appendChild(el("div", { class: "hint", text: "待确认的 LLM 过滤规则（确认前不会生效）：" }));
    for (const proposal of pending) {
      rows.appendChild(el("div", { class: "row" }, [
        el("div", { class: "grow" }, [
          el("div", { text: `过滤「${proposal.pattern}」` }),
          el("div", { class: "hint", text: `${proposal.package_name ? appNameForPackage(proposal.package_name) : "所有已选择应用"}${proposal.reason ? " · " + proposal.reason : ""}` }),
        ]),
        el("button", { class: "btn sm primary", text: "确认规则", onclick: (event) => decideNotificationProposal(proposal.id, true, event.currentTarget) }),
        el("button", { class: "btn sm", text: "忽略", onclick: (event) => decideNotificationProposal(proposal.id, false, event.currentTarget) }),
      ]));
    }
    heading.appendChild(rows);
  }
  if (actionPending.length) {
    const rows = el("div", { class: "rows", style: "margin-top:12px;" });
    rows.appendChild(el("div", { class: "hint", text: "待确认的通知动作提议（目标由本机匹配，点按前不会改动日程）：" }));
    for (const proposal of actionPending) {
      const isCancel = proposal.kind === "cancel_event";
      const event = proposal.event || null;
      const title = event?.title || proposal.event_title || "（原日程已不存在）";
      const originalStart = event?.start ? fmtDTText(event.start) : "原日程已不存在";
      const newStart = proposal.new_start ? fmtDTText(proposal.new_start) : "新时间未提供";
      const summary = isCancel
        ? `建议取消「${title}」（原时间：${originalStart}）`
        : `建议把「${title}」从 ${originalStart} 改到 ${newStart}`;
      rows.appendChild(el("div", { class: "row", style: "align-items:flex-start;" }, [
        el("div", { class: "grow" }, [
          el("div", { text: summary }),
          el("div", { class: "hint", text: proposal.reason || "来自通知分诊；仍需你确认" }),
        ]),
        event ? el("button", { class: `btn sm ${isCancel ? "danger" : "primary"}`, text: isCancel ? `确认取消「${title}」` : "确认改期", onclick: (event) => decideNotificationActionProposal(proposal.id, true, event.currentTarget) }) : null,
        el("button", { class: "btn sm", text: "忽略", onclick: (event) => decideNotificationActionProposal(proposal.id, false, event.currentTarget) }),
      ]));
    }
    heading.appendChild(rows);
  }
  box.appendChild(heading);
}
async function renderNotificationIntelligenceSettings(s) {
  const apps = $("notifIntelApps"), rules = $("notifIntelRules"), pipeline = $("notifPipelineState"), scope = $("notifPriorityPackage");
  // 事后发现机制：某个应用写得比你预期的多，这里能看出来。它不构成授权。
  let autoEventCounts = {};
  try {
    for (const [pkg, n] of await invoke("notif_intelligence_auto_event_counts", { now: nowParam() })) {
      autoEventCounts[pkg] = n;
    }
  } catch (_) { /* 统计拿不到不影响授权界面本身 */ }
  const config = s.config || { capture_mode: "all", allowed_packages: [], auto_event_packages: [], filter_rules: [] };
  const captureMode = config.capture_mode === "allowlist" ? "allowlist" : "all";
  notificationConfigForPicker = config;
  $("notifCaptureMode").value = captureMode;
  $("notifAllowlistControls").hidden = captureMode !== "allowlist";
  $("notifCaptureModeHint").textContent = captureMode === "allowlist"
    ? "只有白名单里的应用消息会进入本机处理管线。"
    : "接收所有第三方应用消息；后台静默整理，需确认或明确开启的提醒才会提示。";
  $("notifBatchInterval").value = String(config.batch_interval_minutes || 15);
  apps.textContent = "";
  const allowed = config.allowed_packages || [];
  const selectedScope = scope.value;
  scope.textContent = "";
  scope.appendChild(el("option", { value: "", text: "全局（所有已接收的应用）" }));
  for (const pkg of allowed) scope.appendChild(el("option", { value: pkg, text: appNameForPackage(pkg) }));
  scope.value = allowed.includes(selectedScope) ? selectedScope : "";
  // 采集丢失必须说出来。这两类都是「告诉过用户会捕获、实际没捕获」的通知，
  // 只写日志等于没说。undeliverable 的原始内容还留在 notif-spool/failed/，
  // 所以文案要区分「彻底丢了」和「留着但没进来」。
  const lostBox = $("notifIntakeLoss");
  if (lostBox) {
    lostBox.textContent = "";
    const overflow = s.intake_overflow || 0, undeliverable = s.intake_undeliverable || 0;
    if (overflow || undeliverable) {
      lostBox.hidden = false;
      const parts = [];
      // marker 本身也有上限（原生侧 2000）。到顶之后就是欠计数了，
      // 这时说「N 条」是在给一个我们拿不到的精度——只能说「至少」。
      if (overflow) {
        const atLeast = overflow >= 2000 ? "至少 " : "";
        parts.push(`${atLeast}${overflow} 条因待处理队列已满被丢弃（内容未保留）`);
      }
      // 这一条是「此刻盘上还留着几份」，不是累计事件数——所以用现在时说。
      if (undeliverable) parts.push(`当前有 ${undeliverable} 条多次尝试后仍未能入库，原始内容留存在 notif-spool/failed/（未删除）`);
      lostBox.appendChild(el("div", { text: `有通知没能进入 息壤：${parts.join("；")}。` }));
      lostBox.appendChild(el("div", { class: "fine", style: "margin-top:4px;",
        text: "常见原因是应用长期未运行或存储空间不足。恢复后新通知会正常捕获；" +
              "被丢弃的无法找回，留存在 failed/ 的仍可人工查看。计数只增不虚报，" +
              "但存储异常时可能连丢弃本身都没能记下，因此是下限。" }));
      lostBox.appendChild(el("button", { class: "btn sm", style: "margin-top:8px;", text: "我知道了",
        onclick: async () => {
          try { await invoke("notif_intelligence_acknowledge_losses"); refreshNotificationIntelligence(); }
          catch (e) { toast(String(e), "danger"); }
        } }));
    } else {
      lostBox.hidden = true;
    }
  }

  // 两种授权分开呈现：读取通知 ≠ 允许它往日历里写东西。
  // 上面那句「选择后才允许读取该应用通知」授的是前者；自动建日程必须在这里
  // 逐个应用单独打开，默认关闭。
  const autoPkgs = config.auto_event_packages || [];
  const managedPackages = captureMode === "allowlist" ? allowed : autoPkgs;
  apps.appendChild(el("div", { class: "rows" }, managedPackages.length
    ? managedPackages.map((pkg) => {
        const on = autoPkgs.includes(pkg);
        const made = (autoEventCounts[pkg] || 0);
        return el("div", { class: "row", style: "flex-wrap:wrap;gap:8px;" }, [
          el("span", { class: "grow", text: appNameForPackage(pkg) }),
          el("span", { class: "hint", style: "flex-basis:100%;order:3;margin:0;" }, [
            el("span", { text: on
              ? `已允许自动建日程${made ? `（近 7 天创建 ${made} 条）` : "（近 7 天未创建）"}`
              : "仅读取通知，不会自动建日程；识别到的内容会进「通知回看」等你确认。" }),
          ]),
          el("button", {
            class: on ? "btn sm danger" : "btn sm",
            text: on ? "撤销自动建日程" : "允许自动建日程",
            title: on
              ? "撤销后，该应用的通知仍会被读取，但不再自动写入日历"
              : "单独授权：允许这个应用的通知在本地解析出确定时间时，直接创建日程和提醒",
            onclick: () => setNotificationAppAutoEvent(pkg, !on),
          }),
          captureMode === "allowlist"
            ? el("button", { class: "btn sm", text: "移出白名单", onclick: () => setNotificationApp(pkg, false) })
            : null,
        ]);
      })
    : [el("div", { class: "hint", text: captureMode === "all"
      ? "当前接收所有第三方应用消息；自动建日程仍保持逐应用、默认关闭。切换到白名单后可管理逐应用授权。"
      : "白名单为空：息壤 不会读取第三方应用消息。" })]));
  rules.textContent = "";
  const allRules = s.priority_rules || [];
  const filterRules = config.filter_rules || [];
  const list = el("div", { class: "rows" });
  list.appendChild(el("div", { class: "hint", text: "重要路由规则（命中只影响分诊时机，不授权任何动作）：" }));
  if (!allRules.length) list.appendChild(el("div", { class: "hint", text: "未配置；允许应用时会加入可编辑的预设。" }));
  for (const rule of allRules) list.appendChild(el("div", { class: "row" }, [
    el("span", { class: "grow", text: `${rule.pattern} · ${rule.package_name ? appNameForPackage(rule.package_name) : "所有已接收应用"} · ${rule.matcher === "regex" ? "正则" : "包含"}` }),
    el("button", { class: "btn icon danger", title: "删除即时规则", "aria-label": "删除即时规则", onclick: () => removeNotificationPriorityRule(rule.id) }, icon("trash", 16)),
  ]));
  if (filterRules.length) {
    list.appendChild(el("div", { class: "hint", style: "margin-top:8px;", text: "已确认过滤规则（可撤销，已过滤通知仍在台账）：" }));
    for (const rule of filterRules) list.appendChild(el("div", { class: "row" }, [
      el("span", { class: "grow", text: `${rule.pattern} · ${rule.package_name ? appNameForPackage(rule.package_name) : "所有已接收应用"}` }),
      el("button", { class: "btn sm", text: "撤销", onclick: () => removeNotificationFilterRule(rule.id) }),
    ]));
  }
  rules.appendChild(list);
  renderNotificationAppPicker(config);
  pipeline.textContent = "";
  const p = s.pipeline || {};
  $("notifPipelineAndroidControls").hidden = !p.supported;
  if (p.supported && !p.running) {
    // Android 上服务没跑才是需要用户行动的事，用横幅；其余是稳态陈述。
    pipeline.appendChild(el("div", { class: "banner", style: "margin-top:12px;" }, [
      icon("alert", 18),
      el("div", { class: "grow" }, [
        el("div", { text: "Android 前台处理服务未运行" }),
        el("div", { class: "hint", text: p.ignoring_battery_optimizations
          ? "已获电池优化豁免；国产 ROM 仍可能需要在系统的后台/自启动设置中手动放行。"
          : "建议允许忽略电池优化；国产 ROM 仍可能需要在系统的后台/自启动设置中手动放行。" }),
      ]),
    ]));
  } else {
    pipeline.appendChild(el("div", { class: "statusline", style: "margin-top:12px;", title: p.supported
        ? "国产 ROM 仍可能需要在系统的后台/自启动设置中手动放行。"
        : "电池优化与 ROM 后台设置只适用于 Android，桌面端无需操作。" }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: p.supported ? "Android 前台处理服务正在运行" : "前台处理服务未运行（桌面端为正常状态）" }),
    ]));
  }
}
async function refreshNotificationIntelligence() {
  try {
    const status = await invoke("notif_intelligence_status");
    try { notificationAppCatalog = await invoke("notif_intelligence_apps"); }
    catch (_) { notificationAppCatalog = []; }
    await renderNotificationIntelligenceSettings(status);
  }
  catch (e) { $("notifPipelineState").textContent = safeUserError(e, "通知处理状态暂时无法读取，请稍后重试。"); }
}
async function setNotificationApp(packageName, enabled) {
  try {
    await invoke("notif_intelligence_set_app", { packageName, enabled });
    toast(enabled ? `已允许接收「${appNameForPackage(packageName)}」的通知` : `已停止接收「${appNameForPackage(packageName)}」的通知`, "ok");
    refreshNotificationIntelligence(); refreshLedger();
  } catch (e) { toast(String(e), "danger"); }
}
async function setNotificationCaptureMode(mode) {
  try {
    await invoke("notif_intelligence_set_capture_mode", { mode });
    toast(mode === "all" ? "已改为接收全应用消息" : "已改为仅接收白名单应用消息", "ok");
    refreshNotificationIntelligence(); refreshLedger();
  } catch (e) { toast(String(e), "danger"); refreshNotificationIntelligence(); }
}
// 第二种授权：允许该应用的通知自动创建日程。与「读取通知」分开，
// 因为它是另一件事——读取是让 息壤 看见，这一步是让这个应用往你的日历里写。
async function setNotificationAppAutoEvent(packageName, enabled) {
  const name = appNameForPackage(packageName);
  if (enabled) {
    const body = el("div", null, [
      el("div", { text: `允许「${name}」的通知在本地解析出确定时间时，直接创建日程和提醒——不再逐条问你。` }),
      el("div", { class: "fine", style: "margin-top:8px;", text:
        "这与「读取该应用通知」是两回事：读取只是让 息壤 看见，这一步是让它据此写入你的日历。" +
        `为防止意外，单个应用每天最多自动创建 ${AUTO_EVENT_DAILY_CAP} 条，超出的转为待你确认；` +
        "创建数量可在本页随时查看，也可随时撤销。" }),
    ]);
    openModal(`允许「${name}」自动建日程`, body, "允许", async () => {
      try {
        await invoke("notif_intelligence_set_app_auto_event", { packageName, enabled: true });
        toast(`已允许「${name}」自动建日程`, "ok");
        refreshNotificationIntelligence();
      } catch (e) { toast(String(e), "danger"); }
    });
    return;
  }
  try {
    await invoke("notif_intelligence_set_app_auto_event", { packageName, enabled: false });
    toast(`已撤销「${name}」的自动建日程（仍会读取其通知）`, "ok");
    refreshNotificationIntelligence();
  } catch (e) { toast(String(e), "danger"); }
}
// 与核心 MAX_AUTO_EVENTS_PER_APP_PER_DAY 保持一致；只用于文案。
const AUTO_EVENT_DAILY_CAP = 20;

function disableNotificationSubmit(button) {
  const buttons = button && button.parentElement
    ? Array.from(button.parentElement.querySelectorAll("button"))
    : button ? [button] : [];
  buttons.forEach((item) => { item.disabled = true; });
  return buttons;
}
function restoreNotificationSubmit(buttons) {
  buttons.forEach((item) => { item.disabled = false; });
}
async function restoreNotificationCapture(id, button) {
  const buttons = disableNotificationSubmit(button);
  try { await invoke("notif_intelligence_restore_capture", { id }); toast("已恢复到处理队列", "ok"); refreshLedger(); }
  catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function promoteNotificationCapture(id, button) {
  const buttons = disableNotificationSubmit(button);
  try { toast(await invoke("notif_intelligence_promote_capture", { id, now: nowParam() }), "ok"); refreshAll(); }
  catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function decideNotificationProposal(id, accepted, button) {
  const buttons = disableNotificationSubmit(button);
  try { await invoke("notif_intelligence_set_filter_proposal", { id, accepted }); toast(accepted ? "过滤规则已确认，可随时撤销" : "已忽略该过滤提议", "ok"); refreshAll(); }
  catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function decideNotificationActionProposal(id, accepted, button) {
  const buttons = disableNotificationSubmit(button);
  try {
    const message = await invoke("notif_intelligence_set_action_proposal", { id, accepted, now: nowParam() });
    toast(message || (accepted ? "已确认通知动作提议" : "已忽略通知动作提议"), "ok");
    refreshAll();
  } catch (e) { restoreNotificationSubmit(buttons); toast(String(e), "danger"); }
}
async function removeNotificationPriorityRule(id) {
  try { await invoke("notif_intelligence_remove_priority_rule", { id }); toast("已删除即时规则", "ok"); refreshNotificationIntelligence(); }
  catch (e) { toast(String(e), "danger"); }
}
async function removeNotificationFilterRule(id) {
  try { await invoke("notif_intelligence_remove_filter_rule", { id }); toast("已撤销过滤规则；历史通知仍可在台账恢复", "ok"); refreshAll(); }
  catch (e) { toast(String(e), "danger"); }
}

$("notifAppSearch").addEventListener("input", () => renderNotificationAppPicker(notificationConfigForPicker));
$("notifCaptureMode").addEventListener("change", (event) => setNotificationCaptureMode(event.target.value));
$("notifBatchInterval").addEventListener("change", async (event) => {
  try { await invoke("notif_intelligence_set_batch_interval", { minutes: Number(event.currentTarget.value) }); toast("普通通知批量间隔已更新", "ok"); }
  catch (e) { toast(String(e), "danger"); refreshNotificationIntelligence(); }
});
$("notifPriorityAddBtn").addEventListener("click", async () => {
  const pattern = $("notifPriorityPattern").value.trim();
  if (!pattern) return toast("请填写重要通知模式", "danger");
  try {
    await invoke("notif_intelligence_add_priority_rule", {
      pattern, packageName: $("notifPriorityPackage").value || null, matcher: $("notifPriorityMatcher").value,
    });
    $("notifPriorityPattern").value = "";
    toast("已添加即时分诊规则", "ok"); refreshNotificationIntelligence();
  } catch (e) { toast(String(e), "danger"); }
});
$("notifBatteryRequestBtn").addEventListener("click", async () => {
  try { await invoke("notif_pipeline_request_battery_optimization"); toast("已打开系统电池优化授权", "ok"); }
  catch (e) { toast(String(e), "danger"); }
});
$("notifBatterySettingsBtn").addEventListener("click", async () => {
  try { await invoke("notif_pipeline_open_battery_settings"); } catch (e) { toast(String(e), "danger"); }
});
$("notifBackgroundSettingsBtn").addEventListener("click", async () => {
  try { await invoke("notif_pipeline_open_background_settings"); } catch (e) { toast(String(e), "danger"); }
});
$("notifBatchNowBtn").addEventListener("click", async () => {
  try { const count = await invoke("notif_intelligence_process_now", { now: nowParam() }); toast(count ? `已处理 ${count} 条普通通知` : "普通队列为空", "ok"); refreshAll(); }
  catch (e) { toast(String(e), "danger"); }
});

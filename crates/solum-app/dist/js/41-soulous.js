"use strict";
/* ============================== Soulous（设置 → 云端） ============================== */
let soulousAccessTail = "";
let soulousRefreshTail = "";
let soulousFormSeeded = false;

function renderSoulousStatus(s) {
  const box = $("soulousStatus");
  box.textContent = "";
  const c = s.cache || {};
  if (s.configured) {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}（本地、gitignore、不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `已配置：${s.server_url} · access:${s.access_token_tail} · refresh:${s.refresh_token_tail}` }),
    ]));
  } else {
    box.appendChild(el("div", { class: "statusline", title: `首次保存会写入：${s.path}` }, [
      el("span", { class: "sdot" }),
      el("span", { class: "t", text: "未配置：Soulous 数据源保持静默关闭。" }),
    ]));
  }
  box.appendChild(el("div", { class: "statusline" }, [
    el("span", { class: "sdot" + (s.push_schedule_events ? " amber" : "") }),
    el("span", { text: s.push_schedule_events
      ? "出站白名单：日程事件已授权；仍须逐次人工确认。"
      : "出站白名单：全部关闭；不会向 Soulous 推送 息壤 数据。" }),
  ]));
  box.appendChild(el("div", { class: "statusline" }, [
    el("span", { class: "sdot" + (c.total ? " ok" : "") }),
    el("span", { text: c.total
      ? `缓存：课表 ${c.courses} 条、考试 ${c.exams} 场、任务 ${c.tasks} 项、打卡快照 ${c.checkins} 条、专注 ${c.focus_sessions} 段；最近成功 ${fmtDTText(c.last_success_at)}`
      : "尚无成功拉取的本地缓存。" }),
  ]));
}

async function refreshSoulous() {
  let s;
  try { s = await invoke("soulous_config_get", { now: nowParam() }); } catch (_) { return; }
  soulousAccessTail = s.access_token_tail || "";
  soulousRefreshTail = s.refresh_token_tail || "";
  renderSoulousStatus(s);
  $("soulousAccess").placeholder = soulousAccessTail ? `已保存（${soulousAccessTail}），留空则沿用` : "首次保存必填";
  $("soulousRefresh").placeholder = soulousRefreshTail ? `已保存（${soulousRefreshTail}），留空则沿用` : "首次保存必填（自动轮换）";
  $("soulousPushScheduleEvents").checked = Boolean(s.push_schedule_events);
  if (soulousFormSeeded) return;
  soulousFormSeeded = true;
  if (s.configured) {
    $("soulousServer").value = s.server_url;
    $("soulousTimeout").value = s.timeout_secs;
  }
}

function soulousCollect() {
  return {
    server_url: $("soulousServer").value.trim(),
    access_token: $("soulousAccess").value.trim(),
    refresh_token: $("soulousRefresh").value.trim(),
    timeout_secs: Math.min(120, Math.max(3, parseInt($("soulousTimeout").value, 10) || 15)),
    push_schedule_events: $("soulousPushScheduleEvents").checked,
  };
}

function soulousValidate(cfg) {
  if (!/^https?:\/\//.test(cfg.server_url)) return "服务器地址需要以 http(s):// 开头";
  if (!cfg.access_token && !soulousAccessTail) return "请填写 access token";
  if (!cfg.refresh_token && !soulousRefreshTail) return "请填写 refresh token";
  return null;
}

$("soulousSaveBtn").addEventListener("click", async () => {
  const cfg = soulousCollect();
  const bad = soulousValidate(cfg);
  if (bad) return toast(bad, "danger");
  $("soulousSaveBtn").disabled = true;
  try {
    await invoke("soulous_config_save", { args: cfg });
    $("soulousAccess").value = "";
    $("soulousRefresh").value = "";
    soulousFormSeeded = false;
    toast("Soulous 本地配置已保存", "ok");
    refreshSoulous();
  } catch (e) { toast(String(e), "danger"); }
  finally { $("soulousSaveBtn").disabled = false; }
});

$("soulousPullBtn").addEventListener("click", async () => {
  const out = $("soulousPullOut");
  out.textContent = "";
  $("soulousPullBtn").disabled = true;
  out.appendChild(el("div", { class: "llm-test", text: "正在拉取 Soulous 数据；失败会继续使用上次缓存…" }));
  try {
    const r = await invoke("soulous_pull", { now: nowParam() });
    out.textContent = "";
    if (!r.configured) {
      out.appendChild(el("div", { class: "llm-test" }, [
        icon("alert", 15), el("span", { text: "未配置，已保持静默离线。" }),
      ]));
      return;
    }
    const p = r.outcome;
    out.appendChild(el("div", { class: "llm-test ok" }, [
      icon("check", 15), el("span", { text:
        `已拉取：课表 ${p.courses} 条、考试 ${p.exams} 场、任务 ${p.tasks} 项、打卡 ${p.checkins} 条、专注 ${p.focus_sessions} 段${p.refreshed_tokens ? "；JWT 已自动刷新" : ""}` }),
    ]));
    toast("Soulous 缓存已更新", "ok");
    refreshSoulous(); refreshSuggest();
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [
      icon("alert", 15), el("span", { text: safeUserError(e, "拉取未完成，已保留上次缓存，请稍后重试。") }),
    ]));
  } finally { $("soulousPullBtn").disabled = false; }
});

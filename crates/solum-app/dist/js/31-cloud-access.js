"use strict";
/* ============================== 云端接入（设置 → 云端） ============================== */
// 预设服务端点与推理模型
const LLM_PRESETS = [
  { id: "custom", label: "自定义端点…", base: "",
    models: [], timeout: 60, temp: "default",
    note: "支持任何兼容 OpenAI 格式的推理服务端点。" },
];
let solumCenterUrl = "https://tannerlab.cn";
let solumActiveModel = "";
let llmKeyTail = "";   // 已保存密钥的尾巴（空 = 还没存过 key，保存时必填）
let llmFormSeeded = false; // 表单只在首次加载/保存后回填，refreshAll 不许覆盖正在编辑的内容

for (const p of LLM_PRESETS) $("llmPreset").appendChild(el("option", { value: p.id, text: p.label }));

function llmApplyPreset(p) {
  $("llmBase").value = p.base;
  const dl = $("llmModels");
  dl.textContent = "";
  for (const m of p.models) dl.appendChild(el("option", { value: m }));
  $("llmModel").value = p.models[0] || "";
  $("llmTimeout").value = p.timeout;
  $("llmTempMode").value = p.temp;
  llmTempModeSync();
  $("llmPresetNote").hidden = !p.note;
  $("llmPresetNote").textContent = p.note || "";
}
function llmTempModeSync() { $("llmTempVal").hidden = $("llmTempMode").value !== "custom"; }
$("llmPreset").addEventListener("change", () => {
  const p = LLM_PRESETS.find((x) => x.id === $("llmPreset").value);
  if (p) llmApplyPreset(p);
});
$("llmTempMode").addEventListener("change", llmTempModeSync);

// 表单 → 后端参数。temperature 三态：默认 0.3 / 自定义 / null（不发送）。
function llmCollect() {
  const mode = $("llmTempMode").value;
  const temperature = mode === "omit" ? null
    : mode === "custom" ? Math.min(2, Math.max(0, Number($("llmTempVal").value) || 0.3))
    : 0.3;
  const mt = parseInt($("llmMaxTok").value, 10);
  return { cfg: {
    base_url: $("llmBase").value.trim(),
    api_key: $("llmKey").value.trim(),
    model: $("llmModel").value.trim(),
    temperature,
    max_tokens: mt > 0 ? mt : null,
    timeout_secs: Math.min(600, Math.max(5, parseInt($("llmTimeout").value, 10) || 30)),
  } };
}
function llmValidate(cfg) {
  if (!/^https?:\/\//.test(cfg.base_url)) return "Base URL 需要以 http(s):// 开头";
  if (!cfg.model) return "请填写模型名";
  if (!cfg.api_key && !llmKeyTail) return "请填写 API Key";
  return null;
}

function renderCloudStatus(s) {
  const box = $("cloudStatus");
  box.textContent = "";
  if (s.configured) {
    // 已配置是稳态陈述：低调状态行，细节进 title；琥珀横幅留给「未配置」这类要行动的事。
    box.appendChild(el("div", { class: "statusline", title: s.source === "env"
        ? "凭据来自环境变量 SOLUM_LLM_*：这里保存只对本次运行生效，下次启动仍以环境变量为准。"
        : `配置文件：${s.path}（已 gitignore，不入库、不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `当前在用：${s.base_url} · ${s.model} · key:${s.key_tail}` }),
    ]));
    if (s.source === "env") box.appendChild(el("div", { class: "hint", style: "margin:4px 0 0 16px;",
      text: "凭据来自环境变量 SOLUM_LLM_*，这里保存只对本次运行生效。" }));
  } else {
    box.appendChild(el("div", { class: "banner" }, [
      icon("alert", 18),
      el("div", { class: "grow" }, [
        el("div", { text: "未配置云端，当前为离线模式——规则路径照常工作，闲聊与兜底抽取不可用。" }),
        el("div", { class: "hint", text: `保存后写入：${s.path}` }),
      ]),
    ]));
  }
}

// 状态横幅每次都刷；表单只在首次 seed（或保存后重新 seed），避免覆盖正在编辑的值。
async function refreshCloud() {
  let s;
  try { s = await invoke("llm_config_get"); } catch (_) { return; }
  llmKeyTail = s.key_tail || "";
  renderCloudStatus(s);
  $("llmKey").placeholder = llmKeyTail ? `已保存（key:${llmKeyTail}），留空则沿用` : "必填";
  if (llmFormSeeded) return;
  llmFormSeeded = true;
  const p = LLM_PRESETS.find((x) => x.base && s.base_url === x.base)
    || LLM_PRESETS[s.configured ? LLM_PRESETS.length - 1 : 0];
  $("llmPreset").value = p.id;
  llmApplyPreset(p);
  if (s.configured) {
    $("llmBase").value = s.base_url;
    $("llmModel").value = s.model;
    $("llmTimeout").value = s.timeout_secs;
    $("llmMaxTok").value = s.max_tokens == null ? "" : s.max_tokens;
    if (s.temperature == null) $("llmTempMode").value = "omit";
    else if (s.temperature === 0.3) $("llmTempMode").value = "default";
    else { $("llmTempMode").value = "custom"; $("llmTempVal").value = s.temperature; }
    llmTempModeSync();
  }
  // 云端接入已锁定为受保护状态，禁止手动随意修改
  $("llmPreset").disabled = true;
  $("llmBase").readOnly = true;
  $("llmKey").readOnly = true;
  $("llmModel").readOnly = true;
  $("llmTempMode").disabled = true;
  $("llmTempVal").readOnly = true;
  $("llmMaxTok").readOnly = true;
  $("llmTimeout").readOnly = true;
  $("llmSaveBtn").disabled = true;
  $("llmSaveBtn").title = "云端接入已由系统托管并锁定保护，不允许更改";
  if ($("llmPresetNote")) {
    $("llmPresetNote").hidden = false;
    $("llmPresetNote").textContent = "【系统锁定】云端接入已由平台安全托管并锁定保护，不允许手动更改。";
  }
}

$("llmSaveBtn").addEventListener("click", async () => {
  toast("云端接入已由系统托管并锁定保护，不允许更改", "amber");
  return;
});

$("llmTestBtn").addEventListener("click", async () => {
  const args = llmCollect();
  const bad = llmValidate(args.cfg);
  if (bad) return toast(bad, "danger");
  const out = $("llmTestOut");
  out.textContent = "";
  out.appendChild(el("div", { class: "llm-test", text: "正在请求端点，最长等待 " + args.cfg.timeout_secs + " 秒…" }));
  $("llmTestBtn").disabled = true;
  try {
    const r = await invoke("llm_config_test", args);
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test ok" }, [
      icon("check", 15),
      el("span", null, [
        "连通正常 · ", el("span", { class: "mono", text: r.latency_ms + " ms" }),
        " · 模型回复：" + String(r.reply).slice(0, 80),
      ]),
    ]));
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [
      icon("alert", 15), el("span", { text: "测试失败：" + safeUserError(e, "云端暂时不可用，请稍后重试。") }),
    ]));
  } finally { $("llmTestBtn").disabled = false; }
});

"use strict";
function renderShellIdentity(s) {
  const loggedIn = !!s.logged_in;
  const profile = loadUserProfile();
  const name = loggedIn ? (s.username || profile.nickname) : profile.nickname;
  const meta = loggedIn ? "已连接 · 自动同步" : "仅保存在本机";
  $("accountShellLabel").textContent = name;
  $("accountShellMeta").textContent = meta;
  $("accountShellAction").textContent = loggedIn ? "管理" : "登录";
  $("mobileAccountLabel").textContent = name;
  $("mobileAccountMeta").textContent = meta;
  $("mobileAccountBtn").textContent = loggedIn ? "管理" : "登录";
  if ($("profileSheetRole")) $("profileSheetRole").textContent = profile.role || (loggedIn ? "已认证成员" : "本地空间");
  if ($("profileSheetBio")) $("profileSheetBio").textContent = profile.bio || "专注当下，记录生活";
  if ($("profileSheetPlanTitle")) $("profileSheetPlanTitle").textContent = loggedIn ? "标准版权益" : "本地访客空间";
  if ($("profileSheetPlanDesc")) $("profileSheetPlanDesc").textContent = loggedIn ? "多端加密同步已就绪" : "数据仅保存在这台设备";
  
  applyUserProfile(profile);
  
  document.body.dataset.account = loggedIn ? "member" : "guest";
  document.querySelectorAll(".utility-btn[data-requires-account='true']").forEach((button) => {
    button.classList.toggle("locked", !loggedIn);
  });
}

function renderAccountStatus(s) {
  const box = $("accountStatus");
  box.textContent = "";
  if (s.logged_in) {
    box.appendChild(el("div", { class: "statusline", title: `会话文件：${s.path}（已 gitignore，不入库、不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `已登录：${s.username} @ ${s.server_url} · ${s.model}` }),
    ]));
    box.appendChild(el("div", { class: "hint", style: "margin:4px 0 0 16px;",
      text: "登录期间云端走账号代理；退出后回到纯本地访客空间。" }));
    if (s.usage) {
      const usage = s.usage;
      const percent = usage.monthly_request_limit > 0
        ? Math.min(100, Math.round(usage.requests_used * 100 / usage.monthly_request_limit)) : 0;
      const reset = usage.reset_at ? new Date(usage.reset_at).toLocaleDateString("zh-CN", {
        month: "long", day: "numeric",
      }) : "下个周期";
      const latest = usage.latest_request;
      let latestText = "还没有云端调用记录";
      if (latest) {
        latestText = `最近实际使用 ${latest.effective_model}`;
        if (latest.fallback_occurred) latestText += ` · 已从 ${latest.requested_model} 回退`;
      }
      box.appendChild(el("div", { class: "account-ai-status" }, [
        el("div", { class: "account-ai-head" }, [
          el("strong", { text: `${usage.plan} 权益` }),
          el("span", { text: `${usage.requests_remaining} 次可用` }),
        ]),
        el("div", { class: "account-quota-track", role: "progressbar", "aria-label": "本月云端调用额度",
          "aria-valuemin": "0", "aria-valuemax": String(usage.monthly_request_limit),
          "aria-valuenow": String(usage.requests_used) }, [
          el("div", { class: "account-quota-fill", style: `--quota-percent:${percent}%` }),
        ]),
        el("div", { class: "account-ai-meta" }, [
          el("span", { text: `已用 ${usage.requests_used} / ${usage.monthly_request_limit}` }),
          el("span", { text: `${reset} 重置` }),
        ]),
        el("div", { class: "hint", text: latestText }),
        latest && latest.fallback_occurred && latest.fallback_reason
          ? el("div", { class: "hint", text: `回退原因：${fallbackReasonLabel(latest.fallback_reason)}` }) : null,
      ].filter(Boolean)));
    } else if (s.state_error) {
      box.appendChild(el("div", { class: "statusline", title: s.state_error }, [
        el("span", { class: "sdot amber" }),
        el("span", { class: "t", text: "模型目录和额度暂时无法刷新；默认模型仍可继续使用。" }),
      ]));
    }
  }
}

function fallbackReasonLabel(reason) {
  if (reason === "model_not_allowed") return "所选模型不在中心目录";
  if (reason === "custom_models_disabled") return "当前权益只允许默认模型";
  if (reason === "upstream_unavailable") return "所选模型服务暂不可用";
  if (reason.startsWith("upstream_http_")) return `所选模型服务返回 ${reason.slice(14)}`;
  return reason;
}

async function refreshAccount() {
  let s;
  try { s = await invoke("account_status_get"); } catch (_) { return; }
  accountLoggedIn = !!s.logged_in;
  if (accountLoggedIn && isAccountExpiryError(s.state_error)) {
    await handleAccountExpiry(s.state_error, "account_status_get");
    return;
  }
  renderShellIdentity(s);
  renderAccountStatus(s);
  $("accountLoginForm").hidden = accountLoggedIn;
  $("accountLoginBtn").hidden = accountLoggedIn;
  $("accountRegisterBtn").hidden = accountLoggedIn;
  if ($("accountOAuthBtn")) $("accountOAuthBtn").hidden = accountLoggedIn;
  $("accountLogoutBtn").hidden = !accountLoggedIn;
  if ($("accountLegacyFold")) $("accountLegacyFold").hidden = accountLoggedIn;
  $("accountModelBtn").hidden = true;
  $("accountSecurityArea").hidden = !accountLoggedIn;
  if (accountLoggedIn) {
    setRecoverMode(false);
    refreshRecoveryCodeStatus();
  } else {
    $("accountRecoverToggle").hidden = false;
  }
  if (accountLoggedIn) $("accountFold").open = true;
  if (s.default_server_url) solumCenterUrl = s.default_server_url;
  const modelSelect = $("accountModel");
  const currentValue = modelSelect.value;
  modelSelect.textContent = "";
  const defaultModel = s.default_model || solumActiveModel;
  solumActiveModel = defaultModel;
  const options = Array.from(new Set([defaultModel, ...(s.model_options || [])]));
  for (const model of options) modelSelect.appendChild(el("option", {
    value: model, text: model === defaultModel ? `${model}（默认）` : model,
  }));
  if (accountFormSeeded && options.includes(currentValue)) modelSelect.value = currentValue;
  $("accountModelNote").replaceChildren(
    icon("shield", 15),
    el("span", { text: `默认模型 ${defaultModel} 始终保留。所选模型在回复开始前失败时，中心会自动回退默认模型，并在这里说明实际使用的模型与原因。` }),
  );
  if (accountFormSeeded) return;
  accountFormSeeded = true;
  if (s.logged_in) {
    $("accountServer").value = s.server_url;
    $("accountUser").value = s.username;
  } else {
    $("accountServer").value = s.server_url || s.default_server_url || solumCenterUrl;
  }
  $("accountModel").value = options.includes(s.model) ? s.model : defaultModel;
}

async function submitSettingsLogin() {
  if (accountAuthSubmitting || $("accountLoginBtn").disabled) return;
  const serverUrl = $("accountServer").value.trim() || solumCenterUrl;
  const username = $("accountUser").value.trim();
  const password = $("accountPass").value;
  if (!username || !password) return toast("请填写账号和密码", "danger");
  accountAuthSubmitting = true;
  $("accountLoginBtn").disabled = true;
  $("accountRegisterBtn").disabled = true;
  try {
    const summary = await invoke("account_login", {
      serverUrl, username, password, model: solumActiveModel,
    });
    $("accountPass").value = ""; // 密码只用于本次登录与本地解封，界面也不留
    toast("已登录，云端改走账号代理", "ok");
    $("llmDot").className = "sdot ok";
    $("llmText").textContent = "云端在线";
    $("llmRow").title = summary;
    accountFormSeeded = false;
    refreshAccount(); refreshCloud(); refreshSyncConfig(); refreshSyncStatus();
  } catch (e) { toast(safeUserError(e, "云端配置未保存，请稍后重试。"), "danger"); }
  finally {
    accountAuthSubmitting = false;
    $("accountLoginBtn").disabled = false;
    $("accountRegisterBtn").disabled = false;
  }
}

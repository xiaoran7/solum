"use strict";
function setupPasswordToggles() {
  document.querySelectorAll(".password-wrap").forEach((wrap) => {
    const input = wrap.querySelector("input");
    const toggleBtn = wrap.querySelector(".btn-eye");
    if (!input || !toggleBtn) return;
    if (toggleBtn.dataset.bound === "true") return;
    toggleBtn.dataset.bound = "true";
    toggleBtn.replaceChildren(icon("eye", 16));
    toggleBtn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const isPass = input.type === "password";
      input.type = isPass ? "text" : "password";
      toggleBtn.replaceChildren(icon(isPass ? "eye_off" : "eye", 16));
      toggleBtn.setAttribute("aria-label", isPass ? "隐藏密码" : "显示密码");
      toggleBtn.setAttribute("title", isPass ? "隐藏密码" : "显示密码");
    });
  });
}

setupPasswordToggles();
applyUserProfile();
setupProfileEditorEvents();
setupInfoModalTriggers();
setupThemeToggle();
if ($("profileSheetSyncBtn")) {
  $("profileSheetSyncBtn").appendChild(icon("refresh", 16));
  $("profileSheetSyncBtn").addEventListener("click", () => {
    syncManual();
    toast("正在同步最新数据…", "sync");
  });
}
if ($("profileSheetStatusIcon")) {
  $("profileSheetStatusIcon").appendChild(icon("cloud", 20));
}


$("accountGatePrivacyIcon")?.appendChild(icon("lock", 20));
$("accountGateCaptchaRefreshBtn")?.appendChild(icon("refresh", 16));
$("accountGateLoginTab")?.addEventListener("click", () => setAccountGateMode("login"));
$("accountGateRegisterTab")?.addEventListener("click", () => setAccountGateMode("register"));
$("accountSubtypeCodeBtn")?.addEventListener("click", () => setAccountLoginSubtype("code"));
$("accountSubtypePassBtn")?.addEventListener("click", () => setAccountLoginSubtype("pass"));
$("accountGateCaptchaPreview")?.addEventListener("click", refreshAccountCaptcha);
$("accountGateCaptchaRefreshBtn")?.addEventListener("click", refreshAccountCaptcha);
$("accountGateSendEmailCodeBtn")?.addEventListener("click", sendEmailVerificationCode);
$("accountGateSendSmsBtn")?.addEventListener("click", sendSmsCode);
$("accountGateSubmit")?.addEventListener("click", submitAccountGate);
$("accountGateHuaweiBtn")?.addEventListener("click", startHuaweiLogin);
$("accountOAuthBtn")?.addEventListener("click", startHuaweiLogin);
$("accountUserTermsLink")?.addEventListener("click", (e) => { e.preventDefault(); openTermsOfService(); });
$("accountPrivacyPolicyLink")?.addEventListener("click", (e) => { e.preventDefault(); showPrivacyPolicyModal(); });
$("accountDeleteBtn")?.addEventListener("click", submitAccountDelete);

$("accountGatePhone")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    if (accountGateMode === "register" || accountLoginSubtype === "code") {
      $("accountGateSmsCode")?.focus();
    } else {
      $("accountGatePass")?.focus();
    }
  }
});
$("accountGateEmail")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    if (accountGateMode === "register") {
      $("accountGateCaptcha")?.focus();
    } else {
      $("accountGatePass")?.focus();
    }
  }
});
$("accountGateCaptcha")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    if (!$("accountGateEmailCode")?.value) {
      sendEmailVerificationCode();
    } else {
      $("accountGatePass")?.focus();
    }
  }
});
$("accountGateEmailCode")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    $("accountGatePass")?.focus();
  }
});
$("accountGateSmsCode")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    if (accountGateMode === "register") {
      $("accountGatePass")?.focus();
    } else {
      submitAccountGate();
    }
  }
});
$("accountGatePass")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    if (accountGateMode === "register") {
      $("accountGateConfirm")?.focus();
    } else {
      submitAccountGate();
    }
  }
});
$("accountGateConfirm")?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") { event.preventDefault(); submitAccountGate(); }
});
$("accountClose")?.addEventListener("click", closeAccountGate);
$("accountShellBtn")?.addEventListener("click", () => {
  if (accountLoggedIn) showView("cloud");
  else openAccountGate("请登录后使用 Solum 完整功能", { dismissible: false, allowGuest: false });
});
$("mobileAccountBtn")?.addEventListener("click", () => {
  closeMobileTools();
  if (accountLoggedIn) showView("cloud");
  else openAccountGate("请登录后使用 Solum 完整功能", { dismissible: false, allowGuest: false });
});

function promptModal(title, message, placeholder, defaultVal) {
  return new Promise((resolve) => {
    let resolved = false;
    const finish = (val) => {
      if (resolved) return;
      resolved = true;
      resolve(val);
    };
    const input = el("input", {
      type: "text",
      class: "input",
      placeholder: placeholder || "",
      value: defaultVal || "",
      style: "width:100%;margin-top:12px;box-sizing:border-box;",
    });
    const clipBtn = el("button", {
      type: "button",
      class: "btn ghost",
      text: "读取剪贴板",
      style: "margin-top:8px;font-size:12px;",
      click: async () => {
        try {
          const text = await navigator.clipboard.readText();
          if (text) input.value = text.trim();
        } catch (_) {
          toast("请长按输入框粘贴", "info");
        }
      }
    });
    const body = el("div", {}, [
      el("p", { text: message, style: "white-space:pre-wrap;line-height:1.5;color:var(--text-muted);" }),
      input,
      el("div", { style: "display:flex;justify-content:flex-end;" }, [clipBtn])
    ]);
    openModal(
      title,
      body,
      "确认",
      () => finish(input.value.trim()),
      {
        okStyle: "primary",
        onDismiss: () => finish(null),
      }
    );
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        closeModal();
        finish(input.value.trim());
      }
    });
    setTimeout(() => input.focus(), 80);
  });
}

function openTermsOfService() {
  openInfoModal(
    "用户服务协议",
    `<div style="line-height:1.75;font-size:0.9rem;color:var(--ink-2);max-height:60vh;overflow-y:auto;padding-right:6px;">
      <h4 style="margin:0 0 8px;color:var(--ink);">Solum（息壤）用户服务协议</h4>
      <p>版本生效日期：2026年9月</p>
      <p>欢迎使用由 Solum 团队提供的本地优先智能工作台服务。在您开始使用前，请仔细阅读本《用户服务协议》（以下简称“本协议”）。当您勾选同意并注册、登录或使用本软件时，即表示您已充分理解并同意接受本协议的全部条款。</p>
      <p><strong>一、服务原则与架构</strong><br>
      1.1 本软件严格遵循“本地优先（Local-first）、零数据上传、最小必要原则”。您的个人记账、日程安排、日记足迹、灵感记忆等所有核心数据仅持久化保存在您的本地物理设备中。<br>
      1.2 本应用不提供自动云端存储或多端云漫游服务，仅支持您在「数据与备份」中自主手动导出或导入本地备份文件。<br>
      1.3 <strong>【特别重要提醒·永久丢失免责】</strong>因为所有数据均只保存在您的本地设备中，若您主动删除数据、卸载应用或清理系统缓存而未提前进行本地手动导出备份，<strong>该部分数据将永久彻底丢失，且本平台与技术服务提供方在技术上完全无法为您恢复，由此造成的任何损失需由您自行承担。</strong></p>
      <p><strong>二、账号注册与使用</strong><br>
      2.1 根据国家关于互联网用户账号信息管理的法律法规要求，您应当使用合法、有效的手机号码进行注册与实名认证，或使用通过官方认证的华为账号快捷登录。<br>
      2.2 您应妥善保管自己的登录账号、密码以及数据端到端加密恢复码。恢复码是在本地生成的唯一密钥凭据，中心服务器不予存储。因您个人原因导致的凭证泄露、误删或恢复码遗失，加密数据将永久无法解开或恢复，本平台对此不承担责任。</p>
      <p><strong>三、账号注销与数据清除</strong><br>
      3.1 您享有随时注销账号的权利。您可以在「设置 → Solum 账号」中点击「注销账号」自主发起注销流程。<br>
      3.2 账号注销完成后，我们在云端为您存储的设备关联凭证与账号信息将立即被永久彻底物理删除，且不可恢复。</p>
      <p><strong>四、用户行为规范</strong><br>
      您在使用本软件过程中不得利用本服务制作、复制、发布、传播违反国家法律法规、危害国家安全、侵犯他人合法权益的内容。</p>
    </div>`
  );
}

function showPrivacyPolicyModal() {
  const meta = privacyDoc ? `${privacyDoc.version} · ${privacyDoc.effective_date}` : "2026-09";
  openInfoModal(
    "隐私政策摘要",
    `<div style="line-height:1.75;font-size:0.9rem;color:var(--ink-2);max-height:60vh;overflow-y:auto;padding-right:6px;">
      <h4 style="margin:0 0 8px;color:var(--ink);">个人信息保护与隐私承诺（${meta}）</h4>
      <p>Solum 非常重视您的个人隐私与数据安全，我们将严格遵守《中华人民共和国个人信息保护法》、《网络安全法》等法律法规：</p>
      <p><strong>1. 我们如何收集和使用信息</strong><br>
      • <strong>手机号码</strong>：用于账号注册、登录身份核验以及短信验证码发送。<br>
      • <strong>华为账号授权</strong>：若您选择华为账号登录，我们仅获取您的华为账号 OpenID/UnionID 及基础公开昵称用于绑定账号。<br>
      • <strong>本地工作数据与免责</strong>：您的日常日记、财务记账明细、个人通知、灵感记忆等数据<strong>完全留在本机存储</strong>，应用不设云端自动同步，仅支持用户手动在本地导出与导入文件。<strong>【郑重提示】若您在本地删除了数据且未手动留存备份，数据将永久彻底丢失，服务器无任何明文或密文备份，平台完全无法恢复，请务必妥善做好重要数据的本地手动备份。</strong><br>
      • <strong>云端推理接入</strong>：云端 AI 接入配置由系统预设托管且处于锁定保护状态，仅用于与安全网关通信以提供智能推理能力，不会沉淀或转存您的私人数据。</p>
      <p><strong>2. 权限使用说明</strong><br>
      • 通知权限：用于待办事项、日程与关键健康提醒在设备本地准时唤醒推送。<br>
      • 网络权限：用于登录认证以及向安全 AI 网关拉取大模型智能回复。</p>
      <p><strong>3. 您的权利</strong><br>
      您可以随时在应用内查看、通过本地文件手动导出您的所有数据，并可以随时在设置中一键注销您的账户。</p>
    </div>`
  );
}

async function submitAccountDelete() {
  const confirmed = await confirmModal(
    "注销账号确认",
    "注销账号是不可逆操作！\n\n注销后，您在 Solum 云端的所有加密备份、多端同步凭证与账户数据将被永久彻底物理销毁，无法找回。\n\n您确定要注销当前账号吗？"
  );
  if (!confirmed) return;
  try {
    toast("正在注销账号…", "info");
    await invoke("account_delete");
    toast("账号已成功注销，数据已清除", "ok");
  } catch (e) {
    toast(safeUserError(e, "注销账号失败"), "danger");
  }
}

async function startHuaweiLogin() {
  if (accountAuthSubmitting) return;

  const agreement = $("accountAgreementCheck");
  if (!$("accountGate").hidden && agreement && !agreement.checked) {
    toast("请先阅读并勾选同意《用户服务协议》和《隐私政策》", "warn", 3500);
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请先勾选同意《用户服务协议》和《隐私政策》";
    agreement.focus();
    return;
  }

  const serverUrl = solumCenterUrl || "https://tannerlab.cn";
  let autoCheckTimer = null;
  let onVisibilityChange = null;
  const statusEl = $("accountPassportStatus") || $("accountGateStatus");
  const updateStatus = (msg, show = true) => {
    if (!statusEl) return;
    statusEl.hidden = !show;
    statusEl.textContent = msg || "";
  };

  try {
    accountAuthSubmitting = true;
    updateStatus("正在连接华为账号服务…");
    toast("正在启动华为账号登录…", "info");

    const cfg = await invoke("account_huawei_config_get", { serverUrl });
    if (!cfg.enabled) {
      toast("服务器暂未开启华为登录服务", "danger");
      updateStatus("服务器暂未开启华为登录服务");
      return;
    }

    const redirectUri = cfg.redirect_uri || "solum://oauth/callback";
    const state = Math.random().toString(36).slice(2);
    const authUrl = `${cfg.authorize_url}?response_type=code&client_id=${encodeURIComponent(cfg.client_id)}&redirect_uri=${encodeURIComponent(redirectUri)}&scope=${encodeURIComponent(cfg.scope || 'openid profile')}&state=${state}`;

    try {
      await invoke("open_external_url", { url: authUrl });
    } catch (_) {
      window.open(authUrl, "_blank");
    }

    updateStatus("已打开华为账号授权页，授权完成后自动返回…");
    toast("已打开华为账号授权页，完成授权后自动登录…", "info");

    let codePromiseResolve = null;
    const codePromise = new Promise((resolve) => {
      codePromiseResolve = resolve;
    });

    const checkDeepLink = async () => {
      try {
        const items = await invoke("account_oauth_android_drain");
        if (Array.isArray(items) && items.length > 0) {
          const match = items.find((it) => it && (it.code || it.error));
          if (match) {
            if (match.error) {
              toast(`华为授权失败: ${match.error}`, "danger");
              updateStatus(`华为授权失败: ${match.error}`);
              if (codePromiseResolve) codePromiseResolve(null);
            } else if (match.code) {
              if (codePromiseResolve) codePromiseResolve(match.code.trim());
            }
          }
        }
      } catch (_) {}
    };

    autoCheckTimer = setInterval(checkDeepLink, 800);
    onVisibilityChange = () => {
      if (document.visibilityState === "visible") checkDeepLink();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("focus", onVisibilityChange);

    let fallbackTimeout = null;
    const timeoutPromise = new Promise((resolve) => {
      fallbackTimeout = setTimeout(() => {
        resolve("__timeout__");
      }, 60000);
    });

    let code = await Promise.race([codePromise, timeoutPromise]);
    if (fallbackTimeout) clearTimeout(fallbackTimeout);

    if (code === "__timeout__") {
      updateStatus("未检测到自动回跳，若授权已完成可手动输入授权码");
      code = await promptModal(
        "华为账号登录",
        "如果授权完成后未能自动返回本应用，请在此输入华为授权码（Code）：",
        "在此输入授权码"
      );
    }

    if (autoCheckTimer) { clearInterval(autoCheckTimer); autoCheckTimer = null; }
    if (onVisibilityChange) {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("focus", onVisibilityChange);
      onVisibilityChange = null;
    }

    if (!code || !code.trim()) {
      updateStatus("已取消华为账号登录");
      toast("已取消华为账号登录", "info");
      return;
    }

    updateStatus("已获得授权，正在安全进入…");
    let recoveryCode = null;
    let attempt = 0;
    while (attempt < 2) {
      attempt++;
      try {
        await invoke("account_huawei_login", {
          args: {
            server_url: serverUrl,
            code: code.trim(),
            redirect_uri: redirectUri,
            model: $("accountModel")?.value?.trim() || solumActiveModel,
            recovery_code: recoveryCode,
          },
        });
        toast("华为账号登录成功", "ok");
        updateStatus("", false);
        if (typeof closeAccountGate === 'function') closeAccountGate();
        accountFormSeeded = false;
        await refreshAccount();
        await refreshCloud();
        await refreshSyncConfig();
        await refreshSyncStatus();
        break;
      } catch (e) {
        const errStr = String(e);
        if (errStr.includes("NEEDS_RECOVERY_CODE") || errStr.includes("恢复码")) {
          recoveryCode = await promptModal(
            "输入数据恢复码",
            "此账号已有端到端加密数据：新设备首次登录，请输入 20 字符恢复码以解密。\n如为初始使用，请重新尝试登录。",
            "XXXXX-XXXXX-XXXXX-XXXXX"
          );
          if (!recoveryCode || !recoveryCode.trim()) {
            toast("未输入恢复码，无法解密现有空间", "danger");
            break;
          }
          continue;
        } else {
          toast(safeUserError(e, "华为登录失败，请重试。"), "danger");
          updateStatus(safeUserError(e, "华为登录失败，请重试。"));
          break;
        }
      }
    }
  } catch (e) {
    toast(safeUserError(e, "华为登录异常"), "danger");
    updateStatus(safeUserError(e, "华为登录异常"));
  } finally {
    if (autoCheckTimer) clearInterval(autoCheckTimer);
    if (onVisibilityChange) {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("focus", onVisibilityChange);
    }
    accountAuthSubmitting = false;
  }
}

$("accountLoginBtn").addEventListener("click", async () => {
  await submitSettingsLogin();
});

$("accountRegisterBtn")?.addEventListener("click", () => {
  setAccountGateMode("register");
  if ($("accountGatePhone")) $("accountGatePhone").value = $("accountUser") ? $("accountUser").value.trim() : "";
  openAccountGate("使用手机号注册新账号", { dismissible: true, allowGuest: false });
});

$("accountLogoutBtn").addEventListener("click", async () => {
  $("accountLogoutBtn").disabled = true;
  try {
    const message = await invoke("account_logout");
    toast(message, "ok");
    // 退出后云端状态取决于是否还有直连配置，页脚交给 refresh 按真实状态刷。
    accountFormSeeded = false;
    llmFormSeeded = false;
    await refreshAccount(); await refreshCloud();
    syncFormSeeded = false;
    await refreshSyncConfig(); await refreshSyncStatus();
    const on = !accountLoggedIn && !!llmKeyTail;
    $("llmDot").className = "sdot" + (on ? " ok" : "");
    $("llmText").textContent = on ? "云端在线" : "离线模式";
  } catch (e) { toast(String(e), "danger"); }
  finally { $("accountLogoutBtn").disabled = false; }
});

$("accountModelBtn").addEventListener("click", async () => {
  $("accountModelBtn").disabled = true;
  try {
    const summary = await invoke("account_model_save", { model: $("accountModel").value.trim() });
    toast("已保存模型", "ok");
    $("llmRow").title = summary;
    accountFormSeeded = false;
    refreshAccount();
  } catch (e) { toast(safeUserError(e, "模型未保存，请稍后重试。"), "danger"); }
  finally { $("accountModelBtn").disabled = false; }
});

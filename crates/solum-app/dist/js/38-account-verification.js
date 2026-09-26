"use strict";
let smsTimer = null;
let smsSeconds = 60;
function startSmsCountdown() {
  const btn = $("accountGateSendSmsBtn");
  if (!btn) return;
  btn.disabled = true;
  smsSeconds = 60;
  btn.textContent = `${smsSeconds}s 后重新获取`;
  if (smsTimer) clearInterval(smsTimer);
  smsTimer = setInterval(() => {
    smsSeconds--;
    if (smsSeconds <= 0) {
      clearInterval(smsTimer);
      smsTimer = null;
      btn.disabled = false;
      btn.textContent = "获取验证码";
    } else {
      btn.textContent = `${smsSeconds}s 后重新获取`;
    }
  }, 1000);
}

async function sendSmsCode() {
  const phone = $("accountGatePhone")?.value?.trim() || "";
  if (!/^1[3-9]\d{9}$/.test(phone)) {
    toast("请输入正确的大陆 11 位手机号码", "danger");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请输入正确的大陆 11 位手机号码";
    $("accountGatePhone")?.focus();
    return;
  }
  const btn = $("accountGateSendSmsBtn");
  if (btn) btn.disabled = true;
  try {
    toast("正在发送短信验证码…", "info");
    await invoke("account_sms_send", {
      serverUrl: solumCenterUrl || null,
      phone,
    });
    toast("验证码已发送，请注意查收短信", "ok");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "验证码已发送，5分钟内有效";
    startSmsCountdown();
  } catch (e) {
    if (btn) btn.disabled = false;
    toast(safeUserError(e, "发送短信验证码失败，请稍后重试"), "danger");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = safeUserError(e, "发送失败");
  }
}

let currentCaptchaId = "";
let emailCountdownTimer = null;

async function refreshAccountCaptcha() {
  const preview = $("accountGateCaptchaPreview");
  const refreshBtn = $("accountGateCaptchaRefreshBtn");
  if (!preview) return;
  preview.textContent = "加载中…";
  if (refreshBtn) refreshBtn.disabled = true;
  try {
    const res = await invoke("account_captcha_get", {
      serverUrl: solumCenterUrl || null,
    });
    currentCaptchaId = res.captcha_id;
    const img = document.createElement("img");
    img.src = res.image;
    img.alt = "验证码";
    preview.textContent = "";
    preview.appendChild(img);
    if ($("accountGateCaptcha")) $("accountGateCaptcha").value = "";
  } catch (e) {
    preview.textContent = "点击重试";
    currentCaptchaId = "";
    toast(safeUserError(e, "获取图形验证码失败"), "warn");
  } finally {
    if (refreshBtn) refreshBtn.disabled = false;
  }
}

function startEmailCountdown() {
  let seconds = 60;
  const btn = $("accountGateSendEmailCodeBtn");
  if (!btn) return;
  btn.disabled = true;
  btn.textContent = `${seconds}s 后重发`;
  if (emailCountdownTimer) clearInterval(emailCountdownTimer);
  emailCountdownTimer = setInterval(() => {
    seconds -= 1;
    if (seconds <= 0) {
      clearInterval(emailCountdownTimer);
      emailCountdownTimer = null;
      btn.disabled = false;
      btn.textContent = "获取验证码";
    } else {
      btn.textContent = `${seconds}s 后重发`;
    }
  }, 1000);
}

async function sendEmailVerificationCode() {
  const email = $("accountGateEmail")?.value?.trim() || "";
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
    toast("请输入有效的邮箱地址", "danger");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请输入有效的邮箱地址";
    $("accountGateEmail")?.focus();
    return;
  }
  const captcha = $("accountGateCaptcha")?.value?.trim() || "";
  if (!captcha) {
    toast("请先输入图形验证码", "warn");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请先输入图形验证码";
    $("accountGateCaptcha")?.focus();
    return;
  }
  if (!currentCaptchaId) {
    refreshAccountCaptcha();
    toast("请等待图形验证码加载完成", "warn");
    return;
  }

  const btn = $("accountGateSendEmailCodeBtn");
  if (btn) btn.disabled = true;
  try {
    toast("正在发送邮件验证码…", "info");
    await invoke("account_email_send_code", {
      args: {
        server_url: solumCenterUrl || null,
        email,
        captcha_id: currentCaptchaId,
        captcha_answer: captcha,
      },
    });
    toast("验证码已发送至您的邮箱，请注意查收", "ok");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "验证码已发送，10分钟内有效";
    startEmailCountdown();
    $("accountGateEmailCode")?.focus();
  } catch (e) {
    if (btn) btn.disabled = false;
    const msg = safeUserError(e, "发送邮件验证码失败，请稍后重试");
    toast(msg, "danger");
    if ($("accountGateStatus")) $("accountGateStatus").textContent = msg;
    refreshAccountCaptcha();
  }
}

async function submitAccountGate() {
  if (accountAuthSubmitting || $("accountGateSubmit").disabled) return;

  const agreement = $("accountAgreementCheck");
  if (agreement && !agreement.checked) {
    toast("请先阅读并勾选同意《用户服务协议》和《隐私政策》", "warn", 3500);
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请先勾选同意《用户服务协议》和《隐私政策》";
    agreement.focus();
    return;
  }

  const email = $("accountGateEmail")?.value?.trim() || "";
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请输入有效的邮箱地址。";
    $("accountGateEmail")?.focus();
    return;
  }

  const password = $("accountGatePass")?.value || "";
  if (!password) {
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "请输入登录密码。";
    $("accountGatePass")?.focus();
    return;
  }

  const registering = accountGateMode === "register";

  if (registering) {
    const code = $("accountGateEmailCode")?.value?.trim() || "";
    if (!code || code.length < 4) {
      if ($("accountGateStatus")) $("accountGateStatus").textContent = "请输入 6 位邮箱验证码。";
      $("accountGateEmailCode")?.focus();
      return;
    }

    const confirm = $("accountGateConfirm")?.value || "";
    if (Array.from(password).length < 8) {
      if ($("accountGateStatus")) $("accountGateStatus").textContent = "密码至少需要 8 个字符。";
      $("accountGatePass")?.focus();
      return;
    }
    if (password !== confirm) {
      if ($("accountGateStatus")) $("accountGateStatus").textContent = "两次输入的密码不一致。";
      $("accountGateConfirm")?.focus();
      return;
    }

    accountAuthSubmitting = true;
    $("accountGateSubmit").disabled = true;
    $("accountGateSubmit").textContent = "正在创建账号…";
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "正在建立端到端加密空间…";
    try {
      await invoke("account_email_register", {
        args: {
          server_url: solumCenterUrl || null,
          email,
          password,
          code,
          model: solumActiveModel || "gemini-3.8-flash-high",
        },
      });
      toast("注册成功，正在进入…", "ok");
      if ($("accountGateStatus")) $("accountGateStatus").textContent = "正在进入空间…";
      accountLoggedIn = true;
      if (typeof closeAccountGate === 'function') closeAccountGate();
      accountFormSeeded = false;
      try {
        await refreshAccount();
        await refreshCloud();
        await refreshSyncConfig();
        await refreshSyncStatus();
      } catch (_) {}
    } catch (e) {
      if ($("accountGateStatus")) $("accountGateStatus").textContent = safeUserError(e, "注册失败，请检查网络后重试。");
      $("accountGateSubmit").disabled = false;
      $("accountGateSubmit").textContent = "注册并进入";
      refreshAccountCaptcha();
    } finally {
      accountAuthSubmitting = false;
    }
  } else {
    accountAuthSubmitting = true;
    $("accountGateSubmit").disabled = true;
    $("accountGateSubmit").textContent = "正在登录…";
    if ($("accountGateStatus")) $("accountGateStatus").textContent = "正在验证账号密码…";
    try {
      await invoke("account_email_login", {
        args: {
          server_url: solumCenterUrl || null,
          email,
          password,
          model: solumActiveModel || "gemini-3.8-flash-high",
        },
      });
      toast("登录成功，正在进入…", "ok");
      if ($("accountGateStatus")) $("accountGateStatus").textContent = "正在进入空间…";
      accountLoggedIn = true;
      if (typeof closeAccountGate === 'function') closeAccountGate();
      accountFormSeeded = false;
      try {
        await refreshAccount();
        await refreshCloud();
        await refreshSyncConfig();
        await refreshSyncStatus();
      } catch (_) {}
    } catch (e) {
      if ($("accountGateStatus")) $("accountGateStatus").textContent = safeUserError(e, "邮箱或密码不正确。");
      $("accountGateSubmit").disabled = false;
      $("accountGateSubmit").textContent = "登录";
    } finally {
      accountAuthSubmitting = false;
    }
  }
}

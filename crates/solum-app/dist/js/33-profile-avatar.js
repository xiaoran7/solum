"use strict";
/* ============================== 账号登录（设置 → 云端，solum-cloud 代理） ============================== */
let accountLoggedIn = false;
let accountFormSeeded = false; // 同 llm 表单：只首次 seed，refreshAll 不覆盖正在编辑的值
let accountGateMode = "login";
let accountAuthSubmitting = false;
const GUEST_CHOICE_KEY = "solum.guest-choice.v1";
const AUTH_EXPIRED_REDIRECT_KEY = "solum.auth-expired-redirect.v1";
const GUEST_ALLOWED_VIEWS = new Set(["chat", "agenda", "notifs", "ledger", "ledger-detail", "journal", "search", "settings", "privacy", "capture", "data"]);
const GUEST_ALLOWED_REFRESHERS = new Set([
  refreshAgenda, refreshNotifs, refreshJournal, refreshLedger, refreshPrivacy, refreshToday,
  refreshReminderDeliveryHealth,
]);

function guestChoiceMade() {
  try { return localStorage.getItem(GUEST_CHOICE_KEY) === "guest"; } catch (_) { return false; }
}

function takeAuthExpiredRedirect() {
  try {
    const pending = localStorage.getItem(AUTH_EXPIRED_REDIRECT_KEY) === "1";
    localStorage.removeItem(AUTH_EXPIRED_REDIRECT_KEY);
    return pending;
  } catch (_) { return false; }
}

let accountExpiryFlight = null;
function handleAccountExpiry(error, command) {
  if (accountExpiryFlight || !accountLoggedIn) return accountExpiryFlight || Promise.resolve();
  accountLoggedIn = false;
  invalidateIpcCache();
  safeLocalSet(AUTH_EXPIRED_REDIRECT_KEY, "1");
  try { localStorage.removeItem(GUEST_CHOICE_KEY); } catch (_) { /* login gate still opens immediately */ }
  renderShellIdentity({ logged_in: false });
  setAccountGateMode("login");
  closeMobileTools();
  console.info("Solum account session expired", command, String(error || ""));
  toast("登录凭据已失效，正在为您重置安全会话…", "danger", 3500);
  accountExpiryFlight = rawInvoke("account_logout")
    .catch(() => null)
    .finally(() => {
      accountExpiryFlight = null;
      // 桌面端或无需重启进程环境下的平滑兜底
      openAccountGate("登录状态已过期，请重新登录", { dismissible: false, allowGuest: true });
      $("accountGateStatus").textContent = "登录状态已失效，请重新登录。";
    });
  return accountExpiryFlight;
}

let accountLoginSubtype = "code"; // "code" (验证码免密登录) 或 "pass" (密码登录)
let smsCountdownTimer = null;
let smsCountdown = 60;

function setAccountLoginSubtype(subtype) {
  accountLoginSubtype = subtype === "pass" ? "pass" : "code";
  const isCode = accountLoginSubtype === "code";
  const btnCode = $("accountSubtypeCodeBtn");
  const btnPass = $("accountSubtypePassBtn");
  if (btnCode) btnCode.className = isCode ? "active" : "";
  if (btnPass) btnPass.className = isCode ? "" : "active";

  const smsField = $("accountGateSmsField");
  const passField = $("accountGatePassField");
  if (smsField) smsField.hidden = !isCode;
  if (passField) passField.hidden = isCode;

  if ($("accountGatePassLabel")) $("accountGatePassLabel").textContent = "登录密码";
  if ($("accountGateModeTitle")) {
    $("accountGateModeTitle").textContent = isCode ? "手机号免密登录" : "手机号密码登录";
  }
  if ($("accountGateSubmit")) {
    $("accountGateSubmit").textContent = isCode ? "免密登录 / 一键注册" : "登录";
  }
}

function setAccountGateMode(mode) {
  accountGateMode = mode === "register" ? "register" : "login";
  const registering = accountGateMode === "register";
  if ($("accountGateLoginTab")) $("accountGateLoginTab").setAttribute("aria-selected", registering ? "false" : "true");
  if ($("accountGateRegisterTab")) $("accountGateRegisterTab").setAttribute("aria-selected", registering ? "true" : "false");

  const subtypes = $("accountLoginSubtypes");
  const phoneWrap = $("accountGatePhoneWrap");
  const emailWrap = $("accountGateEmailWrap");
  const smsField = $("accountGateSmsField");
  const passField = $("accountGatePassField");
  const confirmField = $("accountGateConfirmField");
  const captchaField = $("accountGateCaptchaField");
  const emailCodeField = $("accountGateEmailCodeField");

  // 手机号与短信验证码隐藏，默认优先使用邮箱账号
  if (subtypes) subtypes.style.display = "none";
  if (phoneWrap) phoneWrap.style.display = "none";
  if (emailWrap) emailWrap.style.display = "";
  if (smsField) smsField.hidden = true;
  if (passField) passField.hidden = false;

  if (registering) {
    if (confirmField) confirmField.hidden = false;
    if (captchaField) captchaField.hidden = false;
    if (emailCodeField) emailCodeField.hidden = false;
    if ($("accountGatePassLabel")) $("accountGatePassLabel").textContent = "设置密码（至少 8 位）";
    if ($("accountGateModeTitle")) $("accountGateModeTitle").textContent = "新用户注册";
    if ($("accountGateSubmit")) $("accountGateSubmit").textContent = "注册并进入";
    if ($("accountGatePass")) $("accountGatePass").autocomplete = "new-password";
    refreshAccountCaptcha();
  } else {
    if (confirmField) confirmField.hidden = true;
    if (captchaField) captchaField.hidden = true;
    if (emailCodeField) emailCodeField.hidden = true;
    if ($("accountGatePassLabel")) $("accountGatePassLabel").textContent = "登录密码";
    if ($("accountGateModeTitle")) $("accountGateModeTitle").textContent = "账号登录";
    if ($("accountGateSubmit")) $("accountGateSubmit").textContent = "登录";
    if ($("accountGatePass")) $("accountGatePass").autocomplete = "current-password";
  }

  if ($("accountGateConfirm")) {
    $("accountGateConfirm").required = registering;
    $("accountGateConfirm").value = "";
  }
  if ($("accountGateStatus")) $("accountGateStatus").textContent = "";
}

function openAccountGate(reason, options) {
  if (accountLoggedIn) return;
  options = options || {};
  if ($("accountGateReason")) $("accountGateReason").textContent = reason || "登录后使用完整功能";
  if ($("accountGateClose")) $("accountGateClose").style.display = options.dismissible ? "" : "none";
  if ($("accountGate")) $("accountGate").hidden = false;
  document.body.classList.add("account-gate-open");
  syncOverlayHistory();
  requestAnimationFrame(() => {
    const shell = $("accountGate").querySelector(".account-gate-shell");
    if (!motionPreference.matches && shell && typeof shell.animate === "function") {
      shell.animate([
        { opacity: 0, transform: "translateY(18px) scale(.992)" },
        { opacity: 1, transform: "translateY(0) scale(1)" },
      ], { duration: 420, easing: "cubic-bezier(.22,1,.36,1)" });
    }
    const emailInput = $("accountGateEmail");
    if (emailInput) emailInput.focus();
  });
}

function closeAccountGate() {
  if (!accountLoggedIn) return;
  if ($("accountGate").hidden) return;
  $("accountGate").hidden = true;
  document.body.classList.remove("account-gate-open");
  if ($("accountGateStatus")) $("accountGateStatus").textContent = "";
  syncOverlayHistory();
}

function requireAccount(reason) {
  if (accountLoggedIn) return true;
  closeMobileTools();
  openAccountGate(reason || "这项能力需要 Solum 账号", { dismissible: true, allowGuest: false });
  return false;
}


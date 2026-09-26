"use strict";
/* ============================== 恢复码与改密 ============================== */
let recoveryCodeChecking = false;

// OAuth 登录成功即重启应用，首登签发的恢复码由 Rust 落盘暂存；这里在启动
// 登录态就绪后读取并弹窗提醒，用户确认已抄写前每次启动都会再次提醒。
async function maybeShowPendingRecoveryCode() {
  if (!accountLoggedIn) return;
  let code = null;
  try { code = await invoke("recovery_code_pending"); } catch (_) { return; }
  if (!code) return;
  const copyBtn = el("button", {
    type: "button",
    class: "btn ghost",
    text: "复制恢复码",
    style: "margin-top:8px;",
    click: async () => {
      try {
        await navigator.clipboard.writeText(code);
        toast("已复制到剪贴板，请粘贴到笔记或密码管理器保存", "ok");
      } catch (_) { toast("复制失败，请手动抄写", "danger"); }
    }
  });
  const body = el("div", {}, [
    el("p", { style: "white-space:pre-wrap;line-height:1.5;color:var(--text-muted);",
      text: "这是你的数据恢复码：端到端加密数据的唯一钥匙，只在设备上生成，息壤服务器与任何人都无法找回。换新设备登录时，必须凭它解开同步数据。" }),
    el("div", { style: "font-family:var(--mono,monospace);font-size:1.2rem;letter-spacing:1px;margin:10px 0;user-select:all;",
      text: code }),
    el("div", { style: "display:flex;justify-content:flex-end;" }, [copyBtn]),
    el("p", { class: "hint", style: "margin-top:8px;",
      text: "请立即抄写到纸笔或密码管理器。点「我已保存恢复码」后不再提醒；点「稍后再说」，下次启动会继续提醒。" }),
  ]);
  openModal("请保存你的数据恢复码", body, "我已保存恢复码", async () => {
    try { await invoke("recovery_code_pending_ack"); } catch (_) { /* 下次启动继续提醒 */ }
    toast("恢复码已确认保存", "ok");
  }, { okStyle: "primary" });
}

function showRecoveryCodeExplanation() {
  openInfoModal(
    "恢复码有啥用？（端到端数据钥匙）",
    `<div style="line-height:1.8;font-size:0.9rem;color:var(--ink-2);max-height:60vh;overflow-y:auto;padding-right:6px;">
      <h4 style="margin:0 0 10px;color:var(--ink);">什么是恢复码？（20 字符）</h4>
      <p>息壤（Solum）采用严格的<strong>零知识端到端加密（E2EE）</strong>架构。恢复码是在您设备本地离线生成的唯一一把<strong>主解密私钥信封</strong>。</p>
      <h4 style="margin:12px 0 6px;color:var(--ink);">它到底有什么用？</h4>
      <p><strong>1. 更换新设备登录时解密数据：</strong><br>
      当您换新手机或在新电脑上登录账号时，云端存储的仅为高强度加密的乱码密文。只有输入这串 20 字符恢复码，才能在本地解开您的记忆、日程与资料，完成数据无缝恢复。</p>
      <p><strong>2. 忘记密码时紧急找回：</strong><br>
      如果您遗忘了账号登录密码，恢复码是唯一的紧急通道，可直接凭恢复码重置密码并恢复本地空间。</p>
      <h4 style="margin:12px 0 6px;color:var(--ink);color:var(--danger,#c53030);">【极其重要】如果不保存会有什么后果？</h4>
      <p>息壤团队与中心服务器<strong>绝不保存、也无法查看您的恢复码</strong>。<br>
      若您遗忘了密码，且<strong>没有备份恢复码</strong>，根据密码学原理，任何人都无法为您破解或恢复加密数据，您所记录的本地数据将<strong>永久无法找回</strong>！</p>
      <p style="margin-top:10px;padding:8px 12px;background:var(--bg-card,#f7fafc);border-radius:6px;border:1px solid var(--line);">
        💡 <strong>强烈建议：</strong>生成后立即抄写在实体纸笔上放入钱包，或妥善存入 1Password / Bitwarden / 手机备忘录等安全密码管理器中。
      </p>
    </div>`
  );
}
$("recoveryCodeExplainBtn")?.addEventListener("click", showRecoveryCodeExplanation);

function setRecoverMode(on) {
  $("accountLoginForm").hidden = on || accountLoggedIn;
  $("accountRecoverForm").hidden = !on;
  $("accountRecoverToggle").hidden = on || accountLoggedIn;
}

async function refreshRecoveryCodeStatus() {
  if (!accountLoggedIn || recoveryCodeChecking) return;
  recoveryCodeChecking = true;
  const dot = $("recoveryCodeDot");
  const text = $("recoveryCodeStatusText");
  try {
    const state = await invoke("recovery_code_status");
    if (state.configured) {
      dot.className = "sdot ok";
      text.textContent = "已设置恢复码：忘记密码时可用它登录并找回数据。重新生成会使旧码失效。";
      $("recoveryCodeIssueBtn").textContent = "重新生成恢复码";
    } else {
      dot.className = "sdot amber";
      text.textContent = "还没有恢复码：如果忘记密码，将没有任何办法找回加密数据。建议现在生成一个。";
      $("recoveryCodeIssueBtn").textContent = "生成恢复码";
    }
  } catch (_) {
    dot.className = "sdot";
    text.textContent = "恢复码状态暂时无法确认，可稍后再试。";
  } finally {
    recoveryCodeChecking = false;
  }
}

$("recoveryCodeIssueBtn").addEventListener("click", async () => {
  const btn = $("recoveryCodeIssueBtn");
  if (btn.disabled) return;
  const rotating = btn.textContent.includes("重新生成");
  const confirmed = window.confirm(rotating
    ? "重新生成会让旧恢复码立即失效。确定继续吗？"
    : "恢复码只显示这一次，请准备好离线抄写的地方（纸笔或密码管理器）。继续吗？");
  if (!confirmed) return;
  btn.disabled = true;
  try {
    const code = await invoke("recovery_code_issue");
    const out = $("recoveryCodeResult");
    out.hidden = false;
    out.replaceChildren(
      el("div", { class: "hint", style: "margin:8px 0 4px;font-weight:600;",
        text: "请立即离线抄写下面的恢复码，关闭后将无法再次查看：" }),
      el("div", { style: "font-family:var(--mono,monospace);font-size:1.05rem;", text: code }),
      el("div", { class: "hint", style: "margin-top:4px;",
        text: "息壤 与中心都无法找回这个码；丢失它且忘记密码时，加密数据将无法解开。" }),
    );
    toast("恢复码已生成", "ok");
    refreshRecoveryCodeStatus();
  } catch (e) { toast(safeUserError(e, "恢复码生成失败，请稍后重试。"), "danger"); }
  finally { btn.disabled = false; }
});

$("accountChangePassBtn").addEventListener("click", () => {
  const form = $("accountChangePassForm");
  form.hidden = !form.hidden;
  if (!form.hidden) $("accountCurrentPass").focus();
});

$("accountChangePassCancel").addEventListener("click", () => {
  $("accountChangePassForm").hidden = true;
  $("accountCurrentPass").value = "";
  $("accountNewPass").value = "";
});

$("accountChangePassSubmit").addEventListener("click", async () => {
  const btn = $("accountChangePassSubmit");
  if (btn.disabled) return;
  const currentPassword = $("accountCurrentPass").value;
  const newPassword = $("accountNewPass").value;
  if (!currentPassword) return toast("请输入当前密码", "danger");
  if (Array.from(newPassword).length < 8) return toast("新密码至少需要 8 个字符", "danger");
  btn.disabled = true;
  try {
    const message = await invoke("account_change_password", { currentPassword, newPassword });
    $("accountChangePassForm").hidden = true;
    $("accountCurrentPass").value = "";
    $("accountNewPass").value = "";
    toast(message, "ok");
  } catch (e) { toast(safeUserError(e, "密码没有修改，请稍后重试。"), "danger"); }
  finally { btn.disabled = false; }
});

$("accountRecoverToggle").addEventListener("click", () => setRecoverMode(true));
$("accountRecoverCancel").addEventListener("click", () => {
  $("accountRecoverCode").value = "";
  setRecoverMode(false);
});

$("accountRecoverSubmit").addEventListener("click", async () => {
  const btn = $("accountRecoverSubmit");
  if (btn.disabled || accountAuthSubmitting) return;
  const serverUrl = $("accountServer").value.trim() || solumCenterUrl;
  const username = $("accountRecoverUser").value.trim();
  const code = $("accountRecoverCode").value.trim();
  if (!username || !code) return toast("请填写账号和恢复码", "danger");
  btn.disabled = true;
  accountAuthSubmitting = true;
  try {
    const summary = await invoke("recovery_code_login", { serverUrl, username, code });
    $("accountRecoverCode").value = "";
    toast(summary || "已用恢复码登录", "ok");
    accountFormSeeded = false;
    refreshAccount(); refreshCloud(); refreshSyncConfig(); refreshSyncStatus();
  } catch (e) {
    toast(safeUserError(e, "恢复码登录失败，请稍后重试。"), "danger");
  } finally {
    btn.disabled = false;
    accountAuthSubmitting = false;
  }
});

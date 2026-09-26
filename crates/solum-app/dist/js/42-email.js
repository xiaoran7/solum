"use strict";
/* ============================== 邮箱（F21，本机连接器） ============================== */
let emailSettings = null;
let emailFormSeeded = false;
let emailOauthTimer = 0;

const EMAIL_PRESETS = {
  qq: { imapHost: "imap.qq.com", imapPort: 993, smtpHost: "smtp.qq.com", smtpPort: 465, smtpTls: "wrapper" },
  gmail: { imapHost: "imap.gmail.com", imapPort: 993, smtpHost: "smtp.gmail.com", smtpPort: 465, smtpTls: "wrapper" },
  microsoft: { imapHost: "outlook.office365.com", imapPort: 993, smtpHost: "smtp.office365.com", smtpPort: 587, smtpTls: "start_tls" },
};

function setEmailSelect(id, accounts, selected) {
  const select = $(id);
  const previous = selected || select.value;
  select.textContent = "";
  if (!accounts.length) {
    select.appendChild(el("option", { value: "", text: "请先连接一个邮箱" }));
    select.disabled = true;
    return;
  }
  select.disabled = false;
  for (const account of accounts) {
    select.appendChild(el("option", { value: account.id, text: `${account.label} · ${account.address}` }));
  }
  select.value = accounts.some((a) => a.id === previous) ? previous : accounts[0].id;
}

function renderEmailStatus(settings) {
  const box = $("emailStatus");
  box.textContent = "";
  if (!settings.configured) {
    box.appendChild(el("div", { class: "statusline", title: `首次保存会写入：${settings.path}` }, [
      el("span", { class: "sdot" }),
      el("span", { class: "t", text: "尚未连接邮箱；连接后只扫描邮件头中的高价值事件。" }),
    ]));
    return;
  }
  box.appendChild(el("div", { class: "statusline", title: `配置文件：${settings.path}（本地、gitignore、不进同步）` }, [
    el("span", { class: "sdot ok" }),
    el("span", { class: "t", text: `已连接 ${settings.accounts.length} 个邮箱账户；扫描只在你点击后进行。` }),
  ]));
}

function emailSyncAuthFields() {
  const provider = $("emailProvider").value;
  let oauth = $("emailAuthKind").value === "oauth2";
  if ((provider === "qq" || provider === "custom") && oauth) {
    $("emailAuthKind").value = "app_password";
    oauth = false;
  }
  $("emailPasswordFields").hidden = oauth;
  $("emailOauthFields").hidden = !oauth;
  $("emailOauthBtn").hidden = !oauth;
  $("emailTenantRow").hidden = provider !== "microsoft";
  const hints = {
    qq: "QQ 邮箱请先在网页端开启 IMAP/SMTP，并生成授权码。",
    gmail: oauth ? "推荐 OAuth2；需先在 Google Cloud 创建桌面 OAuth 客户端。" : "建议改用 OAuth2；若使用应用专用密码，须先满足 Gmail 的安全策略。",
    microsoft: oauth ? "需要在 Microsoft Entra 注册桌面应用并授予 IMAP/SMTP 委托权限。" : "Microsoft 通常要求现代身份验证；推荐改用 OAuth2。",
    custom: "自定义 IMAP/SMTP 当前使用授权码或应用专用密码。",
  };
  $("emailAuthHint").textContent = hints[provider];
}

function emailApplyProviderPreset(force) {
  const preset = EMAIL_PRESETS[$("emailProvider").value];
  if (preset) {
    const fields = [["emailImapHost", preset.imapHost], ["emailImapPort", preset.imapPort],
      ["emailSmtpHost", preset.smtpHost], ["emailSmtpPort", preset.smtpPort], ["emailSmtpTls", preset.smtpTls]];
    for (const [id, value] of fields) if (force || !$(id).value) $(id).value = value;
  }
  $("emailEndpointFold").open = $("emailProvider").value === "custom";
  emailSyncAuthFields();
}

function emailSeedForm(account) {
  emailFormSeeded = true;
  if (!account) {
    $("emailAccountId").value = "";
    $("emailAccountLabel").value = "";
    $("emailProvider").value = "qq";
    $("emailAddress").value = "";
    $("emailAuthKind").value = "app_password";
    $("emailAppPassword").value = "";
    $("emailClientId").value = "";
    $("emailClientSecret").value = "";
    $("emailTenant").value = "";
    $("emailRemoveBtn").hidden = true;
    emailApplyProviderPreset(true);
    return;
  }
  $("emailAccountId").value = account.id || "";
  $("emailAccountLabel").value = account.label || "";
  $("emailProvider").value = account.provider || "custom";
  $("emailAddress").value = account.address || "";
  $("emailAuthKind").value = account.authKind || "app_password";
  $("emailAppPassword").value = "";
  $("emailAppPassword").placeholder = account.secretTail ? `已保存（${account.secretTail}），留空则沿用` : "首次保存必填";
  $("emailClientId").value = account.clientId || "";
  $("emailClientSecret").value = "";
  $("emailClientSecret").placeholder = account.authKind === "oauth2" ? "已保存时留空则沿用" : "可选";
  $("emailTenant").value = account.tenant || "";
  $("emailImapHost").value = account.imapHost || "";
  $("emailImapPort").value = account.imapPort || "";
  $("emailSmtpHost").value = account.smtpHost || "";
  $("emailSmtpPort").value = account.smtpPort || "";
  $("emailSmtpTls").value = account.smtpTls || "wrapper";
  $("emailRemoveBtn").hidden = false;
  $("emailEndpointFold").open = account.provider === "custom";
  emailSyncAuthFields();
}

function renderEmailAccounts(settings) {
  const box = $("emailAccounts");
  box.textContent = "";
  for (const account of settings.accounts) {
    const detail = account.authKind === "oauth2"
      ? (account.oauthReady ? "OAuth 已授权" : "尚未授权")
      : `授权码 ${account.secretTail || "未保存"}`;
    box.appendChild(el("button", { type: "button", class: "mail-account-chip", title: `编辑 ${account.label}`,
      onclick: () => { $("emailAccountFold").open = true; emailSeedForm(account); } }, [
      el("span", { class: "sdot" + ((account.authKind !== "oauth2" || account.oauthReady) ? " ok" : " amber") }),
      el("span", { text: `${account.label} · ${detail}` }),
    ]));
  }
}

async function refreshEmail(seedForm) {
  let settings;
  try { settings = await invoke("email_config_get"); } catch (_) { return; }
  const read = $("emailReadAccount").value;
  const compose = $("emailComposeAccount").value;
  const eventAccount = $("emailEventAccount").value;
  emailSettings = settings;
  renderEmailStatus(settings);
  renderEmailAccounts(settings);
  setEmailSelect("emailReadAccount", settings.accounts, read);
  setEmailSelect("emailComposeAccount", settings.accounts, compose);
  setEmailSelect("emailEventAccount", settings.accounts, eventAccount);
  if (seedForm || !emailFormSeeded) emailSeedForm(settings.accounts.find((a) => a.id === eventAccount || a.id === read) || settings.accounts[0]);
  // 账户配置默认收起，但没配过账户时必须自动展开——否则首次进来只有一个空收件箱，
  // 没有任何可见的连接入口（2026-07-20 自查：折叠低频表单不能把首次流程一起折掉）。
  if (!settings.configured) $("emailAccountFold").open = true;
}

function emailCollectAccount() {
  const port = (id) => {
    const value = parseInt($(id).value, 10);
    return Number.isInteger(value) && value > 0 && value <= 65535 ? value : null;
  };
  return {
    id: $("emailAccountId").value.trim(), label: $("emailAccountLabel").value.trim(),
    provider: $("emailProvider").value, address: $("emailAddress").value.trim(),
    auth_kind: $("emailAuthKind").value, app_password: $("emailAppPassword").value,
    client_id: $("emailClientId").value.trim(), client_secret: $("emailClientSecret").value,
    tenant: $("emailTenant").value.trim(), imap_host: $("emailImapHost").value.trim(),
    imap_port: port("emailImapPort"), smtp_host: $("emailSmtpHost").value.trim(),
    smtp_port: port("emailSmtpPort"), smtp_tls: $("emailSmtpTls").value,
  };
}

function emailClearMailbox() {
  $("emailMessageList").textContent = "";
  $("emailMessageOut").textContent = "";
}

async function refreshEmailFolders(loadMessages) {
  const accountId = $("emailReadAccount").value;
  const out = $("emailMailboxOut");
  if (!accountId) { out.textContent = ""; emailClearMailbox(); return; }
  out.textContent = "";
  out.appendChild(el("div", { class: "hint", text: "正在读取文件夹…" }));
  try {
    const folders = await invoke("email_folders", { accountId });
    const old = $("emailFolder").value;
    $("emailFolder").textContent = "";
    for (const folder of folders) $("emailFolder").appendChild(el("option", { value: folder.name, text: folder.name }));
    if (!folders.length) $("emailFolder").appendChild(el("option", { value: "INBOX", text: "INBOX" }));
    $("emailFolder").value = folders.some((f) => f.name === old) ? old : (folders.find((f) => f.name.toUpperCase() === "INBOX") || folders[0] || { name: "INBOX" }).name;
    out.textContent = "";
    if (loadMessages) await loadEmailMessages();
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮箱设置暂时无法读取，请稍后重试。") })]));
  }
}

function renderEmailMessages(messages) {
  const list = $("emailMessageList");
  list.textContent = "";
  if (!messages.length) { list.appendChild(emptyBox("mail", "没有匹配的邮件", "可调整文件夹或搜索关键词后再试。")); return; }
  for (const message of messages) {
    list.appendChild(el("button", { type: "button", class: "mail-row" + (message.seen ? "" : " unread"), onclick: () => openEmailMessage(message.uid) }, [
      el("span", { class: "mail-from", text: message.from || "（未知发件人）" }),
      el("span", { class: "mail-subject", text: message.subject || "（无主题）" }),
      el("span", { class: "mail-date", text: message.date || "" }),
    ]));
  }
}

async function loadEmailMessages(forceSearch) {
  const accountId = $("emailReadAccount").value;
  const mailbox = $("emailFolder").value || "INBOX";
  const query = $("emailSearchQuery").value.trim();
  const out = $("emailMailboxOut");
  if (!accountId) return;
  $("emailMessageOut").textContent = "";
  out.textContent = "";
  out.appendChild(el("div", { class: "hint", text: query || forceSearch ? "正在从邮箱读取…" : "正在读取最近邮件…" }));
  try {
    const args = { accountId, mailbox, limit: 30 };
    const messages = (query || forceSearch)
      ? await invoke("email_search", { ...args, query })
      : await invoke("email_messages", args);
    out.textContent = "";
    out.appendChild(el("div", { class: "statusline" }, [el("span", { class: "sdot ok" }), el("span", { text: `已加载 ${messages.length} 封邮件；内容只保留在当前界面内存。` })]));
    renderEmailMessages(messages);
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮件暂时无法读取，请稍后重试。") })]));
  }
}

async function openEmailMessage(uid) {
  const accountId = $("emailReadAccount").value;
  const mailbox = $("emailFolder").value || "INBOX";
  const box = $("emailMessageOut");
  box.textContent = "";
  box.appendChild(el("div", { class: "hint", text: "正在读取邮件正文…" }));
  try {
    const message = await invoke("email_message", { accountId, mailbox, uid });
    box.textContent = "";
    box.appendChild(el("div", { class: "block", style: "margin:0;" }, [
      el("div", { class: "statusline" }, [el("span", { class: "t", text: message.subject || "（无主题）" })]),
      el("div", { class: "hint", text: `发件人：${message.from || ""}\n收件人：${message.to || ""}${message.cc ? "\n抄送：" + message.cc : ""}\n日期：${message.date || ""}${message.attachmentCount ? "\n附件：" + message.attachmentCount + " 个（当前版本不下载）" : ""}` }),
      el("pre", { class: "mail-content", text: message.text || "（没有可显示的纯文本正文）" }),
    ]));
  } catch (e) {
    box.textContent = "";
    box.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮件正文暂时无法读取，请稍后重试。") })]));
  }
}

function emailList(value) {
  return value.split(/[;,\n]/).map((item) => item.trim()).filter(Boolean);
}

async function beginEmailOAuth() {
  const account = emailCollectAccount();
  if (account.auth_kind !== "oauth2") return;
  const out = $("emailConfigOut");
  $("emailOauthBtn").disabled = true;
  try {
    const settings = await invoke("email_config_save", { args: account });
    emailSettings = settings;
    await refreshEmail(true);
    const begin = await invoke("email_oauth_begin", { accountId: account.id });
    out.textContent = "";
    const link = el("a", { href: begin.authorizationUrl, target: "_blank", rel: "noopener", text: "若浏览器没有打开，请点这里继续授权" });
    out.appendChild(el("div", { class: "llm-test" }, [
      icon("cloud", 15), el("span", { text: `等待浏览器授权；回调地址：${begin.redirectUri}。` }), link,
    ]));
    window.open(begin.authorizationUrl, "_blank", "noopener");
    clearTimeout(emailOauthTimer);
    const poll = async () => {
      try {
        const result = await invoke("email_oauth_poll", { sessionId: begin.sessionId });
        if (result.status === "pending") { emailOauthTimer = setTimeout(poll, 1200); return; }
        out.textContent = "";
        out.appendChild(el("div", { class: "llm-test ok" }, [icon("check", 15), el("span", { text: "OAuth 授权已完成，刷新令牌仅保存于本机。" })]));
        toast("邮箱 OAuth 授权已完成", "ok");
        await refreshEmail(true);
      } catch (e) {
        out.textContent = "";
        out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮箱授权没有完成，请重新授权。") })]));
      }
    };
    emailOauthTimer = setTimeout(poll, 1200);
  } catch (e) { toast(String(e), "danger"); }
  finally { $("emailOauthBtn").disabled = false; }
}

$("emailProvider").addEventListener("change", () => emailApplyProviderPreset(true));
$("emailAuthKind").addEventListener("change", emailSyncAuthFields);
$("emailSaveBtn").addEventListener("click", async () => {
  const account = emailCollectAccount();
  $("emailSaveBtn").disabled = true;
  try {
    await invoke("email_config_save", { args: account });
    $("emailAppPassword").value = "";
    $("emailClientSecret").value = "";
    toast("邮箱账户已保存到本机", "ok");
    await refreshEmail(true);
  } catch (e) { toast(String(e), "danger"); }
  finally { $("emailSaveBtn").disabled = false; }
});
$("emailOauthBtn").addEventListener("click", beginEmailOAuth);
$("emailEventScanBtn").addEventListener("click", async () => {
  const accountId = $("emailEventAccount").value;
  const button = $("emailEventScanBtn");
  const out = $("emailEventOut");
  if (!accountId) return toast("请先连接邮箱账户", "danger");
  button.disabled = true;
  out.textContent = "";
  out.appendChild(el("div", { class: "hint", text: "正在本机扫描最近 100 封邮件的标题信息…" }));
  try {
    const drafts = await invoke("email_high_value_scan", { accountId });
    const count = drafts.length;
    out.textContent = "";
    out.appendChild(el("div", { class: "statusline" }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: count ? `发现 ${count} 条高价值事件，已放入待确认信息箱（重复项会合并）。` : "未发现新的高价值事件。" }),
    ]));
    await refreshCaptureInbox();
    await refreshCapture();
  } catch (e) {
    out.textContent = "";
    out.appendChild(el("div", { class: "llm-test danger" }, [icon("alert", 15), el("span", { text: safeUserError(e, "邮箱事件暂时无法扫描，请稍后重试。") })]));
  } finally { button.disabled = false; }
});
$("emailRemoveBtn").addEventListener("click", () => {
  const accountId = $("emailAccountId").value.trim();
  if (!accountId) return;
  openModal("移除本机邮箱账户", el("div", { text: `将从本机配置移除 “${accountId}” 的凭据和账户信息；不会删除服务商邮箱中的任何邮件。` }), "移除", async () => {
    await invoke("email_config_remove", { accountId });
    emailFormSeeded = false;
    emailClearMailbox();
    await refreshEmail(true);
    toast("已移除本机邮箱账户", "ok");
  }, { icon: "trash" });
});
$("emailReadAccount").addEventListener("change", () => { emailClearMailbox(); refreshEmailFolders(false); });
$("emailFolder").addEventListener("change", () => { emailClearMailbox(); loadEmailMessages(); });
$("emailRefreshBtn").addEventListener("click", () => loadEmailMessages());
$("emailSearchBtn").addEventListener("click", () => loadEmailMessages(true));
$("emailSearchQuery").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); loadEmailMessages(true); } });
$("emailSendBtn").addEventListener("click", async () => {
  const draft = {
    accountId: $("emailComposeAccount").value, to: emailList($("emailTo").value), cc: emailList($("emailCc").value),
    bcc: emailList($("emailBcc").value), subject: $("emailSubject").value, body: $("emailBody").value, html: $("emailHtml").checked,
  };
  if (!draft.accountId) return toast("请先选择发件账户", "danger");
  if (!draft.to.length) return toast("请至少填写一个收件人", "danger");
  try {
    const pending = await invoke("guard_request", { tool: "email_send", args: JSON.stringify(draft), now: nowParam() });
    openGuardConfirm(pending, false);
  } catch (e) { toast(String(e), "danger"); }
});
emailApplyProviderPreset(true);

"use strict";
/* ============================== 同步账户（设置 → 同步） ============================== */
// 账号访问令牌只存在 solum-account.json；这里仅配置中继和端到端加密密码。
// 密码由 Rust 派生为 key 后丢弃，留空保存表示沿用已经派生的 key。
let syncFormSeeded = false;

function renderSyncConfigStatus(s) {
  const box = $("syncConfigStatus");
  box.textContent = "";
  if (s.format === "account") {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}（本地、gitignore，不进同步）` }, [
      el("span", { class: "sdot ok" }),
      el("span", { class: "t", text: `已连接：${s.username} · ${s.url}（设备 ${s.device_id}）` }),
    ]));
  } else if (s.format === "credentials") {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}` }, [
      el("span", { class: "sdot amber" }),
      el("span", { class: "t", text: `检测到旧版用户名密码配置（设备 ${s.device_id}）——登录 Solum 账号并保存即可迁移。` }),
    ]));
  } else if (s.format === "raw") {
    box.appendChild(el("div", { class: "statusline", title: `配置文件：${s.path}` }, [
      el("span", { class: "sdot amber" }),
      el("span", { class: "t", text: `正在使用 legacy 静态令牌（设备 ${s.device_id}）——登录 Solum 账号并保存即可迁移。` }),
    ]));
  } else {
    box.appendChild(el("div", { class: "banner" }, [
      icon("alert", 18),
      el("div", { class: "grow" }, [
        el("div", { text: "同步未配置——这台设备的数据不会同步到其他设备。" }),
        el("div", { class: "hint", text: `保存后写入：${s.path}` }),
      ]),
    ]));
  }
}

async function refreshSyncConfig() {
  let s;
  try { s = await invoke("sync_config_get"); } catch (_) { return; }
  renderSyncConfigStatus(s);
  const identity = $("syncAccountIdentity");
  identity.textContent = "";
  identity.appendChild(el("span", { class: `sdot${s.account_logged_in ? " ok" : ""}` }));
  identity.appendChild(el("span", { class: "t", text: s.account_logged_in ? `已登录 ${s.username}` : "尚未登录，请先到“云端接入”登录" }));
  $("syncSaveBtn").disabled = !s.account_logged_in;
  if (syncFormSeeded) return;
  syncFormSeeded = true;
  $("syncUrl").value = s.url;
}

$("syncSaveBtn").addEventListener("click", async () => {
  const cfg = {
    url: $("syncUrl").value.trim(),
    password: "",
  };
  if (!/^https?:\/\//.test(cfg.url)) return toast("同步服务器地址需要以 http(s):// 开头", "danger");
  $("syncSaveBtn").disabled = true;
  try {
    const summary = await invoke("sync_config_save", { cfg });
    toast(summary, "ok");
    syncFormSeeded = false;
    await refreshSyncConfig();
    await refreshSyncStatus();
  } catch (e) { toast(String(e), "danger"); }
  finally { $("syncSaveBtn").disabled = false; }
});


"use strict";

// Android WebView 下 env(safe-area-inset-*) 可能恒为 0，CSS 里 html.android
// 分支提供状态栏/手势条兜底内边距（见移动端媒体查询注释）。
const IS_ANDROID = /Android/i.test(navigator.userAgent);
if (IS_ANDROID) document.documentElement.classList.add("android");
const rawInvoke = window.__TAURI__ && window.__TAURI__.core
  ? window.__TAURI__.core.invoke.bind(window.__TAURI__.core)
  : () => Promise.reject(new Error("Tauri API 不可用（请从桌面应用打开）"));

// 只缓存幂等、权威数据的短时读取：目的不是复制一份数据库，而是合并同一轮
// refreshToday / refreshAll 发出的重复 IPC。邮件内容、隐私同意状态和配置密钥
// 不进入这里；任何写命令成功后整代失效，删除/改写会立即回到权威存储。
const IPC_CACHE_TTL_MS = new Map([
  ["sync_status", 1200], ["sync_config_get", 1200],
  ["agenda", 900], ["due", 900], ["all_notifications", 900], ["all_events", 900],
  ["suggestions", 900], ["behavior_log", 900], ["health_status", 1200],
  ["ledger", 900], ["routines", 900], ["persona_get", 1200], ["rules", 1200],
  ["proactivity_get", 1200], ["notif_cloud_get", 900],
  ["capture_entry_points", 1200], ["capture_inbox_list", 700],
  ["notif_intelligence_status", 700], ["tools", 1500], ["audit", 900],
  ["llm_config_get", 1200], ["account_status_get", 1200],
  ["soulous_config_get", 1200], ["widget_defs", 900], ["widget_records", 900],
]);
const IPC_READ_ONLY_UNCACHED = new Set([
  "app_info", "email_config_get", "email_folders", "email_message", "email_messages",
  "email_search", "health_samples", "health_trends", "notif_access_status", "notif_intelligence_apps",
  "notif_intelligence_auto_event_counts", "privacy_consent_status", "review",
]);
const ipcCache = new Map();
const ipcInFlight = new Map();
let ipcCacheGeneration = 0;
function ipcCacheKey(command, args) {
  try { return `${command}:${JSON.stringify(args || {})}`; }
  catch (_) { return command; }
}
function invalidateIpcCache() {
  ipcCacheGeneration += 1;
  ipcCache.clear();
}
const ACCOUNT_EXPIRY_PATTERN = /登录已过期|认证已过期|当前账号会话已退出|旧账号会话缺少[^。]*重新登录|(?:^|\s)(?:401|unauthorized)(?:\s|$)/i;
const ACCOUNT_EXPIRY_BYPASS = new Set([
  "account_login", "account_register", "account_logout", "account_status_get",
  "account_oauth_config_get", "account_oauth_login", "account_recover_login",
  "account_email_login", "account_email_register", "account_email_send_code", "account_captcha_get",
  "account_phone_login", "account_phone_register", "account_sms_send", "account_huawei_login",
]);
function isAccountExpiryError(error) {
  return ACCOUNT_EXPIRY_PATTERN.test(String(error || ""));
}
function observeAccountExpiry(command, promise) {
  return promise.catch((error) => {
    if (!ACCOUNT_EXPIRY_BYPASS.has(command) && isAccountExpiryError(error)) {
      void handleAccountExpiry(error, command);
    }
    throw error;
  });
}
function invoke(command, args) {
  const ttl = IPC_CACHE_TTL_MS.get(command);
  if (!ttl) {
    return observeAccountExpiry(command, rawInvoke(command, args).then((value) => {
      if (!IPC_READ_ONLY_UNCACHED.has(command)) invalidateIpcCache();
      return value;
    }));
  }
  const key = ipcCacheKey(command, args);
  const now = performance.now();
  const cached = ipcCache.get(key);
  if (cached && now - cached.at < ttl) return Promise.resolve(cached.value);
  const pending = ipcInFlight.get(key);
  if (pending) return observeAccountExpiry(command, pending);
  const generation = ipcCacheGeneration;
  const request = rawInvoke(command, args).then((value) => {
    if (generation === ipcCacheGeneration) ipcCache.set(key, { at: performance.now(), value });
    return value;
  }).finally(() => ipcInFlight.delete(key));
  ipcInFlight.set(key, request);
  return observeAccountExpiry(command, request);
}

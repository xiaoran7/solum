import fs from "node:fs";
import vm from "node:vm";

const htmlPath = new URL("../crates/solum-app/dist/index.html", import.meta.url);
const appPath = new URL("../crates/solum-app/src/lib.rs", import.meta.url);
const html = fs.readFileSync(htmlPath, "utf8");
const app = fs.readFileSync(appPath, "utf8");

const inlineScripts = [...html.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/gi)];
if (inlineScripts.length === 0) {
  throw new Error("index.html has no inline script to validate");
}
for (const [index, match] of inlineScripts.entries()) {
  new vm.Script(match[1], { filename: `index.html:inline-${index + 1}` });
}

const ids = [...html.matchAll(/\bid\s*=\s*["']([^"']+)["']/gi)].map((match) => match[1]);
const duplicates = [...new Set(ids.filter((id, index) => ids.indexOf(id) !== index))];
if (duplicates.length > 0) {
  throw new Error(`duplicate HTML ids: ${duplicates.join(", ")}`);
}

if (/invoke\s*\(\s*["']event_cancel["']/.test(html)) {
  throw new Error("event_cancel must go through guard_request/guard_confirm, not direct IPC");
}
if (/#\[tauri::command\]\s*fn\s+event_cancel\b/.test(app)) {
  throw new Error("event_cancel must not be exposed as a direct Tauri command");
}

const acceptStart = html.indexOf('$("privacyGateAcceptBtn").addEventListener');
const declineStart = html.indexOf('$("privacyGateDeclineBtn").addEventListener', acceptStart);
const acceptHandler = html.slice(acceptStart, declineStart);
const consentAt = acceptHandler.indexOf('"privacy_consent_accept"');
const notificationAt = acceptHandler.indexOf('"notification_permission_request"');
if (acceptStart < 0 || declineStart < 0 || consentAt < 0 || notificationAt < consentAt) {
  throw new Error("POST_NOTIFICATIONS must be requested only after privacy consent succeeds");
}

const runStart = app.indexOf("pub fn run()");
const handlerStart = app.indexOf(".invoke_handler", runStart);
if (runStart < 0 || handlerStart < 0 || app.slice(runStart, handlerStart).includes(".request_permission()")) {
  throw new Error("Tauri setup must not request notification permission before the privacy gate");
}

const requiredAccountGateIds = [
  "accountGate", "accountGateLoginTab", "accountGateRegisterTab", "accountGateSubmit",
  "accountGuestBtn", "accountShellBtn", "accountShellAvatarM",
];
for (const id of requiredAccountGateIds) {
  if (!ids.includes(id)) throw new Error(`account shell is missing #${id}`);
}
const privacyViewStart = html.indexOf('id="view-privacy"');
const nextViewStart = html.indexOf('<section class="view"', privacyViewStart + 1);
const privacyView = html.slice(privacyViewStart, nextViewStart);
if (!privacyView.includes('id="privacyAccountLock"') || !privacyView.includes('id="privacyAccountControls"')) {
  throw new Error("guest privacy account controls must stay inside #view-privacy");
}
if (!html.includes('const GUEST_ALLOWED_VIEWS = new Set(["chat", "agenda", "notifs", "ledger", "journal", "search", "settings", "privacy"])')) {
  throw new Error("guest view contract changed without updating the frontend gate");
}
if (!html.includes("if (!accountLoggedIn && !GUEST_ALLOWED_VIEWS.has(view))")) {
  throw new Error("showView must guard every account-only route");
}
if (!/GUEST_ALLOWED_REFRESHERS[\s\S]*?refreshPrivacy[\s\S]*?refreshToday/.test(html)) {
  throw new Error("guest startup must refresh the privacy lock state");
}
const letterSpacingDeclarations = [...html.matchAll(/letter-spacing\s*:\s*([^;}{]+)/gi)];
const nonZeroLetterSpacing = letterSpacingDeclarations.filter((match) => !/^0(?:\s*!important)?$/i.test(match[1].trim()));
if (nonZeroLetterSpacing.length > 0) {
  throw new Error("all Solum letter spacing declarations must be zero");
}
if (/font-size\s*:\s*clamp\s*\(/i.test(html)) {
  throw new Error("font size must not scale with viewport width");
}
if (/(?:radial|linear)-gradient\s*\(/i.test(html)) {
  throw new Error("decorative gradients are forbidden in the Solum workbench");
}
if (!app.includes("The account proxy is the only cloud path")) {
  throw new Error("Rust startup must document and enforce account-only cloud access");
}
if (/else\s*\{\s*solum_core::llm::LlmConfig::load\(\)/s.test(app)) {
  throw new Error("guest startup must not load a legacy direct-key reasoner");
}
if (!app.includes('return Err("多设备同步需要先注册或登录 Solum 账号".into());')) {
  throw new Error("manual/background sync must require an account session");
}
const tickerStart = app.indexOf("fn ticker(");
const tickerEnd = app.indexOf("\nfn resync_alarms", tickerStart);
const ticker = app.slice(tickerStart, tickerEnd);
if (!/if full_account \{\s*let _ = o\.materialize_routines\(now\);\s*\}/s.test(ticker)) {
  throw new Error("guest ticker must not materialize account-only routines");
}

const accountOnlyCommands = [
  "llm_config_get", "soulous_config_get", "soulous_config_save", "soulous_pull",
  "email_config_get", "email_config_save", "email_config_remove", "email_oauth_begin", "email_oauth_poll",
  "widget_confirm_preview", "widget_discard_preview", "widget_defs", "widget_records", "widget_add_field",
  "widget_import_events", "widget_promote_record", "widget_record_create", "widget_record_update",
  "rules", "rules_save", "proactivity_get", "proactivity_set", "notif_cloud_get", "notif_cloud_set",
  "notif_intelligence_apps", "notif_intelligence_status", "notif_intelligence_acknowledge_losses",
  "notif_intelligence_set_app", "notif_intelligence_set_app_auto_event", "notif_intelligence_auto_event_counts",
  "notif_intelligence_set_batch_interval", "notif_intelligence_add_priority_rule", "notif_intelligence_remove_priority_rule",
  "notif_intelligence_set_filter_proposal", "notif_intelligence_set_action_proposal", "notif_intelligence_remove_filter_rule",
  "notif_intelligence_restore_capture", "notif_intelligence_promote_capture", "notif_intelligence_process_now",
  "notif_pipeline_status", "notif_pipeline_request_battery_optimization", "notif_pipeline_open_battery_settings",
  "notif_pipeline_open_background_settings", "checkin_now", "suggestions", "suggest_generate", "suggest_set",
  "routines", "routine_set_active", "routine_update", "routine_done", "stats", "review", "daily_brief",
  "persona_get", "persona_set", "persona_import_preview", "persona_import_save", "persona_rollback",
  "notif_access_status", "notif_access_open_settings", "health_status", "health_request", "health_samples",
  "sync_config_get", "sync_status", "sync_gap_acknowledge",
];
for (const command of accountOnlyCommands) {
  const declaration = new RegExp(`(?:async\\s+)?fn\\s+${command}\\s*\\(`);
  const start = app.search(declaration);
  if (start < 0) throw new Error(`account-only command is missing: ${command}`);
  const body = app.slice(start, app.indexOf("\n#[tauri::command]", start + 1) > 0
    ? app.indexOf("\n#[tauri::command]", start + 1)
    : app.length);
  if (!body.includes("require_full_account()?")) {
    throw new Error(`${command} must enforce require_full_account()`);
  }
}
if (!html.includes("登录后生成包含建议与跨设备信息的今日简报")) {
  throw new Error("daily brief entry must explain the account lock to guests");
}

console.log(`frontend checks passed: ${inlineScripts.length} script, ${ids.length} unique ids`);

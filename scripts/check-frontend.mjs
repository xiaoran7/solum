import fs from "node:fs";
import vm from "node:vm";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const htmlPath = new URL("../crates/solum-app/dist/index.html", import.meta.url);
const appJsDir = new URL("../crates/solum-app/dist/", import.meta.url);
const stylesPath = new URL("../crates/solum-app/dist/styles.css", import.meta.url);
const i18nJsPath = new URL("../crates/solum-app/dist/i18n.js", import.meta.url);
const tauriConfPath = new URL("../crates/solum-app/tauri.conf.json", import.meta.url);
const appSrcDir = new URL("../crates/solum-app/src/", import.meta.url);
const accountCorePath = new URL("../crates/solum-core/src/account.rs", import.meta.url);
const defaultAvatarPath = new URL("../crates/solum-app/dist/assets/avatars/default-sprout.png", import.meta.url);
const notificationPipelinePath = new URL(
  "../crates/solum-notif-access/android/src/main/java/dev/solum/notifaccess/NotificationPipelineService.kt",
  import.meta.url,
);
const html = fs.readFileSync(htmlPath, "utf8");
const styles = fs.readFileSync(stylesPath, "utf8");
const i18nJs = fs.readFileSync(i18nJsPath, "utf8");
const tauriConf = fs.readFileSync(tauriConfPath, "utf8");

// The application ships as ordered classic <script src="js/NN-*.js"> modules —
// no bundler, and the CSP has no 'unsafe-inline'. index.html's script order is
// therefore the single source of truth: it is both the runtime load order and
// the declaration order every position-based assertion below depends on. So
// rebuild the one logical program from that list instead of reading a file.
const scriptSources = [...html.matchAll(/<script\s+src="([^"]+)"><\/script>/g)].map((m) => m[1]);
const moduleSources = scriptSources.filter((src) => src.startsWith("js/"));
if (moduleSources.length === 0) {
  throw new Error("index.html must load the application as ordered <script src=\"js/*.js\"> modules");
}
const appJs = moduleSources
  .map((src) => {
    const file = new URL(src, appJsDir);
    if (!fs.existsSync(file)) throw new Error(`index.html loads ${src} but it does not exist`);
    return fs.readFileSync(file, "utf8");
  })
  .join("\n");
const onDisk = fs
  .readdirSync(new URL("js/", appJsDir))
  .filter((name) => name.endsWith(".js"))
  .map((name) => `js/${name}`);
// A module that was split off but never wired into index.html would be dead
// weight that still passes every content assertion, so guard both directions.
for (const name of onDisk) {
  const uses = scriptSources.filter((src) => src === name).length;
  if (uses === 0) throw new Error(`${name} exists but index.html never loads it`);
  if (uses > 1) throw new Error(`${name} is loaded ${uses} times; each module must load exactly once`);
}
// Numeric prefixes carry the load order; the filenames must agree with it so
// that a stray reorder in the directory cannot silently disagree with the HTML.
const ordered = onDisk.slice().sort();
const byHtml = moduleSources.map((src) => `js/${src.slice(src.lastIndexOf("/") + 1)}`);
if (ordered.join("\n") !== byHtml.join("\n")) {
  throw new Error(
    "dist/js filenames and the index.html load order disagree:\n" +
      `  on disk: ${ordered.join(", ")}\n` +
      `  in HTML: ${byHtml.join(", ")}`,
  );
}

function readAllRs(dir) {
  let content = "";
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = new URL(entry.name + (entry.isDirectory() ? "/" : ""), dir);
    if (entry.isDirectory()) {
      content += "\n" + readAllRs(full);
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      content += "\n" + fs.readFileSync(full, "utf8");
    }
  }
  return content;
}
const app = readAllRs(appSrcDir);
const accountCore = fs.readFileSync(accountCorePath, "utf8");
const notificationPipeline = fs.readFileSync(notificationPipelinePath, "utf8");

// The application lives in dist/js/*.js since the CSP dropped 'unsafe-inline'
// (2026-08-26). index.html must not grow a new inline <script> — that would
// be silently blocked in production and silently skipped here if this check
// only looked at inline content.
const inlineScripts = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)];
if (inlineScripts.length !== 0) {
  throw new Error("index.html must not contain inline <script>; put JS in dist/js/ (CSP script-src has no 'unsafe-inline')");
}
// Styles moved out to dist/styles.css so index.html holds only the document
// skeleton. CSP allows 'unsafe-inline' for style-src, so a re-inlined block
// would not be blocked in production — it would just silently make every CSS
// assertion above validate an empty string again.
if (/<style(?![^>]*\bsrc=)[^>]*>/i.test(html)) {
  throw new Error("index.html must not contain inline <style>; put CSS in dist/styles.css");
}
if (!/<link\s+rel="stylesheet"\s+href="styles\.css"\s*\/?>/.test(html)) {
  throw new Error('index.html must load styles via <link rel="stylesheet" href="styles.css">');
}
if (styles.length === 0) throw new Error("dist/styles.css is empty; the design tokens and layout live there");
new vm.Script(i18nJs, { filename: "dist/i18n.js" });
// Parse every module on its own too: concatenated as one blob a file with a
// stray top-level `return` or unbalanced brace can still be masked by a
// neighbour, and each module really is its own classic script at runtime.
for (const src of moduleSources) {
  new vm.Script(fs.readFileSync(new URL(src, appJsDir), "utf8"), { filename: `dist/${src}` });
}
// Every JS-content assertion below reads `js` (= app.js); HTML-only checks
// keep reading `html`. A regression that moves code back into index.html now
// fails loudly instead of validating an empty string.
const js = appJs;
if (!/script-src[^;]*'self'(?![^;]*'unsafe-inline')/.test(tauriConf)) {
  throw new Error("CSP script-src must not re-add 'unsafe-inline'");
}

const rawErrorRenderPatterns = [
  /text\s*:\s*String\((?:e|error)\)/,
  /textContent\s*=\s*String\((?:e|error)\)/,
  /emptyBox\([^\n]*String\((?:e|error)\)/,
  /setPrivacyGateStatus\([^\n]*String\((?:e|error)\)/,
];
for (const pattern of rawErrorRenderPatterns) {
  if (pattern.test(js)) {
    throw new Error(`raw exceptions must pass through safeUserError: ${pattern}`);
  }
}
const backupSortStart = js.indexOf("backups.sort(");
const backupSortEnd = js.indexOf('$("backupCount")', backupSortStart);
const backupSort = js.slice(backupSortStart, backupSortEnd);
if (backupSortStart < 0 || backupSortEnd < 0
    || !/right\.committed_at[\s\S]*?left\.committed_at[\s\S]*?right\.backup_id[\s\S]*?left\.backup_id/.test(backupSort)) {
  throw new Error("backup center must order sealed versions by server committed_at with a stable backup_id tie-breaker");
}
if (backupSort.includes("created_at")) {
  throw new Error("backup center must not determine the latest sealed version from the client clock");
}
if (!js.includes("backupDate(latest.committed_at)") || !js.includes("backupDate(version.committed_at)")) {
  throw new Error("backup center must display server commit time for sealed versions");
}

const ids = [...html.matchAll(/\bid\s*=\s*["']([^"']+)["']/gi)].map((match) => match[1]);
const duplicates = [...new Set(ids.filter((id, index) => ids.indexOf(id) !== index))];
if (duplicates.length > 0) {
  throw new Error(`duplicate HTML ids: ${duplicates.join(", ")}`);
}

if (/invoke\s*\(\s*["']event_cancel["']/.test(js)) {
  throw new Error("event_cancel must go through guard_request/guard_confirm, not direct IPC");
}
if (/#\[tauri::command\]\s*fn\s+event_cancel\b/.test(app)) {
  throw new Error("event_cancel must not be exposed as a direct Tauri command");
}

const acceptStart = js.indexOf('$("privacyGateAcceptBtn").addEventListener');
const declineStart = js.indexOf('$("privacyGateDeclineBtn").addEventListener', acceptStart);
const acceptHandler = js.slice(acceptStart, declineStart);
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
  "accountGateConfirmField", "accountGateConfirm", "accountShellBtn",
  "accountShellAvatarM",
];
for (const id of requiredAccountGateIds) {
  if (!ids.includes(id)) throw new Error(`account shell is missing #${id}`);
}
if (!js.includes('password !== confirm') && !js.includes('password !== $("accountGateConfirm").value')) {
  throw new Error("registration must confirm the exact password before creating an account");
}
if (js.includes('submitAccount("account_register"')) {
  throw new Error("registration must use the single account-gate flow");
}
const privacyViewStart = html.indexOf('id="view-privacy"');
const nextViewStart = html.indexOf('<section class="view"', privacyViewStart + 1);
const privacyView = html.slice(privacyViewStart, nextViewStart);
if (!privacyView.includes('id="privacyAccountLock"') || !privacyView.includes('id="privacyAccountControls"')) {
  throw new Error("guest privacy account controls must stay inside #view-privacy");
}
if (!js.includes('if (!accountLoggedIn')) {
  throw new Error("showView must guard every account-only route");
}
const overlayHistoryStart = js.indexOf("function anyOverlayOpen()");
const overlayHistoryEnd = js.indexOf("function syncOverlayHistory()", overlayHistoryStart);
const overlayHistory = js.slice(overlayHistoryStart, overlayHistoryEnd);
for (const id of ["composerMenu", "sessionMenuMobile", "modalMask", "drawer", "profileEditorModal", "infoModal", "mobileTools", "accountGate"]) {
  if (!overlayHistory.includes(id)) {
    throw new Error(`Android back-stack overlay contract is missing #${id}`);
  }
}
if (html.includes('id="sessionSelectMobile"') || /<select[^>]+选择对话会话/.test(html)) {
  throw new Error("mobile chat history must use the product-styled listbox, never the native select sheet");
}
for (const id of ["sessionPickerMobile", "sessionPickerMobileLabel", "sessionMenuMobile"]) {
  if (!ids.includes(id)) throw new Error(`mobile chat session picker is missing #${id}`);
}
if (!js.includes("collapseUnusedChatSessions(chatSessions.sessions, chatSessions.activeId)")
    || !js.includes("chatSessions.sessions.find(isUnusedChatSession)")) {
  throw new Error("new-chat actions must reuse the one empty session instead of creating unlimited duplicates");
}
if (ids.includes("profileSheetEditAvatarBtn") || html.includes("profile-avatar-badge")) {
  throw new Error("the removed avatar emoji/edit badge must not return; profile editing belongs to the profile action");
}
if (!/\.drawer-head\s*\{[\s\S]*?max\(calc\(12px \+ var\(--sat\)\), 34px\)/.test(styles)) {
  throw new Error("the mobile workbench drawer must keep its controls below the top safe area");
}
if (!js.includes('history.replaceState({ solumView: view }, "")')) {
  throw new Error("navigation from a closing overlay must replace its history entry with the destination view");
}
if (!js.includes('class: "meta capture-preview", text: preview')) {
  throw new Error("capture candidates must expose a bounded raw-text preview before adoption");
}
if (!/GUEST_ALLOWED_REFRESHERS[\s\S]*?refreshPrivacy[\s\S]*?refreshToday/.test(js)) {
  throw new Error("guest startup must refresh the privacy lock state");
}
const sendChatStart = js.indexOf("async function sendChat(");
const sendChatEnd = js.indexOf('$("chatForm").addEventListener', sendChatStart);
const sendChatBody = js.slice(sendChatStart, sendChatEnd);
if (!sendChatBody.includes("refreshAfterIngest()") || sendChatBody.includes("refreshAll()")) {
  throw new Error("chat completion must refresh only affected visible surfaces, never the whole app");
}
const ingestRefreshStart = js.indexOf("function refreshAfterIngest()");
const ingestRefreshEnd = js.indexOf("function refreshAll(", ingestRefreshStart);
const ingestRefreshBody = js.slice(ingestRefreshStart, ingestRefreshEnd);
if (!ingestRefreshBody.includes("[refreshNotifs, refreshToday]") || /refresh(?:Ledger|Email|Backup|Account)/.test(ingestRefreshBody)) {
  throw new Error("post-ingest refresh must not preload hidden or account surfaces");
}
const startupStart = js.indexOf("/* ============================== 启动");
const startupBody = js.slice(startupStart);
if (/await\s+refreshAll\s*\(/.test(startupBody)) {
  throw new Error("startup must load the active view instead of refreshing every hidden view");
}
if (!startupBody.includes('document.visibilityState === "hidden"')) {
  throw new Error("periodic refresh must pause while the WebView is hidden");
}
const accountBase = styles.indexOf(".account-gate-shell {");
const finalMobileOverride = styles.lastIndexOf("@media (max-width: 899px)");
if (accountBase < 0 || finalMobileOverride < accountBase) {
  throw new Error("the final mobile account layout must override the desktop account grid");
}
const letterSpacingDeclarations = [...styles.matchAll(/letter-spacing\s*:\s*([^;}{]+)/gi)];
if (/\*\s*\{[^}]*letter-spacing\s*:/is.test(styles)) {
  throw new Error("letter spacing must be assigned by typography role, never forced globally");
}
const unsafeLetterSpacing = letterSpacingDeclarations.filter((match) => {
  const value = match[1].trim();
  if (/^0(?:\s*!important)?$/i.test(value)) return false;
  const em = /^(-?(?:\d+(?:\.\d+)?|\.\d+))em$/i.exec(value);
  return !em || Math.abs(Number(em[1])) > 0.06;
});
if (unsafeLetterSpacing.length > 0) {
  throw new Error("Solum letter spacing must use a bounded typography-role value (at most 0.06em)");
}
if (!/#chatEmpty h3[\s\S]*?letter-spacing:\s*-\.025em/.test(styles)) {
  throw new Error("the chat-home display heading must keep its deliberate compact tracking");
}
if (!/button, a, summary, label, \[role="button"\]\s*\{[^}]*-webkit-tap-highlight-color:\s*transparent/.test(styles)) {
  throw new Error("touch controls must suppress Android WebView's rectangular tap highlight");
}
if (!/\.signal-button\s*\{[^}]*border-radius:\s*50%/.test(styles)
    || !/\.signal-button:active\s*\{[^}]*background:\s*var\(--accent-soft\)[^}]*transform:\s*scale\(0\.94\)/.test(styles)) {
  throw new Error("the mobile account trigger must use a circular custom pressed state");
}
if (/font-size\s*:\s*clamp\s*\(/i.test(styles)) {
  throw new Error("font size must not scale with viewport width");
}
if (/(?:radial|linear)-gradient\s*\(/i.test(styles)) {
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
if (!/if \(url != account\.serverUrl\) return null[\s\S]*?RelayConfig\(url, account\.accessToken/.test(notificationPipeline)) {
  throw new Error("Android background sync must never send an account token to another origin");
}
if (!accountCore.includes("libc::F_OFD_SETLKW") || !notificationPipeline.includes("handle.channel.lock()")) {
  throw new Error("Rust and Kotlin Android session writers must use interoperable OFD/POSIX record locks");
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
if (!js.includes("登录后生成包含建议与跨设备信息的今日简报")) {
  throw new Error("daily brief entry must explain the account lock to guests");
}

const requiredProfileIds = [
  "profilePreviewAvatar", "profileChooseAvatarBtn", "profileDeleteAvatarBtn", "profileAvatarFile",
];
for (const id of requiredProfileIds) {
  if (!ids.includes(id)) throw new Error(`local avatar editor is missing #${id}`);
}
for (const removedId of ["avatarEmojiPicker", "avatarColorPicker", "profileInputChar", "profileApplyCharBtn"]) {
  if (ids.includes(removedId) || js.includes(removedId)) throw new Error(`removed character/color avatar control still exists: ${removedId}`);
}
if (!fs.existsSync(defaultAvatarPath) || !js.includes('const DEFAULT_AVATAR_SRC = "assets/avatars/default-sprout.png"')) {
  throw new Error("the sprout image must be the only default profile avatar");
}
if (!js.includes("MAX_AVATAR_INPUT_BYTES = 12 * 1024 * 1024")
    || !js.includes('canvas.toBlob(resolve, "image/webp", quality)')
    || !js.includes('editingProfileDraft.avatarData = ""')) {
  throw new Error("local avatars must be bounded, compressed, and deletable back to the sprout default");
}

if (!js.includes("AUTH_EXPIRED_REDIRECT_KEY")
    || !js.includes('rawInvoke("account_logout")')
    || !js.includes('openAccountGate("登录状态已过期，请重新登录"')
    || !/accountLoggedIn\s*&&\s*isAccountExpiryError\(s\.state_error\)/.test(js)) {
  throw new Error("expired account sessions must clear locally and redirect to the login gate");
}

const ledgerViewStart = html.indexOf('id="view-ledger"');
const ledgerViewEnd = html.indexOf('<section class="view', ledgerViewStart + 1);
const ledgerView = html.slice(ledgerViewStart, ledgerViewEnd);
if (!ledgerView.includes('id="ledgerSourceFilter"') || !ledgerView.includes('id="ledgerBox"')
    || /exportBtn|importBtn|soulousOutboundLedger|notificationLedger/.test(ledgerView)) {
  throw new Error("the memory landing page must be a content-only list with source filtering");
}
if (!ids.includes("memoryDetailContent") || !ids.includes("memoryDetailSource") || !ids.includes("memoryDetailTime")) {
  throw new Error("memory rows must open a dedicated detail view");
}
if (!js.includes("function migrateMemoryListUiState()")
    || !js.includes('navLastView.memory = "ledger"')
    || !js.includes('viewScrollPositions.set("ledger", 0)')) {
  throw new Error("upgrades from the old tabbed memory page must reset its stale route and scroll position once");
}
if (!/\.memory-row-summary\s*\{[^}]*-webkit-line-clamp:\s*2/s.test(styles)) {
  throw new Error("long memory content must be clamped to two lines in the list");
}
const refreshLedgerStart = js.indexOf("async function refreshLedger()");
const refreshLedgerEnd = js.indexOf("async function refreshCaptures()", refreshLedgerStart);
const refreshLedgerBody = js.slice(refreshLedgerStart, refreshLedgerEnd);
if (refreshLedgerStart < 0 || refreshLedgerEnd < 0 || /soulous_config_get|notif_intelligence_status/.test(refreshLedgerBody)) {
  throw new Error("the memory list must not reload Soulous or notification intelligence");
}
const dataViewStart = html.indexOf('id="view-data"');
const dataViewEnd = html.indexOf('<section class="view', dataViewStart + 1);
const dataView = html.slice(dataViewStart, dataViewEnd);
if (!/exportBtn/.test(dataView) || !/importBtn/.test(dataView) || !/openBackupCenterBtn/.test(dataView)) {
  throw new Error("export, import, and encrypted backup must live under settings data tools");
}

console.log(`frontend checks passed: ${inlineScripts.length} script, ${ids.length} unique ids`);

// Finally, run the modules for real. Every content assertion above can pass
// while the page still throws on load: a module may read a name that a later
// module declares, which is a ReferenceError at load time and invisible to
// `js.includes(...)`. check-frontend-load.mjs executes them in order against a
// DOM stub, which is the only check that sees the failure.
const loadCheck = spawnSync(process.execPath, [fileURLToPath(new URL("check-frontend-load.mjs", import.meta.url))], {
  stdio: "inherit",
});
if (loadCheck.status !== 0) {
  throw new Error("dist/js modules throw at load; see the output above");
}

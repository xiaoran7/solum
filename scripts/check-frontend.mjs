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

console.log(`frontend checks passed: ${inlineScripts.length} script, ${ids.length} unique ids`);

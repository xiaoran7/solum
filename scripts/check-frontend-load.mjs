// Authoritative check: execute the real modules in order, in a DOM stub, and
// report any load-time ReferenceError. This is what the browser does, so it
// answers the only question that matters — does the split break load? — without
// relying on a hand-rolled scope analysis.
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const DIST = "crates/solum-app/dist";
const files = fs.readdirSync(path.join(DIST, "js")).filter((f) => f.endsWith(".js")).sort();

const problems = [];

// --- minimal DOM/IPC stub ----------------------------------------------------
const noop = () => {};
const element = () => {
  // innerHTML assignment in a real parser leaves child nodes behind; icon()
  // depends on this by returning wrap.firstChild.
  const el = {
    _firstChild: null,
    get firstChild() { return el._firstChild; },
    set innerHTML(v) { el._firstChild = element(); },
    style: { setProperty: noop, removeProperty: noop, getPropertyValue: () => "" }, dataset: {}, classList: { add: noop, remove: noop, contains: () => false, toggle: noop },
    children: [], attributes: {},
    value: "", checked: false, textContent: "", innerHTML: "", title: "", disabled: false,
    append: (c) => { el.children.push(c); return c; },
    appendChild: (c) => { el.children.push(c); return c; },
    insertBefore: (c) => c,
    removeChild: noop,
    after: noop,
    prepend: noop,
    closest: () => null,
    contains: () => false,
    matches: () => false,
    setPointerCapture: noop,
    releasePointerCapture: noop,
    scrollTop: 0,
    scrollHeight: 0,
    offsetWidth: 0,
    offsetHeight: 0,
    replaceChildren: noop, setAttribute: noop, removeAttribute: noop, getAttribute: () => null,
    addEventListener: noop, removeEventListener: noop, focus: noop, blur: noop, click: noop,
    querySelector: () => element(), querySelectorAll: () => [],
    getBoundingClientRect: () => ({ top: 0, left: 0, width: 0, height: 0 }),
    scrollIntoView: noop, insertAdjacentHTML: noop, remove: noop, cloneNode: () => element(),
  };
  return el;
};

const doc = {
  documentElement: element(),
  body: element(),
  head: element(),
  visibilityState: "visible",
  addEventListener: noop,
  removeEventListener: noop,
  getElementById: () => element(),
  querySelector: () => element(),
  querySelectorAll: () => [],
  createElement: () => element(),
  createDocumentFragment: () => element(),
  execCommand: () => true,
  activeElement: null,
};
doc.documentElement.className = "";

const storage = () => {
  const m = new Map();
  return {
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => m.set(k, String(v)),
    removeItem: (k) => m.delete(k),
    clear: () => m.clear(),
    key: () => null,
    get length() { return m.size; },
  };
};

const sandbox = {
  window: null,
  document: doc,
  navigator: { userAgent: "Linux; Android 14", platform: "Linux armv8l", language: "zh-CN" },
  localStorage: storage(),
  sessionStorage: storage(),
  location: { href: "http://localhost/", origin: "http://localhost", protocol: "http:" },
  history: { pushState: noop, replaceState: noop, back: noop, state: null },
  console: { log: noop, warn: noop, error: noop, info: noop, debug: noop },
  setTimeout: (fn) => { try { if (typeof fn === "function") fn(); } catch (_) {} return 0; },
  setInterval: () => 0,
  clearTimeout: noop,
  clearInterval: noop,
  requestAnimationFrame: () => 0,
  cancelAnimationFrame: noop,
  matchMedia: () => ({ matches: false, addEventListener: noop, removeEventListener: noop, addListener: noop, removeListener: noop }),
  fetch: () => Promise.reject(new Error("offline")),
  Headers: function Headers() { return {}; },
  Request: function Request() { return {}; },
  Response: function Response() { return {}; },
  Event: function Event() { return {}; },
  CustomEvent: function CustomEvent() { return {}; },
  MutationObserver: function MutationObserver() { return { observe: noop, disconnect: noop }; },
  ResizeObserver: function ResizeObserver() { return { observe: noop, disconnect: noop }; },
  IntersectionObserver: function IntersectionObserver() { return { observe: noop, disconnect: noop }; },
  indexedDB: undefined,
  crypto: { getRandomValues: (a) => a, subtle: { digest: () => Promise.resolve(new ArrayBuffer(32)) } },
  performance: { now: () => 0, getEntriesByType: () => [] },
  screen: { width: 1280, height: 800 },
  getComputedStyle: () => ({ getPropertyValue: () => "" }),
  TextDecoder: function TextDecoder() { return { decode: () => "" }; },
  TextEncoder: function TextEncoder() { return { encode: () => new Uint8Array() }; },
  URL: globalThis.URL,
  Blob: function Blob() { return {}; },
  FileReader: function FileReader() { return {}; },
  AbortController: function AbortController() { return { signal: {}, abort: noop }; },
  structuredClone: (v) => JSON.parse(JSON.stringify(v)),
};
sandbox.window = sandbox;
sandbox.addEventListener = noop;
sandbox.removeEventListener = noop;
sandbox.dispatchEvent = () => true;
sandbox.postMessage = noop;
sandbox.scrollTo = noop;
sandbox.alert = noop;
sandbox.confirm = () => true;
sandbox.prompt = () => null;
sandbox.innerWidth = 1280;
sandbox.innerHeight = 800;
sandbox.scrollX = 0;
sandbox.scrollY = 0;
sandbox.devicePixelRatio = 1;
sandbox.pageXOffset = 0;
sandbox.pageYOffset = 0;
sandbox.self = sandbox;
sandbox.globalThis = sandbox;
// The Tauri bridge is absent in a plain browser; the app is written to degrade.
sandbox.window.__TAURI__ = undefined;

process.on("unhandledRejection", () => {});
process.on("uncaughtException", () => {});

const ctx = vm.createContext(sandbox);

// i18n.js first, exactly as index.html orders it.
const i18n = fs.readFileSync(path.join(DIST, "i18n.js"), "utf8");
try {
  vm.runInContext(i18n, ctx, { filename: "i18n.js" });
} catch (error) {
  problems.push({ file: "i18n.js", line: 0, message: String(error.message) });
}

for (const file of files) {
  const text = fs.readFileSync(path.join(DIST, "js", file), "utf8");
  try {
    vm.runInContext(text, ctx, { filename: file });
  } catch (error) {
    const line = Number(String(error.stack || "").match(/:(\d+):\d+/)?.[1] ?? 0);
    problems.push({ file, line, message: String(error.message) });
  }
}

if (problems.length === 0) {
  console.log(`OK  all ${files.length} modules execute in load order with no load-time error`);
} else {
  console.log(`FAIL  ${problems.length} module(s) threw at load:\n`);
  for (const p of problems) {
    console.log(`  ${p.file}:${p.line}  ${p.message}`);
  }
  process.exitCode = 1;
}

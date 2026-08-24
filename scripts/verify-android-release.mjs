#!/usr/bin/env node
"use strict";

// Fail-closed Android release evidence. This script reads one already-built
// APK; it never installs, signs, aligns, or changes the artifact.

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDir, "..");

function fail(message) {
  console.error("verify-android-release: " + message);
  process.exit(1);
}

function parseArgs(argv) {
  const parsed = { apk: "", aapt: "", apksigner: "" };
  for (const arg of argv) {
    if (arg === "--help" || arg === "-h") {
      console.log("Usage: node scripts/verify-android-release.mjs <release.apk> [--aapt=PATH] [--apksigner=PATH]");
      process.exit(0);
    }
    if (arg.startsWith("--aapt=")) parsed.aapt = arg.slice("--aapt=".length);
    else if (arg.startsWith("--apksigner=")) parsed.apksigner = arg.slice("--apksigner=".length);
    else if (!arg.startsWith("-") && !parsed.apk) parsed.apk = arg;
    else fail("unknown or duplicate argument: " + arg);
  }
  if (!parsed.apk) fail("pass the exact release APK path; implicit latest-file selection is intentionally disabled");
  return parsed;
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    fail("cannot read strict JSON " + file + ": " + error.message);
  }
}

function workspaceVersion() {
  const cargo = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
  const section = cargo.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/);
  const version = section && section[1].match(/^\s*version\s*=\s*"([^"]+)"\s*$/m);
  if (!version) fail("Cargo.toml is missing [workspace.package].version");
  return version[1];
}

function androidSdkRoot() {
  const candidates = [
    process.env.ANDROID_HOME,
    process.env.ANDROID_SDK_ROOT,
    path.join(os.homedir(), "AppData", "Local", "Android", "Sdk"),
  ].filter(Boolean);
  const localProperties = path.join(root, "crates", "solum-app", "gen", "android", "local.properties");
  if (fs.existsSync(localProperties)) {
    const match = fs.readFileSync(localProperties, "utf8").match(/^\s*sdk\.dir\s*=\s*(.+)\s*$/m);
    if (match) candidates.unshift(match[1].trim().replace(/\\:/g, ":").replace(/\\\\/g, "\\"));
  }
  return candidates.find((candidate) => fs.existsSync(candidate)) || "";
}

function newestBuildTool(sdkRoot, name) {
  if (!sdkRoot) return "";
  const buildTools = path.join(sdkRoot, "build-tools");
  if (!fs.existsSync(buildTools)) return "";
  const executable = process.platform === "win32" ? name + (name === "apksigner" ? ".bat" : ".exe") : name;
  const versions = fs.readdirSync(buildTools, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort((left, right) => right.localeCompare(left, undefined, { numeric: true }));
  for (const version of versions) {
    const candidate = path.join(buildTools, version, executable);
    if (fs.existsSync(candidate)) return candidate;
  }
  return "";
}

function run(command, args, label) {
  if (!command || (path.isAbsolute(command) && !fs.existsSync(command))) {
    fail(label + " executable not found: " + (command || "(empty)"));
  }
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    shell: false,
  });
  const output = (result.stdout || "") + (result.stderr || "");
  if (result.error) fail(label + " could not start: " + result.error.message);
  if (result.status !== 0) fail(label + " failed (" + result.status + "):\n" + output.trim());
  return output;
}

function runApksigner(command, args) {
  if (!/\.(bat|cmd)$/i.test(command)) return run(command, args, "apksigner");
  const jar = path.join(path.dirname(command), "lib", "apksigner.jar");
  if (!fs.existsSync(jar)) fail("apksigner.jar not found next to " + command);
  const java = process.env.JAVA_HOME
    ? path.join(process.env.JAVA_HOME, "bin", process.platform === "win32" ? "java.exe" : "java")
    : "java";
  return run(java, ["-Xmx1024M", "-Xss1m", "-jar", jar, ...args], "apksigner");
}

function zipEntries(bytes) {
  const minimum = 22;
  if (bytes.length < minimum) fail("APK is too short to be a ZIP archive");
  const floor = Math.max(0, bytes.length - 65_557);
  let eocd = -1;
  for (let offset = bytes.length - minimum; offset >= floor; offset -= 1) {
    if (bytes.readUInt32LE(offset) === 0x06054b50) {
      eocd = offset;
      break;
    }
  }
  if (eocd < 0) fail("APK ZIP end-of-central-directory record is missing");
  const entries = bytes.readUInt16LE(eocd + 10);
  const centralSize = bytes.readUInt32LE(eocd + 12);
  const centralOffset = bytes.readUInt32LE(eocd + 16);
  if (entries === 0xffff || centralSize === 0xffffffff || centralOffset === 0xffffffff) {
    fail("ZIP64 APKs are not supported by this bounded verifier");
  }
  if (centralOffset + centralSize > bytes.length) fail("APK central directory is out of bounds");
  const names = [];
  let offset = centralOffset;
  for (let index = 0; index < entries; index += 1) {
    if (offset + 46 > bytes.length || bytes.readUInt32LE(offset) !== 0x02014b50) {
      fail("APK central directory entry " + index + " is malformed");
    }
    const nameLength = bytes.readUInt16LE(offset + 28);
    const extraLength = bytes.readUInt16LE(offset + 30);
    const commentLength = bytes.readUInt16LE(offset + 32);
    const end = offset + 46 + nameLength;
    if (end > bytes.length) fail("APK ZIP filename is out of bounds");
    names.push(bytes.subarray(offset + 46, end).toString("utf8"));
    offset = end + extraLength + commentLength;
  }
  if (offset !== centralOffset + centralSize) fail("APK central directory size does not match its entries");
  return new Set(names);
}

function quotedField(text, name) {
  const match = text.match(new RegExp(name + "='([^']*)'"));
  if (!match) fail("aapt badging is missing " + name);
  return match[1];
}

const args = parseArgs(process.argv.slice(2));
const apk = path.resolve(root, args.apk);
if (!fs.existsSync(apk) || !fs.statSync(apk).isFile()) fail("APK does not exist: " + apk);
if (!apk.toLowerCase().endsWith(".apk")) fail("artifact is not an .apk file: " + apk);

const configPath = path.join(root, "crates", "solum-app", "tauri.conf.json");
const config = readJson(configPath);
const sourceVersion = String(config.version || "");
const sourceVersionCode = config.bundle && config.bundle.android && config.bundle.android.versionCode;
const cargoVersion = workspaceVersion();
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(sourceVersion)) fail("tauri.conf.json version is not semver");
if (sourceVersion !== cargoVersion) fail("version mismatch: tauri.conf.json=" + sourceVersion + ", Cargo workspace=" + cargoVersion);
if (!Number.isInteger(sourceVersionCode) || sourceVersionCode <= 0 || sourceVersionCode > 2_147_483_647) {
  fail("tauri.conf.json bundle.android.versionCode must be a positive 32-bit integer");
}

const generatedConfigPath = path.join(root, "crates", "solum-app", "gen", "android", "app", "src", "main", "assets", "tauri.conf.json");
if (fs.existsSync(generatedConfigPath)) {
  const generated = readJson(generatedConfigPath);
  const generatedCode = generated.bundle && generated.bundle.android && generated.bundle.android.versionCode;
  if (generated.version !== sourceVersion || generatedCode !== sourceVersionCode) {
    fail("generated Android tauri.conf.json is stale relative to the source version/versionCode");
  }
}

const sdkRoot = androidSdkRoot();
const aaptCandidate = args.aapt || newestBuildTool(sdkRoot, "aapt");
const apksignerCandidate = args.apksigner || newestBuildTool(sdkRoot, "apksigner");
if (!aaptCandidate) fail("cannot locate aapt; set ANDROID_HOME or pass --aapt=PATH");
if (!apksignerCandidate) fail("cannot locate apksigner; set ANDROID_HOME or pass --apksigner=PATH");
const aapt = path.resolve(aaptCandidate);
const apksigner = path.resolve(apksignerCandidate);
const badging = run(aapt, ["dump", "badging", apk], "aapt badging");
const packageLine = badging.split(/\r?\n/).find((line) => line.startsWith("package:")) || "";
const packageName = quotedField(packageLine, "name");
const apkVersionCode = Number(quotedField(packageLine, "versionCode"));
const apkVersionName = quotedField(packageLine, "versionName");
if (packageName !== String(config.identifier || "")) fail("package id mismatch: APK=" + packageName + ", config=" + config.identifier);
if (apkVersionName !== sourceVersion) fail("versionName mismatch: APK=" + apkVersionName + ", source=" + sourceVersion);
if (apkVersionCode !== sourceVersionCode) fail("versionCode mismatch: APK=" + apkVersionCode + ", source=" + sourceVersionCode);

const minSdkMatch = badging.match(/^sdkVersion:'([^']+)'/m);
const targetSdkMatch = badging.match(/^targetSdkVersion:'([^']+)'/m);
if (!minSdkMatch || !targetSdkMatch) fail("aapt badging is missing minSdk or targetSdk");
const nativeLine = badging.split(/\r?\n/).find((line) => line.startsWith("native-code:")) || "";
const abis = Array.from(nativeLine.matchAll(/'([^']+)'/g), (match) => match[1]);
if (!abis.length) fail("APK contains no native ABI according to aapt");
for (const abi of abis) {
  if (!["arm64-v8a", "x86_64"].includes(abi)) fail("unexpected release ABI: " + abi);
}

const manifest = run(aapt, ["dump", "xmltree", apk, "AndroidManifest.xml"], "aapt manifest");
for (const component of [
  "dev.solum.alarm.AlarmReceiver",
  "dev.solum.alarm.BootReceiver",
  "dev.solum.app.SolumNotificationListener",
]) {
  if (!manifest.includes(component)) fail("merged manifest is missing component " + component);
}

const signing = runApksigner(apksigner, ["verify", "--verbose", "--print-certs", apk]);
if (!/Verified using v2 scheme[^:]*:\s*true/i.test(signing)) fail("APK Signature Scheme v2 is not verified");
const certificateMatch = signing.match(/Signer #1 certificate SHA-256 digest:\s*([0-9a-f]+)/i);
if (!certificateMatch) fail("apksigner did not report the signer certificate SHA-256");

const bytes = fs.readFileSync(apk);
const entries = zipEntries(bytes);
for (const abi of abis) {
  const library = "lib/" + abi + "/libsolum_app_lib.so";
  if (!entries.has(library)) fail("APK ZIP is missing " + library);
}
for (const required of ["AndroidManifest.xml", "resources.arsc", "assets/tauri.conf.json"]) {
  if (!entries.has(required)) fail("APK ZIP is missing " + required);
}

const sha256 = crypto.createHash("sha256").update(bytes).digest("hex").toUpperCase();
const certificateSha256 = certificateMatch[1].toUpperCase();
console.log("verify-android-release: PASS");
console.log("artifact: " + apk);
console.log("package: " + packageName + " " + apkVersionName + " (" + apkVersionCode + ")");
console.log("sdk: min " + minSdkMatch[1] + " / target " + targetSdkMatch[1]);
console.log("abis: " + abis.join(", "));
console.log("bytes: " + bytes.length);
console.log("sha256: " + sha256);
console.log("signer certificate sha256: " + certificateSha256);
console.log("");
console.log("CHANGELOG evidence:");
console.log(
  "- Android " + path.basename(apk) + " 已通过发布核对：包名 " + packageName +
  "，版本 " + apkVersionName + " (" + apkVersionCode + ")，minSdk " + minSdkMatch[1] +
  " / targetSdk " + targetSdkMatch[1] + "，ABI " + abis.join(" + ") +
  "；AlarmReceiver、BootReceiver、SolumNotificationListener 均存在，APK Signature Scheme v2 有效。" +
  " 文件 " + bytes.length + " 字节，SHA-256 " + sha256 +
  "，签名证书 SHA-256 " + certificateSha256 + "。"
);

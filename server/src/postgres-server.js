'use strict';

const http = require('node:http');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { Pool } = require('pg');
const {
  createRefreshToken,
  createRefreshSuccessorToken,
  createConcurrencyLimiter,
  createFixedWindowLimiter,
  isJsonContentType,
  passwordHashAsync,
  refreshTokenSessionId,
  requestClientAddress,
  requestOriginAllowed,
  secureEqualHex,
  verifyPasswordAsync,
  verifyPasswordOrDummyAsync,
} = require('./http-security');

const ACCESS_TTL_SECONDS = 15 * 60;
const REFRESH_TTL_SECONDS = 30 * 24 * 60 * 60;
const MAX_JSON_BYTES = 1024 * 1024;
const MAX_BLOB_BYTES = 8 * 1024 * 1024;
const MAX_PULL_BYTES = 16 * 1024 * 1024;
const MAX_PULL_ROWS = 500;
const DEFAULT_AI_RESPONSE_BYTES = 8 * 1024 * 1024;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;
const LOGIN_MAX_FAILURES = 5;
const LOGIN_IP_MAX_FAILURES = 50;
const MAX_CONCURRENT_AUTH_KDFS = 8;
const REGISTRATION_MAX_ATTEMPTS = 5;
const REFRESH_IP_MAX_FAILURES = 100;
const MAX_REFRESH_SESSIONS_PER_USER = 32;
const REFRESH_REPLAY_GRACE_MS = 5_000;
const MAX_AI_TOKENS_PER_REQUEST = 10 * 1000 * 1000;
const MAX_ALERT_LATENCY_MS = 24 * 60 * 60 * 1000;
const DEFAULT_BACKUP_MAX_OBJECT_BYTES = 68 * 1024 * 1024;
const DEFAULT_BACKUP_MAX_PACKAGE_BYTES = 700 * 1024 * 1024;
const DEFAULT_BACKUP_TENANT_MAX_BYTES = 2 * 1024 * 1024 * 1024;
const DEFAULT_BACKUP_MAX_OBJECTS = 64;
const DEFAULT_BACKUP_MAX_PENDING = 4;
const DEFAULT_BACKUP_MAX_VERSIONS = 10;
const DEFAULT_BACKUP_PENDING_TTL_HOURS = 24;
const UUID_PATTERN = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const DASHBOARD = fs.readFileSync(path.join(__dirname, 'dashboard.html'));
const SERVICE_VERSION = require('../package.json').version;

class HttpError extends Error {
  constructor(status, code) {
    super(code);
    this.status = status;
    this.code = code;
  }
}

function safeErrorLog(error) {
  const name = error && typeof error.name === 'string' &&
    /^[A-Za-z0-9_-]{1,64}$/.test(error.name) ? error.name : 'Error';
  const code = error && typeof error.code === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(error.code) ?
    error.code : 'unknown';
  return { name, code };
}

function nowSeconds() {
  return Math.floor(Date.now() / 1000);
}

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  });
  res.end(body);
}

function html(res, body) {
  res.writeHead(200, {
    'Content-Type': 'text/html; charset=utf-8',
    'Content-Length': body.length,
    'Cache-Control': 'no-cache',
    'X-Content-Type-Options': 'nosniff',
    'Content-Security-Policy': "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
  });
  res.end(body);
}

async function readBytes(req, limit) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw new HttpError(413, 'request_too_large');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function readExactBytes(req, expected) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > expected) throw new HttpError(409, 'backup_object_mismatch');
    chunks.push(chunk);
  }
  if (size !== expected) throw new HttpError(409, 'backup_object_mismatch');
  return Buffer.concat(chunks, size);
}

async function readJson(req) {
  if (!isJsonContentType(req)) throw new HttpError(415, 'unsupported_media_type');
  const raw = await readBytes(req, MAX_JSON_BYTES);
  try {
    const parsed = JSON.parse(raw.toString('utf8') || '{}');
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new HttpError(400, 'invalid_json');
    }
    return parsed;
  } catch {
    throw new HttpError(400, 'invalid_json');
  }
}

function requiredString(value, code, max = 256) {
  if (typeof value !== 'string' || value.trim().length === 0 || value.length > max) {
    throw new HttpError(400, code);
  }
  return value.trim();
}

// Passwords and bearer-style secrets are opaque. Trimming them made multiple
// login strings authenticate as the same password while the client derived a
// different recovery key from each exact string.
function requiredOpaqueString(value, code, max) {
  if (typeof value !== 'string' || value.length === 0 || value.length > max) {
    throw new HttpError(400, code);
  }
  return value;
}

function optionalString(value, code, max = 2048) {
  if (value === undefined || value === null || value === '') return null;
  if (typeof value !== 'string' || value.length > max) throw new HttpError(400, code);
  return value.trim() || null;
}

function optionalBoundedNumber(value, code, { min, max, integer = false }) {
  if (value === undefined || value === null) return null;
  if (typeof value !== 'number' || !Number.isFinite(value) || value < min || value > max ||
    (integer && !Number.isSafeInteger(value))) throw new HttpError(400, code);
  return value;
}

function optionalHttpUrl(value, code, max = 2048) {
  const text = optionalString(value, code, max);
  if (text === null) return null;
  try {
    const parsed = new URL(text);
    if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error('invalid protocol');
    return parsed.toString();
  } catch {
    throw new HttpError(400, code);
  }
}

function safeDatabaseInteger(value, name) {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`database integer exceeds JSON precision: ${name}`);
  }
  return parsed;
}

function decodeBase64(value, code, maxChars) {
  const text = requiredString(value, code, maxChars);
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(text) || text.length % 4 !== 0) {
    throw new HttpError(400, code);
  }
  const decoded = Buffer.from(text, 'base64');
  if (decoded.toString('base64') !== text) throw new HttpError(400, code);
  return decoded;
}

function parseNonNegativeInt(value, fallback, max = Number.MAX_SAFE_INTEGER) {
  if (value === null || value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(String(value))) throw new HttpError(400, 'invalid_query');
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0 || parsed > max) {
    throw new HttpError(400, 'invalid_query');
  }
  return parsed;
}

function requireExactKeys(value, expected, code) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new HttpError(400, code);
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new HttpError(400, code);
  }
}

function backupUuid(value) {
  if (typeof value !== 'string' || !UUID_PATTERN.test(value) ||
      value === '00000000-0000-0000-0000-000000000000') {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  return value;
}

function backupWriterId(value, code = 'invalid_backup_plan') {
  if (typeof value !== 'string' || !UUID_PATTERN.test(value) ||
      value === '00000000-0000-0000-0000-000000000000') {
    throw new HttpError(code === 'backup_writer_stale' ? 409 : 400, code);
  }
  return value;
}

function backupGeneration(value) {
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  return value;
}

function parseBackupPlan(body, config) {
  requireExactKeys(body,
    ['backup_id', 'generation', 'created_at', 'protocol_version', 'objects'],
    'invalid_backup_plan');
  if (body.protocol_version !== 1) throw new HttpError(400, 'unsupported_backup_version');
  if (typeof body.created_at !== 'string' ||
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(body.created_at) ||
      body.created_at.startsWith('0000-')) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  const parsedDate = new Date(body.created_at);
  if (!Number.isFinite(parsedDate.getTime()) || parsedDate.toISOString() !== body.created_at) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  if (!Array.isArray(body.objects) || body.objects.length < 1 ||
      body.objects.length > config.backupMaxObjects) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  const seen = new Set();
  let mainCount = 0;
  let totalBytes = 0;
  const objects = body.objects.map((item) => {
    requireExactKeys(item, ['object_id', 'kind', 'bytes', 'sha256'], 'invalid_backup_plan');
    const objectId = backupUuid(item.object_id);
    if (seen.has(objectId)) throw new HttpError(400, 'invalid_backup_plan');
    seen.add(objectId);
    if (!['main', 'attachment'].includes(item.kind)) {
      throw new HttpError(400, 'invalid_backup_plan');
    }
    if (item.kind === 'main') mainCount += 1;
    if (!Number.isSafeInteger(item.bytes) || item.bytes < 1) {
      throw new HttpError(400, 'invalid_backup_plan');
    }
    if (item.bytes > config.backupMaxObjectBytes) {
      throw new HttpError(413, 'backup_object_too_large');
    }
    if (typeof item.sha256 !== 'string' || !SHA256_PATTERN.test(item.sha256)) {
      throw new HttpError(400, 'invalid_backup_plan');
    }
    totalBytes += item.bytes;
    if (!Number.isSafeInteger(totalBytes) || totalBytes > config.backupMaxPackageBytes) {
      throw new HttpError(413, 'backup_package_too_large');
    }
    return { object_id: objectId, kind: item.kind, bytes: item.bytes, sha256: item.sha256 };
  }).sort((left, right) => left.object_id < right.object_id ? -1 :
    (left.object_id > right.object_id ? 1 : 0));
  if (mainCount !== 1) throw new HttpError(400, 'invalid_backup_plan');
  const plan = {
    backup_id: backupUuid(body.backup_id),
    generation: backupGeneration(body.generation),
    created_at: body.created_at,
    protocol_version: 1,
    objects,
  };
  const canonical = Buffer.from(JSON.stringify(plan), 'utf8');
  return {
    ...plan,
    totalBytes,
    planSha256: crypto.createHash('sha256').update(canonical).digest(),
  };
}

function parseTakeover(body) {
  requireExactKeys(body, ['expected_generation', 'takeover_id', 'writer_id'],
    'invalid_backup_plan');
  return {
    expectedGeneration: backupGeneration(body.expected_generation),
    takeoverId: backupUuid(body.takeover_id),
    writerId: backupWriterId(body.writer_id),
  };
}

function parseWriterClaim(body) {
  requireExactKeys(body, ['expected_generation', 'writer_id'], 'invalid_backup_plan');
  const expectedGeneration = backupGeneration(body.expected_generation);
  if (expectedGeneration !== 1) throw new HttpError(400, 'invalid_backup_plan');
  return { expectedGeneration, writerId: backupWriterId(body.writer_id) };
}

function quotedDigest(buffer) {
  return `"${buffer.toString('hex')}"`;
}

function parseIfMatch(value) {
  if (typeof value !== 'string') throw new HttpError(428, 'precondition_required');
  const match = /^"([0-9a-f]{64})"$/.exec(value);
  if (!match) throw new HttpError(412, 'precondition_failed');
  return Buffer.from(match[1], 'hex');
}

function isoTimestamp(value) {
  const parsed = value instanceof Date ? value : new Date(value);
  if (!Number.isFinite(parsed.getTime())) throw new Error('invalid database timestamp');
  return parsed.toISOString();
}

function binary(res, body, digest) {
  res.writeHead(200, {
    'Content-Type': 'application/octet-stream',
    'Content-Length': body.length,
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ETag: quotedDigest(digest),
  });
  res.end(body);
}

function secureEqualBuffer(left, right) {
  return Buffer.isBuffer(left) && Buffer.isBuffer(right) && left.length === right.length &&
    crypto.timingSafeEqual(left, right);
}

function tokenHash(token) {
  return crypto.createHash('sha256').update(token).digest();
}

function signAccessToken(user, secret) {
  const payload = Buffer.from(JSON.stringify({
    sub: user.id,
    username: user.username,
    exp: nowSeconds() + ACCESS_TTL_SECONDS,
    nonce: crypto.randomBytes(12).toString('hex'),
  })).toString('base64url');
  const signature = crypto.createHmac('sha256', secret).update(payload).digest('base64url');
  return `${payload}.${signature}`;
}

function verifyAccessToken(token, secret) {
  if (typeof token !== 'string') return null;
  const parts = token.split('.');
  if (parts.length !== 2) return null;
  const expected = crypto.createHmac('sha256', secret).update(parts[0]).digest();
  let actual;
  try {
    actual = Buffer.from(parts[1], 'base64url');
  } catch {
    return null;
  }
  if (actual.length !== expected.length || !crypto.timingSafeEqual(actual, expected)) return null;
  try {
    const payload = JSON.parse(Buffer.from(parts[0], 'base64url').toString('utf8'));
    if (typeof payload.sub !== 'string' || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(payload.sub) ||
      !Number.isInteger(payload.exp) || payload.exp <= nowSeconds()) return null;
    return payload.sub;
  } catch {
    return null;
  }
}

function bearer(req) {
  const value = req.headers.authorization;
  return typeof value === 'string' && value.startsWith('Bearer ') ? value.slice(7) : null;
}

function validateModel(value, fallback) {
  const model = value === undefined ? fallback : requiredString(value, 'invalid_model', 100);
  if (!/^[A-Za-z0-9._:/-]+$/.test(model)) throw new HttpError(400, 'invalid_model');
  return model;
}

function parseModelCatalog(defaultModel, allowedModels) {
  const fallback = validateModel(defaultModel, 'qwen3.5-2b');
  const models = [fallback];
  for (const candidate of String(allowedModels || '').split(',')) {
    const trimmed = candidate.trim();
    if (!trimmed) continue;
    const model = validateModel(trimmed, fallback);
    if (!models.includes(model)) models.push(model);
  }
  return Object.freeze({ defaultModel: fallback, models: Object.freeze(models) });
}

function validateMessages(value) {
  if (!Array.isArray(value) || value.length === 0 || value.length > 16) {
    throw new HttpError(400, 'invalid_messages');
  }
  return value.map((item) => {
    if (!item || typeof item !== 'object' || Array.isArray(item) ||
      !['system', 'user', 'assistant'].includes(item.role) ||
      typeof item.content !== 'string' || item.content.length === 0 || item.content.length > 32000) {
      throw new HttpError(400, 'invalid_messages');
    }
    return { role: item.role, content: item.content };
  });
}

function validateCompletionOptions(body) {
  if (!body || typeof body !== 'object' || Array.isArray(body)) {
    throw new HttpError(400, 'invalid_request');
  }
  if (body.stream !== undefined && typeof body.stream !== 'boolean') {
    throw new HttpError(400, 'invalid_stream');
  }
  if (body.temperature !== undefined &&
    (typeof body.temperature !== 'number' || !Number.isFinite(body.temperature) ||
      body.temperature < 0 || body.temperature > 2)) {
    throw new HttpError(400, 'invalid_temperature');
  }
  if (body.max_tokens !== undefined &&
    (!Number.isInteger(body.max_tokens) || body.max_tokens < 1 || body.max_tokens > 32768)) {
    throw new HttpError(400, 'invalid_max_tokens');
  }
  const tools = validateTools(body.tools);
  const toolChoice = validateToolChoice(body.tool_choice, tools);
  return {
    stream: body.stream === true,
    temperature: body.temperature,
    maxTokens: body.max_tokens,
    tools,
    toolChoice,
  };
}

function validateTools(value) {
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.length === 0 || value.length > 16 ||
      JSON.stringify(value).length > 65536) throw new HttpError(400, 'invalid_tools');
  return value.map((tool) => {
    if (!tool || typeof tool !== 'object' || Array.isArray(tool) || tool.type !== 'function' ||
        Object.keys(tool).some((key) => !['type', 'function'].includes(key))) {
      throw new HttpError(400, 'invalid_tools');
    }
    const fn = tool.function;
    if (!fn || typeof fn !== 'object' || Array.isArray(fn) ||
        Object.keys(fn).some((key) => !['name', 'description', 'parameters', 'strict'].includes(key)) ||
        typeof fn.name !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(fn.name) ||
        (fn.description !== undefined && (typeof fn.description !== 'string' || fn.description.length > 1000)) ||
        !fn.parameters || typeof fn.parameters !== 'object' || Array.isArray(fn.parameters) ||
        (fn.strict !== undefined && typeof fn.strict !== 'boolean')) {
      throw new HttpError(400, 'invalid_tools');
    }
    return { type: 'function', function: { ...fn } };
  });
}

function validateToolChoice(value, tools) {
  if (value === undefined) return undefined;
  if (!tools) throw new HttpError(400, 'invalid_tool_choice');
  if (['auto', 'none', 'required'].includes(value)) return value;
  if (!value || typeof value !== 'object' || Array.isArray(value) || value.type !== 'function' ||
      Object.keys(value).some((key) => !['type', 'function'].includes(key)) ||
      !value.function || typeof value.function !== 'object' || Array.isArray(value.function) ||
      Object.keys(value.function).some((key) => key !== 'name') ||
      typeof value.function.name !== 'string' ||
      !tools.some((tool) => tool.function.name === value.function.name)) {
    throw new HttpError(400, 'invalid_tool_choice');
  }
  return { type: 'function', function: { name: value.function.name } };
}

function parseAllowedOrigins(value) {
  return new Set(String(value || '').split(',').map((item) => item.trim()).filter(Boolean).map((item) => {
    try {
      return new URL(item).origin;
    } catch {
      throw new Error(`invalid SOLUM_ALLOWED_ORIGINS entry: ${item}`);
    }
  }));
}

function buildConfig(overrides = {}) {
  const config = {
    port: Number(process.env.SOLUM_PORT || 8787),
    authSecret: process.env.SOLUM_AUTH_SECRET || '',
    adminUsername: process.env.SOLUM_ADMIN_USERNAME || '',
    adminPassword: process.env.SOLUM_ADMIN_PASSWORD || '',
    registrationMode: process.env.SOLUM_REGISTRATION_MODE || 'closed',
    mimoApiKey: process.env.SOLUM_LLM_API_KEY || process.env.MIMO_API_KEY || '',
    mimoBaseUrl: process.env.SOLUM_LLM_BASE_URL || process.env.MIMO_BASE_URL || 'http://100.79.74.121:8000/v1',
    defaultModel: process.env.SOLUM_DEFAULT_MODEL || 'qwen3.5-2b',
    allowedModels: process.env.SOLUM_ALLOWED_MODELS || 'qwen3.5-2b',
    standardPlan: process.env.SOLUM_STANDARD_PLAN || 'standard',
    standardMonthlyRequestLimit: Number(process.env.SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT || 1000),
    aiMaxResponseBytes: Number(process.env.SOLUM_AI_MAX_RESPONSE_BYTES || DEFAULT_AI_RESPONSE_BYTES),
    aiPendingTimeoutSeconds: Number(process.env.SOLUM_AI_PENDING_TIMEOUT_SECONDS || 600),
    allowedOrigins: process.env.SOLUM_ALLOWED_ORIGINS || '',
    trustProxy: process.env.SOLUM_TRUST_PROXY === 'true',
    poolMax: Number(process.env.SOLUM_DB_POOL_MAX || 10),
    alertRetentionDays: Number(process.env.SOLUM_ALERT_RETENTION_DAYS || 7),
    usageEventRetentionDays: Number(process.env.SOLUM_USAGE_EVENT_RETENTION_DAYS || 90),
    tenantMaxBlobBytes: Number(process.env.SOLUM_TENANT_MAX_BLOB_BYTES || 1073741824),
    tenantMaxBlobs: Number(process.env.SOLUM_TENANT_MAX_BLOBS || 100000),
    tenantMaxDevices: Number(process.env.SOLUM_TENANT_MAX_DEVICES || 32),
    backupMaxObjectBytes: Number(process.env.SOLUM_BACKUP_MAX_OBJECT_BYTES ||
      DEFAULT_BACKUP_MAX_OBJECT_BYTES),
    backupMaxPackageBytes: Number(process.env.SOLUM_BACKUP_MAX_PACKAGE_BYTES ||
      DEFAULT_BACKUP_MAX_PACKAGE_BYTES),
    backupTenantMaxBytes: Number(process.env.SOLUM_BACKUP_TENANT_MAX_BYTES ||
      DEFAULT_BACKUP_TENANT_MAX_BYTES),
    backupMaxObjects: Number(process.env.SOLUM_BACKUP_MAX_OBJECTS || DEFAULT_BACKUP_MAX_OBJECTS),
    backupMaxPending: Number(process.env.SOLUM_BACKUP_MAX_PENDING || DEFAULT_BACKUP_MAX_PENDING),
    backupMaxVersions: Number(process.env.SOLUM_BACKUP_MAX_VERSIONS || DEFAULT_BACKUP_MAX_VERSIONS),
    backupPendingTtlHours: Number(process.env.SOLUM_BACKUP_PENDING_TTL_HOURS ||
      DEFAULT_BACKUP_PENDING_TTL_HOURS),
    buildRevision: process.env.SOLUM_BUILD_REVISION || 'unknown',
    ...overrides,
  };
  if (config.authSecret.length < 32) throw new Error('SOLUM_AUTH_SECRET must be at least 32 characters');
  if (typeof config.adminUsername !== 'string') throw new Error('SOLUM_ADMIN_USERNAME is required');
  config.adminUsername = config.adminUsername.trim();
  if (!config.adminUsername || config.adminUsername.length > 128 ||
      /[\u0000-\u001f\u007f]/.test(config.adminUsername)) {
    throw new Error('SOLUM_ADMIN_USERNAME must be a valid username');
  }
  if (!['closed', 'open'].includes(config.registrationMode)) {
    throw new Error('SOLUM_REGISTRATION_MODE must be closed or open');
  }
  if (!Number.isInteger(config.poolMax) || config.poolMax < 1 || config.poolMax > 100) {
    throw new Error('SOLUM_DB_POOL_MAX must be between 1 and 100');
  }
  if (!Number.isSafeInteger(config.alertRetentionDays) || config.alertRetentionDays < 1 ||
    config.alertRetentionDays > 36500) {
    throw new Error('SOLUM_ALERT_RETENTION_DAYS must be between 1 and 36500');
  }
  if (!Number.isSafeInteger(config.usageEventRetentionDays) ||
    config.usageEventRetentionDays < 30 || config.usageEventRetentionDays > 3650) {
    throw new Error('SOLUM_USAGE_EVENT_RETENTION_DAYS must be between 30 and 3650');
  }
  for (const [name, value, maximum] of [
    ['SOLUM_TENANT_MAX_BLOB_BYTES', config.tenantMaxBlobBytes, Number.MAX_SAFE_INTEGER],
    ['SOLUM_TENANT_MAX_BLOBS', config.tenantMaxBlobs, 10000000],
    ['SOLUM_TENANT_MAX_DEVICES', config.tenantMaxDevices, 10000],
  ]) {
    if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
      throw new Error(`${name} must be a positive safe integer no greater than ${maximum}`);
    }
  }
  for (const [name, value, minimum, maximum] of [
    ['SOLUM_BACKUP_MAX_OBJECT_BYTES', config.backupMaxObjectBytes,
      DEFAULT_BACKUP_MAX_OBJECT_BYTES, 1024 * 1024 * 1024],
    ['SOLUM_BACKUP_MAX_PACKAGE_BYTES', config.backupMaxPackageBytes,
      DEFAULT_BACKUP_MAX_PACKAGE_BYTES, 2 * 1024 * 1024 * 1024],
    ['SOLUM_BACKUP_TENANT_MAX_BYTES', config.backupTenantMaxBytes,
      DEFAULT_BACKUP_TENANT_MAX_BYTES, Number.MAX_SAFE_INTEGER],
    ['SOLUM_BACKUP_MAX_OBJECTS', config.backupMaxObjects,
      DEFAULT_BACKUP_MAX_OBJECTS, DEFAULT_BACKUP_MAX_OBJECTS],
    ['SOLUM_BACKUP_MAX_PENDING', config.backupMaxPending, 1, 100],
    ['SOLUM_BACKUP_MAX_VERSIONS', config.backupMaxVersions, 2, 100],
    ['SOLUM_BACKUP_PENDING_TTL_HOURS', config.backupPendingTtlHours, 1, 24 * 30],
  ]) {
    if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
      throw new Error(`${name} must be a safe integer between ${minimum} and ${maximum}`);
    }
  }
  if (config.backupMaxPackageBytes < config.backupMaxObjectBytes) {
    throw new Error('SOLUM_BACKUP_MAX_PACKAGE_BYTES must cover one maximum-size object');
  }
  if (config.backupTenantMaxBytes < config.backupMaxPackageBytes * 2) {
    throw new Error('SOLUM_BACKUP_TENANT_MAX_BYTES must cover at least two maximum-size backups');
  }
  if (config.buildRevision !== 'unknown' && !/^[0-9a-f]{7,40}$/.test(config.buildRevision)) {
    throw new Error('SOLUM_BUILD_REVISION must be unknown or a 7-40 character lowercase Git SHA');
  }
  validateModel(config.defaultModel, 'qwen3.5-2b');
  config.modelCatalog = parseModelCatalog(config.defaultModel, config.allowedModels);
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(config.standardPlan)) {
    throw new Error('SOLUM_STANDARD_PLAN must be a short identifier');
  }
  if (!Number.isInteger(config.standardMonthlyRequestLimit) ||
    config.standardMonthlyRequestLimit < 1 || config.standardMonthlyRequestLimit > 10000000) {
    throw new Error('SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT must be between 1 and 10000000');
  }
  if (!Number.isInteger(config.aiMaxResponseBytes) ||
    config.aiMaxResponseBytes < 1024 || config.aiMaxResponseBytes > 64 * 1024 * 1024) {
    throw new Error('SOLUM_AI_MAX_RESPONSE_BYTES must be between 1024 and 67108864');
  }
  if (!Number.isInteger(config.aiPendingTimeoutSeconds) ||
    config.aiPendingTimeoutSeconds < 60 || config.aiPendingTimeoutSeconds > 86400) {
    throw new Error('SOLUM_AI_PENDING_TIMEOUT_SECONDS must be between 60 and 86400');
  }
  config.allowedOrigins = parseAllowedOrigins(config.allowedOrigins);
  return config;
}

async function withTenant(pool, tenantId, operation) {
  const client = await pool.connect();
  try {
    await client.query('begin');
    await client.query("select set_config('app.current_tenant_id', $1, true)", [tenantId]);
    const result = await operation(client);
    await client.query('commit');
    return result;
  } catch (error) {
    await client.query('rollback').catch(() => {});
    throw error;
  } finally {
    client.release();
  }
}

async function lockAuthSessionUser(client, userId) {
  await client.query(
    "select pg_advisory_xact_lock(hashtextextended('auth-session:'||$1::text,0))", [userId]
  );
}

async function selectRefreshFamily(client, sessionId, hash, forUpdate = false) {
  const lock = forUpdate ? ' for update of rt' : '';
  const columns = `rt.token_hash,rt.previous_token_hash,rt.legacy_token_hash,rt.rotated_at,
    rt.session_id,rt.user_id,rt.expires_at,u.username,u.status`;
  if (sessionId) {
    return client.query(
      `select ${columns} from auth.refresh_tokens rt join auth.users u on u.id=rt.user_id
       where rt.session_id=$1${lock}`, [sessionId]
    );
  }
  return client.query(
    `select ${columns} from auth.refresh_tokens rt join auth.users u on u.id=rt.user_id
     where rt.token_hash=$1 or rt.previous_token_hash=$1 or rt.legacy_token_hash=$1${lock}`,
    [hash]
  );
}

async function issueSession(client, user, secret) {
  const sessionId = crypto.randomUUID();
  const refresh = createRefreshToken(sessionId, secret);
  await lockAuthSessionUser(client, user.id);
  await client.query('delete from auth.refresh_tokens where user_id=$1 and expires_at<=now()', [user.id]);
  await client.query(
    `delete from auth.refresh_tokens where user_id=$1 and session_id in (
       select session_id from auth.refresh_tokens where user_id=$1
       order by created_at desc,session_id desc offset $2
     )`, [user.id, MAX_REFRESH_SESSIONS_PER_USER - 1]
  );
  await client.query(
    `insert into auth.refresh_tokens(
       token_hash,user_id,session_id,previous_token_hash,legacy_token_hash,
       rotated_at,expires_at
     ) values ($1,$2,$3,null,null,null,now()+interval '30 days')`,
    [tokenHash(refresh), user.id, sessionId]
  );
  return {
    access_token: signAccessToken(user, secret),
    refresh_token: refresh,
    user: { id: user.id, username: user.username },
  };
}

// Session issuance for recovery login: same guarantees as the password path,
// wrapped in its own short advisory-locked transaction.
async function issueSessionForUser(pool, user, secret) {
  const client = await pool.connect();
  let transactionOpen = false;
  try {
    await client.query('begin');
    transactionOpen = true;
    const active = await client.query(
      'select id, username from auth.users where id=$1 and status=\'active\'', [user.id]
    );
    if (active.rowCount !== 1) throw new HttpError(401, 'invalid_credentials');
    const session = await issueSession(client, active.rows[0], secret);
    await client.query('commit');
    transactionOpen = false;
    return session;
  } catch (error) {
    if (transactionOpen) await client.query('rollback').catch(() => {});
    throw error;
  } finally {
    client.release();
  }
}

// Timing uniformity for credential verification against accounts that may not
// exist or may have no recovery code: always pay one scrypt.
async function scryptBurnAsync(input) {
  const salt = crypto.randomBytes(16).toString('hex');
  await new Promise((resolve, reject) => {
    crypto.scrypt(input, salt, 64, (error) => {
      if (error) reject(error); else resolve();
    });
  });
}

async function bootstrapAdmin(pool, config) {
  const existing = await pool.query('select id,username from auth.users where username=$1', [config.adminUsername]);
  if (existing.rowCount > 0) return;
  if ([...config.adminPassword].length < 8) {
    throw new Error('SOLUM_ADMIN_PASSWORD must be at least 8 characters when bootstrapping');
  }
  const salt = crypto.randomBytes(16).toString('hex');
  await pool.query(
    `insert into auth.users(username,password_hash,password_salt,password_version)
     values ($1,$2,$3,2) on conflict(username) do nothing`,
    [config.adminUsername, await passwordHashAsync(config.adminPassword, salt), salt]
  );
}

async function cleanExpiredData(pool, config) {
  const users = await pool.query('select id from auth.users');
  for (const user of users.rows) {
    await withTenant(pool, user.id, async (client) => {
      await client.query(
        "delete from sync.alerts where tenant_id=$1 and received_at<now()-($2::text||' days')::interval",
        [user.id, config.alertRetentionDays]
      );
      await client.query(
        "delete from vault.backups where tenant_id=$1 and status='pending' " +
          "and created_at<now()-($2::text||' hours')::interval",
        [user.id, config.backupPendingTtlHours]
      );
      // Usage events are the only table without any cleanup; monthly
      // aggregates keep serving quota checks after the rows are gone.
      await client.query(
        "delete from ai.usage_events where tenant_id=$1 " +
          "and created_at<now()-($2::text||' days')::interval",
        [user.id, config.usageEventRetentionDays]
      );
    });
  }
  await pool.query('delete from auth.refresh_tokens where expires_at<=now()');
}

async function lockTenantMutation(client, tenantId) {
  await client.query('select pg_advisory_xact_lock(hashtextextended($1::text,0))', [tenantId]);
}

async function backupState(client, tenantId) {
  await client.query(
    `insert into vault.backup_state(tenant_id,active_generation)
     values($1,1) on conflict(tenant_id) do nothing`, [tenantId]
  );
  const selected = await client.query(
    `select active_generation,last_takeover_id,writer_id,updated_at
     from vault.backup_state where tenant_id=$1`, [tenantId]
  );
  if (selected.rowCount !== 1) throw new Error('backup state initialization failed');
  return {
    activeGeneration: safeDatabaseInteger(
      selected.rows[0].active_generation, 'backup_active_generation'
    ),
    lastTakeoverId: selected.rows[0].last_takeover_id,
    writerId: selected.rows[0].writer_id,
    updatedAt: selected.rows[0].updated_at,
  };
}

function requireBackupWriter(req, state) {
  const writerId = backupWriterId(req.headers['x-solum-backup-writer'], 'backup_writer_stale');
  if (state.writerId !== writerId) throw new HttpError(409, 'backup_writer_stale');
  return writerId;
}

function backupObjectJson(row) {
  return {
    object_id: row.object_id,
    kind: row.kind,
    bytes: safeDatabaseInteger(row.declared_bytes, 'backup_object_bytes'),
    sha256: row.declared_sha256.toString('hex'),
  };
}

function backupJson(row, objects) {
  return {
    backup_id: row.backup_id,
    generation: safeDatabaseInteger(row.generation, 'backup_generation'),
    created_at: isoTimestamp(row.client_created_at),
    committed_at: row.committed_at === null ? null : isoTimestamp(row.committed_at),
    protocol_version: Number(row.protocol_version),
    total_bytes: safeDatabaseInteger(row.total_bytes, 'backup_total_bytes'),
    object_count: safeDatabaseInteger(row.object_count, 'backup_object_count'),
    plan_etag: row.plan_sha256.toString('hex'),
    objects,
  };
}

async function tenantBackupBytes(client, tenantId) {
  const usage = await client.query(
    `select coalesce(sum(octet_length(ciphertext)),0) as total_bytes
     from vault.backup_objects where tenant_id=$1 and ciphertext is not null`, [tenantId]
  );
  return safeDatabaseInteger(usage.rows[0].total_bytes, 'tenant_backup_bytes');
}

function isOctetStream(req) {
  const value = req.headers['content-type'];
  return typeof value === 'string' && value.split(';', 1)[0].trim().toLowerCase() ===
    'application/octet-stream';
}

async function touchActiveDevice(client, tenantId, device, maxDevices) {
  const selected = await client.query(
    'select revoked_at from sync.devices where tenant_id=$1 and device_id=$2', [tenantId, device]
  );
  if (selected.rowCount > 0) {
    if (selected.rows[0].revoked_at !== null) throw new HttpError(403, 'device_revoked');
    const updated = await client.query(
      `update sync.devices set last_seen_at=now()
       where tenant_id=$1 and device_id=$2 and revoked_at is null returning device_id`,
      [tenantId, device]
    );
    if (updated.rowCount !== 1) throw new HttpError(403, 'device_revoked');
    return;
  }
  const count = await client.query(
    `select count(*) as device_count from sync.devices
     where tenant_id=$1 and revoked_at is null`, [tenantId]
  );
  if (safeDatabaseInteger(count.rows[0].device_count, 'device_count') >= maxDevices) {
    throw new HttpError(429, 'device_limit_exceeded');
  }
  await client.query(
    'insert into sync.devices(tenant_id,device_id,last_seen_at) values($1,$2,now())',
    [tenantId, device]
  );
}

async function ensureBlobQuota(client, tenantId, blobLength, config) {
  const usage = await client.query(
    `select count(*) as blob_count,coalesce(sum(octet_length(ciphertext)),0) as blob_bytes
     from sync.blobs where tenant_id=$1`, [tenantId]
  );
  const blobCount = safeDatabaseInteger(usage.rows[0].blob_count, 'blob_count');
  const blobBytes = safeDatabaseInteger(usage.rows[0].blob_bytes, 'blob_bytes');
  if (blobCount >= config.tenantMaxBlobs || blobBytes > config.tenantMaxBlobBytes - blobLength) {
    throw new HttpError(507, 'storage_quota_exceeded');
  }
}

async function recoverTenantStaleAiRequests(client, tenantId, timeoutSeconds) {
  await client.query(
    `with recovered as (
       update ai.usage_events set status='interrupted',completed_at=now()
       where tenant_id=$1 and status='pending'
         and created_at<now()-($2::text||' seconds')::interval
       returning period_start,fallback_occurred
     ), totals as (
       select period_start,count(*)::integer as failures,
              count(*) filter(where fallback_occurred)::integer as fallbacks
       from recovered group by period_start
     )
     update ai.usage_monthly usage set
       failure_count=usage.failure_count+totals.failures,
       fallback_count=usage.fallback_count+totals.fallbacks,updated_at=now()
     from totals where usage.tenant_id=$1 and usage.period_start=totals.period_start`,
    [tenantId, timeoutSeconds]
  );
}

async function ensureEntitlement(client, tenantId, config) {
  await client.query(
    `insert into ai.entitlements(tenant_id,plan,monthly_request_limit)
     values($1,$2,$3) on conflict(tenant_id) do nothing`,
    [tenantId, config.standardPlan, config.standardMonthlyRequestLimit]
  );
  const selected = await client.query(
    'select plan,monthly_request_limit,custom_models_enabled from ai.entitlements where tenant_id=$1',
    [tenantId]
  );
  return selected.rows[0];
}

function usageProjection(entitlement, row = {}) {
  const safeCount = (value, name) => {
    const count = Number(value || 0);
    if (!Number.isSafeInteger(count) || count < 0) {
      throw new Error(`invalid AI usage counter: ${name}`);
    }
    return count;
  };
  const limit = safeCount(entitlement.monthly_request_limit, 'monthly_request_limit');
  const used = safeCount(row.request_count, 'request_count');
  const periodText = typeof row.period_start === 'string' ? row.period_start : '';
  const periodMatch = /^(\d{4})-(\d{2})-\d{2}$/.exec(periodText);
  const fallbackDate = row.period_start instanceof Date ? row.period_start : new Date();
  const periodYear = periodMatch ? Number(periodMatch[1]) : fallbackDate.getFullYear();
  const periodMonth = periodMatch ? Number(periodMatch[2]) - 1 : fallbackDate.getMonth();
  const resetAt = new Date(Date.UTC(periodYear, periodMonth + 1, 1));
  return {
    plan: entitlement.plan,
    custom_models_enabled: entitlement.custom_models_enabled !== false,
    monthly_request_limit: limit,
    requests_used: used,
    requests_remaining: Math.max(0, limit - used),
    reset_at: resetAt.toISOString(),
    success_count: safeCount(row.success_count, 'success_count'),
    failure_count: safeCount(row.failure_count, 'failure_count'),
    fallback_count: safeCount(row.fallback_count, 'fallback_count'),
    prompt_tokens: safeCount(row.prompt_tokens, 'prompt_tokens'),
    completion_tokens: safeCount(row.completion_tokens, 'completion_tokens'),
    total_tokens: safeCount(row.total_tokens, 'total_tokens'),
  };
}

async function readAiUsage(pool, tenantId, config) {
  return withTenant(pool, tenantId, async (client) => {
    await recoverTenantStaleAiRequests(client, tenantId, config.aiPendingTimeoutSeconds);
    const entitlement = await ensureEntitlement(client, tenantId, config);
    const selected = await client.query(
      `select to_char(period_start,'YYYY-MM-DD') as period_start,
              request_count,success_count,failure_count,fallback_count,
              prompt_tokens,completion_tokens,total_tokens
       from ai.usage_monthly
       where tenant_id=$1 and period_start=date_trunc('month',now())::date`, [tenantId]
    );
    const recent = await client.query(
      `select request_id,requested_model,effective_model,status,fallback_occurred,fallback_reason,
              upstream_latency_ms,created_at,completed_at
       from ai.usage_events where tenant_id=$1 order by created_at desc limit 1`, [tenantId]
    );
    const latest = recent.rows[0] ? { ...recent.rows[0],
      upstream_latency_ms: recent.rows[0].upstream_latency_ms === null ? null :
        safeDatabaseInteger(recent.rows[0].upstream_latency_ms, 'upstream_latency_ms') } : null;
    return { ...usageProjection(entitlement, selected.rows[0]), latest_request: latest };
  });
}

async function reserveAiRequest(pool, tenantId, requestedModel, config) {
  return withTenant(pool, tenantId, async (client) => {
    await recoverTenantStaleAiRequests(client, tenantId, config.aiPendingTimeoutSeconds);
    const entitlement = await ensureEntitlement(client, tenantId, config);
    const requestedAllowed = config.modelCatalog.models.includes(requestedModel);
    const canUseRequested = requestedAllowed &&
      (requestedModel === config.modelCatalog.defaultModel || entitlement.custom_models_enabled);
    const effectiveModel = canUseRequested ? requestedModel : config.modelCatalog.defaultModel;
    const fallbackReason = canUseRequested ? null :
      (requestedAllowed ? 'custom_models_disabled' : 'model_not_allowed');
    await client.query(
      `insert into ai.usage_monthly(tenant_id,period_start)
       values($1,date_trunc('month',now())::date) on conflict(tenant_id,period_start) do nothing`,
      [tenantId]
    );
    const reserved = await client.query(
      `update ai.usage_monthly usage set request_count=request_count+1,updated_at=now()
       from ai.entitlements entitlement
       where usage.tenant_id=$1 and usage.period_start=date_trunc('month',now())::date
         and entitlement.tenant_id=usage.tenant_id
         and usage.request_count<entitlement.monthly_request_limit
       returning to_char(usage.period_start,'YYYY-MM-DD') as period_start,
                 usage.request_count,usage.success_count,usage.failure_count,
                 usage.fallback_count,usage.prompt_tokens,usage.completion_tokens,usage.total_tokens`,
      [tenantId]
    );
    if (reserved.rowCount !== 1) throw new HttpError(429, 'ai_quota_exhausted');
    const requestId = crypto.randomUUID();
    await client.query(
      `insert into ai.usage_events(request_id,tenant_id,period_start,requested_model,effective_model,
                                   fallback_occurred,fallback_reason)
       values($1,$2,date_trunc('month',now())::date,$3,$4,$5,$6)`,
      [requestId, tenantId, requestedModel, effectiveModel, fallbackReason !== null, fallbackReason]
    );
    return {
      requestId, requestedModel, effectiveModel, fallbackReason,
      quota: usageProjection(entitlement, reserved.rows[0]),
    };
  });
}

async function finalizeAiRequest(pool, tenantId, reservation, outcome) {
  return withTenant(pool, tenantId, async (client) => {
    const rawTokens = outcome.usage || {};
    const tokenCount = (value) => Number.isSafeInteger(value) && value >= 0 &&
      value <= MAX_AI_TOKENS_PER_REQUEST ?
      value : 0;
    const tokens = {
      prompt_tokens: tokenCount(rawTokens.prompt_tokens),
      completion_tokens: tokenCount(rawTokens.completion_tokens),
      total_tokens: tokenCount(rawTokens.total_tokens),
    };
    const updated = await client.query(
      `update ai.usage_events set status=$3,effective_model=$4,fallback_occurred=$5,
              fallback_reason=$6,upstream_latency_ms=$7,prompt_tokens=$8,completion_tokens=$9,
              total_tokens=$10,completed_at=now()
       where tenant_id=$1 and request_id=$2 and status='pending' returning period_start`,
      [tenantId, reservation.requestId, outcome.status, outcome.effectiveModel,
        outcome.fallbackReason !== null, outcome.fallbackReason, outcome.latencyMs,
        tokens.prompt_tokens, tokens.completion_tokens, tokens.total_tokens]
    );
    if (updated.rowCount !== 1) return;
    await client.query(
      `update ai.usage_monthly set
         success_count=success_count+$3,
         failure_count=failure_count+$4,
         fallback_count=fallback_count+$5,
         prompt_tokens=prompt_tokens+$6,
         completion_tokens=completion_tokens+$7,
         total_tokens=total_tokens+$8,updated_at=now()
       where tenant_id=$1 and period_start=$2`,
      [tenantId, updated.rows[0].period_start, outcome.status === 'succeeded' ? 1 : 0,
        outcome.status === 'succeeded' ? 0 : 1, outcome.fallbackReason === null ? 0 : 1,
        tokens.prompt_tokens || 0, tokens.completion_tokens || 0, tokens.total_tokens || 0]
    );
  });
}

function aiResponseHeaders(reservation, effectiveModel, fallbackReason) {
  return {
    'X-Solum-Request-Id': reservation.requestId,
    'X-Solum-Requested-Model': reservation.requestedModel,
    'X-Solum-Effective-Model': effectiveModel,
    'X-Solum-Fallback': fallbackReason === null ? 'false' : 'true',
    'X-Solum-Fallback-Reason': fallbackReason || 'none',
    'X-Solum-Quota-Limit': String(reservation.quota.monthly_request_limit),
    'X-Solum-Quota-Used': String(reservation.quota.requests_used),
    'X-Solum-Quota-Remaining': String(reservation.quota.requests_remaining),
    'X-Solum-Quota-Reset': reservation.quota.reset_at,
  };
}

function requestSignal(req, res, timeoutMs) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(new Error('upstream_timeout')), timeoutMs);
  timeout.unref();
  const abort = () => controller.abort(new Error('client_disconnected'));
  req.once('aborted', abort);
  res.once('close', abort);
  if (req.aborted || res.destroyed) abort();
  return {
    signal: controller.signal,
    release() {
      clearTimeout(timeout);
      req.off('aborted', abort);
      res.off('close', abort);
    },
  };
}

function isEventStreamResponse(response) {
  const contentType = response && response.headers.get('content-type');
  return typeof contentType === 'string' &&
    contentType.split(';', 1)[0].trim().toLowerCase() === 'text/event-stream';
}

async function requestUpstream(config, body, model, wantStream, req, res) {
  const startedAt = Date.now();
  const lifetime = requestSignal(req, res, wantStream ? 300000 : 60000);
  try {
    const headers = { 'Content-Type': 'application/json' };
    if (config.mimoApiKey) headers.Authorization = `Bearer ${config.mimoApiKey}`;
    const response = await fetch(`${config.mimoBaseUrl.replace(/\/+$/, '')}/chat/completions`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ ...body, model }),
      signal: lifetime.signal,
    });
    return {
      response,
      latencyMs: Date.now() - startedAt,
      release: lifetime.release,
      clientDisconnected: () => lifetime.signal.reason?.message === 'client_disconnected',
    };
  } catch {
    const disconnected = lifetime.signal.reason?.message === 'client_disconnected';
    lifetime.release();
    return {
      response: null,
      latencyMs: Date.now() - startedAt,
      release() {},
      clientDisconnected: () => disconnected,
    };
  }
}

async function readBodyLimited(response, maxBytes) {
  const declared = response.headers.get('content-length');
  if (declared !== null && /^\d+$/.test(declared) && Number(declared) > maxBytes) {
    await response.body.cancel().catch(() => {});
    throw new HttpError(502, 'upstream_response_too_large');
  }
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > maxBytes) {
      await response.body.cancel().catch(() => {});
      throw new HttpError(502, 'upstream_response_too_large');
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString('utf8');
}

async function readJsonObjectResponse(response, maxBytes) {
  const text = await readBodyLimited(response, maxBytes);
  try {
    const parsed = JSON.parse(text);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('invalid');
    return { text, parsed };
  } catch {
    throw new HttpError(502, 'upstream_invalid_response');
  }
}

function waitForDrain(res) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      res.off('drain', drained);
      res.off('close', closed);
      res.off('error', failed);
    };
    const drained = () => { cleanup(); resolve(); };
    const closed = () => { cleanup(); reject(new Error('client_disconnected')); };
    const failed = (error) => { cleanup(); reject(error); };
    res.once('drain', drained);
    res.once('close', closed);
    res.once('error', failed);
  });
}

async function nextStreamChunk(reader) {
  try {
    return await reader.read();
  } catch {
    return { done: true, failed: true };
  }
}

async function proxyCompletion(req, res, pool, tenantId, config) {
  const body = await readJson(req);
  const options = validateCompletionOptions(body);
  const wantStream = options.stream;
  const requestedModel = validateModel(body.model, config.defaultModel);
  const upstreamBody = {
    messages: validateMessages(body.messages),
    stream: wantStream,
  };
  if (options.temperature !== undefined) upstreamBody.temperature = options.temperature;
  if (options.maxTokens !== undefined) upstreamBody.max_tokens = options.maxTokens;
  if (options.tools !== undefined) upstreamBody.tools = options.tools;
  if (options.toolChoice !== undefined) upstreamBody.tool_choice = options.toolChoice;
  const reservation = await reserveAiRequest(pool, tenantId, requestedModel, config);
  let effectiveModel = reservation.effectiveModel;
  let fallbackReason = reservation.fallbackReason;
  let attempt = await requestUpstream(config, upstreamBody, effectiveModel, wantStream, req, res);
  let totalLatency = attempt.latencyMs;
  const usable = (candidate) => candidate.response && candidate.response.ok &&
    (!wantStream || isEventStreamResponse(candidate.response));
  if (!usable(attempt) && !attempt.clientDisconnected() &&
    effectiveModel !== config.modelCatalog.defaultModel) {
    if (attempt.response && attempt.response.body) await attempt.response.body.cancel().catch(() => {});
    fallbackReason = attempt.response && attempt.response.ok ? 'upstream_invalid_content_type' :
      (attempt.response ? `upstream_http_${attempt.response.status}` : 'upstream_unavailable');
    effectiveModel = config.modelCatalog.defaultModel;
    attempt.release();
    attempt = await requestUpstream(config, upstreamBody, effectiveModel, wantStream, req, res);
    totalLatency += attempt.latencyMs;
  }
  const upstream = attempt.response;
  if (!usable(attempt)) {
    if (upstream && upstream.body) await upstream.body.cancel().catch(() => {});
    try {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: attempt.clientDisconnected() ? 'interrupted' : 'failed',
        effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
    } finally {
      attempt.release();
    }
    throw new HttpError(502, upstream && upstream.ok ?
      'upstream_invalid_response' : 'upstream_unavailable');
  }
  if (wantStream) {
    let reader = upstream.body.getReader();
    let first = await nextStreamChunk(reader);
    if ((first.failed || first.done) && !attempt.clientDisconnected() &&
      effectiveModel !== config.modelCatalog.defaultModel) {
      await reader.cancel().catch(() => {});
      attempt.release();
      fallbackReason = 'upstream_stream_interrupted_before_first_byte';
      effectiveModel = config.modelCatalog.defaultModel;
      attempt = await requestUpstream(config, upstreamBody, effectiveModel, true, req, res);
      totalLatency += attempt.latencyMs;
      if (usable(attempt)) {
        reader = attempt.response.body.getReader();
        first = await nextStreamChunk(reader);
      }
    }
    if (!usable(attempt) || first.failed || first.done) {
      if (attempt.response && attempt.response.body) {
        await attempt.response.body.cancel().catch(() => {});
      }
      try {
        await finalizeAiRequest(pool, tenantId, reservation, {
          status: attempt.clientDisconnected() ? 'interrupted' : 'failed',
          effectiveModel, fallbackReason, latencyMs: totalLatency,
        });
      } finally {
        attempt.release();
      }
      throw new HttpError(502, 'upstream_unavailable');
    }
    const responseHeaders = aiResponseHeaders(reservation, effectiveModel, fallbackReason);
    res.writeHead(200, {
      'Content-Type': 'text/event-stream; charset=utf-8',
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      ...responseHeaders,
    });
    try {
      if (!first.done && !res.write(first.value)) await waitForDrain(res);
      while (true) {
        const next = await reader.read();
        if (next.done) break;
        if (!res.write(next.value)) await waitForDrain(res);
      }
    } catch {
      await reader.cancel().catch(() => {});
      try {
        await finalizeAiRequest(pool, tenantId, reservation, {
          status: 'interrupted', effectiveModel, fallbackReason, latencyMs: totalLatency,
        });
      } finally {
        attempt.release();
      }
      res.destroy();
      return;
    }
    try {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: 'succeeded', effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
    } finally {
      attempt.release();
    }
    res.end();
    return;
  }
  let completed;
  let bodyError;
  try {
    completed = await readJsonObjectResponse(upstream, config.aiMaxResponseBytes);
  } catch (error) {
    bodyError = error instanceof HttpError ? error : new HttpError(502, 'upstream_interrupted');
  }
  if (bodyError && !attempt.clientDisconnected() &&
      effectiveModel !== config.modelCatalog.defaultModel) {
    fallbackReason = bodyError.code;
    effectiveModel = config.modelCatalog.defaultModel;
    attempt.release();
    attempt = await requestUpstream(config, upstreamBody, effectiveModel, false, req, res);
    totalLatency += attempt.latencyMs;
    if (usable(attempt)) {
      try {
        completed = await readJsonObjectResponse(attempt.response, config.aiMaxResponseBytes);
        bodyError = null;
      } catch (error) {
        bodyError = error instanceof HttpError ? error : new HttpError(502, 'upstream_interrupted');
      }
    } else {
      if (attempt.response && attempt.response.body) {
        await attempt.response.body.cancel().catch(() => {});
      }
      bodyError = new HttpError(502, attempt.response && attempt.response.ok ?
        'upstream_invalid_response' : 'upstream_unavailable');
    }
  }
  if (bodyError) {
    try {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: attempt.clientDisconnected() ? 'interrupted' : 'failed',
        effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
    } finally {
      attempt.release();
    }
    throw bodyError;
  }
  const { text: response, parsed } = completed;
  const usage = parsed.usage && typeof parsed.usage === 'object' ? parsed.usage : undefined;
  try {
    await finalizeAiRequest(pool, tenantId, reservation, {
      status: 'succeeded', effectiveModel, fallbackReason, latencyMs: totalLatency, usage,
    });
  } finally {
    attempt.release();
  }
  const responseHeaders = aiResponseHeaders(reservation, effectiveModel, fallbackReason);
  res.writeHead(200, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ...responseHeaders,
  });
  res.end(response);
}

async function createPostgresServer(overrides = {}) {
  const config = buildConfig(overrides);
  const pool = overrides.pool || new Pool({ max: config.poolMax, idleTimeoutMillis: 30000 });
  await pool.query('select 1');
  await bootstrapAdmin(pool, config);
  let retentionInFlight = null;
  const runRetention = () => {
    if (retentionInFlight) return retentionInFlight;
    retentionInFlight = cleanExpiredData(pool, config)
      .catch((error) => console.error('retention cleanup failed', safeErrorLog(error)))
      .finally(() => { retentionInFlight = null; });
    return retentionInFlight;
  };
  runRetention();
  const retentionTimer = setInterval(() => {
    runRetention();
  }, 6 * 60 * 60 * 1000);
  retentionTimer.unref();
  const loginFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: LOGIN_MAX_FAILURES,
  });
  const loginIpFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: LOGIN_IP_MAX_FAILURES,
  });
  const registrationAttempts = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: REGISTRATION_MAX_ATTEMPTS,
  });
  // Unauthenticated refresh/logout share one per-IP failure window; garbage
  // token floods from one address otherwise hold pool connections in
  // transactions just like login floods did.
  const refreshIpFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: REFRESH_IP_MAX_FAILURES,
  });
  const authKdfConcurrency = createConcurrencyLimiter(MAX_CONCURRENT_AUTH_KDFS);

  async function authenticate(req) {
    const tenantId = verifyAccessToken(bearer(req), config.authSecret);
    if (!tenantId) throw new HttpError(401, 'unauthorized');
    const active = await pool.query(
      "select 1 from auth.users where id=$1 and status='active'", [tenantId]
    );
    if (active.rowCount !== 1) throw new HttpError(401, 'unauthorized');
    return tenantId;
  }

  function loginKey(req, username) {
    const address = requestClientAddress(req, config.trustProxy);
    return { pair: `${address}|${username}`, address };
  }

  async function handle(req, res) {
    const origin = req.headers.origin;
    const originAllowed = typeof origin !== 'string' ||
      requestOriginAllowed(req, origin, config.allowedOrigins);
    if (!originAllowed) throw new HttpError(403, 'origin_not_allowed');
    if (typeof origin === 'string') {
      res.setHeader('Access-Control-Allow-Origin', origin);
      res.setHeader('Vary', 'Origin');
      res.setHeader('Access-Control-Allow-Headers',
        'Content-Type, Authorization, X-Device, X-Solum-Backup-Writer, If-Match');
      res.setHeader('Access-Control-Allow-Methods', 'GET, POST, PUT, DELETE, OPTIONS');
      res.setHeader('Access-Control-Expose-Headers',
        'X-Solum-Request-Id, X-Solum-Requested-Model, X-Solum-Effective-Model, ' +
        'X-Solum-Fallback, X-Solum-Fallback-Reason, X-Solum-Quota-Limit, ' +
        'X-Solum-Quota-Used, X-Solum-Quota-Remaining, X-Solum-Quota-Reset, ETag');
    }
    if (req.method === 'OPTIONS') {
      if (typeof origin !== 'string') throw new HttpError(403, 'origin_not_allowed');
      res.writeHead(204, { 'Content-Length': '0', 'Cache-Control': 'no-store' });
      return res.end();
    }

    const url = new URL(req.url, 'http://localhost');
    if (req.method === 'GET' && url.pathname === '/') {
      return html(res, DASHBOARD);
    }

    if (req.method === 'GET' && url.pathname === '/v1/meta') {
      return json(res, 200, {
        name: 'Solum Center',
        version: SERVICE_VERSION,
        revision: config.buildRevision,
        api_base: '/',
        registration: config.registrationMode,
        features: ['account', 'ai', 'ai_quota', 'sync', 'alerts', 'devices', 'backups'],
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/health') {
      await pool.query('select 1');
      return json(res, 200, {
        status: 'ok', storage: 'postgres', version: SERVICE_VERSION, revision: config.buildRevision,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/register') {
      if (config.registrationMode !== 'open') throw new HttpError(403, 'registration_closed');
      const registrationKey = requestClientAddress(req, config.trustProxy);
      if (!registrationAttempts.consume(registrationKey)) {
        throw new HttpError(429, 'too_many_attempts');
      }
      const body = await readJson(req);
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const password = requiredOpaqueString(body.password, 'invalid_credentials', 1024);
      if ([...password].length < 8 || /[\u0000-\u001f\u007f]/.test(username)) {
        throw new HttpError(400, 'invalid_credentials');
      }
      const salt = crypto.randomBytes(16).toString('hex');
      const releaseKdf = authKdfConcurrency.tryAcquire();
      if (!releaseKdf) throw new HttpError(429, 'too_many_attempts');
      let passwordHash;
      try {
        passwordHash = await passwordHashAsync(password, salt);
      } finally {
        releaseKdf();
      }
      const client = await pool.connect();
      try {
        await client.query('begin');
        const inserted = await client.query(
          `insert into auth.users(username,password_hash,password_salt,password_version)
           values($1,$2,$3,2) returning id,username`,
          [username, passwordHash, salt]
        );
        const session = await issueSession(client, inserted.rows[0], config.authSecret);
        await client.query('commit');
        return json(res, 201, session);
      } catch (error) {
        await client.query('rollback').catch(() => {});
        if (error && error.code === '23505') throw new HttpError(409, 'username_taken');
        throw error;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/login') {
      const body = await readJson(req);
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const password = requiredOpaqueString(body.password, 'invalid_credentials', 1024);
      const key = loginKey(req, username);
      const pairReservation = loginFailures.reserve(key.pair);
      if (!pairReservation) throw new HttpError(429, 'too_many_attempts');
      const ipReservation = loginIpFailures.reserve(key.address);
      if (!ipReservation) {
        pairReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      const releaseKdf = authKdfConcurrency.tryAcquire();
      if (!releaseKdf) {
        pairReservation.release();
        ipReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        // Password verification is CPU work and must not hold a database row
        // lock or an open transaction. Session issuance has its own short,
        // per-user advisory-locked transaction below.
        const selected = await pool.query(
          `select id,username,password_hash,password_salt,password_version
           from auth.users where username=$1 and status='active'`,
          [username]
        );
        const user = selected.rows[0];
        const passwordMatches = await verifyPasswordOrDummyAsync(password, user && {
          salt: user.password_salt,
          storedHash: user.password_hash,
          passwordVersion: user.password_version,
        });
        if (!passwordMatches) {
          pairReservation.commit();
          ipReservation.commit();
          throw new HttpError(401, 'invalid_credentials');
        }
        // Keep version-1 hashes stable. A login cannot prove which
        // whitespace variant is bound to an existing recovery envelope.
        const client = await pool.connect();
        let transactionOpen = false;
        let session;
        try {
          await client.query('begin');
          transactionOpen = true;
          const active = await client.query(
            `select id,username from auth.users where id=$1 and status='active'`, [user.id]
          );
          if (active.rowCount !== 1) {
            pairReservation.commit();
            ipReservation.commit();
            await client.query('rollback');
            transactionOpen = false;
            throw new HttpError(401, 'invalid_credentials');
          }
          session = await issueSession(client, active.rows[0], config.authSecret);
          await client.query('commit');
          transactionOpen = false;
        } catch (error) {
          if (transactionOpen) await client.query('rollback').catch(() => {});
          throw error;
        } finally {
          client.release();
        }
        pairReservation.release();
        ipReservation.release();
        loginFailures.reset(key.pair);
        return json(res, 200, session);
      } catch (error) {
        pairReservation.release();
        ipReservation.release();
        throw error;
      } finally {
        releaseKdf();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/refresh') {
      const body = await readJson(req);
      const refresh = requiredOpaqueString(body.refresh_token, 'invalid_refresh_token', 512);
      // Refresh and logout are unauthenticated endpoints that each trigger a
      // database transaction; without an IP cap, garbage tokens can saturate
      // the pool just like login attempts could.
      const address = requestClientAddress(req, config.trustProxy);
      const ipReservation = refreshIpFailures.reserve(address);
      if (!ipReservation) throw new HttpError(429, 'too_many_attempts');
      const hash = tokenHash(refresh);
      const sessionId = refreshTokenSessionId(refresh, config.authSecret);
      const client = await pool.connect();
      let transactionOpen = false;
      try {
        await client.query('begin');
        transactionOpen = true;
        const preliminary = await selectRefreshFamily(client, sessionId, hash);
        if (preliminary.rowCount === 0) {
          await client.query('commit');
          transactionOpen = false;
          throw new HttpError(401, 'invalid_refresh_token');
        }
        await lockAuthSessionUser(client, preliminary.rows[0].user_id);
        const selected = await selectRefreshFamily(client, sessionId, hash, true);
        const row = selected.rows[0];
        if (!row || row.status !== 'active' || new Date(row.expires_at).getTime() <= Date.now()) {
          if (row) {
            await client.query('delete from auth.refresh_tokens where session_id=$1', [row.session_id]);
          }
          await client.query('commit');
          transactionOpen = false;
          throw new HttpError(401, 'invalid_refresh_token');
        }
        if (!secureEqualBuffer(row.token_hash, hash)) {
          const rotatedAt = row.rotated_at === null ? Number.NaN :
            new Date(row.rotated_at).getTime();
          const grace = secureEqualBuffer(row.previous_token_hash, hash) &&
            Number.isFinite(rotatedAt) && Date.now() >= rotatedAt &&
            Date.now() - rotatedAt <= REFRESH_REPLAY_GRACE_MS;
          if (grace) {
            const recoveredRefresh = createRefreshSuccessorToken(
              row.session_id, refresh, config.authSecret
            );
            if (secureEqualBuffer(row.token_hash, tokenHash(recoveredRefresh))) {
              await client.query('commit');
              transactionOpen = false;
              return json(res, 200, {
                access_token: signAccessToken(
                  { id: row.user_id, username: row.username }, config.authSecret
                ),
                refresh_token: recoveredRefresh,
                user: { id: row.user_id, username: row.username },
              });
            }
          }
          if (!grace) {
            await client.query('delete from auth.refresh_tokens where session_id=$1', [row.session_id]);
          }
          await client.query('commit');
          transactionOpen = false;
          throw new HttpError(401, 'invalid_refresh_token');
        }
        const rotatedRefresh = createRefreshSuccessorToken(
          row.session_id, refresh, config.authSecret
        );
        const updated = await client.query(
          `update auth.refresh_tokens set token_hash=$1,previous_token_hash=$2,
             rotated_at=now(),expires_at=now()+interval '30 days',created_at=now()
           where session_id=$3 and token_hash=$2`,
          [tokenHash(rotatedRefresh), hash, row.session_id]
        );
        if (updated.rowCount !== 1) throw new HttpError(401, 'invalid_refresh_token');
        await client.query('commit');
        transactionOpen = false;
        const session = {
          access_token: signAccessToken(
            { id: row.user_id, username: row.username }, config.authSecret
          ),
          refresh_token: rotatedRefresh,
          user: { id: row.user_id, username: row.username },
        };
        ipReservation.release();
        return json(res, 200, session);
      } catch (error) {
        if (transactionOpen) await client.query('rollback').catch(() => {});
        // A failed token verification is exactly what the limiter counts.
        if (error instanceof HttpError && error.status === 401) {
          ipReservation.commit();
        } else {
          ipReservation.release();
        }
        throw error;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/logout') {
      const body = await readJson(req);
      const address = requestClientAddress(req, config.trustProxy);
      const logoutIpReservation = refreshIpFailures.reserve(address);
      if (!logoutIpReservation) throw new HttpError(429, 'too_many_attempts');
      const refresh = requiredOpaqueString(body.refresh_token, 'invalid_refresh_token', 512);
      const sessionId = refreshTokenSessionId(refresh, config.authSecret);
      const hash = tokenHash(refresh);
      const client = await pool.connect();
      try {
        await client.query('begin');
        const selected = await selectRefreshFamily(client, sessionId, hash);
        if (selected.rowCount > 0) {
          await lockAuthSessionUser(client, selected.rows[0].user_id);
          if (sessionId) {
            await client.query('delete from auth.refresh_tokens where session_id=$1', [sessionId]);
          } else {
            await client.query(
              `delete from auth.refresh_tokens where token_hash=$1 or previous_token_hash=$1
                 or legacy_token_hash=$1`, [hash]
            );
          }
        }
        await client.query('commit');
      } catch (error) {
        await client.query('rollback').catch(() => {});
        if (error instanceof HttpError && (error.status === 400 || error.status === 401)) {
          logoutIpReservation.commit();
        } else {
          logoutIpReservation.release();
        }
        throw error;
      } finally {
        client.release();
      }
      logoutIpReservation.release();
      return json(res, 200, { status: 'ok' });
    }

    // ---- Recovery code: password-independent second key --------------------
    //
    // The plaintext recovery code never travels to the server. Online
    // verification is two-step: the challenge returns the PBKDF2 salt stored
    // inside the envelope (or a deterministic dummy salt for accounts without
    // a code, so this endpoint pair cannot enumerate who has one); the client
    // derives the wrap key locally and submits only its SHA-256 digest.
    if (req.method === 'POST' && url.pathname === '/v1/auth/recover/challenge') {
      const body = await readJson(req);
      requireExactKeys(body, ['username'], 'invalid_credentials');
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const key = loginKey(req, username);
      const reservation = loginFailures.reserve(key.pair);
      if (!reservation) throw new HttpError(429, 'too_many_attempts');
      const ipReservation = loginIpFailures.reserve(key.address);
      if (!ipReservation) {
        reservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        const selected = await pool.query(
          'select id, status from auth.users where username=$1', [username]
        );
        const user = selected.rows[0];
        let saltHex = null;
        if (user && user.status === 'active') {
          const stored = await withTenant(pool, user.id, async (client) => {
            const result = await client.query(
              'select envelope from vault.recovery_codes where tenant_id=$1', [user.id]
            );
            return result.rows[0] || null;
          });
          if (stored) {
            // Envelope layout is salt(16 bytes) || nonce(24) || ciphertext; the
            // client re-derives the wrap key from it without another round trip.
            saltHex = stored.envelope.subarray(0, 16).toString('hex');
          }
        }
        if (!saltHex) {
          // Deterministic per-username dummy so every account sees the same
          // response shape whether or not it has a recovery code.
          saltHex = crypto.createHmac('sha256', config.authSecret)
            .update(`solum-recover-dummy-salt-v1:${username.toLowerCase()}`)
            .digest('hex').slice(0, 32);
        }
        return json(res, 200, { pbkdf2_salt: saltHex, rounds: 600000 });
      } catch (error) {
        reservation.commit();
        ipReservation.commit();
        throw error;
      } finally {
        reservation.release();
        ipReservation.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/recover') {
      const body = await readJson(req);
      requireExactKeys(body, ['username', 'verify_hash'], 'invalid_credentials');
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const digest = requiredOpaqueString(body.verify_hash, 'invalid_credentials', 64);
      if (!SHA256_PATTERN.test(digest)) throw new HttpError(400, 'invalid_credentials');
      const key = loginKey(req, username);
      const pairReservation = loginFailures.reserve(key.pair);
      if (!pairReservation) throw new HttpError(429, 'too_many_attempts');
      const ipReservation = loginIpFailures.reserve(key.address);
      if (!ipReservation) {
        pairReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        const selected = await pool.query(
          'select id, username, status from auth.users where username=$1', [username]
        );
        const row = selected.rows[0];
        const active = row && row.status === 'active' ? row : null;
        // Burn one scrypt even when the account or its recovery code does not
        // exist, keeping failure timing uniform with real verification.
        await scryptBurnAsync(digest);
        const storedHash = active ? await withTenant(pool, active.id, async (client) => {
          const result = await client.query(
            'select verify_hash from vault.recovery_codes where tenant_id=$1', [active.id]
          );
          return result.rows[0] ? result.rows[0].verify_hash : null;
        }) : null;
        if (!storedHash || !secureEqualHex(storedHash, digest)) {
          pairReservation.commit();
          ipReservation.commit();
          throw new HttpError(401, 'invalid_credentials');
        }
        const session = await issueSessionForUser(pool, active, config.authSecret);
        pairReservation.release();
        ipReservation.release();
        loginFailures.reset(key.pair);
        return json(res, 200, session);
      } catch (error) {
        pairReservation.release();
        ipReservation.release();
        throw error;
      }
    }

    // Explicit password change. Authenticated with either the current password
    // or the current recovery code (as its derived-key digest), then one
    // transaction swaps the credential hash, replaces the password envelope
    // with the client's pre-rewrapped one, and revokes all other refresh
    // sessions - "new password live, envelope still old" is unreachable.
    if (req.method === 'POST' && url.pathname === '/v1/account/password') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      // Exact-key whitelist with two optional alternates: exactly one current
      // credential is required; the rewrapped envelope is optional but the
      // only sanctioned way to keep it in sync.
      const allowed = new Set(['new_password', 'current_password',
        'current_code_verify_hash', 'password_envelope']);
      for (const key of Object.keys(body)) {
        if (!allowed.has(key)) throw new HttpError(400, 'invalid_request');
      }
      const newPassword = requiredOpaqueString(body.new_password, 'invalid_credentials', 1024);
      if ([...newPassword].length < 8) throw new HttpError(400, 'invalid_credentials');
      const oldPassword = typeof body.current_password === 'string' &&
        body.current_password.length > 0 ? body.current_password : null;
      const oldCodeDigest = typeof body.current_code_verify_hash === 'string' &&
        SHA256_PATTERN.test(body.current_code_verify_hash)
        ? body.current_code_verify_hash : null;
      if ((oldPassword === null) === (oldCodeDigest === null)) {
        throw new HttpError(400, 'invalid_request');
      }
      const key = loginKey(req, String(tenantId));
      const pairReservation = loginFailures.reserve(key.pair);
      if (!pairReservation) throw new HttpError(429, 'too_many_attempts');
      const releaseKdf = authKdfConcurrency.tryAcquire();
      if (!releaseKdf) {
        pairReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        const currentRefreshHash = tokenHash(bearer(req));
        const client = await pool.connect();
        let transactionOpen = false;
        try {
          await client.query('begin');
          transactionOpen = true;
          await client.query("select set_config('app.current_tenant_id', $1, true)", [tenantId]);
          await lockAuthSessionUser(client, tenantId);
          const users = await client.query(
            `select username, password_hash, password_salt, password_version
             from auth.users where id=$1 and status='active' for update`, [tenantId]
          );
          const user = users.rows[0];
          if (!user) {
            pairReservation.commit();
            throw new HttpError(401, 'unauthorized');
          }
          let authorized;
          if (oldPassword !== null) {
            authorized = await verifyPasswordAsync(oldPassword, user.password_salt,
              user.password_hash, user.password_version);
          } else {
            const codes = await client.query(
              'select verify_hash from vault.recovery_codes where tenant_id=$1 for update',
              [tenantId]
            );
            authorized = codes.rows.length > 0 &&
              secureEqualHex(codes.rows[0].verify_hash, oldCodeDigest);
          }
          if (!authorized) {
            pairReservation.commit();
            throw new HttpError(403, 'invalid_current_secret');
          }
          const newSalt = crypto.randomBytes(16).toString('hex');
          const newHash = await passwordHashAsync(newPassword, newSalt);
          await client.query(
            `update auth.users
             set password_hash=$2, password_salt=$3, password_version=2
             where id=$1`, [tenantId, newHash, newSalt]
          );
          if (typeof body.password_envelope === 'string') {
            const envelope = decodeBase64(body.password_envelope, 'invalid_envelope', 100000);
            if (envelope.length < 40 || envelope.length > 65536) {
              throw new HttpError(400, 'invalid_envelope');
            }
            await client.query(
              `insert into vault.key_envelopes
                 (tenant_id,recipient_device_id,key_version,algorithm,envelope)
               values($1,'account-recovery-v1',1,'recovery-xchacha20poly1305',$2)
               on conflict(tenant_id,recipient_device_id,key_version,algorithm)
               do update set envelope=excluded.envelope, created_at=now()`,
              [tenantId, envelope]
            );
          }
          // Keep only the refresh session this request arrived on; every other
          // family dies with the old credential.
          const mine = await client.query(
            `select session_id from auth.refresh_tokens
             where user_id=$1 and (token_hash=$2 or previous_token_hash=$2
               or legacy_token_hash=$2) limit 1`, [tenantId, currentRefreshHash]
          );
          const keepSessionId = mine.rows.length > 0 ? mine.rows[0].session_id : null;
          if (keepSessionId === null) {
            await client.query('delete from auth.refresh_tokens where user_id=$1', [tenantId]);
          } else {
            await client.query(
              'delete from auth.refresh_tokens where user_id=$1 and session_id<>$2',
              [tenantId, keepSessionId]
            );
          }
          await client.query('commit');
          transactionOpen = false;
        } catch (error) {
          if (transactionOpen) await client.query('rollback').catch(() => {});
          throw error;
        } finally {
          client.release();
        }
        pairReservation.release();
        loginFailures.reset(key.pair);
        return json(res, 200, { status: 'ok' });
      } catch (error) {
        pairReservation.release();
        throw error;
      } finally {
        releaseKdf();
      }
    }

    // Store or rotate the recovery-code envelope (authenticated). Rotation is
    // an atomic replace: the old code stops verifying in the same transaction
    // that publishes the new envelope.
    if (req.method === 'PUT' && url.pathname === '/v1/account/recovery-code') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      requireExactKeys(body, ['key_version', 'algorithm', 'envelope', 'verify_hash'],
        'invalid_envelope');
      if (body.key_version !== 1 ||
          body.algorithm !== 'recovery-code-xchacha20poly1305') {
        throw new HttpError(400, 'invalid_envelope');
      }
      const envelope = decodeBase64(body.envelope, 'invalid_envelope', 100000);
      if (envelope.length < 56 || envelope.length > 65536) {
        throw new HttpError(400, 'invalid_envelope');
      }
      const verifyHash = requiredOpaqueString(body.verify_hash, 'invalid_envelope', 64);
      if (!SHA256_PATTERN.test(verifyHash)) throw new HttpError(400, 'invalid_envelope');
      await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        await client.query(
          `insert into vault.recovery_codes
             (tenant_id,key_version,algorithm,envelope,verify_hash)
           values($1,$2,$3,$4,$5)
           on conflict(tenant_id) do update
             set key_version=excluded.key_version,
                 algorithm=excluded.algorithm,
                 envelope=excluded.envelope,
                 verify_hash=excluded.verify_hash,
                 updated_at=now()`,
          [tenantId, body.key_version, body.algorithm, envelope, verifyHash]
        );
      });
      return json(res, 200, { status: 'ok', key_version: body.key_version });
    }

    if (req.method === 'GET' && url.pathname === '/v1/account/recovery-code') {
      const tenantId = await authenticate(req);
      const state = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select key_version, algorithm, created_at, updated_at
           from vault.recovery_codes where tenant_id=$1`, [tenantId]
        );
        return result.rows[0] || null;
      });
      return json(res, 200, state
        ? { configured: true, key_version: state.key_version,
            algorithm: state.algorithm, created_at: state.created_at,
            updated_at: state.updated_at }
        : { configured: false });
    }

    if (req.method === 'GET' && url.pathname === '/v1/keys/recovery-code') {
      const tenantId = await authenticate(req);
      const code = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select key_version,algorithm,envelope,created_at
           from vault.recovery_codes where tenant_id=$1`, [tenantId]
        );
        if (result.rowCount === 0) return null;
        const row = result.rows[0];
        return { ...row, envelope: row.envelope.toString('base64') };
      });
      return json(res, 200, code || { envelope: null });
    }

    if (req.method === 'GET' && url.pathname === '/v1/ai/models') {
      const tenantId = await authenticate(req);
      const usage = await readAiUsage(pool, tenantId, config);
      return json(res, 200, {
        default_model: config.modelCatalog.defaultModel,
        models: config.modelCatalog.models.map((id) => ({
          id, is_default: id === config.modelCatalog.defaultModel,
        })),
        custom_models_enabled: usage.custom_models_enabled,
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/ai/usage') {
      const tenantId = await authenticate(req);
      return json(res, 200, await readAiUsage(pool, tenantId, config));
    }

    if (req.method === 'POST' && url.pathname === '/v1/ai/chat/completions') {
      const tenantId = await authenticate(req);
      return proxyCompletion(req, res, pool, tenantId, config);
    }

    if (req.method === 'GET' && url.pathname === '/v1/backups/generation') {
      const tenantId = await authenticate(req);
      const state = await withTenant(pool, tenantId, async (client) => backupState(client, tenantId));
      return json(res, 200, {
        active_generation: state.activeGeneration,
        updated_at: isoTimestamp(state.updatedAt),
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/backups/writer') {
      const tenantId = await authenticate(req);
      const claim = parseWriterClaim(await readJson(req));
      const result = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        if (state.activeGeneration !== claim.expectedGeneration) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        if (state.writerId === claim.writerId) {
          return { activeGeneration: state.activeGeneration, claimed: false };
        }
        if (state.writerId !== null) throw new HttpError(409, 'backup_writer_stale');
        const existing = await client.query(
          'select 1 from vault.backups where tenant_id=$1 limit 1', [tenantId]
        );
        if (existing.rowCount !== 0) throw new HttpError(409, 'backup_writer_required');
        await client.query(
          `update vault.backup_state set writer_id=$2,updated_at=now()
           where tenant_id=$1 and writer_id is null`, [tenantId, claim.writerId]
        );
        return { activeGeneration: state.activeGeneration, claimed: true };
      });
      return json(res, 200, {
        active_generation: result.activeGeneration,
        claimed: result.claimed,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/backups/generation') {
      const tenantId = await authenticate(req);
      const takeover = parseTakeover(await readJson(req));
      const advanced = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        if (state.lastTakeoverId === takeover.takeoverId &&
            state.writerId === takeover.writerId) {
          return { activeGeneration: state.activeGeneration, prunedPending: 0, replay: true };
        }
        if (state.lastTakeoverId === takeover.takeoverId) {
          throw new HttpError(409, 'backup_writer_stale');
        }
        if (state.activeGeneration !== takeover.expectedGeneration ||
            state.activeGeneration === Number.MAX_SAFE_INTEGER) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        const next = state.activeGeneration + 1;
        await client.query(
          `update vault.backup_state
           set active_generation=$2,last_takeover_id=$3,writer_id=$4,updated_at=now()
           where tenant_id=$1`, [tenantId, next, takeover.takeoverId, takeover.writerId]
        );
        const removed = await client.query(
          `delete from vault.backups
           where tenant_id=$1 and status='pending' and generation<>$2 returning backup_id`,
          [tenantId, next]
        );
        return { activeGeneration: next, prunedPending: removed.rowCount, replay: false };
      });
      return json(res, 200, {
        active_generation: advanced.activeGeneration,
        pruned_pending: advanced.prunedPending,
        replay: advanced.replay,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/backups') {
      const tenantId = await authenticate(req);
      const plan = parseBackupPlan(await readJson(req), config);
      const created = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        requireBackupWriter(req, state);
        if (state.activeGeneration !== plan.generation) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        const existing = await client.query(
          `select status,plan_sha256 from vault.backups
           where tenant_id=$1 and backup_id=$2`, [tenantId, plan.backup_id]
        );
        if (existing.rowCount === 1) {
          if (!secureEqualBuffer(existing.rows[0].plan_sha256, plan.planSha256)) {
            throw new HttpError(409, 'backup_plan_conflict');
          }
          return { status: existing.rows[0].status, inserted: false };
        }
        const pending = await client.query(
          `select count(*) as pending_count from vault.backups
           where tenant_id=$1 and status='pending'`, [tenantId]
        );
        if (safeDatabaseInteger(pending.rows[0].pending_count, 'backup_pending_count') >=
            config.backupMaxPending) {
          throw new HttpError(429, 'backup_pending_limit');
        }
        await client.query(
          `insert into vault.backups(
             tenant_id,backup_id,generation,protocol_version,client_created_at,
             plan_sha256,total_bytes,object_count
           ) values($1,$2,$3,1,$4,$5,$6,$7)`,
          [tenantId, plan.backup_id, plan.generation, plan.created_at,
            plan.planSha256, plan.totalBytes, plan.objects.length]
        );
        for (const object of plan.objects) {
          await client.query(
            `insert into vault.backup_objects(
               tenant_id,backup_id,object_id,kind,declared_bytes,declared_sha256
             ) values($1,$2,$3,$4,$5,$6)`,
            [tenantId, plan.backup_id, object.object_id, object.kind,
              object.bytes, Buffer.from(object.sha256, 'hex')]
          );
        }
        return { status: 'pending', inserted: true };
      });
      return json(res, created.inserted ? 201 : 200, {
        backup_id: plan.backup_id,
        generation: plan.generation,
        status: created.status,
        plan_etag: plan.planSha256.toString('hex'),
        total_bytes: plan.totalBytes,
        object_count: plan.objects.length,
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/backups') {
      const tenantId = await authenticate(req);
      const limit = parseNonNegativeInt(url.searchParams.get('limit'), 20, 100);
      if (limit < 1) throw new HttpError(400, 'invalid_query');
      const result = await withTenant(pool, tenantId, async (client) => {
        const state = await backupState(client, tenantId);
        const selected = await client.query(
          `select backup_id,generation,protocol_version,client_created_at,committed_at,
                  plan_sha256,total_bytes,object_count
           from vault.backups where tenant_id=$1 and status='complete'
           order by committed_at desc,backup_id desc limit $2`, [tenantId, limit]
        );
        const byBackup = new Map(selected.rows.map((row) => [row.backup_id, []]));
        if (selected.rowCount > 0) {
          const objects = await client.query(
            `select backup_id,object_id,kind,declared_bytes,declared_sha256
             from vault.backup_objects where tenant_id=$1 and backup_id=any($2::uuid[])
             order by backup_id,object_id`, [tenantId, [...byBackup.keys()]]
          );
          for (const row of objects.rows) byBackup.get(row.backup_id).push(backupObjectJson(row));
        }
        return {
          active_generation: state.activeGeneration,
          backups: selected.rows.map((row) => backupJson(row, byBackup.get(row.backup_id))),
        };
      });
      return json(res, 200, result);
    }

    const backupObjectMatch = url.pathname.match(
      /^\/v1\/backups\/([0-9a-f-]{36})\/objects\/([0-9a-f-]{36})$/
    );
    if (backupObjectMatch && req.method === 'PUT') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupObjectMatch[1]);
      const objectId = backupUuid(backupObjectMatch[2]);
      if (!isOctetStream(req)) throw new HttpError(415, 'unsupported_media_type');
      const declared = await withTenant(pool, tenantId, async (client) => {
        const selected = await client.query(
          `select b.status,b.generation,o.declared_bytes,o.declared_sha256,o.ciphertext
           from vault.backups b join vault.backup_objects o
             on o.tenant_id=b.tenant_id and o.backup_id=b.backup_id
           where b.tenant_id=$1 and b.backup_id=$2 and o.object_id=$3`,
          [tenantId, backupId, objectId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_object_not_found');
        return selected.rows[0];
      });
      const expectedBytes = safeDatabaseInteger(declared.declared_bytes, 'backup_object_bytes');
      const contentLength = req.headers['content-length'];
      if (typeof contentLength !== 'string' || !/^\d+$/.test(contentLength)) {
        throw new HttpError(411, 'content_length_required');
      }
      if (Number(contentLength) !== expectedBytes) {
        throw new HttpError(409, 'backup_object_mismatch');
      }
      const ciphertext = await readExactBytes(req, expectedBytes);
      const digest = crypto.createHash('sha256').update(ciphertext).digest();
      if (!secureEqualBuffer(digest, declared.declared_sha256)) {
        throw new HttpError(409, 'backup_object_mismatch');
      }
      const uploaded = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        requireBackupWriter(req, state);
        const selected = await client.query(
          `select b.status,b.generation,o.declared_bytes,o.declared_sha256,o.ciphertext
           from vault.backups b join vault.backup_objects o
             on o.tenant_id=b.tenant_id and o.backup_id=b.backup_id
           where b.tenant_id=$1 and b.backup_id=$2 and o.object_id=$3`,
          [tenantId, backupId, objectId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_object_not_found');
        const row = selected.rows[0];
        if (safeDatabaseInteger(row.generation, 'backup_generation') !== state.activeGeneration) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        if (safeDatabaseInteger(row.declared_bytes, 'backup_object_bytes') !== ciphertext.length ||
            !secureEqualBuffer(row.declared_sha256, digest)) {
          throw new HttpError(409, 'backup_object_mismatch');
        }
        if (row.ciphertext !== null) {
          if (!secureEqualBuffer(row.ciphertext, ciphertext)) {
            throw new HttpError(409, 'backup_object_mismatch');
          }
          return false;
        }
        if (row.status !== 'pending') throw new HttpError(409, 'backup_plan_conflict');
        const used = await tenantBackupBytes(client, tenantId);
        if (used > config.backupTenantMaxBytes - ciphertext.length) {
          throw new HttpError(413, 'backup_quota_exceeded');
        }
        await client.query(
          `update vault.backup_objects set ciphertext=$4,uploaded_at=now()
           where tenant_id=$1 and backup_id=$2 and object_id=$3 and ciphertext is null`,
          [tenantId, backupId, objectId, ciphertext]
        );
        return true;
      });
      return json(res, uploaded ? 201 : 200, {
        backup_id: backupId, object_id: objectId, sha256: digest.toString('hex'),
      });
    }

    if (backupObjectMatch && req.method === 'GET') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupObjectMatch[1]);
      const objectId = backupUuid(backupObjectMatch[2]);
      const object = await withTenant(pool, tenantId, async (client) => {
        const selected = await client.query(
          `select o.ciphertext,o.declared_sha256
           from vault.backups b join vault.backup_objects o
             on o.tenant_id=b.tenant_id and o.backup_id=b.backup_id
           where b.tenant_id=$1 and b.backup_id=$2 and b.status='complete'
             and o.object_id=$3`, [tenantId, backupId, objectId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_object_not_found');
        if (!Buffer.isBuffer(selected.rows[0].ciphertext)) {
          throw new Error('complete backup contains an absent object');
        }
        return selected.rows[0];
      });
      return binary(res, object.ciphertext, object.declared_sha256);
    }

    const backupCommitMatch = url.pathname.match(
      /^\/v1\/backups\/([0-9a-f-]{36})\/commit$/
    );
    if (backupCommitMatch && req.method === 'POST') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupCommitMatch[1]);
      await readExactBytes(req, 0);
      const committed = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        requireBackupWriter(req, state);
        const selected = await client.query(
          `select backup_id,generation,protocol_version,client_created_at,committed_at,
                  plan_sha256,total_bytes,object_count,status
           from vault.backups where tenant_id=$1 and backup_id=$2`, [tenantId, backupId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_not_found');
        const row = selected.rows[0];
        if (safeDatabaseInteger(row.generation, 'backup_generation') !== state.activeGeneration) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        if (row.status === 'pending') {
          const objectRows = await client.query(
            `select object_id,declared_bytes,declared_sha256
             from vault.backup_objects where tenant_id=$1 and backup_id=$2
             order by object_id`,
            [tenantId, backupId]
          );
          if (objectRows.rowCount !== safeDatabaseInteger(row.object_count, 'backup_object_count')) {
            throw new HttpError(409, 'backup_incomplete');
          }
          let verifiedBytes = 0;
          for (const objectRow of objectRows.rows) {
            const stored = await client.query(
              `select ciphertext from vault.backup_objects
               where tenant_id=$1 and backup_id=$2 and object_id=$3`,
              [tenantId, backupId, objectRow.object_id]
            );
            const ciphertext = stored.rows[0] && stored.rows[0].ciphertext;
            const declaredBytes = safeDatabaseInteger(
              objectRow.declared_bytes, 'backup_object_bytes'
            );
            if (!Buffer.isBuffer(ciphertext) || ciphertext.length !== declaredBytes ||
                !secureEqualBuffer(
                  crypto.createHash('sha256').update(ciphertext).digest(),
                  objectRow.declared_sha256
                )) {
              throw new HttpError(409, 'backup_incomplete');
            }
            verifiedBytes += declaredBytes;
          }
          if (verifiedBytes !== safeDatabaseInteger(row.total_bytes, 'backup_total_bytes')) {
            throw new HttpError(409, 'backup_incomplete');
          }
          const updated = await client.query(
            `update vault.backups set status='complete',committed_at=now()
             where tenant_id=$1 and backup_id=$2 and status='pending'
             returning committed_at`, [tenantId, backupId]
          );
          row.status = 'complete';
          row.committed_at = updated.rows[0].committed_at;
        }
        const pruned = await client.query(
          `delete from vault.backups where tenant_id=$1 and backup_id in (
             select backup_id from vault.backups
             where tenant_id=$1 and status='complete'
             order by committed_at desc,backup_id desc offset $2
           ) returning backup_id`, [tenantId, config.backupMaxVersions]
        );
        return {
          backup: backupJson(row, []),
          pruned: pruned.rows.map((item) => item.backup_id),
        };
      });
      return json(res, 200, {
        ...committed.backup,
        status: 'complete',
        pruned_versions: committed.pruned,
      });
    }

    const backupVersionMatch = url.pathname.match(/^\/v1\/backups\/([0-9a-f-]{36})$/);
    if (backupVersionMatch && req.method === 'DELETE') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupVersionMatch[1]);
      const expected = parseIfMatch(req.headers['if-match']);
      await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const selected = await client.query(
          `select plan_sha256 from vault.backups where tenant_id=$1 and backup_id=$2`,
          [tenantId, backupId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_not_found');
        if (!secureEqualBuffer(selected.rows[0].plan_sha256, expected)) {
          throw new HttpError(412, 'precondition_failed');
        }
        await client.query(
          `delete from vault.backups where tenant_id=$1 and backup_id=$2`, [tenantId, backupId]
        );
      });
      return json(res, 200, { backup_id: backupId, deleted: true });
    }

    if (req.method === 'POST' && url.pathname === '/v1/push') {
      const tenantId = await authenticate(req);
      const device = requiredString(req.headers['x-device'], 'invalid_device', 128);
      const blob = await readBytes(req, MAX_BLOB_BYTES);
      if (blob.length === 0) throw new HttpError(400, 'bad_blob');
      const seq = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        await touchActiveDevice(client, tenantId, device, config.tenantMaxDevices);
        await ensureBlobQuota(client, tenantId, blob.length, config);
        const inserted = await client.query(
          'insert into sync.blobs(tenant_id,device_id,ciphertext) values($1,$2,$3) returning seq',
          [tenantId, device, blob]
        );
        return safeDatabaseInteger(inserted.rows[0].seq, 'blob_seq');
      });
      return json(res, 200, { seq });
    }

    if (req.method === 'GET' && url.pathname === '/v1/pull') {
      const tenantId = await authenticate(req);
      const since = parseNonNegativeInt(url.searchParams.get('since'), 0);
      const device = requiredString(url.searchParams.get('device'), 'invalid_device', 128);
      const result = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        await touchActiveDevice(client, tenantId, device, config.tenantMaxDevices);
        const pulled = await client.query(
          `with candidates as (
             select seq,device_id,ciphertext,
                    row_number() over(order by seq) as row_num,
                    sum(octet_length(ciphertext)) over(order by seq) as cumulative_bytes
             from sync.blobs
             where tenant_id=$1 and seq>$2 and device_id<>$3
             order by seq
             limit $4
           )
           select seq,device_id,ciphertext from candidates
           where cumulative_bytes<=$5 or row_num=1 order by seq`,
          [tenantId, since, device, MAX_PULL_ROWS, MAX_PULL_BYTES]
        );
        const oldest = await client.query(
          'select coalesce(min(seq),0) as oldest_seq from sync.blobs where tenant_id=$1', [tenantId]
        );
        return {
          oldest_seq: safeDatabaseInteger(oldest.rows[0].oldest_seq, 'oldest_blob_seq'),
          blobs: pulled.rows.map((row) => ({
            seq: safeDatabaseInteger(row.seq, 'blob_seq'),
            device: row.device_id, blob: row.ciphertext.toString('base64'),
          })),
        };
      });
      return json(res, 200, result);
    }

    if (req.method === 'GET' && url.pathname === '/v1/stats') {
      const tenantId = await authenticate(req);
      const stats = await withTenant(pool, tenantId, async (client) => {
        const totals = await client.query(
          `select count(*) as total_blobs,coalesce(sum(octet_length(ciphertext)),0) as total_bytes,
                  coalesce(min(seq),0) as oldest_seq,coalesce(max(seq),0) as newest_seq
           from sync.blobs where tenant_id=$1`, [tenantId]
        );
        const devices = await client.query(
          `select device_id,count(*) as blob_count,coalesce(sum(octet_length(ciphertext)),0) as bytes,
                  max(seq) as last_seq,max(received_at) as last_received_at
           from sync.blobs where tenant_id=$1 group by device_id order by max(seq) desc`, [tenantId]
        );
        const row = totals.rows[0];
        return {
          total_blobs: safeDatabaseInteger(row.total_blobs, 'total_blobs'),
          total_bytes: safeDatabaseInteger(row.total_bytes, 'total_bytes'),
          oldest_seq: safeDatabaseInteger(row.oldest_seq, 'oldest_blob_seq'),
          newest_seq: safeDatabaseInteger(row.newest_seq, 'newest_blob_seq'),
          devices: devices.rows.map((item) => ({
            device: item.device_id,
            blob_count: safeDatabaseInteger(item.blob_count, 'device_blob_count'),
            bytes: safeDatabaseInteger(item.bytes, 'device_blob_bytes'),
            last_seq: safeDatabaseInteger(item.last_seq, 'device_last_seq'),
            last_received_at: item.last_received_at,
          })),
          tenant: tenantId, auth_mode: 'account',
        };
      });
      return json(res, 200, stats);
    }

    if (req.method === 'GET' && url.pathname === '/v1/devices') {
      const tenantId = await authenticate(req);
      const devices = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select device_id,protocol_version,last_seen_at,revoked_at
           from sync.devices where tenant_id=$1 order by last_seen_at desc`, [tenantId]
        );
        return result.rows;
      });
      return json(res, 200, { devices });
    }

    if (req.method === 'POST' && url.pathname === '/v1/alerts') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      const alert = {
        eventId: requiredString(body.event_id, 'invalid_alert', 256),
        source: requiredString(body.source, 'invalid_alert', 64),
        monitorId: optionalString(body.monitor_id, 'invalid_alert', 256),
        name: optionalString(body.name, 'invalid_alert', 256),
        status: requiredString(body.status, 'invalid_alert', 64),
        latencyMs: optionalBoundedNumber(body.latency_ms, 'invalid_alert',
          { min: 0, max: MAX_ALERT_LATENCY_MS, integer: true }),
        pingLatencyMs: optionalBoundedNumber(body.ping_latency_ms, 'invalid_alert',
          { min: 0, max: MAX_ALERT_LATENCY_MS, integer: true }),
        availability7d: optionalBoundedNumber(body.availability_7d, 'invalid_alert',
          { min: 0, max: 100 }),
        checkedAt: optionalString(body.checked_at, 'invalid_alert', 128),
        detailUrl: optionalHttpUrl(body.detail_url, 'invalid_alert', 2048),
      };
      const seq = await withTenant(pool, tenantId, async (client) => {
        const inserted = await client.query(
          `insert into sync.alerts(tenant_id,event_id,source,monitor_id,name,status,latency_ms,
             ping_latency_ms,availability_7d,checked_at,detail_url)
           values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
           on conflict(tenant_id,event_id) do nothing returning seq`,
          [tenantId, alert.eventId, alert.source, alert.monitorId, alert.name, alert.status,
            alert.latencyMs, alert.pingLatencyMs, alert.availability7d, alert.checkedAt, alert.detailUrl]
        );
        if (inserted.rowCount > 0) return safeDatabaseInteger(inserted.rows[0].seq, 'alert_seq');
        const existing = await client.query(
          'select seq from sync.alerts where tenant_id=$1 and event_id=$2', [tenantId, alert.eventId]
        );
        return safeDatabaseInteger(existing.rows[0].seq, 'alert_seq');
      });
      return json(res, 200, { seq });
    }

    if (req.method === 'GET' && url.pathname === '/v1/alerts') {
      const tenantId = await authenticate(req);
      const since = parseNonNegativeInt(url.searchParams.get('since'), 0);
      const limit = parseNonNegativeInt(url.searchParams.get('limit'), 100, 500) || 100;
      const source = url.searchParams.has('source') ?
        requiredString(url.searchParams.get('source'), 'invalid_query', 64) : null;
      const alerts = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select seq,event_id,source,monitor_id,name,status,latency_ms,ping_latency_ms,
                  availability_7d,checked_at,detail_url,received_at
           from sync.alerts where tenant_id=$1 and seq>$2 and ($3::text is null or source=$3)
           order by seq limit $4`, [tenantId, since, source, limit]
        );
        return result.rows.map((row) => ({ ...row,
          seq: safeDatabaseInteger(row.seq, 'alert_seq'),
          latency_ms: row.latency_ms === null ? null :
            safeDatabaseInteger(row.latency_ms, 'alert_latency_ms'),
          ping_latency_ms: row.ping_latency_ms === null ? null :
            safeDatabaseInteger(row.ping_latency_ms, 'alert_ping_latency_ms'),
          availability_7d: row.availability_7d === null ? null : Number(row.availability_7d) }));
      });
      return json(res, 200, { alerts });
    }

    if (req.method === 'GET' && url.pathname === '/v1/keys/recovery') {
      const tenantId = await authenticate(req);
      const recovery = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select key_version,algorithm,envelope,created_at
           from vault.key_envelopes
           where tenant_id=$1 and recipient_device_id='account-recovery-v1'
             and key_version=1 and algorithm='recovery-xchacha20poly1305'
             and revoked_at is null`, [tenantId]
        );
        if (result.rowCount === 0) return null;
        const row = result.rows[0];
        return { ...row, envelope: row.envelope.toString('base64') };
      });
      return json(res, 200, recovery || { envelope: null });
    }

    if (req.method === 'PUT' && url.pathname === '/v1/keys/recovery') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      if (body.key_version !== 1 || body.algorithm !== 'recovery-xchacha20poly1305') {
        throw new HttpError(400, 'invalid_envelope');
      }
      const envelope = decodeBase64(body.envelope, 'invalid_envelope', 100000);
      if (envelope.length < 40 || envelope.length > 65536) {
        throw new HttpError(400, 'invalid_envelope');
      }
      // Create-only is deliberate. Two first devices may race, but neither is
      // allowed to overwrite an already authoritative key and strand old blobs.
      const effective = await withTenant(pool, tenantId, async (client) => {
        await client.query(
          `insert into vault.key_envelopes
             (tenant_id,recipient_device_id,key_version,algorithm,envelope)
           values($1,'account-recovery-v1',1,'recovery-xchacha20poly1305',$2)
           on conflict(tenant_id,recipient_device_id,key_version,algorithm) do nothing`,
          [tenantId, envelope]
        );
        const saved = await client.query(
          `select key_version,algorithm,envelope,created_at
           from vault.key_envelopes
           where tenant_id=$1 and recipient_device_id='account-recovery-v1'
             and key_version=1 and algorithm='recovery-xchacha20poly1305'
             and revoked_at is null`, [tenantId]
        );
        const row = saved.rows[0];
        return { ...row, envelope: row.envelope.toString('base64') };
      });
      return json(res, 200, effective);
    }

    throw new HttpError(404, 'not_found');
  }

  const server = http.createServer((req, res) => {
    handle(req, res).catch((error) => {
      if (res.headersSent) {
        res.destroy();
        return;
      }
      if (error instanceof HttpError) return json(res, error.status, { error: error.code });
      console.error('request failed', safeErrorLog(error));
      return json(res, 500, { error: 'internal_error' });
    });
  });
  server.headersTimeout = 15_000;
  server.requestTimeout = 60_000;
  server.keepAliveTimeout = 5_000;
  server.maxHeadersCount = 64;

  return {
    server,
    pool,
    async close() {
      clearInterval(retentionTimer);
      await new Promise((resolve) => server.close(resolve));
      if (retentionInFlight) await retentionInFlight;
      if (!overrides.pool) await pool.end();
    },
  };
}

async function main() {
  const app = await createPostgresServer();
  const port = Number(process.env.SOLUM_PORT || 8787);
  app.server.listen(port, '0.0.0.0', () => console.log(`Solum cloud listening on ${port} (PostgreSQL)`));
  const shutdown = async () => {
    await app.close();
    process.exit(0);
  };
  process.once('SIGTERM', shutdown);
  process.once('SIGINT', shutdown);
}

if (require.main === module) {
  main().catch((error) => {
    console.error('startup failed', safeErrorLog(error));
    process.exit(1);
  });
}

module.exports = {
  createPostgresServer, verifyAccessToken, withTenant, parseModelCatalog, usageProjection,
  validateCompletionOptions, readBodyLimited, recoverTenantStaleAiRequests, proxyCompletion,
  optionalBoundedNumber, optionalHttpUrl, requiredOpaqueString, safeDatabaseInteger,
  parseBackupPlan, parseTakeover, parseWriterClaim, parseIfMatch, buildConfig,
};

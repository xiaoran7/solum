'use strict';

const http = require('node:http');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { Pool } = require('pg');

const ACCESS_TTL_SECONDS = 15 * 60;
const REFRESH_TTL_SECONDS = 30 * 24 * 60 * 60;
const MAX_JSON_BYTES = 1024 * 1024;
const MAX_BLOB_BYTES = 8 * 1024 * 1024;
const MAX_PULL_BYTES = 16 * 1024 * 1024;
const MAX_PULL_ROWS = 500;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;
const LOGIN_MAX_FAILURES = 5;
const DASHBOARD = fs.readFileSync(path.join(__dirname, 'dashboard.html'));

class HttpError extends Error {
  constructor(status, code) {
    super(code);
    this.status = status;
    this.code = code;
  }
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
    'Content-Security-Policy': "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
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

async function readJson(req) {
  const raw = await readBytes(req, MAX_JSON_BYTES);
  try {
    return JSON.parse(raw.toString('utf8') || '{}');
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

function optionalString(value, code, max = 2048) {
  if (value === undefined || value === null || value === '') return null;
  if (typeof value !== 'string' || value.length > max) throw new HttpError(400, code);
  return value.trim() || null;
}

function optionalNumber(value, code) {
  if (value === undefined || value === null) return null;
  if (typeof value !== 'number' || !Number.isFinite(value)) throw new HttpError(400, code);
  return value;
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

function passwordHash(password, salt) {
  return crypto.scryptSync(password, salt, 64).toString('hex');
}

function secureEqualHex(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string' || left.length !== right.length) {
    return false;
  }
  try {
    return crypto.timingSafeEqual(Buffer.from(left, 'hex'), Buffer.from(right, 'hex'));
  } catch {
    return false;
  }
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
  const fallback = validateModel(defaultModel, 'mimo-v2.5');
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
    mimoApiKey: process.env.MIMO_API_KEY || '',
    mimoBaseUrl: process.env.MIMO_BASE_URL || 'https://token-plan-cn.xiaomimimo.com/v1',
    defaultModel: process.env.SOLUM_DEFAULT_MODEL || 'mimo-v2.5',
    allowedModels: process.env.SOLUM_ALLOWED_MODELS || 'mimo-v2.5,mimo-v2.5-pro',
    standardPlan: process.env.SOLUM_STANDARD_PLAN || 'standard',
    standardMonthlyRequestLimit: Number(process.env.SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT || 1000),
    allowedOrigins: process.env.SOLUM_ALLOWED_ORIGINS || '',
    poolMax: Number(process.env.SOLUM_DB_POOL_MAX || 10),
    blobRetentionDays: Number(process.env.SOLUM_SYNC_RETENTION_DAYS || 30),
    alertRetentionDays: Number(process.env.SOLUM_ALERT_RETENTION_DAYS || 7),
    ...overrides,
  };
  if (config.authSecret.length < 32) throw new Error('SOLUM_AUTH_SECRET must be at least 32 characters');
  if (!config.adminUsername) throw new Error('SOLUM_ADMIN_USERNAME is required');
  if (!['closed', 'open'].includes(config.registrationMode)) {
    throw new Error('SOLUM_REGISTRATION_MODE must be closed or open');
  }
  if (!Number.isInteger(config.poolMax) || config.poolMax < 1 || config.poolMax > 100) {
    throw new Error('SOLUM_DB_POOL_MAX must be between 1 and 100');
  }
  if (!Number.isInteger(config.blobRetentionDays) || config.blobRetentionDays < 0 ||
    !Number.isInteger(config.alertRetentionDays) || config.alertRetentionDays < 1) {
    throw new Error('retention days must be whole non-negative values');
  }
  validateModel(config.defaultModel, 'mimo-v2.5');
  config.modelCatalog = parseModelCatalog(config.defaultModel, config.allowedModels);
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(config.standardPlan)) {
    throw new Error('SOLUM_STANDARD_PLAN must be a short identifier');
  }
  if (!Number.isInteger(config.standardMonthlyRequestLimit) ||
    config.standardMonthlyRequestLimit < 1 || config.standardMonthlyRequestLimit > 10000000) {
    throw new Error('SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT must be between 1 and 10000000');
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

async function issueSession(client, user, secret) {
  const refresh = crypto.randomBytes(48).toString('base64url');
  await client.query(
    "insert into auth.refresh_tokens(token_hash,user_id,expires_at) values ($1,$2,now()+interval '30 days')",
    [tokenHash(refresh), user.id]
  );
  return {
    access_token: signAccessToken(user, secret),
    refresh_token: refresh,
    user: { id: user.id, username: user.username },
  };
}

async function bootstrapAdmin(pool, config) {
  const existing = await pool.query('select id,username from auth.users where username=$1', [config.adminUsername]);
  if (existing.rowCount > 0) return;
  if (config.adminPassword.length < 12) {
    throw new Error('SOLUM_ADMIN_PASSWORD must be at least 12 characters when bootstrapping');
  }
  const salt = crypto.randomBytes(16).toString('hex');
  await pool.query(
    'insert into auth.users(username,password_hash,password_salt) values ($1,$2,$3) on conflict(username) do nothing',
    [config.adminUsername, passwordHash(config.adminPassword, salt), salt]
  );
}

async function cleanExpiredData(pool, config) {
  const users = await pool.query('select id from auth.users');
  for (const user of users.rows) {
    await withTenant(pool, user.id, async (client) => {
      if (config.blobRetentionDays > 0) {
        await client.query(
          "delete from sync.blobs where tenant_id=$1 and received_at<now()-($2::text||' days')::interval",
          [user.id, config.blobRetentionDays]
        );
      }
      await client.query(
        "delete from sync.alerts where tenant_id=$1 and received_at<now()-($2::text||' days')::interval",
        [user.id, config.alertRetentionDays]
      );
    });
  }
  await pool.query('delete from auth.refresh_tokens where expires_at<=now()');
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
  const limit = Number(entitlement.monthly_request_limit);
  const used = Number(row.request_count || 0);
  const periodStart = row.period_start ? new Date(row.period_start) : new Date();
  const resetAt = new Date(Date.UTC(periodStart.getUTCFullYear(), periodStart.getUTCMonth() + 1, 1));
  return {
    plan: entitlement.plan,
    custom_models_enabled: entitlement.custom_models_enabled !== false,
    monthly_request_limit: limit,
    requests_used: used,
    requests_remaining: Math.max(0, limit - used),
    reset_at: resetAt.toISOString(),
    success_count: Number(row.success_count || 0),
    failure_count: Number(row.failure_count || 0),
    fallback_count: Number(row.fallback_count || 0),
    prompt_tokens: Number(row.prompt_tokens || 0),
    completion_tokens: Number(row.completion_tokens || 0),
    total_tokens: Number(row.total_tokens || 0),
  };
}

async function readAiUsage(pool, tenantId, config) {
  return withTenant(pool, tenantId, async (client) => {
    const entitlement = await ensureEntitlement(client, tenantId, config);
    const selected = await client.query(
      `select period_start,request_count,success_count,failure_count,fallback_count,
              prompt_tokens,completion_tokens,total_tokens
       from ai.usage_monthly
       where tenant_id=$1 and period_start=date_trunc('month',now())::date`, [tenantId]
    );
    const recent = await client.query(
      `select request_id,requested_model,effective_model,status,fallback_occurred,fallback_reason,
              upstream_latency_ms,created_at,completed_at
       from ai.usage_events where tenant_id=$1 order by created_at desc limit 1`, [tenantId]
    );
    return { ...usageProjection(entitlement, selected.rows[0]), latest_request: recent.rows[0] || null };
  });
}

async function reserveAiRequest(pool, tenantId, requestedModel, config) {
  return withTenant(pool, tenantId, async (client) => {
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
       returning usage.period_start,usage.request_count,usage.success_count,usage.failure_count,
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
    const tokenCount = (value) => Number.isSafeInteger(value) && value >= 0 && value <= 2147483647 ?
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

async function requestUpstream(config, body, model, wantStream) {
  const startedAt = Date.now();
  try {
    const response = await fetch(`${config.mimoBaseUrl.replace(/\/+$/, '')}/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${config.mimoApiKey}` },
      body: JSON.stringify({ ...body, model }),
      signal: AbortSignal.timeout(wantStream ? 300000 : 60000),
    });
    return { response, latencyMs: Date.now() - startedAt };
  } catch {
    return { response: null, latencyMs: Date.now() - startedAt };
  }
}

async function proxyCompletion(req, res, pool, tenantId, config) {
  if (!config.mimoApiKey) throw new HttpError(503, 'ai_not_configured');
  const body = await readJson(req);
  const wantStream = body.stream === true;
  const requestedModel = validateModel(body.model, config.defaultModel);
  const upstreamBody = {
    messages: validateMessages(body.messages),
    stream: wantStream,
  };
  if (typeof body.temperature === 'number') upstreamBody.temperature = body.temperature;
  if (Number.isInteger(body.max_tokens) && body.max_tokens > 0) upstreamBody.max_tokens = body.max_tokens;
  const reservation = await reserveAiRequest(pool, tenantId, requestedModel, config);
  let effectiveModel = reservation.effectiveModel;
  let fallbackReason = reservation.fallbackReason;
  let attempt = await requestUpstream(config, upstreamBody, effectiveModel, wantStream);
  let totalLatency = attempt.latencyMs;
  if ((!attempt.response || !attempt.response.ok) && effectiveModel !== config.modelCatalog.defaultModel) {
    if (attempt.response && attempt.response.body) await attempt.response.body.cancel().catch(() => {});
    fallbackReason = attempt.response ? `upstream_http_${attempt.response.status}` : 'upstream_unavailable';
    effectiveModel = config.modelCatalog.defaultModel;
    attempt = await requestUpstream(config, upstreamBody, effectiveModel, wantStream);
    totalLatency += attempt.latencyMs;
  }
  const upstream = attempt.response;
  if (!upstream || !upstream.ok) {
    if (upstream && upstream.body) await upstream.body.cancel().catch(() => {});
    await finalizeAiRequest(pool, tenantId, reservation, {
      status: 'failed', effectiveModel, fallbackReason, latencyMs: totalLatency,
    });
    throw new HttpError(502, 'upstream_unavailable');
  }
  const responseHeaders = aiResponseHeaders(reservation, effectiveModel, fallbackReason);
  if (wantStream) {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream; charset=utf-8',
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      ...responseHeaders,
    });
    try {
      for await (const chunk of upstream.body) res.write(chunk);
    } catch {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: 'interrupted', effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
      res.destroy();
      return;
    }
    await finalizeAiRequest(pool, tenantId, reservation, {
      status: 'succeeded', effectiveModel, fallbackReason, latencyMs: totalLatency,
    });
    res.end();
    return;
  }
  let response;
  try {
    response = await upstream.text();
  } catch {
    await finalizeAiRequest(pool, tenantId, reservation, {
      status: 'interrupted', effectiveModel, fallbackReason, latencyMs: totalLatency,
    });
    throw new HttpError(502, 'upstream_interrupted');
  }
  let usage;
  try {
    const parsed = JSON.parse(response);
    if (parsed && parsed.usage && typeof parsed.usage === 'object') usage = parsed.usage;
  } catch { /* Preserve an upstream-compatible response even when usage is absent. */ }
  await finalizeAiRequest(pool, tenantId, reservation, {
    status: 'succeeded', effectiveModel, fallbackReason, latencyMs: totalLatency, usage,
  });
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
  cleanExpiredData(pool, config).catch((error) => console.error('retention cleanup failed', error));
  const cleanupTimer = setInterval(() => {
    cleanExpiredData(pool, config).catch((error) => console.error('retention cleanup failed', error));
  }, 6 * 60 * 60 * 1000);
  cleanupTimer.unref();
  const loginFailures = new Map();

  function authenticate(req) {
    const tenantId = verifyAccessToken(bearer(req), config.authSecret);
    if (!tenantId) throw new HttpError(401, 'unauthorized');
    return tenantId;
  }

  function loginKey(req, username) {
    return `${req.socket.remoteAddress || 'unknown'}|${username}`;
  }

  function isRateLimited(key) {
    const record = loginFailures.get(key);
    if (!record || Date.now() - record.startedAt > LOGIN_WINDOW_MS) {
      loginFailures.delete(key);
      return false;
    }
    return record.count >= LOGIN_MAX_FAILURES;
  }

  function recordLoginFailure(key) {
    const record = loginFailures.get(key);
    if (!record || Date.now() - record.startedAt > LOGIN_WINDOW_MS) {
      loginFailures.set(key, { count: 1, startedAt: Date.now() });
    } else {
      record.count += 1;
    }
  }

  async function handle(req, res) {
    const origin = req.headers.origin;
    if (typeof origin === 'string' && config.allowedOrigins.has(origin)) {
      res.setHeader('Access-Control-Allow-Origin', origin);
      res.setHeader('Vary', 'Origin');
      res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization, X-Device');
      res.setHeader('Access-Control-Allow-Methods', 'GET, POST, PUT, OPTIONS');
      res.setHeader('Access-Control-Expose-Headers',
        'X-Solum-Request-Id, X-Solum-Requested-Model, X-Solum-Effective-Model, ' +
        'X-Solum-Fallback, X-Solum-Fallback-Reason, X-Solum-Quota-Limit, ' +
        'X-Solum-Quota-Used, X-Solum-Quota-Remaining, X-Solum-Quota-Reset');
    }
    if (req.method === 'OPTIONS') {
      if (typeof origin !== 'string' || !config.allowedOrigins.has(origin)) {
        throw new HttpError(403, 'origin_not_allowed');
      }
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
        api_base: '/',
        registration: config.registrationMode,
        features: ['account', 'ai', 'ai_quota', 'sync', 'alerts', 'devices'],
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/health') {
      await pool.query('select 1');
      return json(res, 200, { status: 'ok', storage: 'postgres' });
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/register') {
      if (config.registrationMode !== 'open') throw new HttpError(403, 'registration_closed');
      const body = await readJson(req);
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const password = requiredString(body.password, 'invalid_credentials', 1024);
      if (password.length < 12 || /[\u0000-\u001f\u007f]/.test(username)) {
        throw new HttpError(400, 'invalid_credentials');
      }
      const salt = crypto.randomBytes(16).toString('hex');
      const client = await pool.connect();
      try {
        await client.query('begin');
        const inserted = await client.query(
          'insert into auth.users(username,password_hash,password_salt) values($1,$2,$3) returning id,username',
          [username, passwordHash(password, salt), salt]
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
      const password = requiredString(body.password, 'invalid_credentials', 1024);
      const key = loginKey(req, username);
      if (isRateLimited(key)) throw new HttpError(429, 'too_many_attempts');
      const selected = await pool.query(
        "select id,username,password_hash,password_salt from auth.users where username=$1 and status='active'",
        [username]
      );
      const user = selected.rows[0];
      const candidate = user ? passwordHash(password, user.password_salt) :
        passwordHash(password, crypto.randomBytes(16).toString('hex'));
      if (!user || !secureEqualHex(candidate, user.password_hash)) {
        recordLoginFailure(key);
        throw new HttpError(401, 'invalid_credentials');
      }
      loginFailures.delete(key);
      const client = await pool.connect();
      try {
        await client.query('begin');
        await client.query('delete from auth.refresh_tokens where expires_at<=now()');
        const session = await issueSession(client, user, config.authSecret);
        await client.query('commit');
        return json(res, 200, session);
      } catch (error) {
        await client.query('rollback').catch(() => {});
        throw error;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/refresh') {
      const body = await readJson(req);
      const hash = tokenHash(requiredString(body.refresh_token, 'invalid_refresh_token', 512));
      const client = await pool.connect();
      try {
        await client.query('begin');
        const selected = await client.query(
          'select rt.user_id,rt.expires_at,u.username from auth.refresh_tokens rt join auth.users u on u.id=rt.user_id where rt.token_hash=$1 for update',
          [hash]
        );
        const row = selected.rows[0];
        if (!row || new Date(row.expires_at).getTime() <= Date.now()) {
          await client.query('delete from auth.refresh_tokens where token_hash=$1', [hash]);
          await client.query('commit');
          throw new HttpError(401, 'invalid_refresh_token');
        }
        await client.query('delete from auth.refresh_tokens where token_hash=$1', [hash]);
        const session = await issueSession(client,
          { id: row.user_id, username: row.username }, config.authSecret);
        await client.query('commit');
        return json(res, 200, session);
      } catch (error) {
        if (!(error instanceof HttpError)) await client.query('rollback').catch(() => {});
        throw error;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/logout') {
      authenticate(req);
      const body = await readJson(req);
      if (typeof body.refresh_token === 'string') {
        await pool.query('delete from auth.refresh_tokens where token_hash=$1', [tokenHash(body.refresh_token)]);
      }
      return json(res, 200, { status: 'ok' });
    }

    if (req.method === 'GET' && url.pathname === '/v1/ai/models') {
      const tenantId = authenticate(req);
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
      const tenantId = authenticate(req);
      return json(res, 200, await readAiUsage(pool, tenantId, config));
    }

    if (req.method === 'POST' && url.pathname === '/v1/ai/chat/completions') {
      const tenantId = authenticate(req);
      return proxyCompletion(req, res, pool, tenantId, config);
    }

    if (req.method === 'POST' && url.pathname === '/v1/push') {
      const tenantId = authenticate(req);
      const device = requiredString(req.headers['x-device'], 'invalid_device', 128);
      const blob = await readBytes(req, MAX_BLOB_BYTES);
      if (blob.length === 0) throw new HttpError(400, 'bad_blob');
      const seq = await withTenant(pool, tenantId, async (client) => {
        const activeDevice = await client.query(
          `insert into sync.devices(tenant_id,device_id,last_seen_at)
           values($1,$2,now()) on conflict(tenant_id,device_id)
           do update set last_seen_at=excluded.last_seen_at
           where sync.devices.revoked_at is null returning device_id`,
          [tenantId, device]
        );
        if (activeDevice.rowCount !== 1) throw new HttpError(403, 'device_revoked');
        const inserted = await client.query(
          'insert into sync.blobs(tenant_id,device_id,ciphertext) values($1,$2,$3) returning seq',
          [tenantId, device, blob]
        );
        return Number(inserted.rows[0].seq);
      });
      return json(res, 200, { seq });
    }

    if (req.method === 'GET' && url.pathname === '/v1/pull') {
      const tenantId = authenticate(req);
      const since = parseNonNegativeInt(url.searchParams.get('since'), 0);
      const device = requiredString(url.searchParams.get('device'), 'invalid_device', 128);
      const result = await withTenant(pool, tenantId, async (client) => {
        const activeDevice = await client.query(
          `insert into sync.devices(tenant_id,device_id,last_seen_at)
           values($1,$2,now()) on conflict(tenant_id,device_id)
           do update set last_seen_at=excluded.last_seen_at
           where sync.devices.revoked_at is null returning device_id`,
          [tenantId, device]
        );
        if (activeDevice.rowCount !== 1) throw new HttpError(403, 'device_revoked');
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
          oldest_seq: Number(oldest.rows[0].oldest_seq),
          blobs: pulled.rows.map((row) => ({
            seq: Number(row.seq), device: row.device_id, blob: row.ciphertext.toString('base64'),
          })),
        };
      });
      return json(res, 200, result);
    }

    if (req.method === 'GET' && url.pathname === '/v1/stats') {
      const tenantId = authenticate(req);
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
          total_blobs: Number(row.total_blobs), total_bytes: Number(row.total_bytes),
          oldest_seq: Number(row.oldest_seq), newest_seq: Number(row.newest_seq),
          devices: devices.rows.map((item) => ({
            device: item.device_id, blob_count: Number(item.blob_count), bytes: Number(item.bytes),
            last_seq: Number(item.last_seq), last_received_at: item.last_received_at,
          })),
          tenant: tenantId, auth_mode: 'account',
        };
      });
      return json(res, 200, stats);
    }

    if (req.method === 'POST' && url.pathname === '/v1/devices/register') {
      const tenantId = authenticate(req);
      const body = await readJson(req);
      const device = requiredString(body.device_id, 'invalid_device', 128);
      const publicKey = body.public_key === undefined || body.public_key === null ? null :
        decodeBase64(body.public_key, 'invalid_device_key', 256);
      const protocolVersion = body.protocol_version === undefined ? 1 : body.protocol_version;
      if ((publicKey !== null && (publicKey.length < 32 || publicKey.length > 128)) ||
        !Number.isInteger(protocolVersion) ||
        protocolVersion < 1 || protocolVersion > 32767) throw new HttpError(400, 'invalid_device');
      await withTenant(pool, tenantId, async (client) => {
        const registered = await client.query(
          `insert into sync.devices(tenant_id,device_id,public_key,protocol_version,last_seen_at)
           values($1,$2,$3,$4,now()) on conflict(tenant_id,device_id)
           do update set public_key=coalesce(excluded.public_key,sync.devices.public_key),
                         protocol_version=excluded.protocol_version,last_seen_at=excluded.last_seen_at
           where sync.devices.revoked_at is null returning device_id`,
          [tenantId, device, publicKey, protocolVersion]
        );
        if (registered.rowCount !== 1) throw new HttpError(403, 'device_revoked');
      });
      return json(res, 200, { status: 'ok' });
    }

    if (req.method === 'GET' && url.pathname === '/v1/devices') {
      const tenantId = authenticate(req);
      const devices = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select device_id,public_key,protocol_version,last_seen_at,revoked_at
           from sync.devices where tenant_id=$1 order by last_seen_at desc`, [tenantId]
        );
        return result.rows.map((row) => ({ ...row,
          public_key: row.public_key === null ? null : row.public_key.toString('base64') }));
      });
      return json(res, 200, { devices });
    }

    if (req.method === 'POST' && url.pathname === '/v1/alerts') {
      const tenantId = authenticate(req);
      const body = await readJson(req);
      const alert = {
        eventId: requiredString(body.event_id, 'invalid_alert', 256),
        source: requiredString(body.source, 'invalid_alert', 64),
        monitorId: optionalString(body.monitor_id, 'invalid_alert', 256),
        name: optionalString(body.name, 'invalid_alert', 256),
        status: requiredString(body.status, 'invalid_alert', 64),
        latencyMs: optionalNumber(body.latency_ms, 'invalid_alert'),
        pingLatencyMs: optionalNumber(body.ping_latency_ms, 'invalid_alert'),
        availability7d: optionalNumber(body.availability_7d, 'invalid_alert'),
        checkedAt: optionalString(body.checked_at, 'invalid_alert', 128),
        detailUrl: optionalString(body.detail_url, 'invalid_alert', 2048),
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
        if (inserted.rowCount > 0) return Number(inserted.rows[0].seq);
        const existing = await client.query(
          'select seq from sync.alerts where tenant_id=$1 and event_id=$2', [tenantId, alert.eventId]
        );
        return Number(existing.rows[0].seq);
      });
      return json(res, 200, { seq });
    }

    if (req.method === 'GET' && url.pathname === '/v1/alerts') {
      const tenantId = authenticate(req);
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
        return result.rows.map((row) => ({ ...row, seq: Number(row.seq),
          latency_ms: row.latency_ms === null ? null : Number(row.latency_ms),
          ping_latency_ms: row.ping_latency_ms === null ? null : Number(row.ping_latency_ms),
          availability_7d: row.availability_7d === null ? null : Number(row.availability_7d) }));
      });
      return json(res, 200, { alerts });
    }

    if (req.method === 'GET' && url.pathname === '/v1/keys/recovery') {
      const tenantId = authenticate(req);
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
      const tenantId = authenticate(req);
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

    if (req.method === 'POST' && url.pathname === '/v1/keys/envelopes') {
      const tenantId = authenticate(req);
      const body = await readJson(req);
      const device = requiredString(body.recipient_device_id, 'invalid_envelope', 128);
      const algorithm = requiredString(body.algorithm, 'invalid_envelope', 64);
      if (device === 'account-recovery-v1' || algorithm !== 'x25519-xchacha20poly1305' ||
        !Number.isInteger(body.key_version) || body.key_version < 1) {
        throw new HttpError(400, 'invalid_envelope');
      }
      const envelope = decodeBase64(body.envelope, 'invalid_envelope', 100000);
      if (envelope.length === 0 || envelope.length > 65536) throw new HttpError(400, 'invalid_envelope');
      const id = await withTenant(pool, tenantId, async (client) => {
        const saved = await client.query(
          `insert into vault.key_envelopes(tenant_id,recipient_device_id,key_version,algorithm,envelope)
           values($1,$2,$3,$4,$5)
           on conflict(tenant_id,recipient_device_id,key_version,algorithm)
           do update set envelope=excluded.envelope,created_at=now(),revoked_at=null returning id`,
          [tenantId, device, body.key_version, algorithm, envelope]
        );
        return saved.rows[0].id;
      });
      return json(res, 200, { id });
    }

    if (req.method === 'GET' && url.pathname === '/v1/keys/envelopes') {
      const tenantId = authenticate(req);
      const device = requiredString(url.searchParams.get('device'), 'invalid_device', 128);
      if (device === 'account-recovery-v1') throw new HttpError(400, 'invalid_device');
      const envelopes = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select id,recipient_device_id,key_version,algorithm,envelope,created_at
           from vault.key_envelopes where tenant_id=$1 and recipient_device_id=$2 and revoked_at is null
           order by key_version desc`, [tenantId, device]
        );
        return result.rows.map((row) => ({ ...row, envelope: row.envelope.toString('base64') }));
      });
      return json(res, 200, { envelopes });
    }

    throw new HttpError(404, 'not_found');
  }

  const server = http.createServer((req, res) => {
    handle(req, res).catch((error) => {
      if (error instanceof HttpError) return json(res, error.status, { error: error.code });
      console.error(error);
      if (!res.headersSent) return json(res, 500, { error: 'internal_error' });
      res.destroy();
    });
  });

  return {
    server,
    pool,
    async close() {
      clearInterval(cleanupTimer);
      await new Promise((resolve) => server.close(resolve));
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
    console.error(error);
    process.exit(1);
  });
}

module.exports = {
  createPostgresServer, verifyAccessToken, withTenant, parseModelCatalog, usageProjection,
};

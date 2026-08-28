'use strict';

const http = require('node:http');
const crypto = require('node:crypto');
const path = require('node:path');
const fs = require('node:fs');
const { DatabaseSync } = require('node:sqlite');
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
  verifyPasswordOrDummyAsync,
} = require('./http-security');

const ACCESS_TTL_SECONDS = 15 * 60;
const REFRESH_TTL_SECONDS = 30 * 24 * 60 * 60;
const MAX_BODY_BYTES = 1024 * 1024;
const DEFAULT_AI_RESPONSE_BYTES = 8 * 1024 * 1024;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;
const LOGIN_MAX_FAILURES = 5;
const LOGIN_IP_MAX_FAILURES = 50;
const MAX_CONCURRENT_AUTH_KDFS = 8;
const MAX_REFRESH_SESSIONS_PER_USER = 32;
const REFRESH_REPLAY_GRACE_SECONDS = 5;
const SERVICE_VERSION = require('../package.json').version;

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

const DASHBOARD = fs.readFileSync(path.join(__dirname, 'dashboard.html'));

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

function parseAllowedOrigins(value) {
  const origins = Array.isArray(value) ? value : String(value || '').split(',');
  return new Set(origins.map((item) => item.trim()).filter(Boolean).map((item) => {
    try {
      return new URL(item).origin;
    } catch {
      throw new Error(`invalid SOLUM_ALLOWED_ORIGINS entry: ${item}`);
    }
  }));
}

async function readJson(req) {
  if (!isJsonContentType(req)) throw new HttpError(415, 'unsupported_media_type');
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_BODY_BYTES) {
      throw new HttpError(413, 'request_too_large');
    }
    chunks.push(chunk);
  }
  try {
    const parsed = JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new HttpError(400, 'invalid_json');
    }
    return parsed;
  } catch {
    throw new HttpError(400, 'invalid_json');
  }
}

class HttpError extends Error {
  constructor(status, code) {
    super(code);
    this.status = status;
    this.code = code;
  }
}

function requiredString(value, code, max = 256) {
  if (typeof value !== 'string' || value.trim().length === 0 || value.length > max) {
    throw new HttpError(400, code);
  }
  return value.trim();
}

function requiredOpaqueString(value, code, max) {
  if (typeof value !== 'string' || value.length === 0 || value.length > max) {
    throw new HttpError(400, code);
  }
  return value;
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

function waitForDrain(res) {
  return new Promise((resolve, reject) => {
    if (res.destroyed || res.writableEnded) {
      reject(new Error('client_disconnected'));
      return;
    }
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

function passwordHashSync(password, salt) {
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
  return crypto.createHash('sha256').update(token).digest('hex');
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
    if (typeof payload.sub !== 'string' ||
      !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(payload.sub) ||
      !Number.isInteger(payload.exp) || payload.exp <= nowSeconds()) {
      return null;
    }
    return payload.sub;
  } catch {
    return null;
  }
}

function bearer(req) {
  const header = req.headers.authorization;
  if (typeof header !== 'string' || !header.startsWith('Bearer ')) return null;
  return header.slice(7);
}

function validateModel(value, fallback) {
  const model = value === undefined ? fallback : requiredString(value, 'invalid_model', 100);
  if (!/^[A-Za-z0-9._:/-]+$/.test(model)) {
    throw new HttpError(400, 'invalid_model');
  }
  return model;
}

function validateMessages(value) {
  if (!Array.isArray(value) || value.length === 0 || value.length > 16) {
    throw new HttpError(400, 'invalid_messages');
  }
  return value.map((item) => {
    if (item === null || typeof item !== 'object' || Array.isArray(item)) {
      throw new HttpError(400, 'invalid_messages');
    }
    const role = item.role;
    const content = item.content;
    if (!['system', 'user', 'assistant'].includes(role) ||
      typeof content !== 'string' || content.length === 0 || content.length > 32000) {
      throw new HttpError(400, 'invalid_messages');
    }
    return { role, content };
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
  return { stream: body.stream === true, temperature: body.temperature, maxTokens: body.max_tokens };
}

async function readResponseBodyLimited(response, maxBytes) {
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

function buildConfig(overrides = {}) {
  const config = {
    port: Number(process.env.SOLUM_PORT || process.env.PA_PORT || 8787),
    dbPath: process.env.SOLUM_DB_PATH || process.env.PA_DB_PATH || path.join(process.cwd(), 'data', 'solum-cloud.db'),
    authSecret: process.env.SOLUM_AUTH_SECRET || process.env.PA_AUTH_SECRET || '',
    adminUsername: process.env.SOLUM_ADMIN_USERNAME || process.env.PA_ADMIN_USERNAME || '',
    adminPassword: process.env.SOLUM_ADMIN_PASSWORD || process.env.PA_ADMIN_PASSWORD || '',
    mimoApiKey: process.env.MIMO_API_KEY || '',
    mimoBaseUrl: process.env.MIMO_BASE_URL || 'https://token-plan-cn.xiaomimimo.com/v1',
    defaultModel: process.env.SOLUM_DEFAULT_MODEL || process.env.PA_DEFAULT_MODEL || 'mimo-v2.5',
    allowedOrigins: process.env.SOLUM_ALLOWED_ORIGINS || '',
    trustProxy: process.env.SOLUM_TRUST_PROXY === 'true',
    aiMaxResponseBytes: Number(process.env.SOLUM_AI_MAX_RESPONSE_BYTES || DEFAULT_AI_RESPONSE_BYTES),
    ...overrides,
  };
  if (config.authSecret.length < 32) throw new Error('SOLUM_AUTH_SECRET must be at least 32 characters');
  if (typeof config.adminUsername !== 'string') throw new Error('SOLUM_ADMIN_USERNAME is required');
  config.adminUsername = config.adminUsername.trim();
  if (!config.adminUsername || config.adminUsername.length > 128 ||
      /[\u0000-\u001f\u007f]/.test(config.adminUsername)) {
    throw new Error('SOLUM_ADMIN_USERNAME must be a valid username');
  }
  validateModel(config.defaultModel, 'mimo-v2.5');
  if (!Number.isSafeInteger(config.aiMaxResponseBytes) ||
    config.aiMaxResponseBytes < 1024 || config.aiMaxResponseBytes > 64 * 1024 * 1024) {
    throw new Error('SOLUM_AI_MAX_RESPONSE_BYTES must be between 1024 and 67108864');
  }
  config.allowedOrigins = parseAllowedOrigins(config.allowedOrigins);
  return config;
}

function openDatabase(config) {
  fs.mkdirSync(path.dirname(config.dbPath), { recursive: true });
  const db = new DatabaseSync(config.dbPath);
  db.exec(`
    PRAGMA journal_mode=WAL;
    PRAGMA foreign_keys=ON;
    PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS users (
      id TEXT NOT NULL UNIQUE,
      username TEXT PRIMARY KEY,
      password_hash TEXT NOT NULL,
      password_salt TEXT NOT NULL,
      password_version INTEGER NOT NULL DEFAULT 2 CHECK (password_version IN (1, 2)),
      created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS refresh_tokens (
      token_hash TEXT PRIMARY KEY,
      user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      session_id TEXT NOT NULL,
      previous_token_hash TEXT,
      legacy_token_hash TEXT,
      rotated_at INTEGER,
      expires_at INTEGER NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_refresh_expiry ON refresh_tokens(expires_at);
  `);
  const userColumns = db.prepare('PRAGMA table_info(users)').all();
  if (!userColumns.some((column) => column.name === 'id')) {
    db.exec('ALTER TABLE users ADD COLUMN id TEXT');
  }
  if (!userColumns.some((column) => column.name === 'password_version')) {
    // Existing rows can come from trimmed registration or exact bootstrap.
    // Keep them at v1: login alone cannot prove which whitespace variant is
    // bound to an existing client-side recovery envelope.
    db.exec('ALTER TABLE users ADD COLUMN password_version INTEGER NOT NULL DEFAULT 1');
  }
  // Repair as well as migrate: if a previous process stopped after ADD COLUMN,
  // the next startup still fills every missing immutable id before continuing.
  const rows = db.prepare("SELECT username FROM users WHERE id IS NULL OR id = ''").all();
  const assignId = db.prepare('UPDATE users SET id = ? WHERE username = ?');
  for (const row of rows) assignId.run(crypto.randomUUID(), row.username);
  db.exec('CREATE UNIQUE INDEX IF NOT EXISTS idx_users_id ON users(id)');
  let refreshColumns = db.prepare('PRAGMA table_info(refresh_tokens)').all();
  if (!refreshColumns.some((column) => column.name === 'user_id')) {
    db.exec(`
      BEGIN IMMEDIATE;
      DROP INDEX IF EXISTS idx_refresh_expiry;
      ALTER TABLE refresh_tokens RENAME TO refresh_tokens_by_username;
      CREATE TABLE refresh_tokens (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        session_id TEXT,
        previous_token_hash TEXT,
        legacy_token_hash TEXT,
        rotated_at INTEGER,
        expires_at INTEGER NOT NULL,
        created_at INTEGER NOT NULL
      );
      INSERT INTO refresh_tokens(
        token_hash, user_id, session_id, previous_token_hash, legacy_token_hash,
        rotated_at, expires_at, created_at
      )
      SELECT rt.token_hash, u.id, NULL, NULL, rt.token_hash, NULL,
        rt.expires_at, rt.created_at
      FROM refresh_tokens_by_username rt JOIN users u ON u.username = rt.username;
      DROP TABLE refresh_tokens_by_username;
      CREATE INDEX idx_refresh_expiry ON refresh_tokens(expires_at);
      COMMIT;
    `);
  }
  refreshColumns = db.prepare('PRAGMA table_info(refresh_tokens)').all();
  if (!refreshColumns.some((column) => column.name === 'session_id')) {
    db.exec('ALTER TABLE refresh_tokens ADD COLUMN session_id TEXT');
  }
  if (!refreshColumns.some((column) => column.name === 'previous_token_hash')) {
    db.exec('ALTER TABLE refresh_tokens ADD COLUMN previous_token_hash TEXT');
  }
  if (!refreshColumns.some((column) => column.name === 'legacy_token_hash')) {
    db.exec('ALTER TABLE refresh_tokens ADD COLUMN legacy_token_hash TEXT');
    db.exec('UPDATE refresh_tokens SET legacy_token_hash = token_hash');
  }
  if (!refreshColumns.some((column) => column.name === 'rotated_at')) {
    db.exec('ALTER TABLE refresh_tokens ADD COLUMN rotated_at INTEGER');
  }
  const sessionIds = new Set();
  const repairSessionId = db.prepare(
    'UPDATE refresh_tokens SET session_id = ? WHERE token_hash = ?'
  );
  for (const row of db.prepare('SELECT token_hash, session_id FROM refresh_tokens').all()) {
    const valid = typeof row.session_id === 'string' &&
      /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(row.session_id) &&
      !sessionIds.has(row.session_id);
    const sessionId = valid ? row.session_id : crypto.randomUUID();
    if (!valid) repairSessionId.run(sessionId, row.token_hash);
    sessionIds.add(sessionId);
  }
  db.exec('CREATE UNIQUE INDEX IF NOT EXISTS idx_refresh_session ON refresh_tokens(session_id)');
  db.exec(`CREATE INDEX IF NOT EXISTS idx_refresh_previous
    ON refresh_tokens(previous_token_hash) WHERE previous_token_hash IS NOT NULL`);
  db.exec(`CREATE INDEX IF NOT EXISTS idx_refresh_legacy
    ON refresh_tokens(legacy_token_hash) WHERE legacy_token_hash IS NOT NULL`);
  const now = nowSeconds();
  db.prepare('DELETE FROM refresh_tokens WHERE expires_at <= ?').run(now);
  const trimSessions = db.prepare(
    `DELETE FROM refresh_tokens WHERE user_id = ? AND session_id IN (
       SELECT session_id FROM refresh_tokens WHERE user_id = ?
       ORDER BY created_at DESC, session_id DESC LIMIT -1 OFFSET ?
     )`
  );
  for (const row of db.prepare('SELECT DISTINCT user_id FROM refresh_tokens').all()) {
    trimSessions.run(row.user_id, row.user_id, MAX_REFRESH_SESSIONS_PER_USER);
  }
  const existing = db.prepare('SELECT id, username FROM users WHERE username = ?').get(config.adminUsername);
  if (!existing) {
    if (config.adminPassword.length < 12) {
      db.close();
      throw new Error('SOLUM_ADMIN_PASSWORD must be at least 12 characters when bootstrapping');
    }
    const salt = crypto.randomBytes(16).toString('hex');
    db.prepare(
      `INSERT INTO users(
         id, username, password_hash, password_salt, password_version, created_at
       ) VALUES (?, ?, ?, ?, 2, ?)`
    ).run(crypto.randomUUID(), config.adminUsername,
      passwordHashSync(config.adminPassword, salt), salt, nowSeconds());
  }
  return db;
}

function issueSession(db, user, secret) {
  const sessionId = crypto.randomUUID();
  const refresh = createRefreshToken(sessionId, secret);
  const now = nowSeconds();
  db.prepare('DELETE FROM refresh_tokens WHERE user_id = ? AND expires_at <= ?').run(user.id, now);
  db.prepare(
    `DELETE FROM refresh_tokens WHERE user_id = ? AND session_id IN (
       SELECT session_id FROM refresh_tokens WHERE user_id = ?
       ORDER BY created_at DESC, session_id DESC LIMIT -1 OFFSET ?
     )`
  ).run(user.id, user.id, MAX_REFRESH_SESSIONS_PER_USER - 1);
  db.prepare(
    `INSERT INTO refresh_tokens(
       token_hash, user_id, session_id, previous_token_hash, legacy_token_hash,
       rotated_at, expires_at, created_at
     ) VALUES (?, ?, ?, NULL, NULL, NULL, ?, ?)`
  ).run(tokenHash(refresh), user.id, sessionId, now + REFRESH_TTL_SECONDS, now);
  return {
    access_token: signAccessToken(user, secret),
    refresh_token: refresh,
    user: { id: user.id, username: user.username },
  };
}

function createPaServer(overrides = {}) {
  const config = buildConfig(overrides);
  const db = openDatabase(config);
  const loginFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: LOGIN_MAX_FAILURES,
  });
  const loginIpFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: LOGIN_IP_MAX_FAILURES,
  });
  const authKdfConcurrency = createConcurrencyLimiter(MAX_CONCURRENT_AUTH_KDFS);

  function authenticate(req) {
    const username = verifyAccessToken(bearer(req), config.authSecret);
    if (username === null) throw new HttpError(401, 'unauthorized');
    return username;
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
      res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization');
      res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
    }
    if (req.method === 'OPTIONS') {
      if (typeof origin !== 'string') throw new HttpError(403, 'origin_not_allowed');
      res.writeHead(204, { 'Content-Length': '0', 'Cache-Control': 'no-store' });
      return res.end();
    }
    if (req.method === 'GET' && req.url === '/v1/health') {
      return json(res, 200, { status: 'ok', storage: 'sqlite', version: SERVICE_VERSION });
    }
    if (req.method === 'GET' && (req.url === '/' || req.url === '/index.html')) {
      return html(res, DASHBOARD);
    }
    if (req.method === 'GET' && req.url === '/v1/meta') {
      return json(res, 200, {
        service: 'solum-cloud',
        version: SERVICE_VERSION,
        registration: config.registrationMode || 'open',
      });
    }

    if (req.method === 'POST' && req.url === '/v1/auth/login') {
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
        const user = db.prepare(
          `SELECT id, username, password_hash, password_salt, password_version
           FROM users WHERE username = ?`
        ).get(username);
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
        db.exec('BEGIN IMMEDIATE');
        let session;
        try {
          // Keep version-1 hashes stable. A login alone cannot prove which
          // whitespace variant is bound to an existing recovery envelope.
          db.prepare('DELETE FROM refresh_tokens WHERE expires_at <= ?').run(nowSeconds());
          session = issueSession(db, user, config.authSecret);
          db.exec('COMMIT');
        } catch (error) {
          db.exec('ROLLBACK');
          throw error;
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

    if (req.method === 'POST' && req.url === '/v1/auth/refresh') {
      const body = await readJson(req);
      const refresh = requiredOpaqueString(body.refresh_token, 'invalid_refresh_token', 512);
      const hash = tokenHash(refresh);
      const sessionId = refreshTokenSessionId(refresh, config.authSecret);
      db.exec('BEGIN IMMEDIATE');
      try {
        const row = sessionId ? db.prepare(
          `SELECT rt.token_hash, rt.previous_token_hash, rt.legacy_token_hash,
             rt.rotated_at, rt.session_id, rt.user_id, rt.expires_at, u.username
           FROM refresh_tokens rt JOIN users u ON u.id = rt.user_id
           WHERE rt.session_id = ?`
        ).get(sessionId) : db.prepare(
          `SELECT rt.token_hash, rt.previous_token_hash, rt.legacy_token_hash,
             rt.rotated_at, rt.session_id, rt.user_id, rt.expires_at, u.username
           FROM refresh_tokens rt JOIN users u ON u.id = rt.user_id
           WHERE rt.token_hash = ? OR rt.previous_token_hash = ? OR rt.legacy_token_hash = ?`
        ).get(hash, hash, hash);
        const now = nowSeconds();
        if (!row || row.expires_at <= now) {
          if (row) db.prepare('DELETE FROM refresh_tokens WHERE session_id = ?').run(row.session_id);
          db.exec('COMMIT');
          throw new HttpError(401, 'invalid_refresh_token');
        }
        if (!secureEqualHex(row.token_hash, hash)) {
          const grace = secureEqualHex(row.previous_token_hash, hash) &&
            Number.isInteger(row.rotated_at) && now >= row.rotated_at &&
            now - row.rotated_at <= REFRESH_REPLAY_GRACE_SECONDS;
          if (grace) {
            const recoveredRefresh = createRefreshSuccessorToken(
              row.session_id, refresh, config.authSecret
            );
            if (secureEqualHex(row.token_hash, tokenHash(recoveredRefresh))) {
              const recoveredSession = {
                access_token: signAccessToken(
                  { id: row.user_id, username: row.username }, config.authSecret
                ),
                refresh_token: recoveredRefresh,
                user: { id: row.user_id, username: row.username },
              };
              db.exec('COMMIT');
              return json(res, 200, recoveredSession);
            }
          }
          if (!grace) {
            db.prepare('DELETE FROM refresh_tokens WHERE session_id = ?').run(row.session_id);
          }
          db.exec('COMMIT');
          throw new HttpError(401, 'invalid_refresh_token');
        }
        const rotatedRefresh = createRefreshSuccessorToken(
          row.session_id, refresh, config.authSecret
        );
        const updated = db.prepare(
          `UPDATE refresh_tokens SET token_hash = ?, previous_token_hash = ?,
             rotated_at = ?, expires_at = ?, created_at = ?
           WHERE session_id = ? AND token_hash = ?`
        ).run(tokenHash(rotatedRefresh), hash, now, now + REFRESH_TTL_SECONDS, now,
          row.session_id, hash);
        if (updated.changes !== 1) {
          db.exec('ROLLBACK');
          throw new HttpError(401, 'invalid_refresh_token');
        }
        const session = {
          access_token: signAccessToken({ id: row.user_id, username: row.username }, config.authSecret),
          refresh_token: rotatedRefresh,
          user: { id: row.user_id, username: row.username },
        };
        db.exec('COMMIT');
        return json(res, 200, session);
      } catch (error) {
        if (db.isTransaction) db.exec('ROLLBACK');
        throw error;
      }
    }

    if (req.method === 'POST' && req.url === '/v1/auth/logout') {
      const body = await readJson(req);
      const refresh = requiredOpaqueString(body.refresh_token, 'invalid_refresh_token', 512);
      const sessionId = refreshTokenSessionId(refresh, config.authSecret);
      if (sessionId) {
        db.prepare('DELETE FROM refresh_tokens WHERE session_id = ?').run(sessionId);
      } else {
        const hash = tokenHash(refresh);
        db.prepare(
          `DELETE FROM refresh_tokens WHERE token_hash = ? OR previous_token_hash = ?
             OR legacy_token_hash = ?`
        ).run(hash, hash, hash);
      }
      return json(res, 200, { status: 'ok' });
    }

    if (req.method === 'POST' && req.url === '/v1/ai/chat/completions') {
      authenticate(req);
      if (!config.mimoApiKey) throw new HttpError(503, 'ai_not_configured');
      const body = await readJson(req);
      const options = validateCompletionOptions(body);
      const wantStream = options.stream;
      const upstreamBody = {
        model: validateModel(body.model, config.defaultModel),
        messages: validateMessages(body.messages),
        stream: wantStream,
      };
      if (options.temperature !== undefined) upstreamBody.temperature = options.temperature;
      if (options.maxTokens !== undefined) upstreamBody.max_tokens = options.maxTokens;
      const lifetime = requestSignal(req, res, wantStream ? 300000 : 60000);
      let upstream;
      try {
        upstream = await fetch(`${config.mimoBaseUrl.replace(/\/+$/, '')}/chat/completions`, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${config.mimoApiKey}`,
          },
          body: JSON.stringify(upstreamBody),
          signal: lifetime.signal,
        });
      } catch {
        lifetime.release();
        throw new HttpError(502, 'upstream_unavailable');
      }
      if (!upstream.ok) {
        console.error(`MiMo upstream returned HTTP ${upstream.status}`);
        await upstream.body.cancel().catch(() => {});
        lifetime.release();
        throw new HttpError(502, 'upstream_unavailable');
      }
      if (wantStream) {
        const contentType = upstream.headers.get('content-type') || '';
        if (contentType.split(';', 1)[0].trim().toLowerCase() !== 'text/event-stream') {
          await upstream.body.cancel().catch(() => {});
          lifetime.release();
          throw new HttpError(502, 'upstream_invalid_response');
        }
        // SSE 逐块透传，不缓冲整包——凑齐再发就不是流式了。字节原样转发，
        // 解析归客户端（服务端不读增量内容，与非流式一样只做鉴权和转发）。
        res.writeHead(200, {
          'Content-Type': 'text/event-stream; charset=utf-8',
          'Cache-Control': 'no-store',
          'X-Content-Type-Options': 'nosniff',
        });
        try {
          for await (const chunk of upstream.body) {
            if (!res.write(chunk)) await waitForDrain(res);
          }
        } catch (error) {
          await upstream.body.cancel().catch(() => {});
          lifetime.release();
          // 200 头已发出，没法再回 HTTP 错误——但绝不能把中断伪装成干净收尾：
          // 硬断连接让客户端看到异常结束，宁可失败也不交付截断的“完整回复”。
          if (lifetime.signal.reason?.message !== 'client_disconnected') {
            console.error(`MiMo stream interrupted: ${error && error.name}`);
          }
          res.destroy();
          return undefined;
        }
        lifetime.release();
        return res.end();
      }
      let responseText;
      try {
        responseText = await readResponseBodyLimited(upstream, config.aiMaxResponseBytes);
      } finally {
        lifetime.release();
      }
      try {
        const parsed = JSON.parse(responseText);
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('invalid');
      } catch {
        throw new HttpError(502, 'upstream_invalid_response');
      }
      res.writeHead(200, {
        'Content-Type': 'application/json; charset=utf-8',
        'Cache-Control': 'no-store',
        'X-Content-Type-Options': 'nosniff',
      });
      return res.end(responseText);
    }

    throw new HttpError(404, 'not_found');
  }

  const server = http.createServer((req, res) => {
    handle(req, res).catch((error) => {
      if (res.headersSent) {
        res.destroy();
        return;
      }
      if (error instanceof HttpError) {
        json(res, error.status, { error: error.code });
        return;
      }
      console.error(error);
      json(res, 500, { error: 'internal_error' });
    });
  });
  server.headersTimeout = 15_000;
  server.requestTimeout = 60_000;
  server.keepAliveTimeout = 5_000;
  server.maxHeadersCount = 64;

  return {
    server,
    close() {
      server.close();
      db.close();
    },
  };
}

if (require.main === module) {
  const app = createPaServer();
  const port = Number(process.env.SOLUM_PORT || process.env.PA_PORT || 8787);
  app.server.listen(port, '0.0.0.0', () => {
    console.log(`Solum cloud listening on ${port}`);
  });
}

module.exports = { createPaServer };

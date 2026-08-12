'use strict';

const http = require('node:http');
const crypto = require('node:crypto');
const path = require('node:path');
const fs = require('node:fs');
const { DatabaseSync } = require('node:sqlite');

const ACCESS_TTL_SECONDS = 15 * 60;
const REFRESH_TTL_SECONDS = 30 * 24 * 60 * 60;
const MAX_BODY_BYTES = 1024 * 1024;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;
const LOGIN_MAX_FAILURES = 5;

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
    return JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
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

function passwordHash(password, salt) {
  return crypto.scryptSync(password, salt, 64).toString('hex');
}

function secureEqualHex(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string' || left.length !== right.length) {
    return false;
  }
  return crypto.timingSafeEqual(Buffer.from(left, 'hex'), Buffer.from(right, 'hex'));
}

function tokenHash(token) {
  return crypto.createHash('sha256').update(token).digest('hex');
}

function signAccessToken(username, secret) {
  const payload = Buffer.from(JSON.stringify({
    sub: username,
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
    if (typeof payload.sub !== 'string' || !Number.isInteger(payload.exp) || payload.exp <= nowSeconds()) {
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
    ...overrides,
  };
  if (config.authSecret.length < 32) throw new Error('SOLUM_AUTH_SECRET must be at least 32 characters');
  if (!config.adminUsername) throw new Error('SOLUM_ADMIN_USERNAME is required');
  validateModel(config.defaultModel, 'mimo-v2.5');
  config.allowedOrigins = parseAllowedOrigins(config.allowedOrigins);
  return config;
}

function openDatabase(config) {
  fs.mkdirSync(path.dirname(config.dbPath), { recursive: true });
  const db = new DatabaseSync(config.dbPath);
  db.exec(`
    PRAGMA journal_mode=WAL;
    PRAGMA foreign_keys=ON;
    CREATE TABLE IF NOT EXISTS users (
      username TEXT PRIMARY KEY,
      password_hash TEXT NOT NULL,
      password_salt TEXT NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS refresh_tokens (
      token_hash TEXT PRIMARY KEY,
      username TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
      expires_at INTEGER NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_refresh_expiry ON refresh_tokens(expires_at);
  `);
  const existing = db.prepare('SELECT username FROM users WHERE username = ?').get(config.adminUsername);
  if (!existing) {
    if (config.adminPassword.length < 12) {
      db.close();
      throw new Error('SOLUM_ADMIN_PASSWORD must be at least 12 characters when bootstrapping');
    }
    const salt = crypto.randomBytes(16).toString('hex');
    db.prepare(
      'INSERT INTO users(username, password_hash, password_salt, created_at) VALUES (?, ?, ?, ?)'
    ).run(config.adminUsername, passwordHash(config.adminPassword, salt), salt, nowSeconds());
  }
  return db;
}

function issueSession(db, username, secret) {
  const refresh = crypto.randomBytes(48).toString('base64url');
  const now = nowSeconds();
  db.prepare(
    'INSERT INTO refresh_tokens(token_hash, username, expires_at, created_at) VALUES (?, ?, ?, ?)'
  ).run(tokenHash(refresh), username, now + REFRESH_TTL_SECONDS, now);
  return {
    access_token: signAccessToken(username, secret),
    refresh_token: refresh,
    user: { username },
  };
}

function createPaServer(overrides = {}) {
  const config = buildConfig(overrides);
  const db = openDatabase(config);
  const loginFailures = new Map();

  function authenticate(req) {
    const username = verifyAccessToken(bearer(req), config.authSecret);
    if (username === null) throw new HttpError(401, 'unauthorized');
    return username;
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
      return;
    }
    record.count += 1;
  }

  async function handle(req, res) {
    const origin = req.headers.origin;
    if (typeof origin === 'string' && config.allowedOrigins.has(origin)) {
      res.setHeader('Access-Control-Allow-Origin', origin);
      res.setHeader('Vary', 'Origin');
      res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization');
      res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
    }
    if (req.method === 'OPTIONS') {
      if (typeof origin !== 'string' || !config.allowedOrigins.has(origin)) {
        throw new HttpError(403, 'origin_not_allowed');
      }
      res.writeHead(204, { 'Content-Length': '0', 'Cache-Control': 'no-store' });
      return res.end();
    }
    if (req.method === 'GET' && req.url === '/v1/health') {
      return json(res, 200, { status: 'ok' });
    }

    if (req.method === 'POST' && req.url === '/v1/auth/login') {
      const body = await readJson(req);
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const password = requiredString(body.password, 'invalid_credentials', 1024);
      const key = loginKey(req, username);
      if (isRateLimited(key)) throw new HttpError(429, 'too_many_attempts');
      const user = db.prepare(
        'SELECT username, password_hash, password_salt FROM users WHERE username = ?'
      ).get(username);
      const candidate = user ? passwordHash(password, user.password_salt) :
        passwordHash(password, crypto.randomBytes(16).toString('hex'));
      if (!user || !secureEqualHex(candidate, user.password_hash)) {
        recordLoginFailure(key);
        throw new HttpError(401, 'invalid_credentials');
      }
      loginFailures.delete(key);
      db.prepare('DELETE FROM refresh_tokens WHERE expires_at <= ?').run(nowSeconds());
      return json(res, 200, issueSession(db, username, config.authSecret));
    }

    if (req.method === 'POST' && req.url === '/v1/auth/refresh') {
      const body = await readJson(req);
      const refresh = requiredString(body.refresh_token, 'invalid_refresh_token', 512);
      const hash = tokenHash(refresh);
      const row = db.prepare(
        'SELECT username, expires_at FROM refresh_tokens WHERE token_hash = ?'
      ).get(hash);
      if (!row || row.expires_at <= nowSeconds()) {
        db.prepare('DELETE FROM refresh_tokens WHERE token_hash = ?').run(hash);
        throw new HttpError(401, 'invalid_refresh_token');
      }
      db.exec('BEGIN IMMEDIATE');
      try {
        db.prepare('DELETE FROM refresh_tokens WHERE token_hash = ?').run(hash);
        const session = issueSession(db, row.username, config.authSecret);
        db.exec('COMMIT');
        return json(res, 200, session);
      } catch (error) {
        db.exec('ROLLBACK');
        throw error;
      }
    }

    if (req.method === 'POST' && req.url === '/v1/auth/logout') {
      authenticate(req);
      const body = await readJson(req);
      if (typeof body.refresh_token === 'string') {
        db.prepare('DELETE FROM refresh_tokens WHERE token_hash = ?').run(tokenHash(body.refresh_token));
      }
      return json(res, 200, { status: 'ok' });
    }

    if (req.method === 'POST' && req.url === '/v1/ai/chat/completions') {
      authenticate(req);
      if (!config.mimoApiKey) throw new HttpError(503, 'ai_not_configured');
      const body = await readJson(req);
      // stream 只认布尔 true；客户端要什么形态就向上游要什么形态，别的值一律非流式。
      const wantStream = body.stream === true;
      const upstreamBody = {
        model: validateModel(body.model, config.defaultModel),
        messages: validateMessages(body.messages),
        stream: wantStream,
      };
      if (typeof body.temperature === 'number') upstreamBody.temperature = body.temperature;
      if (Number.isInteger(body.max_tokens) && body.max_tokens > 0) {
        upstreamBody.max_tokens = body.max_tokens;
      }
      let upstream;
      try {
        upstream = await fetch(`${config.mimoBaseUrl.replace(/\/+$/, '')}/chat/completions`, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${config.mimoApiKey}`,
          },
          body: JSON.stringify(upstreamBody),
          // 该超时覆盖到 body 消费结束。流式给 300s：思考型模型的 reasoning 阶段
          // 不产出可见增量但一直烧时间，60s 会把长回答拦腰掐断。
          signal: AbortSignal.timeout(wantStream ? 300000 : 60000),
        });
      } catch {
        throw new HttpError(502, 'upstream_unavailable');
      }
      if (!upstream.ok) {
        console.error(`MiMo upstream returned HTTP ${upstream.status}`);
        throw new HttpError(502, 'upstream_unavailable');
      }
      if (wantStream) {
        // SSE 逐块透传，不缓冲整包——凑齐再发就不是流式了。字节原样转发，
        // 解析归客户端（服务端不读增量内容，与非流式一样只做鉴权和转发）。
        res.writeHead(200, {
          'Content-Type': 'text/event-stream; charset=utf-8',
          'Cache-Control': 'no-store',
          'X-Content-Type-Options': 'nosniff',
        });
        try {
          for await (const chunk of upstream.body) {
            res.write(chunk);
          }
        } catch (error) {
          // 200 头已发出，没法再回 HTTP 错误——但绝不能把中断伪装成干净收尾：
          // 硬断连接让客户端看到异常结束，宁可失败也不交付截断的“完整回复”。
          console.error(`MiMo stream interrupted: ${error && error.name}`);
          res.destroy();
          return undefined;
        }
        return res.end();
      }
      const responseText = await upstream.text();
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
      if (error instanceof HttpError) {
        json(res, error.status, { error: error.code });
        return;
      }
      console.error(error);
      json(res, 500, { error: 'internal_error' });
    });
  });

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

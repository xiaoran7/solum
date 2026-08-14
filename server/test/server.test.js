'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const { DatabaseSync } = require('node:sqlite');
const { createPaServer } = require('../src/server');
const { proxyCompletion } = require('../src/postgres-server');

async function listen(server) {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return server.address().port;
}

function createAiPool(state) {
  const client = {
    async query(text, values) {
      if (text === 'begin' || text === 'commit' || text === 'rollback' ||
        text.includes("set_config('app.current_tenant_id'")) return { rows: [], rowCount: 0 };
      if (text.includes("update ai.usage_events set status='interrupted'")) {
        return { rows: [], rowCount: 0 };
      }
      if (text.includes('insert into ai.entitlements') || text.includes('insert into ai.usage_monthly')) {
        return { rows: [], rowCount: 0 };
      }
      if (text.startsWith('select plan,monthly_request_limit')) {
        return { rows: [{ plan: 'standard', monthly_request_limit: 10,
          custom_models_enabled: true }], rowCount: 1 };
      }
      if (text.includes('update ai.usage_monthly usage set request_count=request_count+1')) {
        state.reserved += 1;
        return { rows: [{ period_start: '2026-08-01', request_count: state.reserved,
          success_count: 0, failure_count: 0, fallback_count: 0,
          prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 }], rowCount: 1 };
      }
      if (text.includes('insert into ai.usage_events')) {
        state.event = { requestId: values[0], requested: values[2], effective: values[3] };
        return { rows: [], rowCount: 1 };
      }
      if (text.includes('update ai.usage_events set status=')) {
        state.event = { ...state.event, status: values[2], effective: values[3],
          fallback: values[4], reason: values[5] };
        return { rows: [{ period_start: '2026-08-01' }], rowCount: 1 };
      }
      if (text.includes('update ai.usage_monthly set')) {
        state.monthly = { success: values[2], failure: values[3], fallback: values[4] };
        return { rows: [], rowCount: 1 };
      }
      throw new Error(`unexpected query: ${text}`);
    },
    release() {},
  };
  return { async connect() { return client; } };
}

test('an empty stream before its first byte falls back once and spends one request', async (t) => {
  const seenModels = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const model = JSON.parse(Buffer.concat(chunks).toString('utf8')).model;
    seenModels.push(model);
    if (model === 'mimo-v2.5-pro') {
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      res.flushHeaders();
      return setTimeout(() => res.end(), 20);
    }
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.end('data: {"choices":[{"delta":{"content":"ok"}}]}\n\ndata: [DONE]\n\n');
  });
  const upstreamPort = await listen(upstream);
  t.after(async () => {
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
  });

  const tenant = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  const state = { reserved: 0, event: null, monthly: null };
  const pool = createAiPool(state);
  const proxy = http.createServer((req, res) => {
    proxyCompletion(req, res, pool, tenant, {
      mimoApiKey: 'server-only',
      mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
      defaultModel: 'mimo-v2.5',
      modelCatalog: { defaultModel: 'mimo-v2.5', models: ['mimo-v2.5', 'mimo-v2.5-pro'] },
      standardPlan: 'standard', standardMonthlyRequestLimit: 10,
      aiMaxResponseBytes: 1024 * 1024,
    }).catch((error) => {
      if (!res.headersSent) res.writeHead(error.status || 500);
      res.end(error.code || 'internal_error');
    });
  });
  const proxyPort = await listen(proxy);
  t.after(async () => {
    proxy.closeAllConnections();
    await new Promise((resolve) => proxy.close(resolve));
  });
  const response = await fetch(`http://127.0.0.1:${proxyPort}/v1/ai/chat/completions`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ model: 'mimo-v2.5-pro', stream: true,
      messages: [{ role: 'user', content: 'hello' }] }),
  });
  assert.equal(response.status, 200);
  assert.match(await response.text(), /"content":"ok"/);
  assert.deepEqual(seenModels, ['mimo-v2.5-pro', 'mimo-v2.5']);
  assert.equal(state.reserved, 1);
  assert.deepEqual(state.monthly, { success: 1, failure: 0, fallback: 1 });
  assert.deepEqual({ status: state.event.status, effective: state.event.effective,
    fallback: state.event.fallback, reason: state.event.reason }, {
    status: 'succeeded', effective: 'mimo-v2.5', fallback: true,
    reason: 'upstream_stream_interrupted_before_first_byte',
  });
});

test('a malformed successful upstream response is counted as a failure', async (t) => {
  const upstream = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end('not-json');
  });
  const upstreamPort = await listen(upstream);
  t.after(async () => {
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
  });

  const state = { reserved: 0, event: null, monthly: null };
  const tenant = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  const proxy = http.createServer((req, res) => {
    proxyCompletion(req, res, createAiPool(state), tenant, {
      mimoApiKey: 'server-only', mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
      defaultModel: 'mimo-v2.5',
      modelCatalog: { defaultModel: 'mimo-v2.5', models: ['mimo-v2.5'] },
      standardPlan: 'standard', standardMonthlyRequestLimit: 10,
      aiMaxResponseBytes: 1024 * 1024,
    }).catch((error) => {
      if (!res.headersSent) res.writeHead(error.status || 500);
      res.end(error.code || 'internal_error');
    });
  });
  const proxyPort = await listen(proxy);
  t.after(async () => {
    proxy.closeAllConnections();
    await new Promise((resolve) => proxy.close(resolve));
  });

  const response = await fetch(`http://127.0.0.1:${proxyPort}/v1/ai/chat/completions`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ messages: [{ role: 'user', content: 'hello' }] }),
  });
  assert.equal(response.status, 502);
  assert.equal(await response.text(), 'upstream_invalid_response');
  assert.equal(state.event.status, 'failed');
  assert.deepEqual(state.monthly, { success: 0, failure: 1, fallback: 0 });
});

test('a malformed non-default response falls back before committing the response', async (t) => {
  const seenModels = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const model = JSON.parse(Buffer.concat(chunks).toString('utf8')).model;
    seenModels.push(model);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    if (model === 'mimo-v2.5-pro') return res.end('not-json');
    return res.end(JSON.stringify({
      choices: [{ message: { content: 'fallback ok' } }],
      usage: { prompt_tokens: 2, completion_tokens: 3, total_tokens: 5 },
    }));
  });
  const upstreamPort = await listen(upstream);
  t.after(async () => {
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
  });

  const state = { reserved: 0, event: null, monthly: null };
  const tenant = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  const proxy = http.createServer((req, res) => {
    proxyCompletion(req, res, createAiPool(state), tenant, {
      mimoApiKey: 'server-only', mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
      defaultModel: 'mimo-v2.5',
      modelCatalog: { defaultModel: 'mimo-v2.5', models: ['mimo-v2.5', 'mimo-v2.5-pro'] },
      standardPlan: 'standard', standardMonthlyRequestLimit: 10,
      aiMaxResponseBytes: 1024 * 1024,
    }).catch((error) => {
      if (!res.headersSent) res.writeHead(error.status || 500);
      res.end(error.code || 'internal_error');
    });
  });
  const proxyPort = await listen(proxy);
  t.after(async () => {
    proxy.closeAllConnections();
    await new Promise((resolve) => proxy.close(resolve));
  });
  const response = await fetch(`http://127.0.0.1:${proxyPort}/v1/ai/chat/completions`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ model: 'mimo-v2.5-pro',
      messages: [{ role: 'user', content: 'hello' }] }),
  });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).choices[0].message.content, 'fallback ok');
  assert.deepEqual(seenModels, ['mimo-v2.5-pro', 'mimo-v2.5']);
  assert.equal(response.headers.get('x-solum-effective-model'), 'mimo-v2.5');
  assert.equal(response.headers.get('x-solum-fallback-reason'), 'upstream_invalid_response');
  assert.equal(state.reserved, 1);
  assert.deepEqual(state.monthly, { success: 1, failure: 0, fallback: 1 });
});

test('a stream request rejects a 200 response that is not SSE', async (t) => {
  const upstream = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ choices: [{ message: { content: 'not a stream' } }] }));
  });
  const upstreamPort = await listen(upstream);
  t.after(async () => {
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
  });
  const state = { reserved: 0, event: null, monthly: null };
  const proxy = http.createServer((req, res) => {
    proxyCompletion(req, res, createAiPool(state), '9d4df1be-9f7b-4a3a-b986-ec920d2df60e', {
      mimoApiKey: 'server-only', mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
      defaultModel: 'mimo-v2.5',
      modelCatalog: { defaultModel: 'mimo-v2.5', models: ['mimo-v2.5'] },
      standardPlan: 'standard', standardMonthlyRequestLimit: 10,
      aiMaxResponseBytes: 1024 * 1024,
    }).catch((error) => {
      if (!res.headersSent) res.writeHead(error.status || 500);
      res.end(error.code || 'internal_error');
    });
  });
  const proxyPort = await listen(proxy);
  t.after(async () => {
    proxy.closeAllConnections();
    await new Promise((resolve) => proxy.close(resolve));
  });
  const response = await fetch(`http://127.0.0.1:${proxyPort}/v1/ai/chat/completions`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ stream: true, messages: [{ role: 'user', content: 'hello' }] }),
  });
  assert.equal(response.status, 502);
  assert.equal(await response.text(), 'upstream_invalid_response');
  assert.equal(state.event.status, 'failed');
});

test('login, refresh rotation and fixed-upstream model proxy', async (t) => {
  let seenAuthorization = '';
  let seenModel = '';
  let seenStream = null;
  const upstream = http.createServer(async (req, res) => {
    seenAuthorization = req.headers.authorization || '';
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const parsed = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    seenModel = parsed.model;
    seenStream = parsed.stream;
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ choices: [{ message: { content: 'ok' } }] }));
  });
  const upstreamPort = await listen(upstream);
  t.after(() => upstream.close());

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-'));
  const app = createPaServer({
    dbPath: path.join(temp, 'test.db'),
    authSecret: 'a'.repeat(64),
    adminUsername: 'alice',
    adminPassword: 'correct-horse-battery-staple',
    mimoApiKey: 'server-only-secret',
    mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
  });
  const port = await listen(app.server);
  t.after(() => {
    // Windows：keep-alive 连接会拖死 server.close，SQLite 句柄会让 rmSync 撞 EPERM——
    // 强制断连 + 带重试删除。
    app.server.closeAllConnections();
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const base = `http://127.0.0.1:${port}`;

  for (const route of ['login', 'refresh', 'logout']) {
    const nullBody = await fetch(`${base}/v1/auth/${route}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: 'null',
    });
    assert.equal(nullBody.status, 400);
  }

  const bad = await fetch(`${base}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'wrong-password' }),
  });
  assert.equal(bad.status, 401);
  const padded = await fetch(`${base}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: ' correct-horse-battery-staple ' }),
  });
  assert.equal(padded.status, 401);

  const login = await fetch(`${base}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'correct-horse-battery-staple' }),
  });
  assert.equal(login.status, 200);
  const session = await login.json();
  assert.ok(session.access_token);
  assert.ok(session.refresh_token);
  assert.match(session.user.id, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  assert.notEqual(session.user.id, session.user.username);
  const tokenPayload = JSON.parse(Buffer.from(
    session.access_token.split('.')[0], 'base64url').toString('utf8'));
  assert.equal(tokenPayload.sub, session.user.id);
  assert.equal(tokenPayload.username, 'alice');

  const completion = await fetch(`${base}/v1/ai/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${session.access_token}`,
    },
    body: JSON.stringify({
      model: 'team/custom-model:v2',
      messages: [{ role: 'user', content: 'hello' }],
    }),
  });
  assert.equal(completion.status, 200);
  assert.equal((await completion.json()).choices[0].message.content, 'ok');
  assert.equal(seenAuthorization, 'Bearer server-only-secret');
  assert.equal(seenModel, 'team/custom-model:v2');
  // 客户端没要 stream 时，上游必须收到显式的 stream:false，而不是缺字段交给上游猜。
  assert.equal(seenStream, false);

  const refresh = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: session.refresh_token }),
  });
  assert.equal(refresh.status, 200);
  const rotated = await refresh.json();
  assert.notEqual(rotated.refresh_token, session.refresh_token);
  assert.equal(rotated.user.id, session.user.id);
  const recoveredRefresh = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: session.refresh_token }),
  });
  assert.equal(recoveredRefresh.status, 200);
  assert.equal((await recoveredRefresh.json()).refresh_token, rotated.refresh_token,
    'a lost rotation response must be recoverable with the previous token');

  const logoutOldGeneration = await fetch(`${base}/v1/auth/logout`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: session.refresh_token }),
  });
  assert.equal(logoutOldGeneration.status, 200);
  const revokedDescendant = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: rotated.refresh_token }),
  });
  assert.equal(revokedDescendant.status, 401);

  const replayLogin = await fetch(`${base}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'correct-horse-battery-staple' }),
  });
  const replaySeed = await replayLogin.json();
  const replayRotation = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: replaySeed.refresh_token }),
  });
  const replayDescendant = await replayRotation.json();
  const replayRecovery = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: replaySeed.refresh_token }),
  });
  assert.equal(replayRecovery.status, 200);
  assert.equal((await replayRecovery.json()).refresh_token, replayDescendant.refresh_token);
  const replayGrandchildResponse = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: replayDescendant.refresh_token }),
  });
  assert.equal(replayGrandchildResponse.status, 200);
  const replayGrandchild = await replayGrandchildResponse.json();
  const reused = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: replaySeed.refresh_token }),
  });
  assert.equal(reused.status, 401);
  const revokedAfterReplay = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: replayGrandchild.refresh_token }),
  });
  assert.equal(revokedAfterReplay.status, 401);
});

test('bootstrap admin username follows the login normalization rule', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-admin-name-'));
  const dbPath = path.join(temp, 'test.db');
  const app = createPaServer({
    dbPath,
    authSecret: 'a'.repeat(64),
    adminUsername: ' alice ',
    adminPassword: 'correct-horse-battery-staple',
  });
  app.close();
  try {
    const db = new DatabaseSync(dbPath, { readOnly: true });
    assert.equal(db.prepare('SELECT username FROM users').get().username, 'alice');
    db.close();
  } finally {
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('password verification preserves exact leading and trailing whitespace', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-password-'));
  const password = '  correct horse battery staple  ';
  const app = createPaServer({
    dbPath: path.join(temp, 'test.db'),
    authSecret: 'a'.repeat(64),
    adminUsername: 'space-user',
    adminPassword: password,
  });
  const port = await listen(app.server);
  t.after(() => {
    app.server.closeAllConnections();
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const base = `http://127.0.0.1:${port}`;
  const exact = await fetch(`${base}/v1/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'space-user', password }),
  });
  assert.equal(exact.status, 200);
  const trimmed = await fetch(`${base}/v1/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'space-user', password: password.trim() }),
  });
  assert.equal(trimmed.status, 401);
});

test('legacy trimmed password rows remain compatible without rebinding recovery', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-password-migration-'));
  const dbPath = path.join(temp, 'test.db');
  const originalInput = '  original recovery password  ';
  const effectivePassword = originalInput.trim();
  const legacySalt = '00112233445566778899aabbccddeeff';
  const legacyHash = crypto.scryptSync(effectivePassword, legacySalt, 64).toString('hex');
  const legacy = new DatabaseSync(dbPath);
  legacy.exec(`
    CREATE TABLE users (
      id TEXT NOT NULL UNIQUE,
      username TEXT PRIMARY KEY,
      password_hash TEXT NOT NULL,
      password_salt TEXT NOT NULL,
      created_at INTEGER NOT NULL
    )
  `);
  legacy.prepare(
    `INSERT INTO users(id,username,password_hash,password_salt,created_at)
     VALUES(?,?,?,?,?)`
  ).run(crypto.randomUUID(), 'legacy-space-user', legacyHash, legacySalt,
    Math.floor(Date.now() / 1000));
  legacy.close();

  const app = createPaServer({
    dbPath,
    authSecret: 'a'.repeat(64),
    adminUsername: 'legacy-space-user',
    adminPassword: originalInput,
  });
  const port = await listen(app.server);
  t.after(() => {
    app.server.closeAllConnections();
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const base = `http://127.0.0.1:${port}`;
  const originalInputLogin = await fetch(`${base}/v1/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'legacy-space-user', password: originalInput }),
  });
  assert.equal(originalInputLogin.status, 200,
    'the original raw input must still reach its recovery envelope');

  const effectiveLogin = await fetch(`${base}/v1/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'legacy-space-user', password: effectivePassword }),
  });
  assert.equal(effectiveLogin.status, 200,
    'legacy rows continue accepting the historical trimmed password');
  const migrated = new DatabaseSync(dbPath, { readOnly: true });
  assert.equal(migrated.prepare(
    'SELECT password_version FROM users WHERE username = ?'
  ).get('legacy-space-user').password_version, 1);
  migrated.close();
});

test('legacy bootstrap rows verify exact passwords before trim fallback', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-bootstrap-migration-'));
  const dbPath = path.join(temp, 'test.db');
  const password = '  exact bootstrap password  ';
  const salt = '11223344556677889900aabbccddeeff';
  const legacy = new DatabaseSync(dbPath);
  legacy.exec(`CREATE TABLE users (
    id TEXT NOT NULL UNIQUE, username TEXT PRIMARY KEY, password_hash TEXT NOT NULL,
    password_salt TEXT NOT NULL, created_at INTEGER NOT NULL
  )`);
  legacy.prepare('INSERT INTO users VALUES(?,?,?,?,?)').run(
    crypto.randomUUID(), 'legacy-bootstrap',
    crypto.scryptSync(password, salt, 64).toString('hex'), salt,
    Math.floor(Date.now() / 1000)
  );
  legacy.close();
  const app = createPaServer({ dbPath, authSecret: 'a'.repeat(64),
    adminUsername: 'legacy-bootstrap', adminPassword: password });
  const port = await listen(app.server);
  t.after(() => {
    app.server.closeAllConnections(); app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const base = `http://127.0.0.1:${port}`;
  const exact = await fetch(`${base}/v1/auth/login`, { method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'legacy-bootstrap', password }) });
  assert.equal(exact.status, 200);
  const trimmed = await fetch(`${base}/v1/auth/login`, { method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'legacy-bootstrap', password: password.trim() }) });
  assert.equal(trimmed.status, 401);
});

test('concurrent login failures reserve rate-limit capacity before scrypt', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-login-burst-'));
  const app = createPaServer({
    dbPath: path.join(temp, 'test.db'),
    authSecret: 'a'.repeat(64),
    adminUsername: 'burst-user',
    adminPassword: 'correct-horse-battery-staple',
  });
  const port = await listen(app.server);
  t.after(() => {
    app.server.closeAllConnections();
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const responses = await Promise.all(Array.from({ length: 6 }, () => fetch(
    `http://127.0.0.1:${port}/v1/auth/login`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username: 'burst-user', password: 'wrong-password' }),
    }
  )));
  assert.deepEqual(responses.map((response) => response.status).sort(),
    [401, 401, 401, 401, 401, 429]);
});

test('legacy username-keyed accounts migrate to immutable ids without losing refresh sessions', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-legacy-id-'));
  const dbPath = path.join(temp, 'legacy.db');
  const db = new DatabaseSync(dbPath);
  db.exec(`
    PRAGMA foreign_keys=ON;
    CREATE TABLE users (
      username TEXT PRIMARY KEY,
      password_hash TEXT NOT NULL,
      password_salt TEXT NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE TABLE refresh_tokens (
      token_hash TEXT PRIMARY KEY,
      username TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
      expires_at INTEGER NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE INDEX idx_refresh_expiry ON refresh_tokens(expires_at);
  `);
  db.prepare('INSERT INTO users VALUES (?, ?, ?, ?)').run('legacy', 'hash', 'salt', 1);
  db.prepare('INSERT INTO refresh_tokens VALUES (?, ?, ?, ?)')
    .run('old-token-hash', 'legacy', 4102444800, 1);
  db.close();

  const app = createPaServer({
    dbPath, authSecret: 'a'.repeat(64), adminUsername: 'legacy', adminPassword: '',
  });
  t.after(() => {
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const migrated = new DatabaseSync(dbPath);
  const user = migrated.prepare('SELECT id, username FROM users').get();
  const refresh = migrated.prepare('SELECT user_id FROM refresh_tokens').get();
  assert.match(user.id, /^[0-9a-f-]{36}$/);
  assert.equal(refresh.user_id, user.id);
  assert.equal(user.username, 'legacy');
  migrated.close();
});

test('an interrupted id-column migration is repaired on the next startup', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-partial-id-'));
  const dbPath = path.join(temp, 'partial.db');
  const db = new DatabaseSync(dbPath);
  db.exec(`
    CREATE TABLE users (
      username TEXT PRIMARY KEY, password_hash TEXT NOT NULL,
      password_salt TEXT NOT NULL, created_at INTEGER NOT NULL, id TEXT
    );
    CREATE TABLE refresh_tokens (
      token_hash TEXT PRIMARY KEY, username TEXT NOT NULL,
      expires_at INTEGER NOT NULL, created_at INTEGER NOT NULL
    );
  `);
  db.prepare('INSERT INTO users(username, password_hash, password_salt, created_at) VALUES (?, ?, ?, ?)')
    .run('partial', 'hash', 'salt', 1);
  db.close();

  const app = createPaServer({
    dbPath, authSecret: 'a'.repeat(64), adminUsername: 'partial', adminPassword: '',
  });
  t.after(() => {
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const repaired = new DatabaseSync(dbPath);
  assert.match(repaired.prepare('SELECT id FROM users').get().id, /^[0-9a-f-]{36}$/);
  assert.ok(repaired.prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_users_id'").get());
  repaired.close();
});

test('CORS allows only configured dashboard origins', async (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-cors-'));
  const app = createPaServer({
    dbPath: path.join(temp, 'test.db'), authSecret: 'a'.repeat(64),
    adminUsername: 'alice', adminPassword: 'correct-horse-battery-staple',
    allowedOrigins: 'https://sync.example, https://ops.example',
  });
  assert.equal(app.server.headersTimeout, 15_000);
  assert.equal(app.server.requestTimeout, 60_000);
  assert.equal(app.server.keepAliveTimeout, 5_000);
  assert.equal(app.server.maxHeadersCount, 64);
  const port = await listen(app.server);
  t.after(() => {
    app.server.closeAllConnections(); app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const allowed = await fetch(`http://127.0.0.1:${port}/v1/auth/login`, {
    method: 'OPTIONS', headers: { Origin: 'https://sync.example',
      'Access-Control-Request-Method': 'POST' },
  });
  assert.equal(allowed.status, 204);
  assert.equal(allowed.headers.get('access-control-allow-origin'), 'https://sync.example');
  const denied = await fetch(`http://127.0.0.1:${port}/v1/auth/login`, {
    method: 'OPTIONS', headers: { Origin: 'https://evil.example',
      'Access-Control-Request-Method': 'POST' },
  });
  assert.equal(denied.status, 403);
  assert.equal(denied.headers.get('access-control-allow-origin'), null);

  const deniedMutation = await fetch(`http://127.0.0.1:${port}/v1/auth/login`, {
    method: 'POST', headers: { Origin: 'https://evil.example', 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'wrong-password' }),
  });
  assert.equal(deniedMutation.status, 403, 'a denied browser origin must not reach the route');

  const wrongMediaType = await fetch(`http://127.0.0.1:${port}/v1/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'text/plain' },
    body: JSON.stringify({ username: 'alice', password: 'wrong-password' }),
  });
  assert.equal(wrongMediaType.status, 415);

  const sameOrigin = await fetch(`http://127.0.0.1:${port}/v1/auth/login`, {
    method: 'POST', headers: { Origin: `http://127.0.0.1:${port}`,
      'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'correct-horse-battery-staple' }),
  });
  assert.equal(sameOrigin.status, 200, 'the embedded dashboard must work without a duplicate allowlist entry');
});

test('stream:true proxies SSE bytes through as-is', async (t) => {
  const firstChunk = 'data: {"choices":[{"delta":{"content":"你"}}]}\n\n';
  const secondChunk = 'data: {"choices":[{"delta":{"content":"好"}}]}\n\ndata: [DONE]\n\n';
  let seenStream = null;
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    seenStream = JSON.parse(Buffer.concat(chunks).toString('utf8')).stream;
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.write(firstChunk);
    // 分两拍发出，确认代理不是攒齐整包才转发（逐块 write，而非一次 end）。
    setTimeout(() => res.end(secondChunk), 20);
  });
  const upstreamPort = await listen(upstream);
  t.after(() => upstream.close());

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-'));
  const app = createPaServer({
    dbPath: path.join(temp, 'test.db'),
    authSecret: 'a'.repeat(64),
    adminUsername: 'alice',
    adminPassword: 'correct-horse-battery-staple',
    mimoApiKey: 'server-only-secret',
    mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
  });
  const port = await listen(app.server);
  t.after(() => {
    // Windows：keep-alive 连接会拖死 server.close——强制断连。temp 目录的删除在
    // 本测试最后注册的钩子里做（after 钩子按注册顺序执行，必须排在 app2 关库之后）。
    app.server.closeAllConnections();
    app.close();
  });
  const base = `http://127.0.0.1:${port}`;

  const login = await fetch(`${base}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'correct-horse-battery-staple' }),
  });
  const session = await login.json();

  const completion = await fetch(`${base}/v1/ai/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${session.access_token}`,
    },
    body: JSON.stringify({
      stream: true,
      messages: [{ role: 'user', content: 'hello' }],
    }),
  });
  assert.equal(completion.status, 200);
  assert.equal(completion.headers.get('content-type'), 'text/event-stream; charset=utf-8');
  assert.equal(await completion.text(), firstChunk + secondChunk);
  assert.equal(seenStream, true);

  // 上游中途断流：代理必须硬断连接（客户端读 body 报错），不许干净收尾——
  // 干净 EOF 会让客户端把截断内容当完整回复。
  const brokenUpstream = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.write('data: {"choices":[{"delta":{"content":"半"}}]}\n\n');
    setTimeout(() => res.destroy(), 20);
  });
  const brokenPort = await listen(brokenUpstream);
  t.after(() => brokenUpstream.close());
  const app2 = createPaServer({
    dbPath: path.join(temp, 'test2.db'),
    authSecret: 'a'.repeat(64),
    adminUsername: 'alice',
    adminPassword: 'correct-horse-battery-staple',
    mimoApiKey: 'server-only-secret',
    mimoBaseUrl: `http://127.0.0.1:${brokenPort}/v1`,
  });
  const port2 = await listen(app2.server);
  t.after(() => {
    app2.server.closeAllConnections();
    app2.close();
  });
  t.after(() => {
    // 两个 app 的 SQLite 都关掉之后才能删 temp；Windows 句柄释放偶有延迟，带重试。
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const login2 = await fetch(`http://127.0.0.1:${port2}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'correct-horse-battery-staple' }),
  });
  const session2 = await login2.json();
  const interrupted = await fetch(`http://127.0.0.1:${port2}/v1/ai/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${session2.access_token}`,
    },
    body: JSON.stringify({ stream: true, messages: [{ role: 'user', content: 'hello' }] }),
  });
  assert.equal(interrupted.status, 200);
  await assert.rejects(interrupted.text());

  // Ambiguous stream values are rejected instead of silently changing shape.
  const notStream = await fetch(`${base}/v1/ai/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${session.access_token}`,
    },
    body: JSON.stringify({
      stream: 'true',
      messages: [{ role: 'user', content: 'hello' }],
    }),
  });
  assert.equal(notStream.status, 400);
  assert.equal(seenStream, true, 'invalid input must not reach the upstream');
});

test('disconnecting an SSE client cancels the SQLite upstream request', async (t) => {
  let closeUpstream;
  const upstreamClosed = new Promise((resolve) => { closeUpstream = resolve; });
  let interval;
  const upstream = http.createServer(async (req, res) => {
    for await (const _chunk of req) { /* drain */ }
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    interval = setInterval(() => res.write('data: {"tick":true}\n\n'.repeat(256)), 2);
    res.once('close', () => {
      clearInterval(interval);
      closeUpstream();
    });
  });
  const upstreamPort = await listen(upstream);
  t.after(async () => {
    clearInterval(interval);
    upstream.closeAllConnections();
    await new Promise((resolve) => upstream.close(resolve));
  });

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-cloud-stream-abort-'));
  const app = createPaServer({
    dbPath: path.join(temp, 'test.db'),
    authSecret: 'a'.repeat(64),
    adminUsername: 'alice',
    adminPassword: 'correct-horse-battery-staple',
    mimoApiKey: 'server-only-secret',
    mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
  });
  const port = await listen(app.server);
  t.after(() => {
    app.server.closeAllConnections();
    app.close();
    fs.rmSync(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const login = await fetch(`http://127.0.0.1:${port}/v1/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'correct-horse-battery-staple' }),
  });
  const session = await login.json();
  await new Promise((resolve, reject) => {
    const request = http.request({
      host: '127.0.0.1', port, path: '/v1/ai/chat/completions', method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${session.access_token}`,
      },
    });
    request.once('error', reject);
    request.once('response', (response) => {
      response.once('data', () => {
        response.destroy();
        resolve();
      });
    });
    request.end(JSON.stringify({ stream: true, messages: [{ role: 'user', content: 'hello' }] }));
  });
  await Promise.race([
    upstreamClosed,
    new Promise((_, reject) => setTimeout(() => reject(new Error('upstream was not cancelled')), 2000)),
  ]);
});

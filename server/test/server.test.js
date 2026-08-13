'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const { DatabaseSync } = require('node:sqlite');
const { createPaServer } = require('../src/server');

async function listen(server) {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return server.address().port;
}

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

  const bad = await fetch(`${base}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: 'alice', password: 'wrong-password' }),
  });
  assert.equal(bad.status, 401);

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

  const reused = await fetch(`${base}/v1/auth/refresh`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ refresh_token: session.refresh_token }),
  });
  assert.equal(reused.status, 401);
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

  // stream 传非布尔 true 的值（比如字符串）不算流式请求。
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
  assert.equal(notStream.status, 200);
  assert.equal(seenStream, false);
});

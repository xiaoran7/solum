'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { createPostgresServer } = require('../src/postgres-server');
const { passwordHashAsync } = require('../src/http-security');

async function fixture(t, overrides = {}, serverFactory = createPostgresServer) {
  const salt = 'synthetic-test-salt';
  const password = ' sample-password ';
  const user = { id: '11111111-1111-4111-8111-111111111111', username: 'sample',
    password_salt: salt, password_hash: await passwordHashAsync(password, salt),
    password_version: 2, status: 'active' };
  const queries = [];
  const pool = {
    async query(sql, params) {
      queries.push(sql);
      if (sql.includes('from auth.users where username=$1')) return { rows: [user], rowCount: 1 };
      if (sql.includes('from auth.oauth_identities oi')) return { rows: [user], rowCount: 1 };
      if (sql.includes('from auth.users where email = $1') || sql.includes('from auth.users where phone = $1')) {
        return { rows: params[0] === 'missing@example.test' ? [] : [user], rowCount: params[0] === 'missing@example.test' ? 0 : 1 };
      }
      return { rows: [], rowCount: 0 };
    },
    async connect() { return { query: pool.query.bind(pool), release() {} }; },
    async end() {},
  };
  const app = await serverFactory({ pool, authSecret: 'a'.repeat(40),
    adminUsername: 'test-admin', adminPassword: '', registrationMode: 'closed',
    huaweiEnabled: true, huaweiClientId: 'sample-client', huaweiClientSecret: '',
    ...overrides });
  await new Promise(resolve => app.server.listen(0, '127.0.0.1', resolve));
  t.after(async () => { app.server.closeAllConnections(); await app.close(); });
  const base = `http://127.0.0.1:${app.server.address().port}`;
  const post = (route, body) => fetch(base + route, { method: 'POST',
    headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  return { post, base, queries, password, pool };
}

test('unconfigured external authentication fails closed without issuing a session', async t => {
  const { post, base, queries } = await fixture(t);
  const config = await (await fetch(base + '/v1/auth/huawei/config')).json();
  assert.equal(config.enabled, false);
  for (const [route, body, status] of [
    ['/v1/auth/huawei/exchange', { code: 'sample-code' }, 403],
    ['/v1/auth/sms/send', { phone: '13800000000' }, 503],
    ['/v1/auth/phone/register', {}, 503],
    ['/v1/auth/phone/login', { phone: '13800000000', code: '123456' }, 503],
    ['/v1/auth/email/register', { email: 'sample@example.test', password: 'sample-password' }, 403],
  ]) {
    const response = await post(route, body);
    assert.equal(response.status, status, route);
    assert.equal((await response.json()).access_token, undefined);
  }
  assert.equal(queries.some(sql => sql.includes('insert into auth.refresh_tokens')), false);
});

test('phone and email login verify exact password with stored salt and hash', async t => {
  const { post, queries, password } = await fixture(t);
  for (const [route, identity] of [
    ['/v1/auth/email/login', { email: 'sample@example.test' }],
    ['/v1/auth/phone/login', { phone: '13800000000' }],
  ]) {
    const good = await post(route, { ...identity, password });
    assert.equal(good.status, 200, route);
    assert.equal(typeof (await good.json()).access_token, 'string');
    const bad = await post(route, { ...identity, password: 'wrong-password' });
    assert.equal(bad.status, 401);
    await bad.json();
  }
  assert.equal(queries.some(sql => /phone = \$1 or username/.test(sql)), false);
});

test('Huawei exchange requires successful provider identity verification', async t => {
  const originalFetch = globalThis.fetch;
  let mode = 'valid';
  let calls = 0;
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    if (String(url).startsWith('http://127.0.0.1:')) return originalFetch(url, options);
    calls += 1;
    assert.equal(options.redirect, 'error');
    assert.ok(options.signal);
    if (String(url).endsWith('/token')) {
      if (mode === 'token-rejected') return Response.json({ error: 'invalid_grant' }, { status: 400 });
      return Response.json({ access_token: 'synthetic-provider-token' });
    }
    assert.equal(String(url), 'https://account.cloud.huawei.com/rest.php?nsp_fmt=JSON&nsp_svc=GOpen.User.getInfo');
    return Response.json(mode === 'missing-identity' ? {} : { unionID: 'synthetic-subject', displayName: 'Sample' });
  });
  const { post, queries } = await fixture(t, { huaweiClientSecret: 'synthetic-secret' });
  for (const invalid of ['token-rejected', 'missing-identity']) {
    mode = invalid;
    const response = await post('/v1/auth/huawei/exchange', { code: 'synthetic-code' });
    assert.equal(response.status, 401);
    assert.equal((await response.json()).access_token, undefined);
  }
  assert.equal(queries.some(sql => sql.includes('insert into auth.refresh_tokens')), false);
  mode = 'valid';
  const good = await post('/v1/auth/huawei/exchange', { code: 'synthetic-code' });
  assert.equal(good.status, 200);
  assert.equal(typeof (await good.json()).access_token, 'string');
  const previousCalls = calls;
  const invalidRedirect = await post('/v1/auth/huawei/exchange', {
    code: 'synthetic-code', redirect_uri: 'solum://other/callback',
  });
  assert.equal(invalidRedirect.status, 400);
  await invalidRedirect.json();
  assert.equal(calls, previousCalls);
});

test('refresh grace success and connection failure settle IP reservations', async t => {
  const crypto = require('node:crypto');
  const security = require('../src/http-security');
  const originalFactory = security.createFixedWindowLimiter;
  let pending = 0;
  t.mock.method(security, 'createFixedWindowLimiter', options => {
    const limiter = originalFactory(options);
    const reserve = limiter.reserve;
    limiter.reserve = (...args) => {
      const handle = reserve(...args);
      if (!handle) return null;
      pending += 1;
      let settled = false;
      const settle = () => { if (!settled) { settled = true; pending -= 1; } };
      return {
        commit(...values) { settle(); return handle.commit(...values); },
        release() { settle(); return handle.release(); },
      };
    };
    return limiter;
  });
  const modulePath = require.resolve('../src/postgres-server');
  const cached = require.cache[modulePath];
  delete require.cache[modulePath];
  let instrumented;
  try { instrumented = require(modulePath).createPostgresServer; }
  finally { require.cache[modulePath] = cached; }
  const { post, pool } = await fixture(t, {}, instrumented);
  const sessionId = '22222222-2222-4222-8222-222222222222';
  const secret = 'a'.repeat(40);
  const previous = security.createRefreshToken(sessionId, secret);
  const current = security.createRefreshSuccessorToken(sessionId, previous, secret);
  const hash = value => crypto.createHash('sha256').update(value).digest();
  const query = pool.query.bind(pool);
  pool.query = async (sql, params) => {
    if (sql.includes('from auth.refresh_tokens rt join')) {
      return { rowCount: 1, rows: [{ session_id: sessionId,
        user_id: '11111111-1111-4111-8111-111111111111', username: 'sample',
        status: 'active', token_hash: hash(current), previous_token_hash: hash(previous),
        expires_at: new Date(Date.now() + 60000), rotated_at: new Date(),
      }] };
    }
    return query(sql, params);
  };
  const grace = await post('/v1/auth/refresh', { refresh_token: previous });
  assert.equal(grace.status, 200);
  assert.equal((await grace.json()).refresh_token, current);
  assert.equal(pending, 0, 'successful grace return must release its reservation');
  pool.connect = async () => { throw new Error('synthetic pool failure'); };
  const failed = await post('/v1/auth/refresh', { refresh_token: current });
  assert.equal(failed.status, 500);
  await failed.json();
  assert.equal(pending, 0, 'pool failures must release their reservation');
});

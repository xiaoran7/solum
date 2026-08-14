'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { DatabaseSync } = require('node:sqlite');
const { Pool } = require('pg');
const { createPostgresServer } = require('../src/postgres-server');
const { createRefreshToken } = require('../src/http-security');
const { importLegacy, loadLegacy } = require('../scripts/import-legacy-sqlite');

async function listen(server) {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return server.address().port;
}

async function jsonRequest(base, path, { method = 'GET', token, body, headers = {} } = {}) {
  const requestHeaders = { ...headers };
  if (token) requestHeaders.Authorization = `Bearer ${token}`;
  if (body !== undefined && !requestHeaders['Content-Type']) {
    requestHeaders['Content-Type'] = 'application/json';
  }
  const response = await fetch(`${base}${path}`, {
    method,
    headers: requestHeaders,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data = null;
  try { data = await response.json(); } catch {}
  return { response, data };
}

const databaseUrl = process.env.SOLUM_TEST_DATABASE_URL;
const ownerDatabaseUrl = process.env.SOLUM_TEST_OWNER_DATABASE_URL;
const disposableDatabase = process.env.SOLUM_TEST_DATABASE_IS_DISPOSABLE === 'true';

test('PostgreSQL roles retain exact least-privilege attributes and membership',
  { skip: !ownerDatabaseUrl || !databaseUrl || !disposableDatabase }, async () => {
    const ownerPool = new Pool({ connectionString: ownerDatabaseUrl, max: 1 });
    const runtimePool = new Pool({ connectionString: databaseUrl, max: 1 });
    try {
      const roles = await ownerPool.query(`
        select rolname, rolsuper, rolinherit, rolcreaterole, rolcreatedb, rolcanlogin,
               rolreplication, rolbypassrls
        from pg_roles where rolname=any($1::name[]) order by rolname
      `, [['solum_api', 'solum_runtime']]);
      assert.deepEqual(roles.rows, [
        { rolname: 'solum_api', rolsuper: false, rolinherit: false,
          rolcreaterole: false, rolcreatedb: false, rolcanlogin: false,
          rolreplication: false, rolbypassrls: false },
        { rolname: 'solum_runtime', rolsuper: false, rolinherit: true,
          rolcreaterole: false, rolcreatedb: false, rolcanlogin: true,
          rolreplication: false, rolbypassrls: false },
      ]);

      const membership = await ownerPool.query(`
        select granted.rolname as role_name, member.rolname as member_name,
               memberships.admin_option, memberships.inherit_option, memberships.set_option
        from pg_auth_members memberships
        join pg_roles granted on granted.oid=memberships.roleid
        join pg_roles member on member.oid=memberships.member
        where granted.rolname='solum_api' and member.rolname='solum_runtime'
      `);
      assert.deepEqual(membership.rows,
        [{ role_name: 'solum_api', member_name: 'solum_runtime', admin_option: false,
          inherit_option: true, set_option: true }]);

      const tablePrivileges = await ownerPool.query(`
        select grantee, table_schema, table_name, privilege_type
        from information_schema.role_table_grants
        where grantee=any($1::name[]) and table_schema=any($2::name[])
        order by grantee, table_schema, table_name, privilege_type
      `, [['solum_api', 'solum_runtime'], ['auth', 'sync', 'vault', 'ai']]);
      assert.deepEqual(tablePrivileges.rows, [
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'entitlements', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'entitlements', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'usage_events', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'usage_events', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'usage_events', privilege_type: 'UPDATE' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'usage_monthly', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'usage_monthly', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'ai', table_name: 'usage_monthly', privilege_type: 'UPDATE' },
        { grantee: 'solum_api', table_schema: 'auth', table_name: 'refresh_tokens', privilege_type: 'DELETE' },
        { grantee: 'solum_api', table_schema: 'auth', table_name: 'refresh_tokens', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'auth', table_name: 'refresh_tokens', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'auth', table_name: 'refresh_tokens', privilege_type: 'UPDATE' },
        { grantee: 'solum_api', table_schema: 'auth', table_name: 'users', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'auth', table_name: 'users', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'alerts', privilege_type: 'DELETE' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'alerts', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'alerts', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'blobs', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'blobs', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'devices', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'devices', privilege_type: 'SELECT' },
        { grantee: 'solum_api', table_schema: 'sync', table_name: 'devices', privilege_type: 'UPDATE' },
        { grantee: 'solum_api', table_schema: 'vault', table_name: 'key_envelopes', privilege_type: 'INSERT' },
        { grantee: 'solum_api', table_schema: 'vault', table_name: 'key_envelopes', privilege_type: 'SELECT' },
      ]);
      const runtime = await runtimePool.query(
        "select current_user, pg_has_role(current_user, 'solum_api', 'member') as is_api_member"
      );
      assert.deepEqual(runtime.rows, [{ current_user: 'solum_runtime', is_api_member: true }]);
    } finally {
      await Promise.all([ownerPool.end(), runtimePool.end()]);
    }
  });

test('PostgreSQL schema repairs rows with multiple legacy alert violations',
  { skip: !ownerDatabaseUrl || !disposableDatabase }, async () => {
    const pool = new Pool({ connectionString: ownerDatabaseUrl, max: 1 });
    const client = await pool.connect();
    try {
      await client.query('begin');
      const tenantId = '00000000-0000-4000-8000-000000000001';
      await client.query(`
        alter table sync.alerts drop constraint if exists alerts_latency_ms_range;
        alter table sync.alerts drop constraint if exists alerts_ping_latency_ms_range;
        alter table sync.alerts drop constraint if exists alerts_availability_7d_range;
        alter table sync.alerts drop constraint if exists alerts_detail_url_scheme
      `);
      await client.query(`
        insert into auth.users(id, username, password_hash, password_salt)
        values($1, 'legacy-schema-audit', 'unused', 'unused')
        on conflict (id) do nothing
      `, [tenantId]);
      await client.query(`
        insert into sync.alerts(
          tenant_id, event_id, source, status, latency_ms, ping_latency_ms,
          availability_7d, detail_url
        ) values($1, 'legacy-invalid-alert', 'audit', 'down', -1, 86400001,
          100.1, 'javascript:alert(1)')
        on conflict (tenant_id, event_id) do update set
          latency_ms = excluded.latency_ms,
          ping_latency_ms = excluded.ping_latency_ms,
          availability_7d = excluded.availability_7d,
          detail_url = excluded.detail_url
      `, [tenantId]);

      const schemaPath = path.join(__dirname, '..', 'db', 'init', '01-schema.sql');
      const schema = fs.readFileSync(schemaPath, 'utf8')
        .replace(/^\\set ON_ERROR_STOP on\r?\n/, '');
      await client.query(schema);

      const repaired = await client.query(
        `select latency_ms, ping_latency_ms, availability_7d, detail_url
         from sync.alerts where tenant_id=$1 and event_id='legacy-invalid-alert'`,
        [tenantId]
      );
      assert.deepEqual(repaired.rows, [{
        latency_ms: null,
        ping_latency_ms: null,
        availability_7d: null,
        detail_url: null,
      }]);

      const constraints = await client.query(
        `select conname, convalidated from pg_constraint
         where conrelid='sync.alerts'::regclass and conname=any($1::text[])`,
        [[
          'alerts_latency_ms_range', 'alerts_ping_latency_ms_range',
          'alerts_availability_7d_range', 'alerts_detail_url_scheme',
        ]]
      );
      assert.equal(constraints.rowCount, 4);
      assert.equal(constraints.rows.every((row) => row.convalidated), true);
    } finally {
      await client.query('rollback').catch(() => {});
      client.release();
      await pool.end();
    }
  });

test('legacy SQLite importer preserves active sessions, sanitizes alerts and advances identities',
  { skip: !ownerDatabaseUrl || !databaseUrl || !disposableDatabase }, async () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-pg-import-'));
    const cloudPath = path.join(directory, 'cloud.sqlite');
    const relayPath = path.join(directory, 'relay.sqlite');
    const userId = crypto.randomUUID();
    const username = `legacy-${userId.slice(0, 8)}`;
    const importAuthSecret = 'import-integration-secret-'.repeat(3);
    const signedSessionId = crypto.randomUUID();
    const signedRefresh = createRefreshToken(signedSessionId, importAuthSecret);
    const nowSeconds = Math.floor(Date.now() / 1000);
    const activeHashes = [];
    const pool = new Pool({ connectionString: ownerDatabaseUrl, max: 1 });
    try {
      const cloud = new DatabaseSync(cloudPath);
      try {
        cloud.exec(`
          create table users(
            id text primary key, username text unique, password_hash text,
            password_salt text, created_at integer
          );
          create table refresh_tokens(
            token_hash text primary key, user_id text, session_id text,
            expires_at integer, created_at integer
          );
        `);
        cloud.prepare('insert into users values(?,?,?,?,?)')
          .run(userId, username, 'legacy-hash', 'legacy-salt',
            nowSeconds - 2_000);
        const insertToken = cloud.prepare('insert into refresh_tokens values(?,?,?,?,?)');
        for (let index = 0; index < 35; index += 1) {
          const rawToken = index === 34 ? signedRefresh : `active-${userId}-${index}`;
          const sessionId = index === 34 ? signedSessionId : crypto.randomUUID();
          const hash = crypto.createHash('sha256').update(rawToken).digest('hex');
          activeHashes.push(hash);
          insertToken.run(hash, userId, sessionId, nowSeconds + 3_600, nowSeconds - 1_000 + index);
        }
        const expiredHash = crypto.createHash('sha256').update(`expired-${userId}`).digest('hex');
        insertToken.run(expiredHash, userId, crypto.randomUUID(), nowSeconds - 1, nowSeconds + 1);
      } finally {
        cloud.close();
      }

      const relay = new DatabaseSync(relayPath);
      try {
        relay.exec(`
          create table blobs(
            seq integer primary key, tenant_id text, device text, blob blob, received_at text
          );
          create table alerts(
            seq integer primary key, tenant_id text, event_id text, source text,
            monitor_id text, name text, status text, latency_ms integer,
            ping_latency_ms integer, availability_7d real, checked_at text,
            detail_url text, received_at text
          );
        `);
        relay.prepare('insert into blobs values(?,?,?,?,?)').run(
          41, userId, 'legacy-device', Buffer.from([1, 2, 3]), '2026-08-13T00:00:00Z');
        relay.prepare('insert into alerts values(?,?,?,?,?,?,?,?,?,?,?,?,?)').run(
          73, userId, 'legacy-invalid-alert', 'legacy-monitor', null, 'Legacy alert',
          'down', -1, 86_400_001, 100.1, '2026-08-13T00:00:00Z',
          'javascript:alert(1)', '2026-08-13T00:00:01Z');
      } finally {
        relay.close();
      }

      const legacyData = loadLegacy(cloudPath, relayPath);
      await importLegacy(pool, legacyData);

      const importedUser = await pool.query(
        'select password_version from auth.users where id=$1', [userId]
      );
      assert.deepEqual(importedUser.rows, [{ password_version: 1 }]);

      const sessions = await pool.query(`
        select encode(token_hash, 'hex') as token_hash, session_id::text,
               encode(legacy_token_hash, 'hex') as legacy_token_hash,
               previous_token_hash, rotated_at
        from auth.refresh_tokens where user_id=$1 order by created_at desc
      `, [userId]);
      assert.equal(sessions.rowCount, 32);
      assert.deepEqual(sessions.rows.map((row) => row.token_hash), activeHashes.slice(-32).reverse());
      assert.equal(new Set(sessions.rows.map((row) => row.session_id)).size, 32);
      assert.equal(sessions.rows.find((row) => row.token_hash === activeHashes[34]).session_id,
        signedSessionId);
      for (const row of sessions.rows) {
        assert.match(row.session_id,
          /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
        assert.equal(row.legacy_token_hash, row.token_hash);
        assert.equal(row.previous_token_hash, null);
        assert.equal(row.rotated_at, null);
      }

      const alert = await pool.query(`
        select latency_ms, ping_latency_ms, availability_7d, detail_url
        from sync.alerts where tenant_id=$1 and event_id='legacy-invalid-alert'
      `, [userId]);
      assert.deepEqual(alert.rows, [{ latency_ms: null, ping_latency_ms: null,
        availability_7d: null, detail_url: null }]);

      const nextBlob = await pool.query(`
        insert into sync.blobs(tenant_id,device_id,ciphertext)
        values($1,'next-device',$2) returning seq
      `, [userId, Buffer.from([4])]);
      const nextAlert = await pool.query(`
        insert into sync.alerts(tenant_id,event_id,source,status)
        values($1,'next-alert','integration','operational') returning seq
      `, [userId]);
      assert.equal(Number(nextBlob.rows[0].seq), 42);
      assert.equal(Number(nextAlert.rows[0].seq), 74);

      const runtimePool = new Pool({ connectionString: databaseUrl, max: 2 });
      const importedApp = await createPostgresServer({
        pool: runtimePool,
        authSecret: importAuthSecret,
        adminUsername: username,
        adminPassword: 'unused-import-admin-password',
        registrationMode: 'closed',
        buildRevision: 'abcdef0',
      });
      const importedPort = await listen(importedApp.server);
      try {
        const refreshed = await jsonRequest(`http://127.0.0.1:${importedPort}`, '/v1/auth/refresh', {
          method: 'POST', body: { refresh_token: signedRefresh },
        });
        assert.equal(refreshed.response.status, 200);
        assert.equal(refreshed.data.user.id, userId);
        assert.notEqual(refreshed.data.refresh_token, signedRefresh);
      } finally {
        await importedApp.close();
        await runtimePool.end();
      }

      await assert.rejects(importLegacy(pool, legacyData),
        /target PostgreSQL center is not empty/);
      const preserved = await pool.query(`
        select (select count(*)::integer from auth.users where id=$1) as users,
               (select count(*)::integer from auth.refresh_tokens where user_id=$1) as sessions,
               (select count(*)::integer from sync.blobs where tenant_id=$1) as blobs,
               (select count(*)::integer from sync.alerts where tenant_id=$1) as alerts
      `, [userId]);
      assert.deepEqual(preserved.rows, [{ users: 1, sessions: 32, blobs: 2, alerts: 2 }]);
    } finally {
      await pool.query('delete from auth.users where id=$1', [userId]).catch(() => {});
      await pool.end();
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });

test('PostgreSQL runtime enforces auth, RLS, quotas and JSON contracts',
  { skip: !databaseUrl || !disposableDatabase }, async (t) => {
    const upstream = http.createServer(async (req, res) => {
      for await (const _chunk of req) { /* drain */ }
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({
        choices: [{ message: { content: 'ok' } }],
        usage: { prompt_tokens: 4, completion_tokens: 2, total_tokens: 6 },
      }));
    });
    const upstreamPort = await listen(upstream);
    t.after(async () => {
      upstream.closeAllConnections();
      await new Promise((resolve) => upstream.close(resolve));
    });

    const pool = new Pool({ connectionString: databaseUrl, max: 4 });
    const ownerPool = new Pool({ connectionString: ownerDatabaseUrl, max: 1 });
    const app = await createPostgresServer({
      pool,
      authSecret: 'integration-secret-'.repeat(4),
      adminUsername: ' integration-admin ',
      adminPassword: 'integration-admin-password',
      registrationMode: 'open',
      allowedOrigins: 'https://allowed.example',
      trustProxy: true,
      mimoApiKey: 'server-only',
      mimoBaseUrl: `http://127.0.0.1:${upstreamPort}/v1`,
      defaultModel: 'mimo-v2.5',
      allowedModels: 'mimo-v2.5,mimo-v2.5-pro',
      standardPlan: 'standard',
      standardMonthlyRequestLimit: 10,
      aiMaxResponseBytes: 1024 * 1024,
      aiPendingTimeoutSeconds: 600,
      alertRetentionDays: 7,
      tenantMaxBlobBytes: 10,
      tenantMaxBlobs: 2,
      tenantMaxDevices: 1,
      buildRevision: 'abcdef0',
    });
    assert.equal(app.server.headersTimeout, 15_000);
    assert.equal(app.server.requestTimeout, 60_000);
    assert.equal(app.server.keepAliveTimeout, 5_000);
    assert.equal(app.server.maxHeadersCount, 64);
    const port = await listen(app.server);
    const base = `http://127.0.0.1:${port}`;
    const runId = crypto.randomBytes(5).toString('hex');
    const aliceUsername = `alice-${runId}`;
    const bobUsername = `bob-${runId}`;
    const legacyUsername = `legacy-space-${runId}`;
    const legacyBootstrapUsername = `legacy-bootstrap-${runId}`;
    const rateUsername = `rate-${runId}`;
    t.after(async () => {
      app.server.closeAllConnections();
      await app.close();
      await Promise.all([pool.end(), ownerPool.end()]);
    });

    const health = await jsonRequest(base, '/v1/health');
    assert.equal(health.data.storage, 'postgres');
    assert.equal(health.data.revision, 'abcdef0');
    assert.match(health.data.version, /^\d+\.\d+\.\d+$/);
    const constraints = await pool.query(
      `select conname,convalidated from pg_constraint
       where conrelid='sync.alerts'::regclass and conname=any($1::text[])`,
      [[
        'alerts_latency_ms_range', 'alerts_ping_latency_ms_range',
        'alerts_availability_7d_range', 'alerts_detail_url_scheme',
      ]]
    );
    assert.equal(constraints.rowCount, 4);
    assert.equal(constraints.rows.every((row) => row.convalidated), true);

    for (const route of ['login', 'refresh', 'logout']) {
      const nullBody = await fetch(`${base}/v1/auth/${route}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' }, body: 'null',
      });
      assert.equal(nullBody.status, 400);
    }

    const adminLogin = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: {
        username: 'integration-admin', password: 'integration-admin-password',
      },
    });
    assert.equal(adminLogin.response.status, 200,
      'bootstrap username must use the same trimming rule as login');
    assert.equal(adminLogin.data.user.username, 'integration-admin');

    const noMediaType = await fetch(`${base}/v1/auth/login`, {
      method: 'POST', body: JSON.stringify({ username: 'x', password: 'x' }),
    });
    assert.equal(noMediaType.status, 415);
    const deniedOrigin = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: 'x', password: 'x' },
      headers: { Origin: 'https://evil.example' },
    });
    assert.equal(deniedOrigin.response.status, 403);

    const password = '  exact integration password  ';
    const aliceRegistration = await jsonRequest(base, '/v1/auth/register', {
      method: 'POST', body: { username: aliceUsername, password },
      headers: { 'CF-Connecting-IP': '203.0.113.10' },
    });
    assert.equal(aliceRegistration.response.status, 201);
    const alice = aliceRegistration.data;
    assert.match(alice.user.id, /^[0-9a-f-]{36}$/);

    const trimmedLogin = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: aliceUsername, password: password.trim() },
      headers: { 'CF-Connecting-IP': '203.0.113.10' },
    });
    assert.equal(trimmedLogin.response.status, 401);
    const exactLogin = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: aliceUsername, password },
      headers: { 'CF-Connecting-IP': '203.0.113.10' },
    });
    assert.equal(exactLogin.response.status, 200);

    const legacyPassword = '  original legacy recovery password  ';
    const legacySalt = '00112233445566778899aabbccddeeff';
    const legacyHash = crypto.scryptSync(
      legacyPassword.trim(), legacySalt, 64
    ).toString('hex');
    await pool.query(
      `insert into auth.users(username,password_hash,password_salt,password_version)
       values($1,$2,$3,1)`,
      [legacyUsername, legacyHash, legacySalt]
    );
    const legacyPadded = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: legacyUsername, password: legacyPassword },
      headers: { 'CF-Connecting-IP': '203.0.113.13' },
    });
    assert.equal(legacyPadded.response.status, 200);
    const legacyVersion = await pool.query(
      'select password_version from auth.users where username=$1', [legacyUsername]
    );
    assert.deepEqual(legacyVersion.rows, [{ password_version: 1 }]);
    const legacyEffective = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: legacyUsername, password: legacyPassword.trim() },
      headers: { 'CF-Connecting-IP': '203.0.113.13' },
    });
    assert.equal(legacyEffective.response.status, 200);
    const legacyVersionAfterFallback = await pool.query(
      'select password_version from auth.users where username=$1', [legacyUsername]
    );
    assert.deepEqual(legacyVersionAfterFallback.rows, [{ password_version: 1 }]);

    const legacyBootstrapPassword = '  exact legacy bootstrap password  ';
    const legacyBootstrapSalt = 'ffeeddccbbaa00998877665544332211';
    const legacyBootstrapHash = crypto.scryptSync(
      legacyBootstrapPassword, legacyBootstrapSalt, 64
    ).toString('hex');
    await pool.query(
      `insert into auth.users(username,password_hash,password_salt,password_version)
       values($1,$2,$3,1)`,
      [legacyBootstrapUsername, legacyBootstrapHash, legacyBootstrapSalt]
    );
    const legacyBootstrap = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: {
        username: legacyBootstrapUsername, password: legacyBootstrapPassword,
      },
      headers: { 'CF-Connecting-IP': '203.0.113.14' },
    });
    assert.equal(legacyBootstrap.response.status, 200);
    const trimmedBootstrap = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: {
        username: legacyBootstrapUsername, password: legacyBootstrapPassword.trim(),
      },
      headers: { 'CF-Connecting-IP': '203.0.113.14' },
    });
    assert.equal(trimmedBootstrap.response.status, 401);

    const rotated = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: exactLogin.data.refresh_token },
    });
    assert.equal(rotated.response.status, 200);
    const recoveredRotation = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: exactLogin.data.refresh_token },
    });
    assert.equal(recoveredRotation.response.status, 200);
    assert.equal(recoveredRotation.data.refresh_token, rotated.data.refresh_token,
      'a retry after a lost response must recover the same successor token');
    const bobRegistration = await jsonRequest(base, '/v1/auth/register', {
      method: 'POST', body: { username: bobUsername, password: 'bob-integration-password' },
      headers: { 'CF-Connecting-IP': '203.0.113.11' },
    });
    assert.equal(bobRegistration.response.status, 201);
    const paddedBob = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST',
      body: { username: bobUsername, password: ' bob-integration-password ' },
      headers: { 'CF-Connecting-IP': '203.0.113.11' },
    });
    assert.equal(paddedBob.response.status, 401);
    const legacyDevice = await pool.connect();
    try {
      await legacyDevice.query('begin');
      await legacyDevice.query("select set_config('app.current_tenant_id', $1, true)",
        [bobRegistration.data.user.id]);
      await legacyDevice.query(
        `insert into sync.devices(tenant_id,device_id,revoked_at)
         values($1,'revoked-legacy-device',now())`, [bobRegistration.data.user.id]
      );
      await legacyDevice.query('commit');
    } catch (error) {
      await legacyDevice.query('rollback').catch(() => {});
      throw error;
    } finally {
      legacyDevice.release();
    }

    const push = async (token, device, bytes) => fetch(`${base}/v1/push`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, 'X-Device': device,
        'Content-Type': 'application/octet-stream' },
      body: Buffer.alloc(bytes, 7),
    });
    assert.equal((await push(rotated.data.access_token, 'alice-device', 6)).status, 200);
    assert.equal((await push(bobRegistration.data.access_token, 'bob-device', 4)).status, 200);
    assert.equal((await push(rotated.data.access_token, 'alice-device', 5)).status, 507);
    const extraDevice = await fetch(`${base}/v1/pull?since=0&device=alice-device-2`, {
      headers: { Authorization: `Bearer ${rotated.data.access_token}` },
    });
    assert.equal(extraDevice.status, 429);

    const aliceStats = await jsonRequest(base, '/v1/stats', { token: rotated.data.access_token });
    const bobStats = await jsonRequest(base, '/v1/stats', { token: bobRegistration.data.access_token });
    assert.equal(aliceStats.data.total_blobs, 1);
    assert.equal(bobStats.data.total_blobs, 1);
    assert.equal(aliceStats.data.total_bytes, 6);
    assert.equal(bobStats.data.total_bytes, 4);
    await ownerPool.query("update auth.users set status='disabled' where id=$1",
      [bobRegistration.data.user.id]);
    const disabledAccess = await jsonRequest(base, '/v1/stats', {
      token: bobRegistration.data.access_token,
    });
    assert.equal(disabledAccess.response.status, 401,
      'disabling an account must revoke its existing access token immediately');
    await ownerPool.query("update auth.users set status='active' where id=$1",
      [bobRegistration.data.user.id]);

    for (const invalid of [
      { latency_ms: -1 },
      { availability_7d: 100.1 },
      { detail_url: 'javascript:alert(1)' },
    ]) {
      const result = await jsonRequest(base, '/v1/alerts', {
        method: 'POST', token: rotated.data.access_token,
        body: { event_id: `bad-${Object.keys(invalid)[0]}`, source: 'test',
          status: 'operational', ...invalid },
      });
      assert.equal(result.response.status, 400);
    }
    const alert = await jsonRequest(base, '/v1/alerts', {
      method: 'POST', token: rotated.data.access_token,
      body: { event_id: 'good-alert', source: 'test', status: 'operational',
        latency_ms: 12, ping_latency_ms: 3, availability_7d: 99.9,
        detail_url: 'https://example.com/status' },
    });
    assert.equal(alert.response.status, 200);
    const alerts = await jsonRequest(base, '/v1/alerts?since=0&limit=10', {
      token: rotated.data.access_token,
    });
    assert.equal(alerts.data.alerts.length, 1);
    assert.equal(typeof alerts.data.alerts[0].seq, 'number');

    const completion = await jsonRequest(base, '/v1/ai/chat/completions', {
      method: 'POST', token: rotated.data.access_token,
      body: { messages: [{ role: 'user', content: 'hello' }] },
    });
    assert.equal(completion.response.status, 200);
    const usage = await jsonRequest(base, '/v1/ai/usage', { token: rotated.data.access_token });
    assert.equal(usage.data.total_tokens, 6);
    assert.equal(typeof usage.data.latest_request.upstream_latency_ms, 'number');

    const logoutSeed = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: aliceUsername, password },
      headers: { 'CF-Connecting-IP': '203.0.113.20' },
    });
    const logoutDescendant = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: logoutSeed.data.refresh_token },
    });
    const logoutOld = await jsonRequest(base, '/v1/auth/logout', {
      method: 'POST', body: { refresh_token: logoutSeed.data.refresh_token },
    });
    assert.equal(logoutOld.response.status, 200);
    const revokedDescendant = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: logoutDescendant.data.refresh_token },
    });
    assert.equal(revokedDescendant.response.status, 401);

    const replaySeed = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: aliceUsername, password },
      headers: { 'CF-Connecting-IP': '203.0.113.21' },
    });
    const replayDescendant = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: replaySeed.data.refresh_token },
    });
    const replayRecovery = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: replaySeed.data.refresh_token },
    });
    assert.equal(replayRecovery.response.status, 200);
    assert.equal(replayRecovery.data.refresh_token, replayDescendant.data.refresh_token);
    const replayGrandchild = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: replayDescendant.data.refresh_token },
    });
    const replay = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: replaySeed.data.refresh_token },
    });
    assert.equal(replay.response.status, 401);
    const revokedAfterReplay = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: replayGrandchild.data.refresh_token },
    });
    assert.equal(revokedAfterReplay.response.status, 401);

    const raceSeed = await jsonRequest(base, '/v1/auth/login', {
      method: 'POST', body: { username: aliceUsername, password },
      headers: { 'CF-Connecting-IP': '203.0.113.22' },
    });
    const [racedRefresh, racedLogout] = await Promise.all([
      jsonRequest(base, '/v1/auth/refresh', {
        method: 'POST', body: { refresh_token: raceSeed.data.refresh_token },
      }),
      jsonRequest(base, '/v1/auth/logout', {
        method: 'POST', body: { refresh_token: raceSeed.data.refresh_token },
      }),
    ]);
    assert.equal(racedLogout.response.status, 200);
    assert.ok([200, 401].includes(racedRefresh.response.status));
    if (racedRefresh.response.status === 200) {
      const racedDescendant = await jsonRequest(base, '/v1/auth/refresh', {
        method: 'POST', body: { refresh_token: racedRefresh.data.refresh_token },
      });
      assert.equal(racedDescendant.response.status, 401);
    }

    for (let start = 0; start < 35; start += 8) {
      const batch = await Promise.all(Array.from(
        { length: Math.min(8, 35 - start) }, (_, offset) => jsonRequest(base, '/v1/auth/login', {
          method: 'POST', body: { username: aliceUsername, password },
          headers: { 'CF-Connecting-IP': `198.51.100.${start + offset + 1}` },
        })
      ));
      for (const login of batch) assert.equal(login.response.status, 200);
    }
    const activeSessions = await pool.query(
      'select count(*)::integer as count from auth.refresh_tokens where user_id=$1', [alice.user.id]
    );
    assert.equal(activeSessions.rows[0].count, 32);

    const logout = await jsonRequest(base, '/v1/auth/logout', {
      method: 'POST', body: { refresh_token: rotated.data.refresh_token },
    });
    assert.equal(logout.response.status, 200);
    const afterLogout = await jsonRequest(base, '/v1/auth/refresh', {
      method: 'POST', body: { refresh_token: rotated.data.refresh_token },
    });
    assert.equal(afterLogout.response.status, 401);

    for (let index = 0; index < 5; index += 1) {
      const attempt = await jsonRequest(base, '/v1/auth/register', {
        method: 'POST', body: { username: '', password: '' },
        headers: { 'CF-Connecting-IP': '203.0.113.99' },
      });
      assert.equal(attempt.response.status, 400);
    }
    const limited = await jsonRequest(base, '/v1/auth/register', {
      method: 'POST', body: { username: rateUsername, password: 'rate-limit-password' },
      headers: { 'CF-Connecting-IP': '203.0.113.99' },
    });
    assert.equal(limited.response.status, 429);
  });

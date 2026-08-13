'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { DatabaseSync } = require('node:sqlite');
const { verifyAccessToken, withTenant } = require('../src/postgres-server');
const { loadLegacy } = require('../scripts/import-legacy-sqlite');

const root = path.join(__dirname, '..');

test('PostgreSQL schema forces RLS on every tenant-owned table', () => {
  const sql = fs.readFileSync(path.join(root, 'db', 'init', '01-schema.sql'), 'utf8');
  for (const table of ['sync.devices', 'sync.blobs', 'sync.preferences', 'sync.alerts',
    'vault.key_envelopes']) {
    assert.match(sql, new RegExp(`alter table ${table.replace('.', '\\.')}` +
      ' enable row level security;', 'i'));
    assert.match(sql, new RegExp(`alter table ${table.replace('.', '\\.')}` +
      ' force row level security;', 'i'));
  }
  assert.match(sql, /current_setting\('app\.current_tenant_id', true\)/);
  assert.match(sql, /create index if not exists blobs_tenant_seq_idx\s+on sync\.blobs \(tenant_id, seq\)/i);
  assert.doesNotMatch(sql, /grant all/i);
});

test('tenant transaction sets a local database context and always clears it by commit or rollback', async () => {
  const calls = [];
  const client = {
    async query(text, values) {
      calls.push([text, values]);
      return { rows: [] };
    },
    release() { calls.push(['release']); },
  };
  const pool = { async connect() { return client; } };
  const tenant = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  await withTenant(pool, tenant, async (connection) => {
    assert.equal(connection, client);
    await connection.query('select * from sync.blobs');
  });
  assert.deepEqual(calls.slice(0, 3), [
    ['begin', undefined],
    ["select set_config('app.current_tenant_id', $1, true)", [tenant]],
    ['select * from sync.blobs', undefined],
  ]);
  assert.deepEqual(calls.slice(-2), [['commit', undefined], ['release']]);

  calls.length = 0;
  await assert.rejects(withTenant(pool, tenant, async () => { throw new Error('stop'); }), /stop/);
  assert.equal(calls.at(-2)[0], 'rollback');
  assert.equal(calls.at(-1)[0], 'release');
});

test('access tokens accept only signed, unexpired UUID tenants', () => {
  const secret = 'a'.repeat(64);
  const sign = (sub, exp) => {
    const payload = Buffer.from(JSON.stringify({ sub, exp })).toString('base64url');
    return `${payload}.${crypto.createHmac('sha256', secret).update(payload).digest('base64url')}`;
  };
  const tenant = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  assert.equal(verifyAccessToken(sign(tenant, Math.floor(Date.now() / 1000) + 60), secret), tenant);
  assert.equal(verifyAccessToken(sign('alice', Math.floor(Date.now() / 1000) + 60), secret), null);
  assert.equal(verifyAccessToken(sign(tenant, 1), secret), null);
  assert.equal(verifyAccessToken(`${sign(tenant, 4102444800)}x`, secret), null);
});

test('compose exposes only the API and keeps PostgreSQL on the private network', () => {
  const compose = fs.readFileSync(path.join(root, 'compose.yaml'), 'utf8');
  const postgres = compose.slice(compose.indexOf('\n  postgres:\n'), compose.indexOf('\n  solum-cloud:\n'));
  const cloud = compose.slice(compose.indexOf('\n  solum-cloud:\n'), compose.indexOf('\n  migrate:\n'));
  assert.doesNotMatch(postgres, /\n\s+ports:/);
  assert.match(postgres, /networks: \[backend\]/);
  assert.match(cloud, /ports:\s*\n\s+- "127\.0\.0\.1:/);
  assert.match(cloud, /networks: \[backend, edge\]/);
  assert.match(cloud, /127\.0\.0\.1:8787\/v1\/health/);
});

test('center dashboard provides one-origin registration and account workspace', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  const dashboard = fs.readFileSync(path.join(root, 'src', 'dashboard.html'), 'utf8');
  assert.match(source, /url\.pathname === '\/'/);
  assert.match(source, /url\.pathname === '\/v1\/meta'/);
  assert.match(dashboard, /`\/v1\/auth\/\$\{mode\}`/);
  assert.match(dashboard, /data-mode="register"/);
  assert.match(dashboard, /data-mode="login"/);
  assert.match(dashboard, /\/v1\/devices/);
  assert.match(dashboard, /\/v1\/alerts/);
  assert.doesNotMatch(dashboard, /localStorage|sessionStorage/);
});

test('recovery envelope is fixed-scope and create-only', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  assert.match(source, /PUT'.*\/v1\/keys\/recovery/);
  assert.match(source, /recipient_device_id='account-recovery-v1'/);
  assert.match(source, /on conflict\(tenant_id,recipient_device_id,key_version,algorithm\) do nothing/);
  assert.match(source, /device === 'account-recovery-v1'/);
  assert.doesNotMatch(source, /last_seen_at=excluded\.last_seen_at,revoked_at=null/);
  assert.equal((source.match(/where sync\.devices\.revoked_at is null returning device_id/g) || []).length, 3);
  assert.equal((source.match(/throw new HttpError\(403, 'device_revoked'\)/g) || []).length, 3);
});

test('legacy importer preserves tenant ownership and cursor sequences atomically', () => {
  const source = fs.readFileSync(path.join(root, 'scripts', 'import-legacy-sqlite.js'), 'utf8');
  assert.match(source, /new DatabaseSync\([^,]+, \{ readOnly: true \}\)/);
  assert.match(source, /target PostgreSQL center is not empty/);
  assert.match(source, /overriding system value values\(\$1/);
  assert.match(source, /pg_get_serial_sequence/);
  assert.match(source, /await client\.query\('begin'\)/);
  assert.match(source, /await client\.query\('commit'\)/);
  assert.match(source, /await client\.query\('rollback'\)/);
  assert.doesNotMatch(source, /console\.(?:log|error).*password|console\.(?:log|error).*tokenHash/);
});

test('legacy importer maps username tenants and decodes refresh hashes', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-import-'));
  const cloudPath = path.join(directory, 'cloud.sqlite');
  const relayPath = path.join(directory, 'relay.sqlite');
  try {
    const cloud = new DatabaseSync(cloudPath);
    cloud.exec(`
      create table users(username text primary key,password_hash text,password_salt text,created_at integer);
      create table refresh_tokens(token_hash text primary key,username text,expires_at integer,created_at integer);
      insert into users values('alice','hash','salt',1700000000);
      insert into refresh_tokens values('${'ab'.repeat(32)}','alice',1800000000,1700000001);`);
    cloud.close();
    const relay = new DatabaseSync(relayPath);
    relay.exec(`
      create table blobs(seq integer primary key,tenant_id text,device text,blob blob,received_at text);
      create table alerts(seq integer primary key,tenant_id text,event_id text,source text,
        monitor_id text,name text,status text,latency_ms integer,ping_latency_ms integer,
        availability_7d real,checked_at text,detail_url text,received_at text);
      insert into blobs values(8,'alice','desktop',x'0102','2026-08-13T00:00:00Z');
      insert into alerts values(72,'alice','evt','benefit-monitor','m','福利版','operational',10,
        3,100.0,'2026-08-13T00:00:00Z','https://example.com','2026-08-13T00:00:01Z');`);
    relay.close();

    const data = loadLegacy(cloudPath, relayPath);
    assert.match(data.users[0].id, /^[0-9a-f-]{36}$/);
    assert.equal(data.refreshTokens[0].tokenHash.toString('hex'), 'ab'.repeat(32));
    assert.equal(data.blobs[0].tenantId, data.users[0].id);
    assert.equal(data.blobs[0].seq, 8);
    assert.equal(data.alerts[0].tenantId, data.users[0].id);
    assert.equal(data.alerts[0].seq, 72);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

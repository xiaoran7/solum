'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { verifyAccessToken, withTenant } = require('../src/postgres-server');

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

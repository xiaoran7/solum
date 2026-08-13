'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { DatabaseSync } = require('node:sqlite');
const {
  verifyAccessToken, withTenant, parseModelCatalog, usageProjection,
  validateCompletionOptions, readBodyLimited, recoverTenantStaleAiRequests,
} = require('../src/postgres-server');
const { loadLegacy } = require('../scripts/import-legacy-sqlite');

const root = path.join(__dirname, '..');

test('PostgreSQL schema forces RLS on every tenant-owned table', () => {
  const sql = fs.readFileSync(path.join(root, 'db', 'init', '01-schema.sql'), 'utf8');
  for (const table of ['sync.devices', 'sync.blobs', 'sync.preferences', 'sync.alerts',
    'vault.key_envelopes', 'ai.entitlements', 'ai.usage_monthly', 'ai.usage_events']) {
    assert.match(sql, new RegExp(`alter table ${table.replace('.', '\\.')}` +
      ' enable row level security;', 'i'));
    assert.match(sql, new RegExp(`alter table ${table.replace('.', '\\.')}` +
      ' force row level security;', 'i'));
  }
  assert.match(sql, /current_setting\('app\.current_tenant_id', true\)/);
  assert.match(sql, /create index if not exists blobs_tenant_seq_idx\s+on sync\.blobs \(tenant_id, seq\)/i);
  assert.match(sql, /create index if not exists usage_events_tenant_created_idx\s+on ai\.usage_events \(tenant_id, created_at desc\)/i);
  assert.match(sql, /grant select, insert, delete on sync\.blobs to solum_api;/i);
  assert.match(sql, /grant select, insert, delete on sync\.alerts to solum_api;/i);
  assert.match(sql, /grant select, insert, update, delete on auth\.refresh_tokens to solum_api;/i);
  assert.doesNotMatch(sql, /grant all/i);
});

test('deployment shell scripts are normalized to LF for Alpine sh', () => {
  const attributes = fs.readFileSync(path.join(root, '..', '.gitattributes'), 'utf8');
  assert.match(attributes, /^\*\.sh text eol=lf$/m);
  const roles = fs.readFileSync(path.join(root, 'db', 'init', '00-roles.sh'));
  assert.equal(roles.includes(Buffer.from('\r\n')), false);
});

test('AI model catalog always retains one validated default and removes duplicates', () => {
  const catalog = parseModelCatalog('mimo-v2.5', 'mimo-v2.5-pro,mimo-v2.5, custom/model');
  assert.equal(catalog.defaultModel, 'mimo-v2.5');
  assert.deepEqual(catalog.models, ['mimo-v2.5', 'mimo-v2.5-pro', 'custom/model']);
  assert.throws(() => parseModelCatalog('bad model', ''), /invalid_model/);
});

test('AI usage projection is server-authoritative and never reports negative remaining quota', () => {
  const usage = usageProjection(
    { plan: 'standard', monthly_request_limit: 10, custom_models_enabled: false },
    { period_start: '2026-08-01', request_count: 12, success_count: 8, failure_count: 4 }
  );
  assert.equal(usage.plan, 'standard');
  assert.equal(usage.custom_models_enabled, false);
  assert.equal(usage.requests_used, 12);
  assert.equal(usage.requests_remaining, 0);
  assert.equal(usage.reset_at, '2026-09-01T00:00:00.000Z');
  assert.throws(() => usageProjection(
    { plan: 'standard', monthly_request_limit: 10, custom_models_enabled: true },
    { total_tokens: '9007199254740993' }
  ), /invalid AI usage counter/);
});

test('AI quota reservation is a conditional atomic update before the usage event insert', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  assert.match(source, /usage\.request_count<entitlement\.monthly_request_limit/);
  assert.match(source, /if \(reserved\.rowCount !== 1\) throw new HttpError\(429, 'ai_quota_exhausted'\)/);
  assert.ok(source.indexOf('request_count=request_count+1') <
    source.indexOf('insert into ai.usage_events'));
  assert.match(source, /effectiveModel !== config\.modelCatalog\.defaultModel/);
  assert.match(source, /X-Solum-Effective-Model/);
  assert.match(source, /if \(!res\.write\(next\.value\)\) await waitForDrain\(res\)/);
  assert.match(source, /upstream_stream_interrupted_before_first_byte/);
  assert.match(source, /clientDisconnected\(\) \? 'interrupted' : 'failed'/);
});

test('AI completion options reject ambiguous or unbounded values', () => {
  assert.deepEqual(validateCompletionOptions({}), {
    stream: false, temperature: undefined, maxTokens: undefined,
  });
  assert.deepEqual(validateCompletionOptions({ stream: true, temperature: 0, max_tokens: 32768 }), {
    stream: true, temperature: 0, maxTokens: 32768,
  });
  for (const input of [
    null, [], { stream: 'true' }, { temperature: Number.NaN }, { temperature: -0.1 },
    { temperature: 2.1 }, { max_tokens: 0 }, { max_tokens: 1.5 }, { max_tokens: 32769 },
  ]) assert.throws(() => validateCompletionOptions(input));
});

test('non-streaming upstream bodies are bounded before buffering', async () => {
  const small = new Response(Buffer.from('1234'));
  assert.equal(await readBodyLimited(small, 4), '1234');
  const oversized = new Response(Buffer.from('12345'));
  await assert.rejects(readBodyLimited(oversized, 4), /upstream_response_too_large/);
});

test('stale pending AI calls are recovered for the active tenant and counted once', async () => {
  const calls = [];
  const client = {
    async query(text, values) { calls.push([text, values]); return { rows: [] }; },
  };
  const tenant = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  await recoverTenantStaleAiRequests(client, tenant, 600);
  const recovery = calls.find(([text]) => text.includes("status='interrupted'"));
  assert.ok(recovery);
  assert.match(recovery[0], /status='pending'/);
  assert.match(recovery[0], /failure_count=usage\.failure_count\+totals\.failures/);
  assert.deepEqual(recovery[1], [tenant, 600]);
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

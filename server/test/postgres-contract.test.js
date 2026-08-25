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
  optionalBoundedNumber, optionalHttpUrl, requiredOpaqueString, safeDatabaseInteger,
  parseBackupPlan, parseTakeover, parseWriterClaim, parseIfMatch, buildConfig,
} = require('../src/postgres-server');
const {
  createRefreshToken,
  createRefreshSuccessorToken,
  createConcurrencyLimiter,
  createFixedWindowLimiter, isJsonContentType, passwordHashAsync,
  refreshTokenSessionId, requestClientAddress, requestOriginAllowed,
  verifyPasswordAsync,
} = require('../src/http-security');
const { loadLegacy } = require('../scripts/import-legacy-sqlite');

const root = path.join(__dirname, '..');

test('PostgreSQL schema forces RLS on every tenant-owned table', () => {
  const sql = fs.readFileSync(path.join(root, 'db', 'init', '01-schema.sql'), 'utf8');
  for (const table of ['sync.devices', 'sync.blobs', 'sync.preferences', 'sync.alerts',
    'vault.key_envelopes', 'vault.recovery_codes', 'vault.backup_state', 'vault.backups',
    'vault.backup_objects', 'ai.entitlements', 'ai.usage_monthly', 'ai.usage_events']) {
    assert.match(sql, new RegExp(`alter table ${table.replace('.', '\\.')}` +
      ' enable row level security;', 'i'));
    assert.match(sql, new RegExp(`alter table ${table.replace('.', '\\.')}` +
      ' force row level security;', 'i'));
  }
  assert.match(sql, /current_setting\('app\.current_tenant_id', true\)/);
  assert.match(sql, /create index if not exists blobs_tenant_seq_idx\s+on sync\.blobs \(tenant_id, seq\)/i);
  assert.match(sql, /create index if not exists usage_events_tenant_created_idx\s+on ai\.usage_events \(tenant_id, created_at desc\)/i);
  assert.match(sql, /revoke all on all tables in schema auth, sync, vault, ai from solum_api, solum_runtime;/i);
  assert.match(sql, /grant select, insert on auth\.users to solum_api;/i);
  assert.match(sql, /grant select, insert on sync\.blobs to solum_api;/i);
  assert.match(sql, /grant select, insert, delete on sync\.alerts to solum_api;/i);
  assert.match(sql, /grant select, insert on vault\.key_envelopes to solum_api;/i);
  assert.match(sql, /grant select, insert, update on vault\.recovery_codes to solum_api;/i);
  // The password change endpoint may only touch credential columns.
  assert.match(sql,
    /grant update \(password_hash, password_salt, password_version\) on auth\.users to solum_api;/i);
  assert.match(sql, /grant select, insert, update on vault\.backup_state to solum_api;/i);
  assert.match(sql, /grant select, insert, update, delete on vault\.backups to solum_api;/i);
  assert.match(sql, /grant select, insert, update on vault\.backup_objects to solum_api;/i);
  assert.match(sql, /grant select, insert, update, delete on auth\.refresh_tokens to solum_api;/i);
  assert.doesNotMatch(sql, /grant[^;]*delete[^;]*on sync\.blobs/i);
  // No table-level UPDATE on credentials or envelopes; the password change
  // path is limited to an explicit column-scoped grant.
  assert.doesNotMatch(sql, /grant update on (?:auth\.users|vault\.key_envelopes)\b/i);
  assert.match(sql, /users_password_version_check/i);
  assert.match(sql, /refresh_tokens_session_idx\s+on auth\.refresh_tokens \(session_id\)/i);
  assert.match(sql, /refresh_tokens_previous_idx\s+on auth\.refresh_tokens \(previous_token_hash\)/i);
  assert.match(sql, /alerts_detail_url_scheme/i);
  assert.match(sql, /validate constraint alerts_latency_ms_range/i);
  assert.match(sql, /validate constraint alerts_ping_latency_ms_range/i);
  assert.match(sql, /validate constraint alerts_availability_7d_range/i);
  assert.match(sql, /validate constraint alerts_detail_url_scheme/i);
  assert.match(sql, /alerts_tenant_received_at_idx\s+on sync\.alerts \(tenant_id, received_at\)/i);
  assert.match(sql, /backups_tenant_complete_idx\s+on vault\.backups \(tenant_id, committed_at desc, backup_id desc\)/i);
  assert.match(sql, /foreign key \(tenant_id, backup_id\) references vault\.backups\(tenant_id, backup_id\)\s+on delete cascade/i);
  for (const policy of ['backup_state_tenant_policy', 'backups_tenant_policy',
    'backup_objects_tenant_policy']) assert.match(sql, new RegExp(policy, 'i'));
  assert.doesNotMatch(sql, /grant all/i);
});

test('backup upload plans have a strict cross-platform canonical digest', () => {
  const config = buildConfig({
    authSecret: 'x'.repeat(32), adminUsername: 'admin', adminPassword: '',
  });
  const main = {
    object_id: '22222222-2222-4222-8222-222222222222', kind: 'main', bytes: 123,
    sha256: '00'.repeat(32),
  };
  const attachment = {
    object_id: '33333333-3333-4333-8333-333333333333', kind: 'attachment', bytes: 456,
    sha256: '11'.repeat(32),
  };
  const body = {
    backup_id: '11111111-1111-4111-8111-111111111111',
    generation: 7,
    created_at: '2026-01-02T03:04:05.000Z',
    protocol_version: 1,
    objects: [attachment, main],
  };
  const parsed = parseBackupPlan(body, config);
  assert.deepEqual(parsed.objects, [main, attachment]);
  assert.equal(parsed.totalBytes, 579);
  assert.equal(parsed.planSha256.toString('hex'),
    'ace8fa6bbf286d860abacaa16ce605a92da9cf67a9816cc9e8a37d7ee344338c');
  assert.equal(parseBackupPlan({ ...body, objects: [main, attachment] }, config)
    .planSha256.equals(parsed.planSha256), true);
  assert.equal(parseBackupPlan({ ...body, generation: 1, objects: [main] }, config)
    .planSha256.toString('hex'),
    '3ae4b633307d12c2f82fac4ddb36539f5ed8d6c5a18e5e9a09a68d2b96979528');

  for (const invalid of [
    { ...body, extra: true },
    { ...body, protocol_version: 2 },
    { ...body, created_at: '2026-01-02T03:04:05Z' },
    { ...body, created_at: '0000-01-02T03:04:05.000Z' },
    { ...body, objects: [main, main] },
    { ...body, objects: [attachment] },
    { ...body, generation: Number.MAX_SAFE_INTEGER + 1 },
    { ...body, backup_id: '00000000-0000-0000-0000-000000000000' },
    { ...body, objects: [{ ...main, sha256: 'AA'.repeat(32) }] },
  ]) assert.throws(() => parseBackupPlan(invalid, config));
  assert.throws(() => parseBackupPlan({ ...body,
    objects: [{ ...main, bytes: config.backupMaxObjectBytes + 1 }] }, config),
  /backup_object_too_large/);
});

test('backup takeover and delete preconditions reject ambiguous inputs', () => {
  assert.deepEqual(parseTakeover({
    expected_generation: 3,
    takeover_id: '44444444-4444-4444-8444-444444444444',
    writer_id: '55555555-5555-4555-8555-555555555555',
  }), {
    expectedGeneration: 3,
    takeoverId: '44444444-4444-4444-8444-444444444444',
    writerId: '55555555-5555-4555-8555-555555555555',
  });
  assert.throws(() => parseTakeover({ expected_generation: 3,
    takeover_id: '44444444-4444-4444-8444-444444444444',
    writer_id: '55555555-5555-4555-8555-555555555555', tenant_id: 'leak' }));
  assert.deepEqual(parseWriterClaim({
    expected_generation: 1,
    writer_id: '55555555-5555-4555-8555-555555555555',
  }), {
    expectedGeneration: 1,
    writerId: '55555555-5555-4555-8555-555555555555',
  });
  assert.throws(() => parseWriterClaim({
    expected_generation: 2,
    writer_id: '55555555-5555-4555-8555-555555555555',
  }));
  assert.equal(parseIfMatch(`"${'ab'.repeat(32)}"`).toString('hex'), 'ab'.repeat(32));
  assert.throws(() => parseIfMatch(undefined), /precondition_required/);
  assert.throws(() => parseIfMatch('ab'.repeat(32)), /precondition_failed/);
});

test('backup limits retain protocol capacity floors', () => {
  const base = { authSecret: 'x'.repeat(32), adminUsername: 'admin', adminPassword: '' };
  const config = buildConfig(base);
  assert.equal(config.backupMaxObjectBytes, 68 * 1024 * 1024);
  assert.equal(config.backupMaxPackageBytes, 700 * 1024 * 1024);
  assert.equal(config.backupTenantMaxBytes, 2 * 1024 * 1024 * 1024);
  assert.equal(config.backupMaxVersions, 10);
  assert.throws(() => buildConfig({ ...base, backupMaxObjectBytes: 64 * 1024 * 1024 }));
  assert.throws(() => buildConfig({ ...base, backupMaxObjects: 65 }));
  assert.throws(() => buildConfig({ ...base, backupMaxVersions: 1 }));
  assert.throws(() => buildConfig({ ...base, backupTenantMaxBytes: 1024 * 1024 * 1024 }));
});

test('backup routes keep ciphertext opaque and commit before retention pruning', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  const start = source.indexOf("url.pathname === '/v1/backups/generation'");
  const end = source.indexOf("url.pathname === '/v1/push'", start);
  assert.ok(start > 0 && end > start);
  const routes = source.slice(start, end);
  assert.doesNotMatch(routes, /toString\('base64'\)|JSON\.parse\(ciphertext|tenant_id\s*:\s*body/);
  assert.match(routes, /createHash\('sha256'\)\.update\(ciphertext\)\.digest\(\)/);
  assert.match(routes, /status='complete',committed_at=now\(\)/);
  assert.ok(routes.indexOf("status='complete',committed_at=now()") <
    routes.indexOf('order by committed_at desc,backup_id desc offset $2'));
  assert.match(routes, /If-Match|if-match/);
  assert.match(source,
    /Content-Type, Authorization, X-Device, X-Solum-Backup-Writer, If-Match/);
  assert.match(source, /GET, POST, PUT, DELETE, OPTIONS/);
  assert.match(source, /delete from vault\.backups where tenant_id=\$1 and status='pending'/);
  assert.doesNotMatch(source, /console\.error\(error\)|console\.error\('request failed', error\)/);
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

test('password and refresh-token inputs preserve exact surrounding whitespace', () => {
  assert.equal(requiredOpaqueString('  exact secret  ', 'invalid', 64), '  exact secret  ');
  assert.throws(() => requiredOpaqueString('', 'invalid', 64), /invalid/);
  assert.throws(() => requiredOpaqueString('x'.repeat(65), 'invalid', 64), /invalid/);

  const sessionId = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  const refresh = createRefreshToken(sessionId, 'test-secret');
  assert.equal(refreshTokenSessionId(refresh, 'test-secret'), sessionId);
  assert.equal(refreshTokenSessionId(`${refresh}x`, 'test-secret'), null);
  assert.equal(refreshTokenSessionId(refresh, 'wrong-secret'), null);
  const successor = createRefreshSuccessorToken(sessionId, refresh, 'test-secret');
  assert.equal(createRefreshSuccessorToken(sessionId, refresh, 'test-secret'), successor);
  assert.equal(refreshTokenSessionId(successor, 'test-secret'), sessionId);
  assert.notEqual(createRefreshSuccessorToken(sessionId, `${refresh}x`, 'test-secret'), successor);
});

test('public auth helpers are asynchronous, bounded and proxy-aware', async () => {
  const salt = '00112233445566778899aabbccddeeff';
  const expected = crypto.scryptSync('password', salt, 64).toString('hex');
  assert.equal(await passwordHashAsync('password', salt), expected);
  const exactHash = 'ab'.repeat(64);
  const kdfInputs = [];
  const paddedMatch = await verifyPasswordAsync(' padded ', salt, exactHash, 2,
    async (candidate) => {
      kdfInputs.push(candidate);
      return candidate === ' padded ' ? exactHash : 'cd'.repeat(64);
    });
  assert.equal(paddedMatch, true);
  assert.deepEqual(kdfInputs, [' padded ', 'padded'],
    'padded attempts must use the same two KDFs even after an exact match');

  const limiter = createFixedWindowLimiter({ windowMs: 100, maxAttempts: 2, maxKeys: 2 });
  assert.equal(limiter.consume('one', 10), true);
  assert.equal(limiter.consume('one', 20), true);
  assert.equal(limiter.consume('one', 30), false);
  limiter.record('two', 30);
  limiter.record('three', 30);
  assert.equal(limiter.size, 2, 'attacker-controlled keys must stay bounded');
  assert.equal(limiter.isLimited('one', 200), false, 'expired windows must not persist');

  const burst = createFixedWindowLimiter({ windowMs: 100, maxAttempts: 2 });
  const first = burst.reserve('same', 10);
  const second = burst.reserve('same', 10);
  assert.ok(first);
  assert.ok(second);
  assert.equal(burst.reserve('same', 10), null, 'in-flight attempts consume the limit');
  first.release();
  const replacement = burst.reserve('same', 20);
  assert.ok(replacement);
  second.commit(20);
  replacement.commit(20);
  assert.equal(burst.isLimited('same', 20), true);

  const concurrency = createConcurrencyLimiter(2);
  const releaseFirst = concurrency.tryAcquire();
  const releaseSecond = concurrency.tryAcquire();
  assert.ok(releaseFirst);
  assert.ok(releaseSecond);
  assert.equal(concurrency.tryAcquire(), null);
  releaseFirst();
  releaseFirst();
  assert.equal(concurrency.active, 1, 'release handles must be idempotent');
  assert.ok(concurrency.tryAcquire());

  const req = { headers: { host: 'center.example', 'content-type': 'application/problem+json',
    'cf-connecting-ip': '203.0.113.8' }, socket: { remoteAddress: '127.0.0.1' } };
  assert.equal(isJsonContentType(req), true);
  assert.equal(requestOriginAllowed(req, 'https://center.example', new Set()), true);
  assert.equal(requestOriginAllowed(req, 'https://evil.example', new Set()), false);
  assert.equal(requestClientAddress(req, true), '203.0.113.8');
  assert.equal(requestClientAddress(req, false), '127.0.0.1');
});

test('alert metrics, URLs and database integers reject lossy or dangerous values', () => {
  assert.equal(optionalBoundedNumber(42, 'bad', { min: 0, max: 100, integer: true }), 42);
  for (const value of [-1, 101, 1.5, Number.POSITIVE_INFINITY]) {
    assert.throws(() => optionalBoundedNumber(value, 'bad', { min: 0, max: 100, integer: true }));
  }
  assert.equal(optionalHttpUrl('https://example.com/path', 'bad'), 'https://example.com/path');
  assert.throws(() => optionalHttpUrl('javascript:alert(1)', 'bad'));
  assert.equal(safeDatabaseInteger('9007199254740991', 'seq'), Number.MAX_SAFE_INTEGER);
  assert.throws(() => safeDatabaseInteger('9007199254740992', 'seq'), /JSON precision/);
});

test('sync history is durable and tenant storage is explicitly bounded', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  const compose = fs.readFileSync(path.join(root, 'compose.yaml'), 'utf8');
  assert.doesNotMatch(source, /delete from sync\.blobs where/);
  assert.doesNotMatch(compose, /SOLUM_SYNC_RETENTION_DAYS/);
  assert.match(source, /storage_quota_exceeded/);
  assert.match(source, /device_limit_exceeded/);
  assert.match(source, /pg_advisory_xact_lock/);
  assert.match(compose, /SOLUM_TENANT_MAX_BLOB_BYTES/);
});

test('refresh rotation locks one token family and revokes descendants on reuse', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  assert.match(source, /where rt\.session_id=\$1\$\{lock\}/);
  assert.match(source, /for update of rt/);
  assert.match(source, /previous_token_hash=\$2/);
  assert.match(source, /delete from auth\.refresh_tokens where session_id=\$1/);
  assert.match(source, /row\.status !== 'active'/);
  assert.match(source, /auth-session:/, 'session issuance must serialize per user');
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

test('SQLite streaming cancels disconnected upstreams and honors response backpressure', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'server.js'), 'utf8');
  assert.match(source, /const lifetime = requestSignal\(req, res,/);
  assert.match(source, /if \(!res\.write\(chunk\)\) await waitForDrain\(res\)/);
  assert.match(source, /await upstream\.body\.cancel\(\)\.catch\(\(\) => \{\}\)/);
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
  const dockerfile = fs.readFileSync(path.join(root, 'Dockerfile'), 'utf8');
  const postgres = compose.slice(compose.indexOf('\n  postgres:\n'), compose.indexOf('\n  solum-cloud:\n'));
  const cloud = compose.slice(compose.indexOf('\n  solum-cloud:\n'), compose.indexOf('\n  migrate:\n'));
  assert.doesNotMatch(postgres, /\n\s+ports:/);
  assert.match(postgres, /networks: \[backend\]/);
  assert.match(cloud, /ports:\s*\n\s+- "127\.0\.0\.1:/);
  assert.match(cloud, /networks: \[backend, edge\]/);
  assert.match(cloud, /127\.0\.0\.1:8787\/v1\/health/);
  assert.match(dockerfile, /^USER node$/m,
    'the public API container must not run as root');
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
  assert.match(dashboard, /refreshInFlight/);
  assert.match(dashboard, /rotateSession/);
  assert.match(dashboard, /authEpoch/);
  assert.match(dashboard, /expectedEpoch!==authEpoch/);
  assert.match(dashboard, /revokeRefreshToken\(session\.refresh_token\)/);
  assert.match(dashboard, /data:image\/svg\+xml/);
  assert.match(source, /img-src 'self' data:/);
  assert.match(dashboard, /账号已登录，但数据加载失败/);
  assert.doesNotMatch(dashboard, /localStorage|sessionStorage/);
});

test('recovery envelope is fixed-scope and create-only', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  assert.match(source, /PUT'.*\/v1\/keys\/recovery/);
  assert.match(source, /recipient_device_id='account-recovery-v1'/);
  assert.match(source, /on conflict\(tenant_id,recipient_device_id,key_version,algorithm\) do nothing/);
  assert.doesNotMatch(source, /\/v1\/keys\/envelopes/);
  assert.doesNotMatch(source, /\/v1\/devices\/register/);
  assert.doesNotMatch(source, /last_seen_at=excluded\.last_seen_at,revoked_at=null/);
  assert.match(source, /select revoked_at from sync\.devices/);
  assert.match(source, /where tenant_id=\$1 and device_id=\$2 and revoked_at is null returning device_id/);
  assert.match(source, /where tenant_id=\$1 and revoked_at is null/);
  assert.match(source, /device_limit_exceeded/);
  assert.equal((source.match(/throw new HttpError\(403, 'device_revoked'\)/g) || []).length, 2);
});

test('recovery code endpoints never accept the plaintext code', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  // Two-step online verification: challenge returns only the PBKDF2 salt.
  assert.match(source, /POST' && url\.pathname === '\/v1\/auth\/recover\/challenge'/);
  assert.match(source, /POST' && url\.pathname === '\/v1\/auth\/recover'/);
  assert.match(source, /solum-recover-dummy-salt-v1/);
  assert.match(source, /scryptBurnAsync/);
  // Envelope storage and rotation are authenticated and atomic-replace.
  assert.match(source, /PUT' && url\.pathname === '\/v1\/account\/recovery-code'/);
  assert.match(source, /GET' && url\.pathname === '\/v1\/keys\/recovery-code'/);
  assert.match(source, /recovery-code-xchacha20poly1305/);
  assert.match(source, /on conflict\(tenant_id\) do update/i);
  // Explicit password change swaps hash + envelope + other sessions in one path.
  assert.match(source, /invalid_current_secret/);
  assert.match(source, /password_version=2/);
  assert.match(source, /do update set envelope=excluded\.envelope, created_at=now\(\)/);
  // Strict request shapes: no field anywhere can carry a plaintext code.
  assert.match(source,
    /requireExactKeys\(body, \['username', 'verify_hash'\], 'invalid_credentials'\)/);
  const passwordBody = source.slice(
    source.indexOf("'/v1/account/password'"), source.indexOf("'/v1/account/recovery-code'"));
  for (const forbidden of ['current_code"', 'code_plaintext', 'recovery_code"']) {
    assert.ok(!passwordBody.includes(forbidden), `forbidden field: ${forbidden}`);
  }
});

test('unauthenticated refresh and logout are IP-rate-limited', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  assert.match(source, /REFRESH_IP_MAX_FAILURES/);
  const refreshBody = source.slice(
    source.indexOf("'/v1/auth/refresh'"), source.indexOf("'/v1/auth/logout'"));
  assert.ok(refreshBody.includes('refreshIpFailures.reserve(address)'),
    'refresh endpoint must reserve from the per-IP window');
  // A failed verification counts toward the window; server faults do not.
  assert.match(refreshBody, /ipReservation\.commit\(\)/);
  const logoutBody = source.slice(
    source.indexOf("'/v1/auth/logout'"), source.indexOf("// ---- Recovery code"));
  assert.ok(logoutBody.includes('logoutIpReservation.commit()'),
    'logout failures must count toward the window');
});

test('usage events have a retention policy like every other table', () => {
  const source = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
  assert.match(source, /SOLUM_USAGE_EVENT_RETENTION_DAYS \|\| 90/);
  assert.ok(source.includes('delete from ai.usage_events where tenant_id=$1 '),
    'retention cleanup must cover ai.usage_events');
  assert.ok(source.includes("and created_at<now()-($2::text||' days')::interval"),
    'cleanup must be age-based');
  assert.match(source, /usageEventRetentionDays < 30/);
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
    assert.equal(data.users[0].passwordVersion, 1);
    assert.equal(data.refreshTokens[0].tokenHash.toString('hex'), 'ab'.repeat(32));
    assert.equal(data.blobs[0].tenantId, data.users[0].id);
    assert.equal(data.blobs[0].seq, 8);
    assert.equal(data.alerts[0].tenantId, data.users[0].id);
    assert.equal(data.alerts[0].seq, 72);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('legacy importer preserves and validates refresh-token session ids', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-import-session-id-'));
  const cloudPath = path.join(directory, 'cloud.sqlite');
  const relayPath = path.join(directory, 'relay.sqlite');
  const sessionId = '9d4df1be-9f7b-4a3a-b986-ec920d2df60e';
  try {
    const cloud = new DatabaseSync(cloudPath);
    cloud.exec(`
      create table users(
        id text primary key, username text unique, password_hash text,
        password_salt text, created_at integer
      );
      create table refresh_tokens(
        token_hash text primary key, user_id text, session_id text,
        expires_at integer, created_at integer
      );
      insert into users values(
        '00000000-0000-4000-8000-000000000001','session-user','hash','salt',1700000000
      );
      insert into refresh_tokens values(
        '${'cd'.repeat(32)}','00000000-0000-4000-8000-000000000001',
        '${sessionId}',4102444800,1700000001
      );
    `);
    cloud.close();
    new DatabaseSync(relayPath).close();

    const data = loadLegacy(cloudPath, relayPath);
    assert.equal(data.refreshTokens[0].sessionId, sessionId);

    const invalid = new DatabaseSync(cloudPath);
    invalid.prepare('update refresh_tokens set session_id=?').run('not-a-uuid');
    invalid.close();
    assert.throws(() => loadLegacy(cloudPath, relayPath),
      /invalid legacy refresh-token session id/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('legacy importer rejects an unknown password compatibility version', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-import-password-version-'));
  const cloudPath = path.join(directory, 'cloud.sqlite');
  const relayPath = path.join(directory, 'relay.sqlite');
  try {
    const cloud = new DatabaseSync(cloudPath);
    cloud.exec(`
      create table users(
        id text primary key, username text unique, password_hash text,
        password_salt text, password_version integer, created_at integer
      );
      insert into users values(
        '00000000-0000-4000-8000-000000000001',
        'invalid-version', 'hash', 'salt', 9, 1700000000
      );
    `);
    cloud.close();
    const relay = new DatabaseSync(relayPath);
    relay.exec('create table blobs(seq integer, tenant_id text, device text, blob blob, received_at text)');
    relay.close();
    assert.throws(() => loadLegacy(cloudPath, relayPath), /invalid legacy password version/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test('legacy importer rejects null values and missing required alert columns', () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'solum-import-required-'));
  const cloudPath = path.join(directory, 'cloud.sqlite');
  const relayPath = path.join(directory, 'relay.sqlite');
  try {
    const cloud = new DatabaseSync(cloudPath);
    cloud.exec(`
      create table users(username text,password_hash text,password_salt text,created_at integer);
      insert into users values(null,null,null,1700000000);
    `);
    cloud.close();
    new DatabaseSync(relayPath).close();
    assert.throws(() => loadLegacy(cloudPath, relayPath), /invalid legacy username/);

    const repairedCloud = new DatabaseSync(cloudPath);
    repairedCloud.exec("update users set username='alice',password_hash='hash',password_salt='salt'");
    repairedCloud.close();
    const relay = new DatabaseSync(relayPath);
    relay.exec(`
      create table alerts(
        seq integer,tenant_id text,event_id text,source text,status text,received_at text
      );
      insert into alerts values(1,'alice','evt',null,'up','2026-08-14T00:00:00Z');
    `);
    relay.close();
    assert.throws(() => loadLegacy(cloudPath, relayPath), /invalid legacy alert source/);

    const missingColumn = new DatabaseSync(relayPath);
    missingColumn.exec(`
      drop table alerts;
      create table alerts(
        seq integer,tenant_id text,event_id text,status text,received_at text
      );
    `);
    missingColumn.close();
    assert.throws(() => loadLegacy(cloudPath, relayPath), /unsupported relay alerts schema/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

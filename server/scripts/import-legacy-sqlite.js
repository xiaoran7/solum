'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const { DatabaseSync } = require('node:sqlite');
const { Pool } = require('pg');

const MAX_REFRESH_SESSIONS_PER_USER = 32;
const MAX_ALERT_LATENCY_MS = 24 * 60 * 60 * 1000;
const UUID_PATTERN = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;

function usage() {
  return 'Usage: node scripts/import-legacy-sqlite.js <cloud.sqlite> <relay.sqlite>';
}

function tableExists(db, name) {
  return Boolean(db.prepare("select 1 from sqlite_master where type='table' and name=?").get(name));
}

function columns(db, table) {
  if (!tableExists(db, table)) return new Set();
  return new Set(db.prepare(`pragma table_info(${table})`).all().map((row) => row.name));
}

function timestamp(value) {
  if (value == null || value === '') throw new Error('missing legacy timestamp');
  const date = typeof value === 'number' || /^\d+$/.test(String(value))
    ? new Date(Number(value) * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) throw new Error(`invalid timestamp: ${value}`);
  return date;
}

function tokenHash(value) {
  if (Buffer.isBuffer(value)) return value;
  const text = String(value);
  if (!/^[0-9a-f]{64}$/i.test(text)) throw new Error('invalid legacy refresh-token hash');
  return Buffer.from(text, 'hex');
}

function boundedInteger(value, min, max) {
  const parsed = value == null || value === '' ? null : Number(value);
  return Number.isSafeInteger(parsed) && parsed >= min && parsed <= max ? parsed : null;
}

function boundedAvailability(value) {
  const parsed = value == null || value === '' ? null : Number(value);
  return Number.isFinite(parsed) && parsed >= 0 && parsed <= 100 ? parsed : null;
}

function safeDetailUrl(value) {
  if (value == null || value === '') return null;
  const text = String(value);
  return text.length <= 2048 && /^https?:\/\//i.test(text) && !/[\r\n]/.test(text)
    ? text : null;
}

function requiredLegacyText(value, field, max, canonical = false) {
  if (typeof value !== 'string' || value.length < 1 || value.length > max ||
      (canonical && (value !== value.trim() || /[\u0000-\u001f\u007f]/.test(value)))) {
    throw new Error(`invalid legacy ${field}`);
  }
  return value;
}

function loadLegacy(cloudPath, relayPath) {
  for (const file of [cloudPath, relayPath]) {
    if (!file || !fs.statSync(file).isFile()) throw new Error(`SQLite file not found: ${file}`);
  }
  const cloud = new DatabaseSync(cloudPath, { readOnly: true });
  const relay = new DatabaseSync(relayPath, { readOnly: true });
  try {
    const userColumns = columns(cloud, 'users');
    if (!userColumns.has('username') || !userColumns.has('password_hash') ||
        !userColumns.has('password_salt')) throw new Error('unsupported cloud users schema');
    const users = cloud.prepare('select * from users order by username').all().map((row) => {
      const passwordVersion = userColumns.has('password_version')
        ? Number(row.password_version) : 1;
      if (!Number.isInteger(passwordVersion) || ![1, 2].includes(passwordVersion)) {
        throw new Error('invalid legacy password version');
      }
      const rawId = userColumns.has('id') && row.id != null ? String(row.id).toLowerCase() : null;
      if (rawId !== null && !UUID_PATTERN.test(rawId)) throw new Error('invalid legacy user id');
      return {
        id: rawId || crypto.randomUUID(),
        username: requiredLegacyText(row.username, 'username', 128, true),
        passwordHash: requiredLegacyText(row.password_hash, 'password hash', 1024),
        passwordSalt: requiredLegacyText(row.password_salt, 'password salt', 1024),
        passwordVersion,
        createdAt: timestamp(row.created_at),
      };
    });
    if (users.length === 0) throw new Error('legacy cloud contains no users');
    const byUsername = new Map(users.map((user) => [user.username, user]));
    if (byUsername.size !== users.length) throw new Error('duplicate legacy usernames');

    const refreshColumns = columns(cloud, 'refresh_tokens');
    const now = Date.now();
    const sessionIds = new Set();
    const refreshTokens = tableExists(cloud, 'refresh_tokens')
      ? cloud.prepare('select * from refresh_tokens order by created_at desc').all().map((row) => {
        let user;
        if (refreshColumns.has('user_id')) user = users.find((item) => item.id === String(row.user_id));
        if (!user && refreshColumns.has('username')) user = byUsername.get(String(row.username));
        if (!user) throw new Error('refresh token references an unknown user');
        const expiresAt = timestamp(row.expires_at);
        const rawSessionId = refreshColumns.has('session_id') && row.session_id != null
          ? String(row.session_id).trim().toLowerCase() : '';
        if (rawSessionId && !UUID_PATTERN.test(rawSessionId)) {
          throw new Error('invalid legacy refresh-token session id');
        }
        const sessionId = rawSessionId || crypto.randomUUID();
        if (sessionIds.has(sessionId)) {
          throw new Error('duplicate legacy refresh-token session id');
        }
        sessionIds.add(sessionId);
        return { tokenHash: tokenHash(row.token_hash), userId: user.id,
          expiresAt, createdAt: timestamp(row.created_at),
          sessionId, legacyTokenHash: tokenHash(row.token_hash) };
      }).filter((item) => item.expiresAt.getTime() > now) : [];
    const refreshByUser = new Map();
    for (const item of refreshTokens) {
      const bucket = refreshByUser.get(item.userId) || [];
      if (bucket.length < MAX_REFRESH_SESSIONS_PER_USER) bucket.push(item);
      refreshByUser.set(item.userId, bucket);
    }
    const retainedRefreshTokens = [...refreshByUser.values()].flat();

    function tenantFor(row) {
      const raw = row.tenant_id == null ? null : String(row.tenant_id);
      if (raw && byUsername.has(raw)) return byUsername.get(raw).id;
      const byId = raw && users.find((user) => user.id === raw);
      if (byId) return byId.id;
      if ((!raw || raw === 'legacy') && users.length === 1) return users[0].id;
      throw new Error(`relay tenant cannot be mapped to an account: ${raw || '(missing)'}`);
    }

    const blobColumns = columns(relay, 'blobs');
    if (tableExists(relay, 'blobs') && (!blobColumns.has('device') || !blobColumns.has('blob'))) {
      throw new Error('unsupported relay blobs schema');
    }
    const blobs = tableExists(relay, 'blobs')
      ? relay.prepare('select * from blobs order by seq').all().map((row) => ({
        seq: boundedInteger(row.seq, 1, Number.MAX_SAFE_INTEGER), tenantId: tenantFor(row),
        deviceId: requiredLegacyText(row.device, 'blob device', 128, true),
        ciphertext: row.blob instanceof Uint8Array ? Buffer.from(row.blob) : Buffer.alloc(0),
        protocolVersion: 1,
        receivedAt: timestamp(row.received_at),
      })).map((item) => {
        if (item.seq === null || item.deviceId.length === 0 || item.deviceId.length > 128 ||
            item.ciphertext.length < 1 || item.ciphertext.length > 8 * 1024 * 1024) {
          throw new Error(`invalid legacy blob at seq ${item.seq ?? '(unknown)'}`);
        }
        return item;
      }) : [];

    const alertColumns = columns(relay, 'alerts');
    if (tableExists(relay, 'alerts') &&
        (!alertColumns.has('event_id') || !alertColumns.has('source') ||
         !alertColumns.has('status'))) {
      throw new Error('unsupported relay alerts schema');
    }
    const alerts = tableExists(relay, 'alerts')
      ? relay.prepare('select * from alerts order by seq').all().map((row) => ({
        seq: boundedInteger(row.seq, 1, Number.MAX_SAFE_INTEGER), tenantId: tenantFor(row),
        eventId: requiredLegacyText(row.event_id, 'alert event id', 256, true),
        source: requiredLegacyText(row.source, 'alert source', 64, true),
        monitorId: row.monitor_id == null ? null : String(row.monitor_id),
        name: row.name == null ? null : String(row.name),
        status: requiredLegacyText(row.status, 'alert status', 64, true),
        latencyMs: boundedInteger(row.latency_ms, 0, MAX_ALERT_LATENCY_MS),
        pingLatencyMs: boundedInteger(row.ping_latency_ms, 0, MAX_ALERT_LATENCY_MS),
        availability7d: boundedAvailability(row.availability_7d),
        checkedAt: row.checked_at == null ? null : String(row.checked_at),
        detailUrl: safeDetailUrl(row.detail_url),
        receivedAt: timestamp(row.received_at),
      })).map((item) => {
        if (item.seq === null || item.eventId.length < 1 || item.eventId.length > 256 ||
            item.source.length < 1 || item.source.length > 64 ||
            item.status.length < 1 || item.status.length > 64) {
          throw new Error(`invalid legacy alert at seq ${item.seq ?? '(unknown)'}`);
        }
        return item;
      }) : [];
    return { users, refreshTokens: retainedRefreshTokens, blobs, alerts };
  } finally {
    cloud.close();
    relay.close();
  }
}

async function assertEmpty(client) {
  const result = await client.query(`
    select (select count(*) from auth.users) as users,
           (select count(*) from auth.refresh_tokens) as refresh_tokens,
           (select count(*) from sync.blobs) as blobs,
           (select count(*) from sync.alerts) as alerts`);
  if (Object.values(result.rows[0]).some((value) => Number(value) !== 0)) {
    throw new Error('target PostgreSQL center is not empty');
  }
}

async function resetIdentity(client, table) {
  await client.query(`select setval(pg_get_serial_sequence('${table}','seq'),
    greatest(coalesce((select max(seq) from ${table}),0),1), exists(select 1 from ${table}))`);
}

async function importLegacy(pool, data) {
  const client = await pool.connect();
  try {
    await client.query('begin');
    await assertEmpty(client);
    for (const user of data.users) {
      await client.query(
        `insert into auth.users(
           id,username,password_hash,password_salt,password_version,created_at,updated_at
         ) values($1,$2,$3,$4,$5,$6,$6)`,
        [user.id, user.username, user.passwordHash, user.passwordSalt,
          user.passwordVersion, user.createdAt]);
    }
    for (const item of data.refreshTokens) {
      await client.query(
        `insert into auth.refresh_tokens(
           token_hash,user_id,session_id,previous_token_hash,legacy_token_hash,rotated_at,
           expires_at,created_at
         ) values($1,$2,$3,null,$4,null,$5,$6)`,
        [item.tokenHash, item.userId, item.sessionId, item.legacyTokenHash,
          item.expiresAt, item.createdAt]);
    }
    for (const item of data.blobs) {
      await client.query(
        `insert into sync.blobs(seq,tenant_id,device_id,ciphertext,protocol_version,received_at)
         overriding system value values($1,$2,$3,$4,$5,$6)`,
        [item.seq, item.tenantId, item.deviceId, item.ciphertext, item.protocolVersion, item.receivedAt]);
    }
    for (const item of data.alerts) {
      await client.query(
        `insert into sync.alerts(seq,tenant_id,event_id,source,monitor_id,name,status,latency_ms,
          ping_latency_ms,availability_7d,checked_at,detail_url,received_at)
         overriding system value values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)`,
        [item.seq, item.tenantId, item.eventId, item.source, item.monitorId, item.name, item.status,
          item.latencyMs, item.pingLatencyMs, item.availability7d, item.checkedAt, item.detailUrl,
          item.receivedAt]);
    }
    await resetIdentity(client, 'sync.blobs');
    await resetIdentity(client, 'sync.alerts');
    await client.query('commit');
  } catch (error) {
    await client.query('rollback');
    throw error;
  } finally {
    client.release();
  }
}

async function main(argv = process.argv.slice(2)) {
  if (argv.length !== 2) throw new Error(usage());
  const data = loadLegacy(argv[0], argv[1]);
  const pool = new Pool();
  try { await importLegacy(pool, data); } finally { await pool.end(); }
  console.log(`Imported ${data.users.length} user(s), ${data.refreshTokens.length} refresh token(s), ` +
    `${data.blobs.length} blob(s), and ${data.alerts.length} alert(s).`);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(`Legacy import failed: ${error.message}`);
    process.exitCode = 1;
  });
}

module.exports = { importLegacy, loadLegacy, tokenHash };

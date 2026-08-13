'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const { DatabaseSync } = require('node:sqlite');
const { Pool } = require('pg');

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
    const users = cloud.prepare('select * from users order by username').all().map((row) => ({
      id: userColumns.has('id') && row.id ? String(row.id) : crypto.randomUUID(),
      username: String(row.username),
      passwordHash: String(row.password_hash), passwordSalt: String(row.password_salt),
      createdAt: timestamp(row.created_at),
    }));
    if (users.length === 0) throw new Error('legacy cloud contains no users');
    const byUsername = new Map(users.map((user) => [user.username, user]));
    if (byUsername.size !== users.length) throw new Error('duplicate legacy usernames');

    const refreshColumns = columns(cloud, 'refresh_tokens');
    const refreshTokens = tableExists(cloud, 'refresh_tokens')
      ? cloud.prepare('select * from refresh_tokens order by created_at').all().map((row) => {
        let user;
        if (refreshColumns.has('user_id')) user = users.find((item) => item.id === String(row.user_id));
        if (!user && refreshColumns.has('username')) user = byUsername.get(String(row.username));
        if (!user) throw new Error('refresh token references an unknown user');
        return { tokenHash: tokenHash(row.token_hash), userId: user.id,
          expiresAt: timestamp(row.expires_at), createdAt: timestamp(row.created_at) };
      }) : [];

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
        seq: Number(row.seq), tenantId: tenantFor(row), deviceId: String(row.device),
        ciphertext: Buffer.from(row.blob), protocolVersion: 1,
        receivedAt: timestamp(row.received_at),
      })) : [];

    const alertColumns = columns(relay, 'alerts');
    if (tableExists(relay, 'alerts') &&
        (!alertColumns.has('event_id') || !alertColumns.has('status'))) {
      throw new Error('unsupported relay alerts schema');
    }
    const alerts = tableExists(relay, 'alerts')
      ? relay.prepare('select * from alerts order by seq').all().map((row) => ({
        seq: Number(row.seq), tenantId: tenantFor(row), eventId: String(row.event_id),
        source: String(row.source), monitorId: row.monitor_id == null ? null : String(row.monitor_id),
        name: row.name == null ? null : String(row.name), status: String(row.status),
        latencyMs: row.latency_ms == null ? null : Number(row.latency_ms),
        pingLatencyMs: row.ping_latency_ms == null ? null : Number(row.ping_latency_ms),
        availability7d: row.availability_7d == null ? null : Number(row.availability_7d),
        checkedAt: String(row.checked_at),
        detailUrl: row.detail_url == null ? null : String(row.detail_url),
        receivedAt: timestamp(row.received_at),
      })) : [];
    return { users, refreshTokens, blobs, alerts };
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
        `insert into auth.users(id,username,password_hash,password_salt,created_at,updated_at)
         values($1,$2,$3,$4,$5,$5)`,
        [user.id, user.username, user.passwordHash, user.passwordSalt, user.createdAt]);
    }
    for (const item of data.refreshTokens) {
      await client.query(
        'insert into auth.refresh_tokens(token_hash,user_id,expires_at,created_at) values($1,$2,$3,$4)',
        [item.tokenHash, item.userId, item.expiresAt, item.createdAt]);
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

async function cleanExpiredData(pool, config) {
  const users = await pool.query('select id from auth.users');
  for (const user of users.rows) {
    await withTenant(pool, user.id, async (client) => {
      await client.query(
        "delete from sync.alerts where tenant_id=$1 and received_at<now()-($2::text||' days')::interval",
        [user.id, config.alertRetentionDays]
      );
      await client.query(
        "delete from vault.backups where tenant_id=$1 and status='pending' " +
          "and created_at<now()-($2::text||' hours')::interval",
        [user.id, config.backupPendingTtlHours]
      );
      // Usage events are the only table without any cleanup; monthly
      // aggregates keep serving quota checks after the rows are gone.
      await client.query(
        "delete from ai.usage_events where tenant_id=$1 " +
          "and created_at<now()-($2::text||' days')::interval",
        [user.id, config.usageEventRetentionDays]
      );
    });
  }
  await pool.query('delete from auth.refresh_tokens where expires_at<=now()');
}

async function lockTenantMutation(client, tenantId) {
  await client.query('select pg_advisory_xact_lock(hashtextextended($1::text,0))', [tenantId]);
}

async function backupState(client, tenantId) {
  await client.query(
    `insert into vault.backup_state(tenant_id,active_generation)
     values($1,1) on conflict(tenant_id) do nothing`, [tenantId]
  );
  const selected = await client.query(
    `select active_generation,last_takeover_id,writer_id,updated_at
     from vault.backup_state where tenant_id=$1`, [tenantId]
  );
  if (selected.rowCount !== 1) throw new Error('backup state initialization failed');
  return {
    activeGeneration: safeDatabaseInteger(
      selected.rows[0].active_generation, 'backup_active_generation'
    ),
    lastTakeoverId: selected.rows[0].last_takeover_id,
    writerId: selected.rows[0].writer_id,
    updatedAt: selected.rows[0].updated_at,
  };
}

function requireBackupWriter(req, state) {
  const writerId = backupWriterId(req.headers['x-solum-backup-writer'], 'backup_writer_stale');
  if (state.writerId !== writerId) throw new HttpError(409, 'backup_writer_stale');
  return writerId;
}

function backupObjectJson(row) {
  return {
    object_id: row.object_id,
    kind: row.kind,
    bytes: safeDatabaseInteger(row.declared_bytes, 'backup_object_bytes'),
    sha256: row.declared_sha256.toString('hex'),
  };
}

function backupJson(row, objects) {
  return {
    backup_id: row.backup_id,
    generation: safeDatabaseInteger(row.generation, 'backup_generation'),
    created_at: isoTimestamp(row.client_created_at),
    committed_at: row.committed_at === null ? null : isoTimestamp(row.committed_at),
    protocol_version: Number(row.protocol_version),
    total_bytes: safeDatabaseInteger(row.total_bytes, 'backup_total_bytes'),
    object_count: safeDatabaseInteger(row.object_count, 'backup_object_count'),
    plan_etag: row.plan_sha256.toString('hex'),
    objects,
  };
}

async function tenantBackupBytes(client, tenantId) {
  const usage = await client.query(
    `select coalesce(sum(octet_length(ciphertext)),0) as total_bytes
     from vault.backup_objects where tenant_id=$1 and ciphertext is not null`, [tenantId]
  );
  return safeDatabaseInteger(usage.rows[0].total_bytes, 'tenant_backup_bytes');
}

function isOctetStream(req) {
  const value = req.headers['content-type'];
  return typeof value === 'string' && value.split(';', 1)[0].trim().toLowerCase() ===
    'application/octet-stream';
}

async function touchActiveDevice(client, tenantId, device, maxDevices) {
  const selected = await client.query(
    'select revoked_at from sync.devices where tenant_id=$1 and device_id=$2', [tenantId, device]
  );
  if (selected.rowCount > 0) {
    if (selected.rows[0].revoked_at !== null) throw new HttpError(403, 'device_revoked');
    const updated = await client.query(
      `update sync.devices set last_seen_at=now()
       where tenant_id=$1 and device_id=$2 and revoked_at is null returning device_id`,
      [tenantId, device]
    );
    if (updated.rowCount !== 1) throw new HttpError(403, 'device_revoked');
    return;
  }
  const count = await client.query(
    `select count(*) as device_count from sync.devices
     where tenant_id=$1 and revoked_at is null`, [tenantId]
  );
  if (safeDatabaseInteger(count.rows[0].device_count, 'device_count') >= maxDevices) {
    throw new HttpError(429, 'device_limit_exceeded');
  }
  await client.query(
    'insert into sync.devices(tenant_id,device_id,last_seen_at) values($1,$2,now())',
    [tenantId, device]
  );
}

async function ensureBlobQuota(client, tenantId, blobLength, config) {
  const usage = await client.query(
    `select count(*) as blob_count,coalesce(sum(octet_length(ciphertext)),0) as blob_bytes
     from sync.blobs where tenant_id=$1`, [tenantId]
  );
  const blobCount = safeDatabaseInteger(usage.rows[0].blob_count, 'blob_count');
  const blobBytes = safeDatabaseInteger(usage.rows[0].blob_bytes, 'blob_bytes');
  if (blobCount >= config.tenantMaxBlobs || blobBytes > config.tenantMaxBlobBytes - blobLength) {
    throw new HttpError(507, 'storage_quota_exceeded');
  }
}


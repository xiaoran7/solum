    if (req.method === 'GET' && url.pathname === '/v1/backups/generation') {
      const tenantId = await authenticate(req);
      const state = await withTenant(pool, tenantId, async (client) => backupState(client, tenantId));
      return json(res, 200, {
        active_generation: state.activeGeneration,
        updated_at: isoTimestamp(state.updatedAt),
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/backups/writer') {
      const tenantId = await authenticate(req);
      const claim = parseWriterClaim(await readJson(req));
      const result = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        if (state.activeGeneration !== claim.expectedGeneration) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        if (state.writerId === claim.writerId) {
          return { activeGeneration: state.activeGeneration, claimed: false };
        }
        if (state.writerId !== null) throw new HttpError(409, 'backup_writer_stale');
        const existing = await client.query(
          'select 1 from vault.backups where tenant_id=$1 limit 1', [tenantId]
        );
        if (existing.rowCount !== 0) throw new HttpError(409, 'backup_writer_required');
        await client.query(
          `update vault.backup_state set writer_id=$2,updated_at=now()
           where tenant_id=$1 and writer_id is null`, [tenantId, claim.writerId]
        );
        return { activeGeneration: state.activeGeneration, claimed: true };
      });
      return json(res, 200, {
        active_generation: result.activeGeneration,
        claimed: result.claimed,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/backups/generation') {
      const tenantId = await authenticate(req);
      const takeover = parseTakeover(await readJson(req));
      const advanced = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        if (state.lastTakeoverId === takeover.takeoverId &&
            state.writerId === takeover.writerId) {
          return { activeGeneration: state.activeGeneration, prunedPending: 0, replay: true };
        }
        if (state.lastTakeoverId === takeover.takeoverId) {
          throw new HttpError(409, 'backup_writer_stale');
        }
        if (state.activeGeneration !== takeover.expectedGeneration ||
            state.activeGeneration === Number.MAX_SAFE_INTEGER) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        const next = state.activeGeneration + 1;
        await client.query(
          `update vault.backup_state
           set active_generation=$2,last_takeover_id=$3,writer_id=$4,updated_at=now()
           where tenant_id=$1`, [tenantId, next, takeover.takeoverId, takeover.writerId]
        );
        const removed = await client.query(
          `delete from vault.backups
           where tenant_id=$1 and status='pending' and generation<>$2 returning backup_id`,
          [tenantId, next]
        );
        return { activeGeneration: next, prunedPending: removed.rowCount, replay: false };
      });
      return json(res, 200, {
        active_generation: advanced.activeGeneration,
        pruned_pending: advanced.prunedPending,
        replay: advanced.replay,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/backups') {
      const tenantId = await authenticate(req);
      const plan = parseBackupPlan(await readJson(req), config);
      const created = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        requireBackupWriter(req, state);
        if (state.activeGeneration !== plan.generation) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        const existing = await client.query(
          `select status,plan_sha256 from vault.backups
           where tenant_id=$1 and backup_id=$2`, [tenantId, plan.backup_id]
        );
        if (existing.rowCount === 1) {
          if (!secureEqualBuffer(existing.rows[0].plan_sha256, plan.planSha256)) {
            throw new HttpError(409, 'backup_plan_conflict');
          }
          return { status: existing.rows[0].status, inserted: false };
        }
        const pending = await client.query(
          `select count(*) as pending_count from vault.backups
           where tenant_id=$1 and status='pending'`, [tenantId]
        );
        if (safeDatabaseInteger(pending.rows[0].pending_count, 'backup_pending_count') >=
            config.backupMaxPending) {
          throw new HttpError(429, 'backup_pending_limit');
        }
        await client.query(
          `insert into vault.backups(
             tenant_id,backup_id,generation,protocol_version,client_created_at,
             plan_sha256,total_bytes,object_count
           ) values($1,$2,$3,1,$4,$5,$6,$7)`,
          [tenantId, plan.backup_id, plan.generation, plan.created_at,
            plan.planSha256, plan.totalBytes, plan.objects.length]
        );
        for (const object of plan.objects) {
          await client.query(
            `insert into vault.backup_objects(
               tenant_id,backup_id,object_id,kind,declared_bytes,declared_sha256
             ) values($1,$2,$3,$4,$5,$6)`,
            [tenantId, plan.backup_id, object.object_id, object.kind,
              object.bytes, Buffer.from(object.sha256, 'hex')]
          );
        }
        return { status: 'pending', inserted: true };
      });
      return json(res, created.inserted ? 201 : 200, {
        backup_id: plan.backup_id,
        generation: plan.generation,
        status: created.status,
        plan_etag: plan.planSha256.toString('hex'),
        total_bytes: plan.totalBytes,
        object_count: plan.objects.length,
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/backups') {
      const tenantId = await authenticate(req);
      const limit = parseNonNegativeInt(url.searchParams.get('limit'), 20, 100);
      if (limit < 1) throw new HttpError(400, 'invalid_query');
      const result = await withTenant(pool, tenantId, async (client) => {
        const state = await backupState(client, tenantId);
        const selected = await client.query(
          `select backup_id,generation,protocol_version,client_created_at,committed_at,
                  plan_sha256,total_bytes,object_count
           from vault.backups where tenant_id=$1 and status='complete'
           order by committed_at desc,backup_id desc limit $2`, [tenantId, limit]
        );
        const byBackup = new Map(selected.rows.map((row) => [row.backup_id, []]));
        if (selected.rowCount > 0) {
          const objects = await client.query(
            `select backup_id,object_id,kind,declared_bytes,declared_sha256
             from vault.backup_objects where tenant_id=$1 and backup_id=any($2::uuid[])
             order by backup_id,object_id`, [tenantId, [...byBackup.keys()]]
          );
          for (const row of objects.rows) byBackup.get(row.backup_id).push(backupObjectJson(row));
        }
        return {
          active_generation: state.activeGeneration,
          backups: selected.rows.map((row) => backupJson(row, byBackup.get(row.backup_id))),
        };
      });
      return json(res, 200, result);
    }

    const backupObjectMatch = url.pathname.match(
      /^\/v1\/backups\/([0-9a-f-]{36})\/objects\/([0-9a-f-]{36})$/
    );
    if (backupObjectMatch && req.method === 'PUT') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupObjectMatch[1]);
      const objectId = backupUuid(backupObjectMatch[2]);
      if (!isOctetStream(req)) throw new HttpError(415, 'unsupported_media_type');
      const declared = await withTenant(pool, tenantId, async (client) => {
        const selected = await client.query(
          `select b.status,b.generation,o.declared_bytes,o.declared_sha256,o.ciphertext
           from vault.backups b join vault.backup_objects o
             on o.tenant_id=b.tenant_id and o.backup_id=b.backup_id
           where b.tenant_id=$1 and b.backup_id=$2 and o.object_id=$3`,
          [tenantId, backupId, objectId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_object_not_found');
        return selected.rows[0];
      });
      const expectedBytes = safeDatabaseInteger(declared.declared_bytes, 'backup_object_bytes');
      const contentLength = req.headers['content-length'];
      if (typeof contentLength !== 'string' || !/^\d+$/.test(contentLength)) {
        throw new HttpError(411, 'content_length_required');
      }
      if (Number(contentLength) !== expectedBytes) {
        throw new HttpError(409, 'backup_object_mismatch');
      }
      const ciphertext = await readExactBytes(req, expectedBytes);
      const digest = crypto.createHash('sha256').update(ciphertext).digest();
      if (!secureEqualBuffer(digest, declared.declared_sha256)) {
        throw new HttpError(409, 'backup_object_mismatch');
      }
      const uploaded = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        requireBackupWriter(req, state);
        const selected = await client.query(
          `select b.status,b.generation,o.declared_bytes,o.declared_sha256,o.ciphertext
           from vault.backups b join vault.backup_objects o
             on o.tenant_id=b.tenant_id and o.backup_id=b.backup_id
           where b.tenant_id=$1 and b.backup_id=$2 and o.object_id=$3`,
          [tenantId, backupId, objectId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_object_not_found');
        const row = selected.rows[0];
        if (safeDatabaseInteger(row.generation, 'backup_generation') !== state.activeGeneration) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        if (safeDatabaseInteger(row.declared_bytes, 'backup_object_bytes') !== ciphertext.length ||
            !secureEqualBuffer(row.declared_sha256, digest)) {
          throw new HttpError(409, 'backup_object_mismatch');
        }
        if (row.ciphertext !== null) {
          if (!secureEqualBuffer(row.ciphertext, ciphertext)) {
            throw new HttpError(409, 'backup_object_mismatch');
          }
          return false;
        }
        if (row.status !== 'pending') throw new HttpError(409, 'backup_plan_conflict');
        const used = await tenantBackupBytes(client, tenantId);
        if (used > config.backupTenantMaxBytes - ciphertext.length) {
          throw new HttpError(413, 'backup_quota_exceeded');
        }
        await client.query(
          `update vault.backup_objects set ciphertext=$4,uploaded_at=now()
           where tenant_id=$1 and backup_id=$2 and object_id=$3 and ciphertext is null`,
          [tenantId, backupId, objectId, ciphertext]
        );
        return true;
      });
      return json(res, uploaded ? 201 : 200, {
        backup_id: backupId, object_id: objectId, sha256: digest.toString('hex'),
      });
    }

    if (backupObjectMatch && req.method === 'GET') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupObjectMatch[1]);
      const objectId = backupUuid(backupObjectMatch[2]);
      const object = await withTenant(pool, tenantId, async (client) => {
        const selected = await client.query(
          `select o.ciphertext,o.declared_sha256
           from vault.backups b join vault.backup_objects o
             on o.tenant_id=b.tenant_id and o.backup_id=b.backup_id
           where b.tenant_id=$1 and b.backup_id=$2 and b.status='complete'
             and o.object_id=$3`, [tenantId, backupId, objectId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_object_not_found');
        if (!Buffer.isBuffer(selected.rows[0].ciphertext)) {
          throw new Error('complete backup contains an absent object');
        }
        return selected.rows[0];
      });
      return binary(res, object.ciphertext, object.declared_sha256);
    }

    const backupCommitMatch = url.pathname.match(
      /^\/v1\/backups\/([0-9a-f-]{36})\/commit$/
    );
    if (backupCommitMatch && req.method === 'POST') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupCommitMatch[1]);
      await readExactBytes(req, 0);
      const committed = await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const state = await backupState(client, tenantId);
        requireBackupWriter(req, state);
        const selected = await client.query(
          `select backup_id,generation,protocol_version,client_created_at,committed_at,
                  plan_sha256,total_bytes,object_count,status
           from vault.backups where tenant_id=$1 and backup_id=$2`, [tenantId, backupId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_not_found');
        const row = selected.rows[0];
        if (safeDatabaseInteger(row.generation, 'backup_generation') !== state.activeGeneration) {
          throw new HttpError(409, 'backup_generation_stale');
        }
        if (row.status === 'pending') {
          // Verify inside PostgreSQL: shipping every ciphertext to Node only
          // to hash it would move up to the whole package across the wire
          // while holding the tenant lock open.
          const objectRows = await client.query(
            `select object_id,declared_bytes,
                    ciphertext is not null and octet_length(ciphertext)=declared_bytes
                      and sha256(ciphertext)=declared_sha256 as verified
             from vault.backup_objects where tenant_id=$1 and backup_id=$2
             order by object_id`,
            [tenantId, backupId]
          );
          if (objectRows.rowCount !== safeDatabaseInteger(row.object_count, 'backup_object_count')) {
            throw new HttpError(409, 'backup_incomplete');
          }
          let verifiedBytes = 0;
          for (const objectRow of objectRows.rows) {
            if (!objectRow.verified) throw new HttpError(409, 'backup_incomplete');
            verifiedBytes += safeDatabaseInteger(objectRow.declared_bytes, 'backup_object_bytes');
          }
          if (verifiedBytes !== safeDatabaseInteger(row.total_bytes, 'backup_total_bytes')) {
            throw new HttpError(409, 'backup_incomplete');
          }
          const updated = await client.query(
            `update vault.backups set status='complete',committed_at=now()
             where tenant_id=$1 and backup_id=$2 and status='pending'
             returning committed_at`, [tenantId, backupId]
          );
          row.status = 'complete';
          row.committed_at = updated.rows[0].committed_at;
        }
        const pruned = await client.query(
          `delete from vault.backups where tenant_id=$1 and backup_id in (
             select backup_id from vault.backups
             where tenant_id=$1 and status='complete'
             order by committed_at desc,backup_id desc offset $2
           ) returning backup_id`, [tenantId, config.backupMaxVersions]
        );
        return {
          backup: backupJson(row, []),
          pruned: pruned.rows.map((item) => item.backup_id),
        };
      });
      return json(res, 200, {
        ...committed.backup,
        status: 'complete',
        pruned_versions: committed.pruned,
      });
    }

    const backupVersionMatch = url.pathname.match(/^\/v1\/backups\/([0-9a-f-]{36})$/);
    if (backupVersionMatch && req.method === 'DELETE') {
      const tenantId = await authenticate(req);
      const backupId = backupUuid(backupVersionMatch[1]);
      const expected = parseIfMatch(req.headers['if-match']);
      await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        const selected = await client.query(
          `select plan_sha256 from vault.backups where tenant_id=$1 and backup_id=$2`,
          [tenantId, backupId]
        );
        if (selected.rowCount !== 1) throw new HttpError(404, 'backup_not_found');
        if (!secureEqualBuffer(selected.rows[0].plan_sha256, expected)) {
          throw new HttpError(412, 'precondition_failed');
        }
        await client.query(
          `delete from vault.backups where tenant_id=$1 and backup_id=$2`, [tenantId, backupId]
        );
      });
      return json(res, 200, { backup_id: backupId, deleted: true });
    }


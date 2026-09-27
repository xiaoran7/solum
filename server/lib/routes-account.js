    if (req.method === 'POST' && url.pathname === '/v1/account/password') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      // Exact-key whitelist with two optional alternates: exactly one current
      // credential is required; the rewrapped envelope is optional but the
      // only sanctioned way to keep it in sync.
      const allowed = new Set(['new_password', 'current_password',
        'current_code_verify_hash', 'password_envelope']);
      for (const key of Object.keys(body)) {
        if (!allowed.has(key)) throw new HttpError(400, 'invalid_request');
      }
      const newPassword = requiredOpaqueString(body.new_password, 'invalid_credentials', 1024);
      if ([...newPassword].length < 8) throw new HttpError(400, 'invalid_credentials');
      const oldPassword = typeof body.current_password === 'string' &&
        body.current_password.length > 0 ? body.current_password : null;
      const oldCodeDigest = typeof body.current_code_verify_hash === 'string' &&
        SHA256_PATTERN.test(body.current_code_verify_hash)
        ? body.current_code_verify_hash : null;
      if ((oldPassword === null) === (oldCodeDigest === null)) {
        throw new HttpError(400, 'invalid_request');
      }
      const key = loginKey(req, String(tenantId));
      const pairReservation = loginFailures.reserve(key.pair);
      if (!pairReservation) throw new HttpError(429, 'too_many_attempts');
      const releaseKdf = authKdfConcurrency.tryAcquire();
      if (!releaseKdf) {
        pairReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        const currentRefreshHash = tokenHash(bearer(req));
        const client = await pool.connect();
        let transactionOpen = false;
        try {
          await client.query('begin');
          transactionOpen = true;
          await client.query("select set_config('app.current_tenant_id', $1, true)", [tenantId]);
          await lockAuthSessionUser(client, tenantId);
          const users = await client.query(
            `select username, password_hash, password_salt, password_version
             from auth.users where id=$1 and status='active' for update`, [tenantId]
          );
          const user = users.rows[0];
          if (!user) {
            pairReservation.commit();
            throw new HttpError(401, 'unauthorized');
          }
          let authorized;
          if (oldPassword !== null) {
            authorized = await verifyPasswordAsync(oldPassword, user.password_salt,
              user.password_hash, user.password_version);
          } else {
            const codes = await client.query(
              'select verify_hash from vault.recovery_codes where tenant_id=$1 for update',
              [tenantId]
            );
            authorized = codes.rows.length > 0 &&
              secureEqualHex(codes.rows[0].verify_hash, oldCodeDigest);
          }
          if (!authorized) {
            pairReservation.commit();
            throw new HttpError(403, 'invalid_current_secret');
          }
          const newSalt = crypto.randomBytes(16).toString('hex');
          const newHash = await passwordHashAsync(newPassword, newSalt);
          await client.query(
            `update auth.users
             set password_hash=$2, password_salt=$3, password_version=2
             where id=$1`, [tenantId, newHash, newSalt]
          );
          if (typeof body.password_envelope === 'string') {
            const envelope = decodeBase64(body.password_envelope, 'invalid_envelope', 100000);
            if (envelope.length < 40 || envelope.length > 65536) {
              throw new HttpError(400, 'invalid_envelope');
            }
            await client.query(
              `insert into vault.key_envelopes
                 (tenant_id,recipient_device_id,key_version,algorithm,envelope)
               values($1,'account-recovery-v1',1,'recovery-xchacha20poly1305',$2)
               on conflict(tenant_id,recipient_device_id,key_version,algorithm)
               do update set envelope=excluded.envelope, created_at=now()`,
              [tenantId, envelope]
            );
          }
          // Keep only the refresh session this request arrived on; every other
          // family dies with the old credential.
          const mine = await client.query(
            `select session_id from auth.refresh_tokens
             where user_id=$1 and (token_hash=$2 or previous_token_hash=$2
               or legacy_token_hash=$2) limit 1`, [tenantId, currentRefreshHash]
          );
          const keepSessionId = mine.rows.length > 0 ? mine.rows[0].session_id : null;
          if (keepSessionId === null) {
            await client.query('delete from auth.refresh_tokens where user_id=$1', [tenantId]);
          } else {
            await client.query(
              'delete from auth.refresh_tokens where user_id=$1 and session_id<>$2',
              [tenantId, keepSessionId]
            );
          }
          await client.query('commit');
          transactionOpen = false;
        } catch (error) {
          if (transactionOpen) await client.query('rollback').catch(() => {});
          throw error;
        } finally {
          client.release();
        }
        pairReservation.release();
        loginFailures.reset(key.pair);
        return json(res, 200, { status: 'ok' });
      } catch (error) {
        pairReservation.release();
        throw error;
      } finally {
        releaseKdf();
      }
    }

    // Store or rotate the recovery-code envelope (authenticated). Rotation is
    // an atomic replace: the old code stops verifying in the same transaction
    // that publishes the new envelope.
    if (req.method === 'PUT' && url.pathname === '/v1/account/recovery-code') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      requireExactKeys(body, ['key_version', 'algorithm', 'envelope', 'verify_hash'],
        'invalid_envelope');
      if (body.key_version !== 1 ||
          body.algorithm !== 'recovery-code-xchacha20poly1305') {
        throw new HttpError(400, 'invalid_envelope');
      }
      const envelope = decodeBase64(body.envelope, 'invalid_envelope', 100000);
      if (envelope.length < 56 || envelope.length > 65536) {
        throw new HttpError(400, 'invalid_envelope');
      }
      const verifyHash = requiredOpaqueString(body.verify_hash, 'invalid_envelope', 64);
      if (!SHA256_PATTERN.test(verifyHash)) throw new HttpError(400, 'invalid_envelope');
      await withTenant(pool, tenantId, async (client) => {
        await lockTenantMutation(client, tenantId);
        await client.query(
          `insert into vault.recovery_codes
             (tenant_id,key_version,algorithm,envelope,verify_hash)
           values($1,$2,$3,$4,$5)
           on conflict(tenant_id) do update
             set key_version=excluded.key_version,
                 algorithm=excluded.algorithm,
                 envelope=excluded.envelope,
                 verify_hash=excluded.verify_hash,
                 updated_at=now()`,
          [tenantId, body.key_version, body.algorithm, envelope, verifyHash]
        );
      });
      return json(res, 200, { status: 'ok', key_version: body.key_version });
    }

    if (req.method === 'GET' && url.pathname === '/v1/account/recovery-code') {
      const tenantId = await authenticate(req);
      const state = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select key_version, algorithm, created_at, updated_at
           from vault.recovery_codes where tenant_id=$1`, [tenantId]
        );
        return result.rows[0] || null;
      });
      return json(res, 200, state
        ? { configured: true, key_version: state.key_version,
            algorithm: state.algorithm, created_at: state.created_at,
            updated_at: state.updated_at }
        : { configured: false });
    }

    if (req.method === 'GET' && url.pathname === '/v1/keys/recovery-code') {
      const tenantId = await authenticate(req);
      const code = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select key_version,algorithm,envelope,created_at
           from vault.recovery_codes where tenant_id=$1`, [tenantId]
        );
        if (result.rowCount === 0) return null;
        const row = result.rows[0];
        return { ...row, envelope: row.envelope.toString('base64') };
      });
      return json(res, 200, code || { envelope: null });
    }


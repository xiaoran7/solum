    if (req.method === 'POST' && url.pathname === '/v1/auth/register') {
      if (config.registrationMode !== 'open') throw new HttpError(403, 'registration_closed');
      const registrationKey = requestClientAddress(req, config.trustProxy);
      if (!registrationAttempts.consume(registrationKey)) {
        throw new HttpError(429, 'too_many_attempts');
      }
      const body = await readJson(req);
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const password = requiredOpaqueString(body.password, 'invalid_credentials', 1024);
      if ([...password].length < 8 || /[\u0000-\u001f\u007f]/.test(username)) {
        throw new HttpError(400, 'invalid_credentials');
      }
      const salt = crypto.randomBytes(16).toString('hex');
      const releaseKdf = authKdfConcurrency.tryAcquire();
      if (!releaseKdf) throw new HttpError(429, 'too_many_attempts');
      let passwordHash;
      try {
        passwordHash = await passwordHashAsync(password, salt);
      } finally {
        releaseKdf();
      }
      const client = await pool.connect();
      try {
        await client.query('begin');
        const inserted = await client.query(
          `insert into auth.users(username,password_hash,password_salt,password_version)
           values($1,$2,$3,2) returning id,username`,
          [username, passwordHash, salt]
        );
        const session = await issueSession(client, inserted.rows[0], config.authSecret);
        await client.query('commit');
        return json(res, 201, session);
      } catch (error) {
        await client.query('rollback').catch(() => {});
        if (error && error.code === '23505') throw new HttpError(409, 'username_taken');
        throw error;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/login') {
      const body = await readJson(req);
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const password = requiredOpaqueString(body.password, 'invalid_credentials', 1024);
      const key = loginKey(req, username);
      const pairReservation = loginFailures.reserve(key.pair);
      if (!pairReservation) throw new HttpError(429, 'too_many_attempts');
      const ipReservation = loginIpFailures.reserve(key.address);
      if (!ipReservation) {
        pairReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      const releaseKdf = authKdfConcurrency.tryAcquire();
      if (!releaseKdf) {
        pairReservation.release();
        ipReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        // Password verification is CPU work and must not hold a database row
        // lock or an open transaction. Session issuance has its own short,
        // per-user advisory-locked transaction below.
        const selected = await pool.query(
          `select id,username,password_hash,password_salt,password_version
           from auth.users where username=$1 and status='active'`,
          [username]
        );
        const user = selected.rows[0];
        const passwordMatches = await verifyPasswordOrDummyAsync(password, user && {
          salt: user.password_salt,
          storedHash: user.password_hash,
          passwordVersion: user.password_version,
        });
        if (!passwordMatches) {
          pairReservation.commit();
          ipReservation.commit();
          throw new HttpError(401, 'invalid_credentials');
        }
        // Keep version-1 hashes stable. A login cannot prove which
        // whitespace variant is bound to an existing recovery envelope.
        const client = await pool.connect();
        let transactionOpen = false;
        let session;
        try {
          await client.query('begin');
          transactionOpen = true;
          const active = await client.query(
            `select id,username from auth.users where id=$1 and status='active'`, [user.id]
          );
          if (active.rowCount !== 1) {
            pairReservation.commit();
            ipReservation.commit();
            await client.query('rollback');
            transactionOpen = false;
            throw new HttpError(401, 'invalid_credentials');
          }
          session = await issueSession(client, active.rows[0], config.authSecret);
          await client.query('commit');
          transactionOpen = false;
        } catch (error) {
          if (transactionOpen) await client.query('rollback').catch(() => {});
          throw error;
        } finally {
          client.release();
        }
        pairReservation.release();
        ipReservation.release();
        loginFailures.reset(key.pair);
        return json(res, 200, session);
      } catch (error) {
        pairReservation.release();
        ipReservation.release();
        throw error;
      } finally {
        releaseKdf();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/refresh') {
      const body = await readJson(req);
      const refresh = requiredOpaqueString(body.refresh_token, 'invalid_refresh_token', 512);
      // Refresh and logout are unauthenticated endpoints that each trigger a
      // database transaction; without an IP cap, garbage tokens can saturate
      // the pool just like login attempts could.
      const address = requestClientAddress(req, config.trustProxy);
      const ipReservation = refreshIpFailures.reserve(address);
      if (!ipReservation) throw new HttpError(429, 'too_many_attempts');
      const hash = tokenHash(refresh);
      const sessionId = refreshTokenSessionId(refresh, config.authSecret);
      let client;
      let transactionOpen = false;
      try {
        client = await pool.connect();
        await client.query('begin');
        transactionOpen = true;
        const preliminary = await selectRefreshFamily(client, sessionId, hash);
        if (preliminary.rowCount === 0) {
          await client.query('commit');
          transactionOpen = false;
          throw new HttpError(401, 'invalid_refresh_token');
        }
        await lockAuthSessionUser(client, preliminary.rows[0].user_id);
        const selected = await selectRefreshFamily(client, sessionId, hash, true);
        const row = selected.rows[0];
        if (!row || row.status !== 'active' || new Date(row.expires_at).getTime() <= Date.now()) {
          if (row) {
            await client.query('delete from auth.refresh_tokens where session_id=$1', [row.session_id]);
          }
          await client.query('commit');
          transactionOpen = false;
          throw new HttpError(401, 'invalid_refresh_token');
        }
        if (!secureEqualBuffer(row.token_hash, hash)) {
          const rotatedAt = row.rotated_at === null ? Number.NaN :
            new Date(row.rotated_at).getTime();
          const grace = secureEqualBuffer(row.previous_token_hash, hash) &&
            Number.isFinite(rotatedAt) && Date.now() >= rotatedAt &&
            Date.now() - rotatedAt <= REFRESH_REPLAY_GRACE_MS;
          if (grace) {
            const recoveredRefresh = createRefreshSuccessorToken(
              row.session_id, refresh, config.authSecret
            );
            if (secureEqualBuffer(row.token_hash, tokenHash(recoveredRefresh))) {
              await client.query('commit');
              transactionOpen = false;
              return json(res, 200, {
                access_token: signAccessToken(
                  { id: row.user_id, username: row.username }, config.authSecret
                ),
                refresh_token: recoveredRefresh,
                user: { id: row.user_id, username: row.username },
              });
            }
          }
          if (!grace) {
            await client.query('delete from auth.refresh_tokens where session_id=$1', [row.session_id]);
          }
          await client.query('commit');
          transactionOpen = false;
          throw new HttpError(401, 'invalid_refresh_token');
        }
        const rotatedRefresh = createRefreshSuccessorToken(
          row.session_id, refresh, config.authSecret
        );
        const updated = await client.query(
          `update auth.refresh_tokens set token_hash=$1,previous_token_hash=$2,
             rotated_at=now(),expires_at=now()+interval '30 days',created_at=now()
           where session_id=$3 and token_hash=$2`,
          [tokenHash(rotatedRefresh), hash, row.session_id]
        );
        if (updated.rowCount !== 1) throw new HttpError(401, 'invalid_refresh_token');
        await client.query('commit');
        transactionOpen = false;
        const session = {
          access_token: signAccessToken(
            { id: row.user_id, username: row.username }, config.authSecret
          ),
          refresh_token: rotatedRefresh,
          user: { id: row.user_id, username: row.username },
        };
        ipReservation.release();
        return json(res, 200, session);
      } catch (error) {
        if (transactionOpen) await client.query('rollback').catch(() => {});
        // A failed token verification is exactly what the limiter counts.
        if (error instanceof HttpError && error.status === 401) {
          ipReservation.commit();
        } else {
          ipReservation.release();
        }
        throw error;
      } finally {
        ipReservation.release();
        if (client) client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/logout') {
      const body = await readJson(req);
      const address = requestClientAddress(req, config.trustProxy);
      const logoutIpReservation = refreshIpFailures.reserve(address);
      if (!logoutIpReservation) throw new HttpError(429, 'too_many_attempts');
      const refresh = requiredOpaqueString(body.refresh_token, 'invalid_refresh_token', 512);
      const sessionId = refreshTokenSessionId(refresh, config.authSecret);
      const hash = tokenHash(refresh);
      const client = await pool.connect();
      try {
        await client.query('begin');
        const selected = await selectRefreshFamily(client, sessionId, hash);
        if (selected.rowCount > 0) {
          await lockAuthSessionUser(client, selected.rows[0].user_id);
          if (sessionId) {
            await client.query('delete from auth.refresh_tokens where session_id=$1', [sessionId]);
          } else {
            await client.query(
              `delete from auth.refresh_tokens where token_hash=$1 or previous_token_hash=$1
                 or legacy_token_hash=$1`, [hash]
            );
          }
        }
        await client.query('commit');
      } catch (error) {
        await client.query('rollback').catch(() => {});
        if (error instanceof HttpError && (error.status === 400 || error.status === 401)) {
          logoutIpReservation.commit();
        } else {
          logoutIpReservation.release();
        }
        throw error;
      } finally {
        client.release();
      }
      logoutIpReservation.release();
      return json(res, 200, { status: 'ok' });
    }

    // ---- Recovery code: password-independent second key --------------------
    //
    // The plaintext recovery code never travels to the server. Online
    // verification is two-step: the challenge returns the PBKDF2 salt stored
    // inside the envelope (or a deterministic dummy salt for accounts without
    // a code, so this endpoint pair cannot enumerate who has one); the client
    // derives the wrap key locally and submits only its SHA-256 digest.
    if (req.method === 'POST' && url.pathname === '/v1/auth/recover/challenge') {
      const body = await readJson(req);
      requireExactKeys(body, ['username'], 'invalid_credentials');
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const key = loginKey(req, username);
      const reservation = loginFailures.reserve(key.pair);
      if (!reservation) throw new HttpError(429, 'too_many_attempts');
      const ipReservation = loginIpFailures.reserve(key.address);
      if (!ipReservation) {
        reservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        const selected = await pool.query(
          'select id, status from auth.users where username=$1', [username]
        );
        const user = selected.rows[0];
        let saltHex = null;
        if (user && user.status === 'active') {
          const stored = await withTenant(pool, user.id, async (client) => {
            const result = await client.query(
              'select envelope from vault.recovery_codes where tenant_id=$1', [user.id]
            );
            return result.rows[0] || null;
          });
          if (stored) {
            // Envelope layout is salt(16 bytes) || nonce(24) || ciphertext; the
            // client re-derives the wrap key from it without another round trip.
            saltHex = stored.envelope.subarray(0, 16).toString('hex');
          }
        }
        if (!saltHex) {
          // Deterministic per-username dummy so every account sees the same
          // response shape whether or not it has a recovery code.
          saltHex = crypto.createHmac('sha256', config.authSecret)
            .update(`solum-recover-dummy-salt-v1:${username.toLowerCase()}`)
            .digest('hex').slice(0, 32);
        }
        return json(res, 200, { pbkdf2_salt: saltHex, rounds: 600000 });
      } catch (error) {
        reservation.commit();
        ipReservation.commit();
        throw error;
      } finally {
        reservation.release();
        ipReservation.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/recover') {
      const body = await readJson(req);
      requireExactKeys(body, ['username', 'verify_hash'], 'invalid_credentials');
      const username = requiredString(body.username, 'invalid_credentials', 128);
      const digest = requiredOpaqueString(body.verify_hash, 'invalid_credentials', 64);
      if (!SHA256_PATTERN.test(digest)) throw new HttpError(400, 'invalid_credentials');
      const key = loginKey(req, username);
      const pairReservation = loginFailures.reserve(key.pair);
      if (!pairReservation) throw new HttpError(429, 'too_many_attempts');
      const ipReservation = loginIpFailures.reserve(key.address);
      if (!ipReservation) {
        pairReservation.release();
        throw new HttpError(429, 'too_many_attempts');
      }
      try {
        const selected = await pool.query(
          'select id, username, status from auth.users where username=$1', [username]
        );
        const row = selected.rows[0];
        const active = row && row.status === 'active' ? row : null;
        // Burn one scrypt even when the account or its recovery code does not
        // exist, keeping failure timing uniform with real verification.
        await scryptBurnAsync(digest);
        const storedHash = active ? await withTenant(pool, active.id, async (client) => {
          const result = await client.query(
            'select verify_hash from vault.recovery_codes where tenant_id=$1', [active.id]
          );
          return result.rows[0] ? result.rows[0].verify_hash : null;
        }) : null;
        if (!storedHash || !secureEqualHex(storedHash, digest)) {
          pairReservation.commit();
          ipReservation.commit();
          throw new HttpError(401, 'invalid_credentials');
        }
        const session = await issueSessionForUser(pool, active, config.authSecret);
        pairReservation.release();
        ipReservation.release();
        loginFailures.reset(key.pair);
        return json(res, 200, session);
      } catch (error) {
        pairReservation.release();
        ipReservation.release();
        throw error;
      }
    }

    // Explicit password change. Authenticated with either the current password
    // or the current recovery code (as its derived-key digest), then one
    // transaction swaps the credential hash, replaces the password envelope
    // with the client's pre-rewrapped one, and revokes all other refresh
    // sessions - "new password live, envelope still old" is unreachable.

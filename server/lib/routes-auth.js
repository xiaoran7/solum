    if (req.method === 'POST' && url.pathname === '/v1/account/delete') {
      const tenantId = await authenticate(req);
      const client = await pool.connect();
      try {
        await client.query('begin');
        await client.query('delete from auth.oauth_identities where user_id = $1', [tenantId]);
        await client.query('delete from auth.refresh_tokens where user_id = $1', [tenantId]);
        await client.query('delete from sync.alerts where tenant_id = $1', [tenantId]);
        await client.query('delete from ai.usage_events where tenant_id = $1', [tenantId]);
        await client.query('delete from auth.users where id = $1', [tenantId]);
        await client.query('commit');
        return json(res, 200, { ok: true, message: '账号已成功注销' });
      } catch (err) {
        await client.query('rollback').catch(() => {});
        throw err;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/email/register') {
      if (config.registrationMode !== 'open') throw new HttpError(403, 'registration_closed');
      const address = requestClientAddress(req, config.trustProxy);
      if (!registrationAttempts.consume(address)) {
        throw new HttpError(429, 'too_many_attempts');
      }
      const body = await readJson(req);
      const email = requiredString(body.email, 'invalid_email', 128).trim().toLowerCase();
      const password = requiredOpaqueString(body.password, 'invalid_credentials', 1024);
      const code = requiredString(body.code, 'invalid_verification_code', 16);

      if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
        throw new HttpError(400, 'invalid_email_format');
      }
      if ([...password].length < 8) {
        throw new HttpError(400, 'password_too_short');
      }

      const verifyRes = emailVerificationService.verifyCode(email, code);
      if (!verifyRes || !verifyRes.ok) {
        throw new HttpError(400, 'invalid_verification_code');
      }

      const client = await pool.connect();
      try {
        await client.query('begin');
        const existing = await client.query('select id from auth.users where email = $1', [email]);
        if (existing.rowCount > 0) {
          throw new HttpError(409, 'email_already_registered');
        }

        const baseUsername = email.split('@')[0].replace(/[\u0000-\u001f\u007f]/g, '').slice(0, 32) || 'user';
        let candidate = baseUsername;
        let counter = 1;
        while (true) {
          const check = await client.query('select 1 from auth.users where username = $1', [candidate]);
          if (check.rowCount === 0) break;
          candidate = `${baseUsername}_${counter++}`;
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

        const inserted = await client.query(
          `insert into auth.users (username, email, password_hash, password_salt, password_version, status)
           values ($1, $2, $3, $4, 2, 'active') returning id, username`,
          [candidate, email, passwordHash, salt]
        );
        const user = inserted.rows[0];
        const session = await issueSession(client, user, config.authSecret);
        await client.query('commit');
        return json(res, 201, session);
      } catch (error) {
        await client.query('rollback').catch(() => {});
        throw error;
      } finally {
        client.release();
      }
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/email/login') {
      const body = await readJson(req);
      const email = requiredString(body.email, 'invalid_email', 128).trim().toLowerCase();
      const password = requiredOpaqueString(body.password, 'invalid_credentials', 1024);

      if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
        throw new HttpError(400, 'invalid_email_format');
      }

      const key = loginKey(req, email);
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
        const userResult = await pool.query(
          `select id, username, email, password_hash, password_salt, password_version, status
           from auth.users where email = $1`,
          [email]
        );
        const user = userResult.rows[0];
        const valid = user && user.password_hash && user.password_salt
          ? await verifyPasswordAsync(password, user.password_salt, user.password_hash, user.password_version)
          : await verifyPasswordOrDummyAsync(password, null);

        if (!user || !valid) {
          pairReservation.commit();
          ipReservation.commit();
          throw new HttpError(401, 'invalid_credentials');
        }
        if (user.status === 'disabled') throw new HttpError(403, 'account_disabled');

        const client = await pool.connect();
        try {
          await client.query('begin');
          const session = await issueSession(client, user, config.authSecret);
          await client.query('commit');
          return json(res, 200, session);
        } catch (err) {
          await client.query('rollback').catch(() => {});
          throw err;
        } finally {
          client.release();
        }
      } finally {
        releaseKdf();
        pairReservation.release();
        ipReservation.release();
      }
    }

    if (req.method === 'GET' && url.pathname === '/v1/auth/oauth/config') {
      return json(res, 200, {
        enabled: config.oauthEnabled,
        issuer: config.oauthIssuer,
        client_id: config.oauthClientId,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/oauth/exchange') {
      if (!config.oauthEnabled) throw new HttpError(403, 'oauth_disabled');
      const body = await readJson(req);
      const code = requiredString(body.code, 'invalid_code', 2048);
      const codeVerifier = requiredString(body.code_verifier, 'invalid_code_verifier', 256);
      const redirectUri = optionalString(body.redirect_uri, 'invalid_redirect_uri', 2048) || '';

      // Unauthenticated but issuer-facing: without a per-IP cap a garbage-code
      // flood turns this endpoint into free outbound traffic and database
      // transactions, exactly like the refresh-flood the pool protection
      // below already guards against.
      const address = requestClientAddress(req, config.trustProxy);
      const ipReservation = refreshIpFailures.reserve(address);
      if (!ipReservation) throw new HttpError(429, 'too_many_attempts');
      try {
        const tokenUrl = `${config.oauthIssuer.replace(/\/+$/, '')}/token`;
        const userinfoUrl = `${config.oauthIssuer.replace(/\/+$/, '')}/userinfo`;

        const formParams = new URLSearchParams({
          grant_type: 'authorization_code',
          code,
          code_verifier: codeVerifier,
          client_id: config.oauthClientId,
        });
        if (redirectUri) formParams.set('redirect_uri', redirectUri);
        if (config.oauthClientSecret) formParams.set('client_secret', config.oauthClientSecret);

        let tokenResponse;
        try {
          tokenResponse = await fetch(tokenUrl, {
            method: 'POST',
            headers: {
              'Content-Type': 'application/x-www-form-urlencoded',
              'Accept': 'application/json',
            },
            body: formParams.toString(),
          });
        } catch (err) {
          throw new HttpError(502, 'oauth_provider_unreachable');
        }

        if (!tokenResponse.ok) {
          throw new HttpError(401, 'oauth_exchange_failed');
        }

        const tokenData = await tokenResponse.json().catch(() => ({}));
        const accessToken = tokenData.access_token;
        if (!accessToken) throw new HttpError(401, 'oauth_token_missing');

        let userinfoResponse;
        try {
          userinfoResponse = await fetch(userinfoUrl, {
            headers: {
              'Authorization': `Bearer ${accessToken}`,
              'Accept': 'application/json',
            },
          });
        } catch (err) {
          throw new HttpError(502, 'oauth_provider_unreachable');
        }

        if (!userinfoResponse.ok) {
          throw new HttpError(401, 'oauth_userinfo_failed');
        }

        const userinfo = await userinfoResponse.json().catch(() => ({}));
        const subject = userinfo.sub || userinfo.id;
        if (!subject) throw new HttpError(401, 'oauth_subject_missing');

        const email = userinfo.email || null;
        const profileName = userinfo.name || userinfo.preferred_username ||
          userinfo.username || email || subject;

        const client = await pool.connect();
        try {
          await client.query('begin');
          const existingIdentity = await client.query(
            `select u.id, u.username, u.status from auth.oauth_identities oi
             join auth.users u on u.id = oi.user_id
             where oi.provider = $1 and oi.subject = $2`,
            ['tg-oauth', String(subject)]
          );

          let user;
          if (existingIdentity.rowCount === 1) {
            user = existingIdentity.rows[0];
            if (user.status !== 'active') throw new HttpError(403, 'user_disabled');
            await client.query(
              `update auth.oauth_identities set email = $1, profile_name = $2, updated_at = now()
               where provider = $3 and subject = $4`,
              [email, profileName, 'tg-oauth', String(subject)]
            );
          } else {
            if (config.registrationMode !== 'open') throw new HttpError(403, 'registration_closed');
            let usernameCandidate = String(profileName).replace(/[\u0000-\u001f\u007f]/g, '').trim();
            if (!usernameCandidate || usernameCandidate.length > 120) {
              usernameCandidate = `user_${String(subject).slice(0, 16)}`;
            }
            let finalUsername = usernameCandidate;
            let suffix = 1;
            while (true) {
              const existingUser = await client.query(
                'select id from auth.users where username = $1', [finalUsername]
              );
              if (existingUser.rowCount === 0) break;
              finalUsername = `${usernameCandidate}_${suffix++}`;
            }

            const insertedUser = await client.query(
              `insert into auth.users (username, status, password_version)
               values ($1, 'active', 2)
               returning id, username, status`,
              [finalUsername]
            );
            user = insertedUser.rows[0];

            await client.query(
              `insert into auth.oauth_identities (user_id, provider, subject, email, profile_name)
               values ($1, $2, $3, $4, $5)`,
              [user.id, 'tg-oauth', String(subject), email, profileName]
            );
          }

          const session = await issueSession(client, user, config.authSecret);
          await client.query('commit');
          return json(res, 200, session);
        } catch (error) {
          await client.query('rollback').catch(() => {});
          throw error;
        } finally {
          client.release();
        }
      } catch (error) {
        // Issuer-rejected exchanges (401) are brute-force signals; provider
        // outages (502) and disabled accounts (403) are not.
        if (error instanceof HttpError && error.status === 401) ipReservation.commit();
        throw error;
      } finally {
        ipReservation.release();
      }
    }


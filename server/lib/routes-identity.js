    if (req.method === 'GET' && url.pathname === '/v1/auth/huawei/config') {
      return json(res, 200, {
        enabled: Boolean(config.huaweiEnabled && config.huaweiClientId && config.huaweiClientSecret),
        client_id: config.huaweiClientId,
        authorize_url: 'https://oauth-login.cloud.huawei.com/oauth2/v3/authorize',
        redirect_uri: config.huaweiRedirectUri,
        scope: 'openid profile',
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/huawei/exchange') {
      if (!config.huaweiEnabled || !config.huaweiClientId || !config.huaweiClientSecret) {
        throw new HttpError(403, 'huawei_login_disabled');
      }
      const body = await readJson(req);
      const code = requiredString(body.code, 'invalid_code', 2048);
      const redirectUri = optionalString(body.redirect_uri, 'invalid_redirect_uri', 2048) || config.huaweiRedirectUri;

      const address = requestClientAddress(req, config.trustProxy);
      const ipReservation = refreshIpFailures.reserve(address);
      if (!ipReservation) throw new HttpError(429, 'too_many_attempts');
      try {
        if (redirectUri !== config.huaweiRedirectUri) throw new HttpError(400, 'invalid_redirect_uri');
        const tokenRes = await fetch('https://oauth-login.cloud.huawei.com/oauth2/v3/token', {
          method: 'POST', redirect: 'error', signal: AbortSignal.timeout(10000),
          headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
          body: new URLSearchParams({ grant_type: 'authorization_code', code,
            client_id: config.huaweiClientId, client_secret: config.huaweiClientSecret,
            redirect_uri: config.huaweiRedirectUri }).toString(),
        });
        if (!tokenRes.ok) throw new HttpError(401, 'huawei_exchange_failed');
        const { parsed: token } = await readJsonObjectResponse(tokenRes, 65536);
        if (typeof token.access_token !== 'string' || !token.access_token || token.access_token.length > 8192) {
          throw new HttpError(401, 'huawei_exchange_failed');
        }
        const userinfoRes = await fetch('https://account.cloud.huawei.com/rest.php?nsp_fmt=JSON&nsp_svc=GOpen.User.getInfo', {
          redirect: 'error', signal: AbortSignal.timeout(10000),
          headers: { Authorization: `Bearer ${token.access_token}` },
        });
        if (!userinfoRes.ok) throw new HttpError(401, 'huawei_exchange_failed');
        const { parsed: profile } = await readJsonObjectResponse(userinfoRes, 65536);
        const subject = profile.unionID || profile.openID;
        if (typeof subject !== 'string' || !subject.trim() || subject.length > 256 || profile.error) {
          throw new HttpError(401, 'huawei_exchange_failed');
        }
        const profileName = typeof profile.displayName === 'string' ? profile.displayName.slice(0, 128) : '华为用户';

        const client = await pool.connect();
        try {
          await client.query('begin');
          const existing = await client.query(
            `select u.id, u.username, u.status from auth.oauth_identities oi
             join auth.users u on u.id = oi.user_id
             where oi.provider = $1 and oi.subject = $2`,
            ['huawei', String(subject)]
          );

          let user;
          if (existing.rowCount > 0) {
            user = existing.rows[0];
            if (user.status === 'disabled') throw new HttpError(403, 'account_disabled');
            await client.query(
              `update auth.oauth_identities set profile_name = $1, updated_at = now()
               where provider = $2 and subject = $3`,
              [profileName, 'huawei', String(subject)]
            );
          } else {
            if (config.registrationMode !== 'open') throw new HttpError(403, 'registration_closed');
            const baseUsername = `hw_${String(subject).slice(0, 12)}`;
            let candidate = baseUsername;
            let counter = 1;
            while (true) {
              const check = await client.query('select 1 from auth.users where username = $1', [candidate]);
              if (check.rowCount === 0) break;
              candidate = `${baseUsername}_${counter++}`;
            }

            const insertedUser = await client.query(
              `insert into auth.users (username, status, password_version)
               values ($1, 'active', 2) returning id, username`,
              [candidate]
            );
            user = insertedUser.rows[0];
            await client.query(
              `insert into auth.oauth_identities (user_id, provider, subject, profile_name)
               values ($1, $2, $3, $4)`,
              [user.id, 'huawei', String(subject), profileName]
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
      } finally {
        ipReservation.commit();
      }
    }

    // SMS authentication remains unavailable until a real delivery provider is integrated.
    if (req.method === 'POST' && (url.pathname === '/v1/auth/sms/send' ||
        url.pathname === '/v1/auth/phone/register')) {
      throw new HttpError(503, 'sms_unavailable');
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/phone/login') {
      const body = await readJson(req);
      const phone = requiredString(body.phone, 'invalid_phone', 32).trim();
      if (!/^1[3-9]\d{9}$/.test(phone)) throw new HttpError(400, 'invalid_phone_format');

      const code = optionalString(body.code, 'invalid_code', 16);
      const password = body.password === undefined ? null : requiredOpaqueString(body.password, 'invalid_credentials', 1024);

      if (code) {
        throw new HttpError(503, 'sms_unavailable');
      } else if (password) {
        const key = loginKey(req, phone);
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
            `select id, username, password_hash, password_salt, password_version, status
             from auth.users where phone = $1`,
            [phone]
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
      } else {
        throw new HttpError(400, 'missing_code_or_password');
      }
    }


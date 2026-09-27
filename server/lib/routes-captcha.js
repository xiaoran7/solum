    if (req.method === 'GET' && url.pathname === '/v1/auth/captcha') {
      const challenge = captchaService.create();
      return json(res, 200, {
        captcha_id: challenge.captchaId,
        image: challenge.image,
        expires_in: 300,
      });
    }

    if (req.method === 'POST' && url.pathname === '/v1/auth/email/send-code') {
      const body = await readJson(req);
      requireExactKeys(body, ['email', 'captcha_id', 'captcha_answer'], 'invalid_request');
      const email = requiredString(body.email, 'invalid_email', 256).toLowerCase();
      if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) throw new HttpError(400, 'invalid_email');
      const captchaId = requiredString(body.captcha_id, 'invalid_captcha', 64);
      const captchaAnswer = requiredString(body.captcha_answer, 'invalid_captcha', 64);

      const capResult = captchaService.verify(captchaId, captchaAnswer);
      if (!capResult || !capResult.ok) {
        throw new HttpError(400, 'invalid_captcha');
      }

      const client = await pool.connect();
      try {
        const existing = await client.query('select 1 from auth.users where email = $1', [email]);
        if (existing.rowCount > 0) throw new HttpError(409, 'email_already_registered');
      } finally {
        client.release();
      }

      try {
        const sendResult = await emailVerificationService.sendCode({
          email,
          smtpConfig: {
            host: config.smtpHost,
            port: config.smtpPort,
            secure: config.smtpSecure,
            user: config.smtpUser,
            pass: config.smtpPass,
            from: config.smtpFrom,
            fromName: config.smtpFromName,
            subjectPrefix: config.smtpSubjectPrefix,
            enabled: config.smtpEnabled,
            exposeCodeForTest: config.exposeCodeForTest,
          },
        });
        return json(res, 200, sendResult);
      } catch (err) {
        if (err.code === 'cooldown_active') {
          throw new HttpError(429, 'cooldown_active');
        }
        throw err;
      }
    }


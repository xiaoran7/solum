    if (req.method === 'GET' && url.pathname === '/v1/keys/recovery') {
      const tenantId = await authenticate(req);
      const recovery = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select key_version,algorithm,envelope,created_at
           from vault.key_envelopes
           where tenant_id=$1 and recipient_device_id='account-recovery-v1'
             and key_version=1 and algorithm='recovery-xchacha20poly1305'
             and revoked_at is null`, [tenantId]
        );
        if (result.rowCount === 0) return null;
        const row = result.rows[0];
        return { ...row, envelope: row.envelope.toString('base64') };
      });
      return json(res, 200, recovery || { envelope: null });
    }

    if (req.method === 'PUT' && url.pathname === '/v1/keys/recovery') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      if (body.key_version !== 1 || body.algorithm !== 'recovery-xchacha20poly1305') {
        throw new HttpError(400, 'invalid_envelope');
      }
      const envelope = decodeBase64(body.envelope, 'invalid_envelope', 100000);
      if (envelope.length < 40 || envelope.length > 65536) {
        throw new HttpError(400, 'invalid_envelope');
      }
      // Create-only is deliberate. Two first devices may race, but neither is
      // allowed to overwrite an already authoritative key and strand old blobs.
      const effective = await withTenant(pool, tenantId, async (client) => {
        await client.query(
          `insert into vault.key_envelopes
             (tenant_id,recipient_device_id,key_version,algorithm,envelope)
           values($1,'account-recovery-v1',1,'recovery-xchacha20poly1305',$2)
           on conflict(tenant_id,recipient_device_id,key_version,algorithm) do nothing`,
          [tenantId, envelope]
        );
        const saved = await client.query(
          `select key_version,algorithm,envelope,created_at
           from vault.key_envelopes
           where tenant_id=$1 and recipient_device_id='account-recovery-v1'
             and key_version=1 and algorithm='recovery-xchacha20poly1305'
             and revoked_at is null`, [tenantId]
        );
        const row = saved.rows[0];
        return { ...row, envelope: row.envelope.toString('base64') };
      });
      return json(res, 200, effective);
    }

    throw new HttpError(404, 'not_found');
  }

  const server = http.createServer((req, res) => {
    handle(req, res).catch((error) => {
      if (res.headersSent) {
        res.destroy();
        return;
      }
      if (error instanceof HttpError) return json(res, error.status, { error: error.code });
      console.error('request failed', safeErrorLog(error));
      return json(res, 500, { error: 'internal_error' });
    });
  });
  server.headersTimeout = 15_000;
  server.requestTimeout = 60_000;
  server.keepAliveTimeout = 5_000;
  server.maxHeadersCount = 64;

  return {
    server,
    pool,
    captchaService,
    emailVerificationService,
    async close() {
      clearInterval(retentionTimer);
      await new Promise((resolve) => server.close(resolve));
      if (retentionInFlight) await retentionInFlight;
      if (!overrides.pool) await pool.end();
    },
  };
}


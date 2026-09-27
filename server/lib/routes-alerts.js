    if (req.method === 'POST' && url.pathname === '/v1/alerts') {
      const tenantId = await authenticate(req);
      const body = await readJson(req);
      const alert = {
        eventId: requiredString(body.event_id, 'invalid_alert', 256),
        source: requiredString(body.source, 'invalid_alert', 64),
        monitorId: optionalString(body.monitor_id, 'invalid_alert', 256),
        name: optionalString(body.name, 'invalid_alert', 256),
        status: requiredString(body.status, 'invalid_alert', 64),
        latencyMs: optionalBoundedNumber(body.latency_ms, 'invalid_alert',
          { min: 0, max: MAX_ALERT_LATENCY_MS, integer: true }),
        pingLatencyMs: optionalBoundedNumber(body.ping_latency_ms, 'invalid_alert',
          { min: 0, max: MAX_ALERT_LATENCY_MS, integer: true }),
        availability7d: optionalBoundedNumber(body.availability_7d, 'invalid_alert',
          { min: 0, max: 100 }),
        checkedAt: optionalString(body.checked_at, 'invalid_alert', 128),
        detailUrl: optionalHttpUrl(body.detail_url, 'invalid_alert', 2048),
      };
      const seq = await withTenant(pool, tenantId, async (client) => {
        const inserted = await client.query(
          `insert into sync.alerts(tenant_id,event_id,source,monitor_id,name,status,latency_ms,
             ping_latency_ms,availability_7d,checked_at,detail_url)
           values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
           on conflict(tenant_id,event_id) do nothing returning seq`,
          [tenantId, alert.eventId, alert.source, alert.monitorId, alert.name, alert.status,
            alert.latencyMs, alert.pingLatencyMs, alert.availability7d, alert.checkedAt, alert.detailUrl]
        );
        if (inserted.rowCount > 0) return safeDatabaseInteger(inserted.rows[0].seq, 'alert_seq');
        const existing = await client.query(
          'select seq from sync.alerts where tenant_id=$1 and event_id=$2', [tenantId, alert.eventId]
        );
        return safeDatabaseInteger(existing.rows[0].seq, 'alert_seq');
      });
      return json(res, 200, { seq });
    }

    if (req.method === 'GET' && url.pathname === '/v1/alerts') {
      const tenantId = await authenticate(req);
      const since = parseNonNegativeInt(url.searchParams.get('since'), 0);
      const limit = parseNonNegativeInt(url.searchParams.get('limit'), 100, 500);
      if (limit < 1) throw new HttpError(400, 'invalid_query');
      const source = url.searchParams.has('source') ?
        requiredString(url.searchParams.get('source'), 'invalid_query', 64) : null;
      const alerts = await withTenant(pool, tenantId, async (client) => {
        const result = await client.query(
          `select seq,event_id,source,monitor_id,name,status,latency_ms,ping_latency_ms,
                  availability_7d,checked_at,detail_url,received_at
           from sync.alerts where tenant_id=$1 and seq>$2 and ($3::text is null or source=$3)
           order by seq limit $4`, [tenantId, since, source, limit]
        );
        return result.rows.map((row) => ({ ...row,
          seq: safeDatabaseInteger(row.seq, 'alert_seq'),
          latency_ms: row.latency_ms === null ? null :
            safeDatabaseInteger(row.latency_ms, 'alert_latency_ms'),
          ping_latency_ms: row.ping_latency_ms === null ? null :
            safeDatabaseInteger(row.ping_latency_ms, 'alert_ping_latency_ms'),
          availability_7d: row.availability_7d === null ? null : Number(row.availability_7d) }));
      });
      return json(res, 200, { alerts });
    }


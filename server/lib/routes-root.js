    if (req.method === 'GET' && url.pathname === '/') {
      return html(res, DASHBOARD);
    }

    if (req.method === 'GET' && url.pathname === '/v1/meta') {
      return json(res, 200, {
        name: 'Solum Center',
        version: SERVICE_VERSION,
        revision: config.buildRevision,
        api_base: '/',
        registration: config.registrationMode,
        features: ['account', 'ai', 'ai_quota', 'sync', 'alerts', 'devices', 'backups'],
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/health') {
      await pool.query('select 1');
      return json(res, 200, {
        status: 'ok', storage: 'postgres', version: SERVICE_VERSION, revision: config.buildRevision,
      });
    }


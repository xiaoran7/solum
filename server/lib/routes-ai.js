    if (req.method === 'GET' && url.pathname === '/v1/ai/models') {
      const tenantId = await authenticate(req);
      const usage = await readAiUsage(pool, tenantId, config);
      return json(res, 200, {
        default_model: config.modelCatalog.defaultModel,
        models: config.modelCatalog.models.map((id) => ({
          id, is_default: id === config.modelCatalog.defaultModel,
        })),
        custom_models_enabled: usage.custom_models_enabled,
      });
    }

    if (req.method === 'GET' && url.pathname === '/v1/ai/usage') {
      const tenantId = await authenticate(req);
      return json(res, 200, await readAiUsage(pool, tenantId, config));
    }

    if (req.method === 'POST' && url.pathname === '/v1/ai/chat/completions') {
      const tenantId = await authenticate(req);
      return proxyCompletion(req, res, pool, tenantId, config);
    }


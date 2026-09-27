function aiResponseHeaders(reservation, effectiveModel, fallbackReason) {
  return {
    'X-Solum-Request-Id': reservation.requestId,
    'X-Solum-Requested-Model': reservation.requestedModel,
    'X-Solum-Effective-Model': effectiveModel,
    'X-Solum-Fallback': fallbackReason === null ? 'false' : 'true',
    'X-Solum-Fallback-Reason': fallbackReason || 'none',
    'X-Solum-Quota-Limit': String(reservation.quota.monthly_request_limit),
    'X-Solum-Quota-Used': String(reservation.quota.requests_used),
    'X-Solum-Quota-Remaining': String(reservation.quota.requests_remaining),
    'X-Solum-Quota-Reset': reservation.quota.reset_at,
  };
}

function requestSignal(req, res, timeoutMs) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(new Error('upstream_timeout')), timeoutMs);
  timeout.unref();
  const abort = () => controller.abort(new Error('client_disconnected'));
  req.once('aborted', abort);
  res.once('close', abort);
  if (req.aborted || res.destroyed) abort();
  return {
    signal: controller.signal,
    release() {
      clearTimeout(timeout);
      req.off('aborted', abort);
      res.off('close', abort);
    },
  };
}

function isEventStreamResponse(response) {
  const contentType = response && response.headers.get('content-type');
  return typeof contentType === 'string' &&
    contentType.split(';', 1)[0].trim().toLowerCase() === 'text/event-stream';
}

async function requestUpstream(config, body, model, wantStream, req, res) {
  const startedAt = Date.now();
  const lifetime = requestSignal(req, res, wantStream ? 300000 : 60000);
  try {
    const headers = { 'Content-Type': 'application/json' };
    if (config.mimoApiKey) headers.Authorization = `Bearer ${config.mimoApiKey}`;
    const response = await fetch(`${config.mimoBaseUrl.replace(/\/+$/, '')}/chat/completions`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ ...body, model }),
      signal: lifetime.signal,
    });
    return {
      response,
      latencyMs: Date.now() - startedAt,
      release: lifetime.release,
      clientDisconnected: () => lifetime.signal.reason?.message === 'client_disconnected',
    };
  } catch {
    const disconnected = lifetime.signal.reason?.message === 'client_disconnected';
    lifetime.release();
    return {
      response: null,
      latencyMs: Date.now() - startedAt,
      release() {},
      clientDisconnected: () => disconnected,
    };
  }
}

async function readBodyLimited(response, maxBytes) {
  const declared = response.headers.get('content-length');
  if (declared !== null && /^\d+$/.test(declared) && Number(declared) > maxBytes) {
    await response.body.cancel().catch(() => {});
    throw new HttpError(502, 'upstream_response_too_large');
  }
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > maxBytes) {
      await response.body.cancel().catch(() => {});
      throw new HttpError(502, 'upstream_response_too_large');
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString('utf8');
}

async function readJsonObjectResponse(response, maxBytes) {
  const text = await readBodyLimited(response, maxBytes);
  try {
    const parsed = JSON.parse(text);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('invalid');
    return { text, parsed };
  } catch {
    throw new HttpError(502, 'upstream_invalid_response');
  }
}

function waitForDrain(res) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      res.off('drain', drained);
      res.off('close', closed);
      res.off('error', failed);
    };
    const drained = () => { cleanup(); resolve(); };
    const closed = () => { cleanup(); reject(new Error('client_disconnected')); };
    const failed = (error) => { cleanup(); reject(error); };
    res.once('drain', drained);
    res.once('close', closed);
    res.once('error', failed);
  });
}

async function nextStreamChunk(reader) {
  try {
    return await reader.read();
  } catch {
    return { done: true, failed: true };
  }
}

async function proxyCompletion(req, res, pool, tenantId, config) {
  const body = await readJson(req);
  const options = validateCompletionOptions(body);
  const wantStream = options.stream;
  const requestedModel = validateModel(body.model, config.defaultModel);
  const upstreamBody = {
    messages: validateMessages(body.messages),
    stream: wantStream,
  };
  if (options.temperature !== undefined) upstreamBody.temperature = options.temperature;
  if (options.maxTokens !== undefined) upstreamBody.max_tokens = options.maxTokens;
  if (options.tools !== undefined) upstreamBody.tools = options.tools;
  if (options.toolChoice !== undefined) upstreamBody.tool_choice = options.toolChoice;
  const reservation = await reserveAiRequest(pool, tenantId, requestedModel, config);
  let effectiveModel = reservation.effectiveModel;
  let fallbackReason = reservation.fallbackReason;
  let attempt = await requestUpstream(config, upstreamBody, effectiveModel, wantStream, req, res);
  let totalLatency = attempt.latencyMs;
  const usable = (candidate) => candidate.response && candidate.response.ok &&
    (!wantStream || isEventStreamResponse(candidate.response));
  if (!usable(attempt) && !attempt.clientDisconnected() &&
    effectiveModel !== config.modelCatalog.defaultModel) {
    if (attempt.response && attempt.response.body) await attempt.response.body.cancel().catch(() => {});
    fallbackReason = attempt.response && attempt.response.ok ? 'upstream_invalid_content_type' :
      (attempt.response ? `upstream_http_${attempt.response.status}` : 'upstream_unavailable');
    effectiveModel = config.modelCatalog.defaultModel;
    attempt.release();
    attempt = await requestUpstream(config, upstreamBody, effectiveModel, wantStream, req, res);
    totalLatency += attempt.latencyMs;
  }
  const upstream = attempt.response;
  if (!usable(attempt)) {
    if (upstream && upstream.body) await upstream.body.cancel().catch(() => {});
    try {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: attempt.clientDisconnected() ? 'interrupted' : 'failed',
        effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
    } finally {
      attempt.release();
    }
    throw new HttpError(502, upstream && upstream.ok ?
      'upstream_invalid_response' : 'upstream_unavailable');
  }
  if (wantStream) {
    let reader = upstream.body.getReader();
    let first = await nextStreamChunk(reader);
    if ((first.failed || first.done) && !attempt.clientDisconnected() &&
      effectiveModel !== config.modelCatalog.defaultModel) {
      await reader.cancel().catch(() => {});
      attempt.release();
      fallbackReason = 'upstream_stream_interrupted_before_first_byte';
      effectiveModel = config.modelCatalog.defaultModel;
      attempt = await requestUpstream(config, upstreamBody, effectiveModel, true, req, res);
      totalLatency += attempt.latencyMs;
      if (usable(attempt)) {
        reader = attempt.response.body.getReader();
        first = await nextStreamChunk(reader);
      }
    }
    if (!usable(attempt) || first.failed || first.done) {
      if (attempt.response && attempt.response.body) {
        await attempt.response.body.cancel().catch(() => {});
      }
      try {
        await finalizeAiRequest(pool, tenantId, reservation, {
          status: attempt.clientDisconnected() ? 'interrupted' : 'failed',
          effectiveModel, fallbackReason, latencyMs: totalLatency,
        });
      } finally {
        attempt.release();
      }
      throw new HttpError(502, 'upstream_unavailable');
    }
    const responseHeaders = aiResponseHeaders(reservation, effectiveModel, fallbackReason);
    res.writeHead(200, {
      'Content-Type': 'text/event-stream; charset=utf-8',
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      ...responseHeaders,
    });
    try {
      if (!first.done && !res.write(first.value)) await waitForDrain(res);
      while (true) {
        const next = await reader.read();
        if (next.done) break;
        if (!res.write(next.value)) await waitForDrain(res);
      }
    } catch {
      await reader.cancel().catch(() => {});
      try {
        await finalizeAiRequest(pool, tenantId, reservation, {
          status: 'interrupted', effectiveModel, fallbackReason, latencyMs: totalLatency,
        });
      } finally {
        attempt.release();
      }
      res.destroy();
      return;
    }
    try {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: 'succeeded', effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
    } finally {
      attempt.release();
    }
    res.end();
    return;
  }
  let completed;
  let bodyError;
  try {
    completed = await readJsonObjectResponse(upstream, config.aiMaxResponseBytes);
  } catch (error) {
    bodyError = error instanceof HttpError ? error : new HttpError(502, 'upstream_interrupted');
  }
  if (bodyError && !attempt.clientDisconnected() &&
      effectiveModel !== config.modelCatalog.defaultModel) {
    fallbackReason = bodyError.code;
    effectiveModel = config.modelCatalog.defaultModel;
    attempt.release();
    attempt = await requestUpstream(config, upstreamBody, effectiveModel, false, req, res);
    totalLatency += attempt.latencyMs;
    if (usable(attempt)) {
      try {
        completed = await readJsonObjectResponse(attempt.response, config.aiMaxResponseBytes);
        bodyError = null;
      } catch (error) {
        bodyError = error instanceof HttpError ? error : new HttpError(502, 'upstream_interrupted');
      }
    } else {
      if (attempt.response && attempt.response.body) {
        await attempt.response.body.cancel().catch(() => {});
      }
      bodyError = new HttpError(502, attempt.response && attempt.response.ok ?
        'upstream_invalid_response' : 'upstream_unavailable');
    }
  }
  if (bodyError) {
    try {
      await finalizeAiRequest(pool, tenantId, reservation, {
        status: attempt.clientDisconnected() ? 'interrupted' : 'failed',
        effectiveModel, fallbackReason, latencyMs: totalLatency,
      });
    } finally {
      attempt.release();
    }
    throw bodyError;
  }
  const { text: response, parsed } = completed;
  const usage = parsed.usage && typeof parsed.usage === 'object' ? parsed.usage : undefined;
  try {
    await finalizeAiRequest(pool, tenantId, reservation, {
      status: 'succeeded', effectiveModel, fallbackReason, latencyMs: totalLatency, usage,
    });
  } finally {
    attempt.release();
  }
  const responseHeaders = aiResponseHeaders(reservation, effectiveModel, fallbackReason);
  res.writeHead(200, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ...responseHeaders,
  });
  res.end(response);
}


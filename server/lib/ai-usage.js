async function recoverTenantStaleAiRequests(client, tenantId, timeoutSeconds) {
  await client.query(
    `with recovered as (
       update ai.usage_events set status='interrupted',completed_at=now()
       where tenant_id=$1 and status='pending'
         and created_at<now()-($2::text||' seconds')::interval
       returning period_start,fallback_occurred
     ), totals as (
       select period_start,count(*)::integer as failures,
              count(*) filter(where fallback_occurred)::integer as fallbacks
       from recovered group by period_start
     )
     update ai.usage_monthly usage set
       failure_count=usage.failure_count+totals.failures,
       fallback_count=usage.fallback_count+totals.fallbacks,updated_at=now()
     from totals where usage.tenant_id=$1 and usage.period_start=totals.period_start`,
    [tenantId, timeoutSeconds]
  );
}

async function ensureEntitlement(client, tenantId, config) {
  await client.query(
    `insert into ai.entitlements(tenant_id,plan,monthly_request_limit)
     values($1,$2,$3) on conflict(tenant_id) do nothing`,
    [tenantId, config.standardPlan, config.standardMonthlyRequestLimit]
  );
  const selected = await client.query(
    'select plan,monthly_request_limit,custom_models_enabled from ai.entitlements where tenant_id=$1',
    [tenantId]
  );
  return selected.rows[0];
}

function usageProjection(entitlement, row = {}) {
  const safeCount = (value, name) => {
    const count = Number(value || 0);
    if (!Number.isSafeInteger(count) || count < 0) {
      throw new Error(`invalid AI usage counter: ${name}`);
    }
    return count;
  };
  const limit = safeCount(entitlement.monthly_request_limit, 'monthly_request_limit');
  const used = safeCount(row.request_count, 'request_count');
  const periodText = typeof row.period_start === 'string' ? row.period_start : '';
  const periodMatch = /^(\d{4})-(\d{2})-\d{2}$/.exec(periodText);
  const fallbackDate = row.period_start instanceof Date ? row.period_start : new Date();
  const periodYear = periodMatch ? Number(periodMatch[1]) : fallbackDate.getFullYear();
  const periodMonth = periodMatch ? Number(periodMatch[2]) - 1 : fallbackDate.getMonth();
  const resetAt = new Date(Date.UTC(periodYear, periodMonth + 1, 1));
  return {
    plan: entitlement.plan,
    custom_models_enabled: entitlement.custom_models_enabled !== false,
    monthly_request_limit: limit,
    requests_used: used,
    requests_remaining: Math.max(0, limit - used),
    reset_at: resetAt.toISOString(),
    success_count: safeCount(row.success_count, 'success_count'),
    failure_count: safeCount(row.failure_count, 'failure_count'),
    fallback_count: safeCount(row.fallback_count, 'fallback_count'),
    prompt_tokens: safeCount(row.prompt_tokens, 'prompt_tokens'),
    completion_tokens: safeCount(row.completion_tokens, 'completion_tokens'),
    total_tokens: safeCount(row.total_tokens, 'total_tokens'),
  };
}

async function readAiUsage(pool, tenantId, config) {
  return withTenant(pool, tenantId, async (client) => {
    await recoverTenantStaleAiRequests(client, tenantId, config.aiPendingTimeoutSeconds);
    const entitlement = await ensureEntitlement(client, tenantId, config);
    const selected = await client.query(
      `select to_char(period_start,'YYYY-MM-DD') as period_start,
              request_count,success_count,failure_count,fallback_count,
              prompt_tokens,completion_tokens,total_tokens
       from ai.usage_monthly
       where tenant_id=$1 and period_start=date_trunc('month',now())::date`, [tenantId]
    );
    const recent = await client.query(
      `select request_id,requested_model,effective_model,status,fallback_occurred,fallback_reason,
              upstream_latency_ms,created_at,completed_at
       from ai.usage_events where tenant_id=$1 order by created_at desc limit 1`, [tenantId]
    );
    const latest = recent.rows[0] ? { ...recent.rows[0],
      upstream_latency_ms: recent.rows[0].upstream_latency_ms === null ? null :
        safeDatabaseInteger(recent.rows[0].upstream_latency_ms, 'upstream_latency_ms') } : null;
    return { ...usageProjection(entitlement, selected.rows[0]), latest_request: latest };
  });
}

async function reserveAiRequest(pool, tenantId, requestedModel, config) {
  return withTenant(pool, tenantId, async (client) => {
    await recoverTenantStaleAiRequests(client, tenantId, config.aiPendingTimeoutSeconds);
    const entitlement = await ensureEntitlement(client, tenantId, config);
    const requestedAllowed = config.modelCatalog.models.includes(requestedModel);
    const canUseRequested = requestedAllowed &&
      (requestedModel === config.modelCatalog.defaultModel || entitlement.custom_models_enabled);
    const effectiveModel = canUseRequested ? requestedModel : config.modelCatalog.defaultModel;
    const fallbackReason = canUseRequested ? null :
      (requestedAllowed ? 'custom_models_disabled' : 'model_not_allowed');
    await client.query(
      `insert into ai.usage_monthly(tenant_id,period_start)
       values($1,date_trunc('month',now())::date) on conflict(tenant_id,period_start) do nothing`,
      [tenantId]
    );
    const reserved = await client.query(
      `update ai.usage_monthly usage set request_count=request_count+1,updated_at=now()
       from ai.entitlements entitlement
       where usage.tenant_id=$1 and usage.period_start=date_trunc('month',now())::date
         and entitlement.tenant_id=usage.tenant_id
         and usage.request_count<entitlement.monthly_request_limit
       returning to_char(usage.period_start,'YYYY-MM-DD') as period_start,
                 usage.request_count,usage.success_count,usage.failure_count,
                 usage.fallback_count,usage.prompt_tokens,usage.completion_tokens,usage.total_tokens`,
      [tenantId]
    );
    if (reserved.rowCount !== 1) throw new HttpError(429, 'ai_quota_exhausted');
    const requestId = crypto.randomUUID();
    await client.query(
      `insert into ai.usage_events(request_id,tenant_id,period_start,requested_model,effective_model,
                                   fallback_occurred,fallback_reason)
       values($1,$2,date_trunc('month',now())::date,$3,$4,$5,$6)`,
      [requestId, tenantId, requestedModel, effectiveModel, fallbackReason !== null, fallbackReason]
    );
    return {
      requestId, requestedModel, effectiveModel, fallbackReason,
      quota: usageProjection(entitlement, reserved.rows[0]),
    };
  });
}

async function finalizeAiRequest(pool, tenantId, reservation, outcome) {
  return withTenant(pool, tenantId, async (client) => {
    const rawTokens = outcome.usage || {};
    const tokenCount = (value) => Number.isSafeInteger(value) && value >= 0 &&
      value <= MAX_AI_TOKENS_PER_REQUEST ?
      value : 0;
    const tokens = {
      prompt_tokens: tokenCount(rawTokens.prompt_tokens),
      completion_tokens: tokenCount(rawTokens.completion_tokens),
      total_tokens: tokenCount(rawTokens.total_tokens),
    };
    const updated = await client.query(
      `update ai.usage_events set status=$3,effective_model=$4,fallback_occurred=$5,
              fallback_reason=$6,upstream_latency_ms=$7,prompt_tokens=$8,completion_tokens=$9,
              total_tokens=$10,completed_at=now()
       where tenant_id=$1 and request_id=$2 and status='pending' returning period_start`,
      [tenantId, reservation.requestId, outcome.status, outcome.effectiveModel,
        outcome.fallbackReason !== null, outcome.fallbackReason, outcome.latencyMs,
        tokens.prompt_tokens, tokens.completion_tokens, tokens.total_tokens]
    );
    if (updated.rowCount !== 1) return;
    await client.query(
      `update ai.usage_monthly set
         success_count=success_count+$3,
         failure_count=failure_count+$4,
         fallback_count=fallback_count+$5,
         prompt_tokens=prompt_tokens+$6,
         completion_tokens=completion_tokens+$7,
         total_tokens=total_tokens+$8,updated_at=now()
       where tenant_id=$1 and period_start=$2`,
      [tenantId, updated.rows[0].period_start, outcome.status === 'succeeded' ? 1 : 0,
        outcome.status === 'succeeded' ? 0 : 1, outcome.fallbackReason === null ? 0 : 1,
        tokens.prompt_tokens || 0, tokens.completion_tokens || 0, tokens.total_tokens || 0]
    );
  });
}


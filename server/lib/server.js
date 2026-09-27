async function createPostgresServer(overrides = {}) {
  const config = buildConfig(overrides);
  const pool = overrides.pool || new Pool({ max: config.poolMax, idleTimeoutMillis: 30000 });
  await pool.query('select 1');
  await pool.query(`
    alter table auth.users add column if not exists phone text;
    create unique index if not exists users_phone_idx on auth.users (phone) where phone is not null;
    alter table auth.users add column if not exists email text;
    create unique index if not exists users_email_idx on auth.users (email) where email is not null;
  `).catch(() => {});
  await bootstrapAdmin(pool, config);
  let retentionInFlight = null;
  const runRetention = () => {
    if (retentionInFlight) return retentionInFlight;
    retentionInFlight = cleanExpiredData(pool, config)
      .catch((error) => console.error('retention cleanup failed', safeErrorLog(error)))
      .finally(() => { retentionInFlight = null; });
    return retentionInFlight;
  };
  runRetention();
  const retentionTimer = setInterval(() => {
    runRetention();
  }, 6 * 60 * 60 * 1000);
  retentionTimer.unref();
  const loginFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: LOGIN_MAX_FAILURES,
  });
  const loginIpFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: LOGIN_IP_MAX_FAILURES,
  });
  const registrationAttempts = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: REGISTRATION_MAX_ATTEMPTS,
  });
  // Unauthenticated refresh/logout share one per-IP failure window; garbage
  // token floods from one address otherwise hold pool connections in
  // transactions just like login floods did.
  const refreshIpFailures = createFixedWindowLimiter({
    windowMs: LOGIN_WINDOW_MS, maxAttempts: REFRESH_IP_MAX_FAILURES,
  });
  const authKdfConcurrency = createConcurrencyLimiter(MAX_CONCURRENT_AUTH_KDFS);

  const captchaService = new CaptchaService();
  const emailVerificationService = new EmailVerificationService(config);

  async function authenticate(req) {
    const tenantId = verifyAccessToken(bearer(req), config.authSecret);
    if (!tenantId) throw new HttpError(401, 'unauthorized');
    const active = await pool.query(
      "select 1 from auth.users where id=$1 and status='active'", [tenantId]
    );
    if (active.rowCount !== 1) throw new HttpError(401, 'unauthorized');
    return tenantId;
  }

  function loginKey(req, username) {
    const address = requestClientAddress(req, config.trustProxy);
    return { pair: `${address}|${username}`, address };
  }

  async function handle(req, res) {
    const origin = req.headers.origin;
    const originAllowed = typeof origin !== 'string' ||
      requestOriginAllowed(req, origin, config.allowedOrigins);
    if (!originAllowed) throw new HttpError(403, 'origin_not_allowed');
    if (typeof origin === 'string') {
      res.setHeader('Access-Control-Allow-Origin', origin);
      res.setHeader('Vary', 'Origin');
      res.setHeader('Access-Control-Allow-Headers',
        'Content-Type, Authorization, X-Device, X-Solum-Backup-Writer, If-Match');
      res.setHeader('Access-Control-Allow-Methods', 'GET, POST, PUT, DELETE, OPTIONS');
      res.setHeader('Access-Control-Expose-Headers',
        'X-Solum-Request-Id, X-Solum-Requested-Model, X-Solum-Effective-Model, ' +
        'X-Solum-Fallback, X-Solum-Fallback-Reason, X-Solum-Quota-Limit, ' +
        'X-Solum-Quota-Used, X-Solum-Quota-Remaining, X-Solum-Quota-Reset, ETag');
    }
    if (req.method === 'OPTIONS') {
      if (typeof origin !== 'string') throw new HttpError(403, 'origin_not_allowed');
      res.writeHead(204, { 'Content-Length': '0', 'Cache-Control': 'no-store' });
      return res.end();
    }

    const url = new URL(req.url, 'http://localhost');

function buildConfig(overrides = {}) {
  const config = {
    port: Number(process.env.SOLUM_PORT || 8787),
    authSecret: process.env.SOLUM_AUTH_SECRET || '',
    adminUsername: process.env.SOLUM_ADMIN_USERNAME || '',
    adminPassword: process.env.SOLUM_ADMIN_PASSWORD || '',
    registrationMode: process.env.SOLUM_REGISTRATION_MODE || 'closed',
    oauthEnabled: process.env.SOLUM_OAUTH_ENABLED !== 'false',
    oauthIssuer: process.env.SOLUM_OAUTH_ISSUER || 'https://auth.tannerlab.cn',
    oauthClientId: process.env.SOLUM_OAUTH_CLIENT_ID || 'solum-center',
    oauthClientSecret: process.env.SOLUM_OAUTH_CLIENT_SECRET || '',
    huaweiEnabled: process.env.SOLUM_HUAWEI_ENABLED !== 'false',
    huaweiClientId: process.env.SOLUM_HUAWEI_CLIENT_ID || process.env.HUAWEI_APP_ID || 'solum_huawei_client',
    huaweiClientSecret: process.env.SOLUM_HUAWEI_CLIENT_SECRET || process.env.HUAWEI_APP_SECRET || '',
    huaweiRedirectUri: process.env.SOLUM_HUAWEI_REDIRECT_URI || 'solum://oauth/callback',
    mimoApiKey: process.env.SOLUM_LLM_API_KEY || process.env.MIMO_API_KEY || '',
    mimoBaseUrl: process.env.SOLUM_LLM_BASE_URL || process.env.MIMO_BASE_URL || 'https://cpa.tannerlab.cn/v1',
    defaultModel: process.env.SOLUM_DEFAULT_MODEL || 'gemini-3.8-flash-high',
    allowedModels: process.env.SOLUM_ALLOWED_MODELS || 'gemini-3.8-flash-high',
    standardPlan: process.env.SOLUM_STANDARD_PLAN || 'standard',
    standardMonthlyRequestLimit: Number(process.env.SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT || 1000),
    standardDailyRequestLimit: Number(process.env.SOLUM_STANDARD_DAILY_REQUEST_LIMIT || 1000),
    standardWeeklyRequestLimit: Number(process.env.SOLUM_STANDARD_WEEKLY_REQUEST_LIMIT || 3000),
    aiMaxResponseBytes: Number(process.env.SOLUM_AI_MAX_RESPONSE_BYTES || DEFAULT_AI_RESPONSE_BYTES),
    aiPendingTimeoutSeconds: Number(process.env.SOLUM_AI_PENDING_TIMEOUT_SECONDS || 600),
    allowedOrigins: process.env.SOLUM_ALLOWED_ORIGINS || '',
    trustProxy: process.env.SOLUM_TRUST_PROXY === 'true',
    poolMax: Number(process.env.SOLUM_DB_POOL_MAX || 10),
    alertRetentionDays: Number(process.env.SOLUM_ALERT_RETENTION_DAYS || 7),
    usageEventRetentionDays: Number(process.env.SOLUM_USAGE_EVENT_RETENTION_DAYS || 90),
    tenantMaxBlobBytes: Number(process.env.SOLUM_TENANT_MAX_BLOB_BYTES || 1073741824),
    tenantMaxBlobs: Number(process.env.SOLUM_TENANT_MAX_BLOBS || 100000),
    tenantMaxDevices: Number(process.env.SOLUM_TENANT_MAX_DEVICES || 32),
    backupMaxObjectBytes: Number(process.env.SOLUM_BACKUP_MAX_OBJECT_BYTES ||
      DEFAULT_BACKUP_MAX_OBJECT_BYTES),
    backupMaxPackageBytes: Number(process.env.SOLUM_BACKUP_MAX_PACKAGE_BYTES ||
      DEFAULT_BACKUP_MAX_PACKAGE_BYTES),
    backupTenantMaxBytes: Number(process.env.SOLUM_BACKUP_TENANT_MAX_BYTES ||
      DEFAULT_BACKUP_TENANT_MAX_BYTES),
    backupMaxObjects: Number(process.env.SOLUM_BACKUP_MAX_OBJECTS || DEFAULT_BACKUP_MAX_OBJECTS),
    backupMaxPending: Number(process.env.SOLUM_BACKUP_MAX_PENDING || DEFAULT_BACKUP_MAX_PENDING),
    backupMaxVersions: Number(process.env.SOLUM_BACKUP_MAX_VERSIONS || DEFAULT_BACKUP_MAX_VERSIONS),
    backupPendingTtlHours: Number(process.env.SOLUM_BACKUP_PENDING_TTL_HOURS ||
      DEFAULT_BACKUP_PENDING_TTL_HOURS),
    buildRevision: process.env.SOLUM_BUILD_REVISION || 'unknown',
    smtpEnabled: process.env.SOLUM_EMAIL_CODE_ENABLED !== 'false' && process.env.SOLUM_SMTP_ENABLED !== 'false',
    smtpHost: process.env.SOLUM_MAIL_HOST || process.env.SOLUM_SMTP_HOST || process.env.SMTP_HOST || '',
    smtpPort: Number(process.env.SOLUM_MAIL_PORT || process.env.SOLUM_SMTP_PORT || process.env.SMTP_PORT || 465),
    smtpSecure: process.env.SOLUM_MAIL_SECURE !== undefined
      ? process.env.SOLUM_MAIL_SECURE !== 'false'
      : (process.env.SOLUM_SMTP_SECURE !== undefined ? process.env.SOLUM_SMTP_SECURE !== 'false' : undefined),
    smtpUser: process.env.SOLUM_MAIL_USERNAME || process.env.SOLUM_SMTP_USER || process.env.SMTP_USER || '',
    smtpPass: process.env.SOLUM_MAIL_PASSWORD || process.env.SOLUM_SMTP_PASS || process.env.SMTP_PASS || '',
    smtpFrom: process.env.SOLUM_MAIL_FROM || process.env.SOLUM_SMTP_FROM || process.env.SMTP_FROM || '',
    smtpFromName: process.env.SOLUM_MAIL_FROM_NAME || 'Solum',
    smtpSubjectPrefix: process.env.SOLUM_MAIL_SUBJECT_PREFIX || '[Solum] ',
    exposeCodeForTest: false,
    ...overrides,
  };
  if (config.authSecret.length < 32) throw new Error('SOLUM_AUTH_SECRET must be at least 32 characters');
  if (typeof config.adminUsername !== 'string') throw new Error('SOLUM_ADMIN_USERNAME is required');
  config.adminUsername = config.adminUsername.trim();
  if (!config.adminUsername || config.adminUsername.length > 128 ||
      /[\u0000-\u001f\u007f]/.test(config.adminUsername)) {
    throw new Error('SOLUM_ADMIN_USERNAME must be a valid username');
  }
  if (!['closed', 'open'].includes(config.registrationMode)) {
    throw new Error('SOLUM_REGISTRATION_MODE must be closed or open');
  }
  if (!Number.isInteger(config.poolMax) || config.poolMax < 1 || config.poolMax > 100) {
    throw new Error('SOLUM_DB_POOL_MAX must be between 1 and 100');
  }
  if (!Number.isSafeInteger(config.alertRetentionDays) || config.alertRetentionDays < 1 ||
    config.alertRetentionDays > 36500) {
    throw new Error('SOLUM_ALERT_RETENTION_DAYS must be between 1 and 36500');
  }
  if (!Number.isSafeInteger(config.usageEventRetentionDays) ||
    config.usageEventRetentionDays < 30 || config.usageEventRetentionDays > 3650) {
    throw new Error('SOLUM_USAGE_EVENT_RETENTION_DAYS must be between 30 and 3650');
  }
  for (const [name, value, maximum] of [
    ['SOLUM_TENANT_MAX_BLOB_BYTES', config.tenantMaxBlobBytes, Number.MAX_SAFE_INTEGER],
    ['SOLUM_TENANT_MAX_BLOBS', config.tenantMaxBlobs, 10000000],
    ['SOLUM_TENANT_MAX_DEVICES', config.tenantMaxDevices, 10000],
  ]) {
    if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
      throw new Error(`${name} must be a positive safe integer no greater than ${maximum}`);
    }
  }
  for (const [name, value, minimum, maximum] of [
    ['SOLUM_BACKUP_MAX_OBJECT_BYTES', config.backupMaxObjectBytes,
      DEFAULT_BACKUP_MAX_OBJECT_BYTES, 1024 * 1024 * 1024],
    ['SOLUM_BACKUP_MAX_PACKAGE_BYTES', config.backupMaxPackageBytes,
      DEFAULT_BACKUP_MAX_PACKAGE_BYTES, 2 * 1024 * 1024 * 1024],
    ['SOLUM_BACKUP_TENANT_MAX_BYTES', config.backupTenantMaxBytes,
      DEFAULT_BACKUP_TENANT_MAX_BYTES, Number.MAX_SAFE_INTEGER],
    ['SOLUM_BACKUP_MAX_OBJECTS', config.backupMaxObjects,
      DEFAULT_BACKUP_MAX_OBJECTS, DEFAULT_BACKUP_MAX_OBJECTS],
    ['SOLUM_BACKUP_MAX_PENDING', config.backupMaxPending, 1, 100],
    ['SOLUM_BACKUP_MAX_VERSIONS', config.backupMaxVersions, 2, 100],
    ['SOLUM_BACKUP_PENDING_TTL_HOURS', config.backupPendingTtlHours, 1, 24 * 30],
  ]) {
    if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
      throw new Error(`${name} must be a safe integer between ${minimum} and ${maximum}`);
    }
  }
  if (config.backupMaxPackageBytes < config.backupMaxObjectBytes) {
    throw new Error('SOLUM_BACKUP_MAX_PACKAGE_BYTES must cover one maximum-size object');
  }
  if (config.backupTenantMaxBytes < config.backupMaxPackageBytes * 2) {
    throw new Error('SOLUM_BACKUP_TENANT_MAX_BYTES must cover at least two maximum-size backups');
  }
  if (config.buildRevision !== 'unknown' && !/^[0-9a-f]{7,40}$/.test(config.buildRevision)) {
    throw new Error('SOLUM_BUILD_REVISION must be unknown or a 7-40 character lowercase Git SHA');
  }
  validateModel(config.defaultModel, 'qwen3.5-2b');
  config.modelCatalog = parseModelCatalog(config.defaultModel, config.allowedModels);
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(config.standardPlan)) {
    throw new Error('SOLUM_STANDARD_PLAN must be a short identifier');
  }
  if (!Number.isInteger(config.standardMonthlyRequestLimit) ||
    config.standardMonthlyRequestLimit < 1 || config.standardMonthlyRequestLimit > 10000000) {
    throw new Error('SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT must be between 1 and 10000000');
  }
  if (!Number.isInteger(config.standardDailyRequestLimit) ||
    config.standardDailyRequestLimit < 1 || config.standardDailyRequestLimit > 10000000) {
    throw new Error('SOLUM_STANDARD_DAILY_REQUEST_LIMIT must be between 1 and 10000000');
  }
  if (!Number.isInteger(config.standardWeeklyRequestLimit) ||
    config.standardWeeklyRequestLimit < 1 || config.standardWeeklyRequestLimit > 10000000) {
    throw new Error('SOLUM_STANDARD_WEEKLY_REQUEST_LIMIT must be between 1 and 10000000');
  }
  // 日窗必须小于周窗，否则「每天 1000 / 每周 3000」这类配置会出现
  // 日窗永远打不满、周窗先见底的倒挂，限额读起来会自相矛盾。
  if (config.standardDailyRequestLimit > config.standardWeeklyRequestLimit) {
    throw new Error(
      'SOLUM_STANDARD_DAILY_REQUEST_LIMIT must not exceed SOLUM_STANDARD_WEEKLY_REQUEST_LIMIT');
  }
  if (!Number.isInteger(config.aiMaxResponseBytes) ||
    config.aiMaxResponseBytes < 1024 || config.aiMaxResponseBytes > 64 * 1024 * 1024) {
    throw new Error('SOLUM_AI_MAX_RESPONSE_BYTES must be between 1024 and 67108864');
  }
  if (!Number.isInteger(config.aiPendingTimeoutSeconds) ||
    config.aiPendingTimeoutSeconds < 60 || config.aiPendingTimeoutSeconds > 86400) {
    throw new Error('SOLUM_AI_PENDING_TIMEOUT_SECONDS must be between 60 and 86400');
  }
  config.allowedOrigins = parseAllowedOrigins(config.allowedOrigins);
  return config;
}

async function withTenant(pool, tenantId, operation) {
  const client = await pool.connect();
  try {
    await client.query('begin');
    await client.query("select set_config('app.current_tenant_id', $1, true)", [tenantId]);
    const result = await operation(client);
    await client.query('commit');
    return result;
  } catch (error) {
    await client.query('rollback').catch(() => {});
    throw error;
  } finally {
    client.release();
  }
}

async function lockAuthSessionUser(client, userId) {
  await client.query(
    "select pg_advisory_xact_lock(hashtextextended('auth-session:'||$1::text,0))", [userId]
  );
}

async function selectRefreshFamily(client, sessionId, hash, forUpdate = false) {
  const lock = forUpdate ? ' for update of rt' : '';
  const columns = `rt.token_hash,rt.previous_token_hash,rt.legacy_token_hash,rt.rotated_at,
    rt.session_id,rt.user_id,rt.expires_at,u.username,u.status`;
  if (sessionId) {
    return client.query(
      `select ${columns} from auth.refresh_tokens rt join auth.users u on u.id=rt.user_id
       where rt.session_id=$1${lock}`, [sessionId]
    );
  }
  return client.query(
    `select ${columns} from auth.refresh_tokens rt join auth.users u on u.id=rt.user_id
     where rt.token_hash=$1 or rt.previous_token_hash=$1 or rt.legacy_token_hash=$1${lock}`,
    [hash]
  );
}

async function issueSession(client, user, secret) {
  const sessionId = crypto.randomUUID();
  const refresh = createRefreshToken(sessionId, secret);
  await lockAuthSessionUser(client, user.id);
  await client.query('delete from auth.refresh_tokens where user_id=$1 and expires_at<=now()', [user.id]);
  await client.query(
    `delete from auth.refresh_tokens where user_id=$1 and session_id in (
       select session_id from auth.refresh_tokens where user_id=$1
       order by created_at desc,session_id desc offset $2
     )`, [user.id, MAX_REFRESH_SESSIONS_PER_USER - 1]
  );
  await client.query(
    `insert into auth.refresh_tokens(
       token_hash,user_id,session_id,previous_token_hash,legacy_token_hash,
       rotated_at,expires_at
     ) values ($1,$2,$3,null,null,null,now()+interval '30 days')`,
    [tokenHash(refresh), user.id, sessionId]
  );
  return {
    access_token: signAccessToken(user, secret),
    refresh_token: refresh,
    user: { id: user.id, username: user.username },
  };
}

// Session issuance for recovery login: same guarantees as the password path,
// wrapped in its own short advisory-locked transaction.
async function issueSessionForUser(pool, user, secret) {
  const client = await pool.connect();
  let transactionOpen = false;
  try {
    await client.query('begin');
    transactionOpen = true;
    const active = await client.query(
      'select id, username from auth.users where id=$1 and status=\'active\'', [user.id]
    );
    if (active.rowCount !== 1) throw new HttpError(401, 'invalid_credentials');
    const session = await issueSession(client, active.rows[0], secret);
    await client.query('commit');
    transactionOpen = false;
    return session;
  } catch (error) {
    if (transactionOpen) await client.query('rollback').catch(() => {});
    throw error;
  } finally {
    client.release();
  }
}

// Timing uniformity for credential verification against accounts that may not
// exist or may have no recovery code: always pay one scrypt.
async function scryptBurnAsync(input) {
  const salt = crypto.randomBytes(16).toString('hex');
  await new Promise((resolve, reject) => {
    crypto.scrypt(input, salt, 64, (error) => {
      if (error) reject(error); else resolve();
    });
  });
}

async function bootstrapAdmin(pool, config) {
  const existing = await pool.query('select id,username from auth.users where username=$1', [config.adminUsername]);
  if (existing.rowCount > 0) return;
  if ([...config.adminPassword].length < 8) {
    throw new Error('SOLUM_ADMIN_PASSWORD must be at least 8 characters when bootstrapping');
  }
  const salt = crypto.randomBytes(16).toString('hex');
  await pool.query(
    `insert into auth.users(username,password_hash,password_salt,password_version)
     values ($1,$2,$3,2) on conflict(username) do nothing`,
    [config.adminUsername, await passwordHashAsync(config.adminPassword, salt), salt]
  );
}


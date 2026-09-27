class HttpError extends Error {
  constructor(status, code) {
    super(code);
    this.status = status;
    this.code = code;
  }
}

function safeErrorLog(error) {
  const name = error && typeof error.name === 'string' &&
    /^[A-Za-z0-9_-]{1,64}$/.test(error.name) ? error.name : 'Error';
  const code = error && typeof error.code === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(error.code) ?
    error.code : 'unknown';
  return { name, code };
}

function nowSeconds() {
  return Math.floor(Date.now() / 1000);
}

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  });
  res.end(body);
}

function html(res, body) {
  res.writeHead(200, {
    'Content-Type': 'text/html; charset=utf-8',
    'Content-Length': body.length,
    'Cache-Control': 'no-cache',
    'X-Content-Type-Options': 'nosniff',
    'Content-Security-Policy': "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
  });
  res.end(body);
}

async function readBytes(req, limit) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw new HttpError(413, 'request_too_large');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function readExactBytes(req, expected) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > expected) throw new HttpError(409, 'backup_object_mismatch');
    chunks.push(chunk);
  }
  if (size !== expected) throw new HttpError(409, 'backup_object_mismatch');
  return Buffer.concat(chunks, size);
}

async function readJson(req) {
  if (!isJsonContentType(req)) throw new HttpError(415, 'unsupported_media_type');
  const raw = await readBytes(req, MAX_JSON_BYTES);
  try {
    const parsed = JSON.parse(raw.toString('utf8') || '{}');
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new HttpError(400, 'invalid_json');
    }
    return parsed;
  } catch {
    throw new HttpError(400, 'invalid_json');
  }
}

function requiredString(value, code, max = 256) {
  if (typeof value !== 'string' || value.trim().length === 0 || value.length > max) {
    throw new HttpError(400, code);
  }
  return value.trim();
}

// Passwords and bearer-style secrets are opaque. Trimming them made multiple
// login strings authenticate as the same password while the client derived a
// different recovery key from each exact string.
function requiredOpaqueString(value, code, max) {
  if (typeof value !== 'string' || value.length === 0 || value.length > max) {
    throw new HttpError(400, code);
  }
  return value;
}

function optionalString(value, code, max = 2048) {
  if (value === undefined || value === null || value === '') return null;
  if (typeof value !== 'string' || value.length > max) throw new HttpError(400, code);
  return value.trim() || null;
}

function optionalBoundedNumber(value, code, { min, max, integer = false }) {
  if (value === undefined || value === null) return null;
  if (typeof value !== 'number' || !Number.isFinite(value) || value < min || value > max ||
    (integer && !Number.isSafeInteger(value))) throw new HttpError(400, code);
  return value;
}

function optionalHttpUrl(value, code, max = 2048) {
  const text = optionalString(value, code, max);
  if (text === null) return null;
  try {
    const parsed = new URL(text);
    if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error('invalid protocol');
    return parsed.toString();
  } catch {
    throw new HttpError(400, code);
  }
}

function safeDatabaseInteger(value, name) {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`database integer exceeds JSON precision: ${name}`);
  }
  return parsed;
}

function decodeBase64(value, code, maxChars) {
  const text = requiredString(value, code, maxChars);
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(text) || text.length % 4 !== 0) {
    throw new HttpError(400, code);
  }
  const decoded = Buffer.from(text, 'base64');
  if (decoded.toString('base64') !== text) throw new HttpError(400, code);
  return decoded;
}

function parseNonNegativeInt(value, fallback, max = Number.MAX_SAFE_INTEGER) {
  if (value === null || value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(String(value))) throw new HttpError(400, 'invalid_query');
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0 || parsed > max) {
    throw new HttpError(400, 'invalid_query');
  }
  return parsed;
}

function requireExactKeys(value, expected, code) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new HttpError(400, code);
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new HttpError(400, code);
  }
}

function backupUuid(value) {
  if (typeof value !== 'string' || !UUID_PATTERN.test(value) ||
      value === '00000000-0000-0000-0000-000000000000') {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  return value;
}

function backupWriterId(value, code = 'invalid_backup_plan') {
  if (typeof value !== 'string' || !UUID_PATTERN.test(value) ||
      value === '00000000-0000-0000-0000-000000000000') {
    throw new HttpError(code === 'backup_writer_stale' ? 409 : 400, code);
  }
  return value;
}

function backupGeneration(value) {
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  return value;
}

function parseBackupPlan(body, config) {
  requireExactKeys(body,
    ['backup_id', 'generation', 'created_at', 'protocol_version', 'objects'],
    'invalid_backup_plan');
  if (body.protocol_version !== 1) throw new HttpError(400, 'unsupported_backup_version');
  if (typeof body.created_at !== 'string' ||
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(body.created_at) ||
      body.created_at.startsWith('0000-')) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  const parsedDate = new Date(body.created_at);
  if (!Number.isFinite(parsedDate.getTime()) || parsedDate.toISOString() !== body.created_at) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  if (!Array.isArray(body.objects) || body.objects.length < 1 ||
      body.objects.length > config.backupMaxObjects) {
    throw new HttpError(400, 'invalid_backup_plan');
  }
  const seen = new Set();
  let mainCount = 0;
  let totalBytes = 0;
  const objects = body.objects.map((item) => {
    requireExactKeys(item, ['object_id', 'kind', 'bytes', 'sha256'], 'invalid_backup_plan');
    const objectId = backupUuid(item.object_id);
    if (seen.has(objectId)) throw new HttpError(400, 'invalid_backup_plan');
    seen.add(objectId);
    if (!['main', 'attachment'].includes(item.kind)) {
      throw new HttpError(400, 'invalid_backup_plan');
    }
    if (item.kind === 'main') mainCount += 1;
    if (!Number.isSafeInteger(item.bytes) || item.bytes < 1) {
      throw new HttpError(400, 'invalid_backup_plan');
    }
    if (item.bytes > config.backupMaxObjectBytes) {
      throw new HttpError(413, 'backup_object_too_large');
    }
    if (typeof item.sha256 !== 'string' || !SHA256_PATTERN.test(item.sha256)) {
      throw new HttpError(400, 'invalid_backup_plan');
    }
    totalBytes += item.bytes;
    if (!Number.isSafeInteger(totalBytes) || totalBytes > config.backupMaxPackageBytes) {
      throw new HttpError(413, 'backup_package_too_large');
    }
    return { object_id: objectId, kind: item.kind, bytes: item.bytes, sha256: item.sha256 };
  }).sort((left, right) => left.object_id < right.object_id ? -1 :
    (left.object_id > right.object_id ? 1 : 0));
  if (mainCount !== 1) throw new HttpError(400, 'invalid_backup_plan');
  const plan = {
    backup_id: backupUuid(body.backup_id),
    generation: backupGeneration(body.generation),
    created_at: body.created_at,
    protocol_version: 1,
    objects,
  };
  const canonical = Buffer.from(JSON.stringify(plan), 'utf8');
  return {
    ...plan,
    totalBytes,
    planSha256: crypto.createHash('sha256').update(canonical).digest(),
  };
}

function parseTakeover(body) {
  requireExactKeys(body, ['expected_generation', 'takeover_id', 'writer_id'],
    'invalid_backup_plan');
  return {
    expectedGeneration: backupGeneration(body.expected_generation),
    takeoverId: backupUuid(body.takeover_id),
    writerId: backupWriterId(body.writer_id),
  };
}

function parseWriterClaim(body) {
  requireExactKeys(body, ['expected_generation', 'writer_id'], 'invalid_backup_plan');
  const expectedGeneration = backupGeneration(body.expected_generation);
  if (expectedGeneration !== 1) throw new HttpError(400, 'invalid_backup_plan');
  return { expectedGeneration, writerId: backupWriterId(body.writer_id) };
}

function quotedDigest(buffer) {
  return `"${buffer.toString('hex')}"`;
}

function parseIfMatch(value) {
  if (typeof value !== 'string') throw new HttpError(428, 'precondition_required');
  const match = /^"([0-9a-f]{64})"$/.exec(value);
  if (!match) throw new HttpError(412, 'precondition_failed');
  return Buffer.from(match[1], 'hex');
}

function isoTimestamp(value) {
  const parsed = value instanceof Date ? value : new Date(value);
  if (!Number.isFinite(parsed.getTime())) throw new Error('invalid database timestamp');
  return parsed.toISOString();
}

function binary(res, body, digest) {
  res.writeHead(200, {
    'Content-Type': 'application/octet-stream',
    'Content-Length': body.length,
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ETag: quotedDigest(digest),
  });
  res.end(body);
}

function secureEqualBuffer(left, right) {
  return Buffer.isBuffer(left) && Buffer.isBuffer(right) && left.length === right.length &&
    crypto.timingSafeEqual(left, right);
}

function tokenHash(token) {
  return crypto.createHash('sha256').update(token).digest();
}

function signAccessToken(user, secret) {
  const payload = Buffer.from(JSON.stringify({
    sub: user.id,
    username: user.username,
    exp: nowSeconds() + ACCESS_TTL_SECONDS,
    nonce: crypto.randomBytes(12).toString('hex'),
  })).toString('base64url');
  const signature = crypto.createHmac('sha256', secret).update(payload).digest('base64url');
  return `${payload}.${signature}`;
}

function verifyAccessToken(token, secret) {
  if (typeof token !== 'string') return null;
  const parts = token.split('.');
  if (parts.length !== 2) return null;
  const expected = crypto.createHmac('sha256', secret).update(parts[0]).digest();
  let actual;
  try {
    actual = Buffer.from(parts[1], 'base64url');
  } catch {
    return null;
  }
  if (actual.length !== expected.length || !crypto.timingSafeEqual(actual, expected)) return null;
  try {
    const payload = JSON.parse(Buffer.from(parts[0], 'base64url').toString('utf8'));
    if (typeof payload.sub !== 'string' || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(payload.sub) ||
      !Number.isInteger(payload.exp) || payload.exp <= nowSeconds()) return null;
    return payload.sub;
  } catch {
    return null;
  }
}

function bearer(req) {
  const value = req.headers.authorization;
  return typeof value === 'string' && value.startsWith('Bearer ') ? value.slice(7) : null;
}

function validateModel(value, fallback) {
  const model = value === undefined ? fallback : requiredString(value, 'invalid_model', 100);
  if (!/^[A-Za-z0-9._:/-]+$/.test(model)) throw new HttpError(400, 'invalid_model');
  return model;
}

function parseModelCatalog(defaultModel, allowedModels) {
  const fallback = validateModel(defaultModel, 'qwen3.5-2b');
  const models = [fallback];
  for (const candidate of String(allowedModels || '').split(',')) {
    const trimmed = candidate.trim();
    if (!trimmed) continue;
    const model = validateModel(trimmed, fallback);
    if (!models.includes(model)) models.push(model);
  }
  return Object.freeze({ defaultModel: fallback, models: Object.freeze(models) });
}

function validateMessages(value) {
  if (!Array.isArray(value) || value.length === 0 || value.length > 16) {
    throw new HttpError(400, 'invalid_messages');
  }
  return value.map((item) => {
    if (!item || typeof item !== 'object' || Array.isArray(item) ||
      !['system', 'user', 'assistant'].includes(item.role) ||
      typeof item.content !== 'string' || item.content.length === 0 || item.content.length > 32000) {
      throw new HttpError(400, 'invalid_messages');
    }
    return { role: item.role, content: item.content };
  });
}

function validateCompletionOptions(body) {
  if (!body || typeof body !== 'object' || Array.isArray(body)) {
    throw new HttpError(400, 'invalid_request');
  }
  if (body.stream !== undefined && typeof body.stream !== 'boolean') {
    throw new HttpError(400, 'invalid_stream');
  }
  if (body.temperature !== undefined &&
    (typeof body.temperature !== 'number' || !Number.isFinite(body.temperature) ||
      body.temperature < 0 || body.temperature > 2)) {
    throw new HttpError(400, 'invalid_temperature');
  }
  if (body.max_tokens !== undefined &&
    (!Number.isInteger(body.max_tokens) || body.max_tokens < 1 || body.max_tokens > 32768)) {
    throw new HttpError(400, 'invalid_max_tokens');
  }
  const tools = validateTools(body.tools);
  const toolChoice = validateToolChoice(body.tool_choice, tools);
  return {
    stream: body.stream === true,
    temperature: body.temperature,
    maxTokens: body.max_tokens,
    tools,
    toolChoice,
  };
}

function validateTools(value) {
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.length === 0 || value.length > 16 ||
      JSON.stringify(value).length > 65536) throw new HttpError(400, 'invalid_tools');
  return value.map((tool) => {
    if (!tool || typeof tool !== 'object' || Array.isArray(tool) || tool.type !== 'function' ||
        Object.keys(tool).some((key) => !['type', 'function'].includes(key))) {
      throw new HttpError(400, 'invalid_tools');
    }
    const fn = tool.function;
    if (!fn || typeof fn !== 'object' || Array.isArray(fn) ||
        Object.keys(fn).some((key) => !['name', 'description', 'parameters', 'strict'].includes(key)) ||
        typeof fn.name !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(fn.name) ||
        (fn.description !== undefined && (typeof fn.description !== 'string' || fn.description.length > 1000)) ||
        !fn.parameters || typeof fn.parameters !== 'object' || Array.isArray(fn.parameters) ||
        (fn.strict !== undefined && typeof fn.strict !== 'boolean')) {
      throw new HttpError(400, 'invalid_tools');
    }
    return { type: 'function', function: { ...fn } };
  });
}

function validateToolChoice(value, tools) {
  if (value === undefined) return undefined;
  if (!tools) throw new HttpError(400, 'invalid_tool_choice');
  if (['auto', 'none', 'required'].includes(value)) return value;
  if (!value || typeof value !== 'object' || Array.isArray(value) || value.type !== 'function' ||
      Object.keys(value).some((key) => !['type', 'function'].includes(key)) ||
      !value.function || typeof value.function !== 'object' || Array.isArray(value.function) ||
      Object.keys(value.function).some((key) => key !== 'name') ||
      typeof value.function.name !== 'string' ||
      !tools.some((tool) => tool.function.name === value.function.name)) {
    throw new HttpError(400, 'invalid_tool_choice');
  }
  return { type: 'function', function: { name: value.function.name } };
}

function parseAllowedOrigins(value) {
  return new Set(String(value || '').split(',').map((item) => item.trim()).filter(Boolean).map((item) => {
    try {
      return new URL(item).origin;
    } catch {
      throw new Error(`invalid SOLUM_ALLOWED_ORIGINS entry: ${item}`);
    }
  }));
}


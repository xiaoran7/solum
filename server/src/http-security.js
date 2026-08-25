'use strict';

const crypto = require('node:crypto');
const net = require('node:net');

const DEFAULT_MAX_RATE_LIMIT_KEYS = 10000;
const UUID_PATTERN = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const DUMMY_PASSWORD_HASH = '0'.repeat(128);

function passwordHashAsync(password, salt) {
  return new Promise((resolve, reject) => {
    crypto.scrypt(password, salt, 64, (error, derivedKey) => {
      if (error) reject(error);
      else resolve(derivedKey.toString('hex'));
    });
  });
}

function secureEqualHex(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string' || left.length !== right.length) {
    return false;
  }
  try {
    return crypto.timingSafeEqual(Buffer.from(left, 'hex'), Buffer.from(right, 'hex'));
  } catch {
    return false;
  }
}

// Released auth routes trimmed passwords, while bootstrap-admin creation did
// not. Exact-first verification supports both histories without guessing the
// row's origin. Legacy rows stay on version 1 until an explicit password reset
// or recovery flow can rebind any client-side password envelope. Automatically
// replacing a legacy hash after login would make a trim-equivalent password
// win over the original input and could strand that envelope.
async function verifyPasswordAsync(password, salt, storedHash, passwordVersion,
  hashPassword = passwordHashAsync) {
  const exactHash = await hashPassword(password, salt);
  const exactMatches = secureEqualHex(exactHash, storedHash);
  const trimmed = password.trim();
  let legacyMatches = false;
  if (password !== trimmed) {
    // A padded attempt always performs both KDFs, regardless of whether the
    // account is current, legacy, or absent. Otherwise legacy rows would be
    // distinguishable by their roughly doubled failure latency.
    const trimmedHash = await hashPassword(trimmed, salt);
    legacyMatches = passwordVersion === 1 && secureEqualHex(trimmedHash, storedHash);
  }
  return exactMatches || legacyMatches;
}

async function verifyPasswordOrDummyAsync(password, credentials) {
  if (credentials) {
    return verifyPasswordAsync(password, credentials.salt, credentials.storedHash,
      credentials.passwordVersion);
  }
  await verifyPasswordAsync(password, crypto.randomBytes(16).toString('hex'),
    DUMMY_PASSWORD_HASH, 2);
  return false;
}

function signRefreshToken(sessionId, nonce, secret) {
  if (!UUID_PATTERN.test(sessionId) || !/^[A-Za-z0-9_-]{64}$/.test(nonce) ||
      typeof secret !== 'string' || secret.length === 0) {
    throw new Error('invalid refresh token signing input');
  }
  const payload = `${sessionId}.${nonce}`;
  const signature = crypto.createHmac('sha256', secret)
    .update(`solum-refresh-v1:${payload}`).digest('base64url');
  return `${payload}.${signature}`;
}

function createRefreshToken(sessionId, secret) {
  return signRefreshToken(sessionId, crypto.randomBytes(48).toString('base64url'), secret);
}

function createRefreshSuccessorToken(sessionId, currentToken, secret) {
  if (typeof currentToken !== 'string' || currentToken.length === 0) {
    throw new Error('invalid refresh token successor input');
  }
  const currentHash = crypto.createHash('sha256').update(currentToken).digest('hex');
  const nonce = crypto.createHmac('sha384', secret)
    .update(`solum-refresh-successor-v1:${sessionId}:${currentHash}`)
    .digest('base64url');
  return signRefreshToken(sessionId, nonce, secret);
}

function refreshTokenSessionId(token, secret) {
  if (typeof token !== 'string' || typeof secret !== 'string') return null;
  const parts = token.split('.');
  if (parts.length !== 3 || !UUID_PATTERN.test(parts[0]) ||
    !/^[A-Za-z0-9_-]{64}$/.test(parts[1]) || !/^[A-Za-z0-9_-]{43}$/.test(parts[2])) {
    return null;
  }
  const payload = `${parts[0]}.${parts[1]}`;
  const expected = crypto.createHmac('sha256', secret)
    .update(`solum-refresh-v1:${payload}`).digest();
  const actual = Buffer.from(parts[2], 'base64url');
  if (actual.length !== expected.length || !crypto.timingSafeEqual(actual, expected)) return null;
  return parts[0];
}

function isJsonContentType(req) {
  const value = req.headers['content-type'];
  if (typeof value !== 'string') return false;
  const mediaType = value.split(';', 1)[0].trim().toLowerCase();
  return mediaType === 'application/json' ||
    (mediaType.startsWith('application/') && mediaType.endsWith('+json'));
}

function requestOriginAllowed(req, origin, allowedOrigins) {
  if (allowedOrigins.has(origin)) return true;
  try {
    const requestHost = typeof req.headers.host === 'string' ? req.headers.host.toLowerCase() : '';
    return requestHost.length > 0 && new URL(origin).host.toLowerCase() === requestHost;
  } catch {
    return false;
  }
}

function requestClientAddress(req, trustProxy = false) {
  if (trustProxy) {
    for (const header of ['cf-connecting-ip', 'x-real-ip']) {
      const value = req.headers[header];
      if (typeof value === 'string' && net.isIP(value.trim())) return value.trim();
    }
  }
  return req.socket.remoteAddress || 'unknown';
}

function createFixedWindowLimiter({ windowMs, maxAttempts, maxKeys = DEFAULT_MAX_RATE_LIMIT_KEYS }) {
  if (!Number.isSafeInteger(windowMs) || windowMs < 1 ||
    !Number.isSafeInteger(maxAttempts) || maxAttempts < 1 ||
    !Number.isSafeInteger(maxKeys) || maxKeys < 1) {
    throw new Error('invalid rate limiter configuration');
  }
  const entries = new Map();

  function makeRoom() {
    if (entries.size < maxKeys) return true;
    for (const [key, record] of entries) {
      if (record.inFlight === 0) {
        entries.delete(key);
        return true;
      }
    }
    return false;
  }

  function createRecord(key, now) {
    if (!makeRoom()) return null;
    const record = { count: 0, inFlight: 0, startedAt: now };
    entries.set(key, record);
    return record;
  }

  function activeRecord(key, now) {
    const record = entries.get(key);
    if (record && now - record.startedAt < windowMs) return record;
    if (record) entries.delete(key);
    return null;
  }

  function record(key, now = Date.now()) {
    const existing = activeRecord(key, now);
    if (existing) {
      existing.count += 1;
      return existing.count;
    }
    const created = createRecord(key, now);
    if (!created) return null;
    created.count = 1;
    return 1;
  }

  function isLimited(key, now = Date.now()) {
    const existing = activeRecord(key, now);
    return existing !== null && existing.count + existing.inFlight >= maxAttempts;
  }

  return {
    isLimited,
    record,
    consume(key, now = Date.now()) {
      if (isLimited(key, now)) return false;
      return record(key, now) !== null;
    },
    reserve(key, now = Date.now()) {
      let target = activeRecord(key, now);
      if (target && target.count + target.inFlight >= maxAttempts) return null;
      if (!target) target = createRecord(key, now);
      if (!target) return null;
      target.inFlight += 1;
      let settled = false;
      const settle = (commit, settledAt = Date.now()) => {
        if (settled) return;
        settled = true;
        if (entries.get(key) !== target) {
          if (commit) record(key, settledAt);
          return;
        }
        target.inFlight = Math.max(0, target.inFlight - 1);
        if (commit) target.count += 1;
        if (target.count === 0 && target.inFlight === 0) entries.delete(key);
      };
      return {
        commit(settledAt) { settle(true, settledAt); },
        release() { settle(false); },
      };
    },
    reset(key) {
      const record = entries.get(key);
      if (!record) return;
      record.count = 0;
      if (record.inFlight === 0) entries.delete(key);
    },
    get size() {
      return entries.size;
    },
  };
}

function createConcurrencyLimiter(maxConcurrent) {
  if (!Number.isSafeInteger(maxConcurrent) || maxConcurrent < 1) {
    throw new Error('invalid concurrency limiter configuration');
  }
  let active = 0;
  return {
    tryAcquire() {
      if (active >= maxConcurrent) return null;
      active += 1;
      let released = false;
      return () => {
        if (released) return;
        released = true;
        active -= 1;
      };
    },
    get active() {
      return active;
    },
  };
}

module.exports = {
  createRefreshToken,
  createRefreshSuccessorToken,
  createConcurrencyLimiter,
  createFixedWindowLimiter,
  isJsonContentType,
  passwordHashAsync,
  refreshTokenSessionId,
  requestClientAddress,
  requestOriginAllowed,
  secureEqualHex,
  verifyPasswordAsync,
  verifyPasswordOrDummyAsync,
};

'use strict';

const http = require('node:http');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { Pool } = require('pg');
const {
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
} = require('./http-security');
const { CaptchaService } = require('./captcha');
const { EmailVerificationService } = require('./mailer');

const ACCESS_TTL_SECONDS = 15 * 60;
const REFRESH_TTL_SECONDS = 30 * 24 * 60 * 60;
const MAX_JSON_BYTES = 1024 * 1024;
const MAX_BLOB_BYTES = 8 * 1024 * 1024;
const MAX_PULL_BYTES = 16 * 1024 * 1024;
const MAX_PULL_ROWS = 500;
const DEFAULT_AI_RESPONSE_BYTES = 8 * 1024 * 1024;
const LOGIN_WINDOW_MS = 15 * 60 * 1000;
const LOGIN_MAX_FAILURES = 5;
const LOGIN_IP_MAX_FAILURES = 50;
const MAX_CONCURRENT_AUTH_KDFS = 8;
const REGISTRATION_MAX_ATTEMPTS = 5;
const REFRESH_IP_MAX_FAILURES = 100;
const MAX_REFRESH_SESSIONS_PER_USER = 32;
const REFRESH_REPLAY_GRACE_MS = 5_000;
const MAX_AI_TOKENS_PER_REQUEST = 10 * 1000 * 1000;
const MAX_ALERT_LATENCY_MS = 24 * 60 * 60 * 1000;
const DEFAULT_BACKUP_MAX_OBJECT_BYTES = 68 * 1024 * 1024;
const DEFAULT_BACKUP_MAX_PACKAGE_BYTES = 700 * 1024 * 1024;
const DEFAULT_BACKUP_TENANT_MAX_BYTES = 2 * 1024 * 1024 * 1024;
const DEFAULT_BACKUP_MAX_OBJECTS = 64;
const DEFAULT_BACKUP_MAX_PENDING = 4;
const DEFAULT_BACKUP_MAX_VERSIONS = 10;
const DEFAULT_BACKUP_PENDING_TTL_HOURS = 24;
const UUID_PATTERN = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const DASHBOARD = fs.readFileSync(path.join(__dirname, 'dashboard.html'));
const SERVICE_VERSION = require('../package.json').version;


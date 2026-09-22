'use strict';

const crypto = require('node:crypto');
const net = require('node:net');
const tls = require('node:tls');

const CODE_TTL_MS = 600_000; // 10 minutes
const COOLDOWN_MS = 60_000;  // 60 seconds
const MAX_VERIFY_ATTEMPTS = 5;
const SWEEP_INTERVAL_MS = 60_000;

function wrapBase64(str, maxLen = 76) {
  const b64 = Buffer.from(str).toString('base64');
  const lines = [];
  for (let i = 0; i < b64.length; i += maxLen) {
    lines.push(b64.slice(i, i + maxLen));
  }
  return lines.join('\r\n');
}

/**
 * 验证码用途定义，对齐 BloomXP 的 Purpose 与邮件文案规范
 */
const PURPOSES = {
  REGISTER: {
    body: '注册',
    subject: '注册验证码',
  },
  RESET: {
    body: '重置密码',
    subject: '重置密码验证码',
  },
  LOGIN: {
    body: '登录',
    subject: '登录验证码',
  },
  CHANGE_EMAIL: {
    body: '更换邮箱',
    subject: '更换邮箱验证码',
  },
};

/**
 * 发件人地址解析，对齐 BloomXP MailFrom.resolve 逻辑：
 * 优先显式配置的发件地址，回退至 SMTP 认证用户名（如 QQ/Foxmail 邮箱要求必须一致），
 * 两者皆空时使用占位地址。
 */
function resolveMailFrom(configuredFrom, mailUsername) {
  if (configuredFrom && String(configuredFrom).trim()) return String(configuredFrom).trim();
  if (mailUsername && String(mailUsername).trim()) return String(mailUsername).trim();
  return 'no-reply@solum.local';
}

class SimpleSmtpClient {
  /**
   * @param {object} options
   * @param {string} options.host
   * @param {number} [options.port]
   * @param {boolean} [options.secure]
   * @param {string} options.user
   * @param {string} options.pass
   * @param {string} [options.from]
   * @param {string} [options.fromName]
   */
  constructor(options) {
    this.host = options.host;
    this.port = Number(options.port) || (options.secure ? 465 : 587);
    this.secure = options.secure !== undefined ? Boolean(options.secure) : (this.port === 465);
    this.user = options.user;
    this.pass = options.pass;
    this.from = options.from || options.user;
    this.fromName = options.fromName || 'Solum';
  }

  async sendMail({ to, subject, html, text }) {
    return new Promise((resolve, reject) => {
      let socket;
      let rawSocket;
      let buffer = '';
      let step = 0;
      const timeoutMs = 15000;

      const timer = setTimeout(() => {
        cleanup();
        reject(new Error('SMTP connection timed out'));
      }, timeoutMs);

      const cleanup = () => {
        clearTimeout(timer);
        if (socket && !socket.destroyed) {
          socket.end();
        }
        if (rawSocket && !rawSocket.destroyed && rawSocket !== socket) {
          rawSocket.end();
        }
      };

      const sendLine = (line) => {
        socket.write(line + '\r\n');
      };

      const sendMailBody = () => {
        const messageId = `<${Date.now()}.${crypto.randomBytes(8).toString('hex')}@${this.host}>`;
        const date = new Date().toUTCString();
        const boundary = `----=_Part_${crypto.randomBytes(8).toString('hex')}`;

        let rawMail = `From: "${this.fromName}" <${this.from}>\r\n`;
        rawMail += `To: <${to}>\r\n`;
        rawMail += `Subject: =?UTF-8?B?${Buffer.from(subject).toString('base64')}?=\r\n`;
        rawMail += `Date: ${date}\r\n`;
        rawMail += `Message-ID: ${messageId}\r\n`;
        rawMail += `MIME-Version: 1.0\r\n`;
        rawMail += `Content-Type: multipart/alternative; boundary="${boundary}"\r\n\r\n`;

        if (text) {
          rawMail += `--${boundary}\r\n`;
          rawMail += `Content-Type: text/plain; charset=UTF-8\r\n`;
          rawMail += `Content-Transfer-Encoding: base64\r\n\r\n`;
          rawMail += wrapBase64(text) + '\r\n\r\n';
        }

        if (html) {
          rawMail += `--${boundary}\r\n`;
          rawMail += `Content-Type: text/html; charset=UTF-8\r\n`;
          rawMail += `Content-Transfer-Encoding: base64\r\n\r\n`;
          rawMail += wrapBase64(html) + '\r\n\r\n';
        }

        rawMail += `--${boundary}--\r\n.\r\n`;
        socket.write(rawMail);
      };

      const handleResponse = (code, line) => {
        try {
          if (!this.secure && this.port !== 465) {
            // STARTTLS 升级流程 (如 587 端口)
            if (step === 0 && code === '220') {
              step = 1;
              sendLine(`EHLO ${this.host}`);
            } else if (step === 1 && code === '250') {
              step = 2;
              sendLine('STARTTLS');
            } else if (step === 2 && code === '220') {
              socket.removeAllListeners('data');
              socket.removeAllListeners('error');
              const secureSocket = tls.connect({
                socket: rawSocket,
                host: this.host,
                servername: this.host,
              }, () => {
                socket = secureSocket;
                socket.on('data', onData);
                socket.on('error', onError);
                step = 3;
                sendLine(`EHLO ${this.host}`);
              });
              secureSocket.on('error', (err) => {
                cleanup();
                reject(err);
              });
              return;
            } else if (step === 3 && code === '250') {
              step = 4;
              sendLine('AUTH LOGIN');
            } else if (step === 4 && code === '334') {
              step = 5;
              sendLine(Buffer.from(this.user).toString('base64'));
            } else if (step === 5 && code === '334') {
              step = 6;
              sendLine(Buffer.from(this.pass).toString('base64'));
            } else if (step === 6 && code === '235') {
              step = 7;
              sendLine(`MAIL FROM:<${this.from}>`);
            } else if (step === 7 && code === '250') {
              step = 8;
              sendLine(`RCPT TO:<${to}>`);
            } else if (step === 8 && code === '250') {
              step = 9;
              sendLine('DATA');
            } else if (step === 9 && code === '354') {
              step = 10;
              sendMailBody();
            } else if (step === 10 && code === '250') {
              step = 11;
              sendLine('QUIT');
              cleanup();
              resolve(true);
            } else if (code.startsWith('4') || code.startsWith('5')) {
              cleanup();
              reject(new Error(`SMTP error (${code}): ${line}`));
            }
          } else {
            // 直接 TLS/SSL 连接流程 (如 465 端口)
            if (step === 0 && code === '220') {
              step = 1;
              sendLine(`EHLO ${this.host}`);
            } else if (step === 1 && code === '250') {
              step = 2;
              sendLine('AUTH LOGIN');
            } else if (step === 2 && code === '334') {
              step = 3;
              sendLine(Buffer.from(this.user).toString('base64'));
            } else if (step === 3 && code === '334') {
              step = 4;
              sendLine(Buffer.from(this.pass).toString('base64'));
            } else if (step === 4 && code === '235') {
              step = 5;
              sendLine(`MAIL FROM:<${this.from}>`);
            } else if (step === 5 && code === '250') {
              step = 6;
              sendLine(`RCPT TO:<${to}>`);
            } else if (step === 6 && code === '250') {
              step = 7;
              sendLine('DATA');
            } else if (step === 7 && code === '354') {
              step = 8;
              sendMailBody();
            } else if (step === 8 && code === '250') {
              step = 9;
              sendLine('QUIT');
              cleanup();
              resolve(true);
            } else if (code.startsWith('4') || code.startsWith('5')) {
              cleanup();
              reject(new Error(`SMTP error (${code}): ${line}`));
            }
          }
        } catch (err) {
          cleanup();
          reject(err);
        }
      };

      const onData = (chunk) => {
        buffer += chunk.toString();
        const lines = buffer.split('\r\n');
        buffer = lines.pop() || '';
        for (const line of lines) {
          if (!line) continue;
          const match = line.match(/^(\d{3})([ -])(.*)$/);
          if (match && match[2] === ' ') {
            handleResponse(match[1], line);
          }
        }
      };

      const onError = (err) => {
        cleanup();
        reject(err);
      };

      if (this.secure || this.port === 465) {
        socket = tls.connect({ host: this.host, port: this.port, timeout: timeoutMs }, () => {
          // Direct TLS connected
        });
        rawSocket = socket;
      } else {
        socket = net.connect({ host: this.host, port: this.port, timeout: timeoutMs }, () => {
          // Plain TCP connected
        });
        rawSocket = socket;
      }

      socket.on('data', onData);
      socket.on('error', onError);
    });
  }
}

class EmailVerificationService {
  constructor(options = {}) {
    /** @type {Map<string, { codeHash: string, expiresAt: number, attempts: number, lastSentAt: number }>} */
    this.store = new Map();
    this.sweepTimer = setInterval(() => this.sweep(), SWEEP_INTERVAL_MS);
    if (this.sweepTimer.unref) {
      this.sweepTimer.unref();
    }
    this.lastDevCode = null; // Stored for testing / dev mode
  }

  sweep() {
    const now = Date.now();
    for (const [email, entry] of this.store.entries()) {
      if (entry.expiresAt <= now) {
        this.store.delete(email);
      }
    }
  }

  hashCode(code) {
    return crypto.createHash('sha256').update(String(code).trim()).digest('hex');
  }

  /**
   * HTML 邮件模版，结构与 BloomXP 纯文本逻辑对齐，使用 Solum 视觉排版
   */
  buildVerificationHtml(code, purposeBody = '注册', minutes = 10) {
    return `<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Solum ${purposeBody}验证码</title>
</head>
<body style="margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; background-color: #0b0f19; color: #f1f5f9;">
  <table role="presentation" width="100%" cellspacing="0" cellpadding="0" style="background-color: #0b0f19; padding: 36px 16px;">
    <tr>
      <td align="center">
        <table role="presentation" width="100%" style="max-width: 480px; background-color: #151d2c; border: 1px solid #283548; border-radius: 14px; padding: 32px 28px; box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);">
          <tr>
            <td align="center" style="padding-bottom: 24px;">
              <h1 style="margin: 0; font-size: 24px; font-weight: 700; color: #ffffff; letter-spacing: 0.5px;">Solum</h1>
              <p style="margin: 6px 0 0; font-size: 13px; color: #94a3b8;">本地优先端到端加密事务系统</p>
            </td>
          </tr>
          <tr>
            <td style="border-top: 1px solid #283548; padding-top: 24px;">
              <p style="margin: 0 0 16px; font-size: 15px; color: #f1f5f9; line-height: 1.5;">您好：</p>
              <p style="margin: 0 0 20px; font-size: 14px; color: #cbd5e1; line-height: 1.6;">
                你的 Solum ${purposeBody}验证码是：
              </p>
              <div style="background-color: #0b0f19; border: 1px dashed #3b82f6; border-radius: 10px; padding: 20px 0; text-align: center; margin: 24px 0;">
                <span style="font-size: 34px; font-weight: 700; letter-spacing: 8px; color: #60a5fa; font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;">${code}</span>
              </div>
              <p style="margin: 0 0 8px; font-size: 13px; color: #94a3b8; line-height: 1.6;">
                • 验证码 <strong>${minutes} 分钟</strong> 内有效，请勿泄露给他人。<br>
                • 若非您本人操作，请忽略此邮件。
              </p>
            </td>
          </tr>
          <tr>
            <td style="border-top: 1px solid #283548; margin-top: 24px; padding-top: 20px; text-align: center;">
              <p style="margin: 0; font-size: 12px; color: #64748b;">
                — Solum
              </p>
            </td>
          </tr>
        </table>
      </td>
    </tr>
  </table>
</body>
</html>`;
  }

  /**
   * 发送 6 位数字验证码
   * @param {object} params
   * @param {string} params.email
   * @param {string} [params.purpose='REGISTER']
   * @param {object} [params.smtpConfig]
   * @returns {Promise<{ ok: boolean, cooldown: number, code?: string }>}
   */
  async sendCode({ email, purpose = 'REGISTER', smtpConfig = {} }) {
    const normalizedEmail = String(email).trim().toLowerCase();
    const existing = this.store.get(normalizedEmail);
    const now = Date.now();

    if (existing && (now - existing.lastSentAt) < COOLDOWN_MS) {
      const remainingSeconds = Math.ceil((COOLDOWN_MS - (now - existing.lastSentAt)) / 1000);
      const err = new Error('cooldown_active');
      err.code = 'cooldown_active';
      err.remainingSeconds = remainingSeconds;
      throw err;
    }

    const code = crypto.randomInt(100_000, 1_000_000).toString();
    this.lastDevCode = code;

    // 对齐 BloomXP 环境变量命名与默认配置，同时向下兼容 SMTP_*
    const host = smtpConfig.host || process.env.SOLUM_MAIL_HOST || process.env.SOLUM_SMTP_HOST || process.env.SMTP_HOST;
    const user = smtpConfig.user || process.env.SOLUM_MAIL_USERNAME || process.env.SOLUM_SMTP_USER || process.env.SMTP_USER;
    const pass = smtpConfig.pass || process.env.SOLUM_MAIL_PASSWORD || process.env.SOLUM_SMTP_PASS || process.env.SMTP_PASS;
    const port = smtpConfig.port || Number(process.env.SOLUM_MAIL_PORT || process.env.SOLUM_SMTP_PORT || process.env.SMTP_PORT || (host === 'smtp.qq.com' ? 465 : 587));
    const from = resolveMailFrom(smtpConfig.from || process.env.SOLUM_MAIL_FROM || process.env.SOLUM_SMTP_FROM || process.env.SMTP_FROM, user);
    const fromName = smtpConfig.fromName || process.env.SOLUM_MAIL_FROM_NAME || 'Solum';
    const subjectPrefix = smtpConfig.subjectPrefix || process.env.SOLUM_MAIL_SUBJECT_PREFIX || '[Solum] ';
    const secure = smtpConfig.secure !== undefined ? smtpConfig.secure : (port === 465);

    const enabled = smtpConfig.enabled !== false && process.env.SOLUM_EMAIL_CODE_ENABLED !== 'false';

    if (enabled && host && user && pass) {
      const p = PURPOSES[purpose] || PURPOSES.REGISTER;
      const minutes = Math.max(1, Math.floor(CODE_TTL_MS / 60000));
      const subject = `${subjectPrefix}${p.subject}`;
      const text = `你的 Solum ${p.body}验证码是：${code}\n\n验证码 ${minutes} 分钟内有效，请勿泄露给他人。\n\n— Solum`;
      const html = this.buildVerificationHtml(code, p.body, minutes);

      try {
        const client = new SimpleSmtpClient({
          host,
          port,
          secure,
          user,
          pass,
          from,
          fromName,
        });

        await client.sendMail({
          to: normalizedEmail,
          subject,
          text,
          html,
        });
      } catch (err) {
        console.warn(`[Solum Mailer] Email code dispatch failed to ${normalizedEmail}:`, err.message);
        const sendErr = new Error('email_send_failed');
        sendErr.code = 'email_send_failed';
        throw sendErr;
      }
    } else {
      // 开发或未配置 SMTP 时回退为日志输出（保障自动化测试与本地开发）
      console.log(`[Solum Dev Mailer] Verification code for ${normalizedEmail}: ${code}`);
    }

    this.store.set(normalizedEmail, {
      codeHash: this.hashCode(code),
      expiresAt: now + CODE_TTL_MS,
      attempts: 0,
      lastSentAt: now,
    });

    return {
      ok: true,
      cooldown: 60,
      code: smtpConfig.exposeCodeForTest ? code : undefined,
    };
  }

  /**
   * 校验邮箱验证码
   * @param {string} email
   * @param {string} code
   * @returns {{ ok: boolean, reason?: string }}
   */
  verifyCode(email, code) {
    const normalizedEmail = String(email).trim().toLowerCase();
    const entry = this.store.get(normalizedEmail);
    if (!entry) {
      return { ok: false, reason: 'code_expired_or_not_found' };
    }

    if (entry.expiresAt <= Date.now()) {
      this.store.delete(normalizedEmail);
      return { ok: false, reason: 'code_expired_or_not_found' };
    }

    entry.attempts += 1;
    if (entry.attempts > MAX_VERIFY_ATTEMPTS) {
      this.store.delete(normalizedEmail);
      return { ok: false, reason: 'too_many_attempts' };
    }

    const providedHash = this.hashCode(code || '');
    let matches = false;
    try {
      matches = crypto.timingSafeEqual(
        Buffer.from(providedHash, 'hex'),
        Buffer.from(entry.codeHash, 'hex')
      );
    } catch {
      matches = false;
    }

    if (matches) {
      this.store.delete(normalizedEmail);
      return { ok: true };
    }

    if (entry.attempts >= MAX_VERIFY_ATTEMPTS) {
      this.store.delete(normalizedEmail);
      return { ok: false, reason: 'too_many_attempts' };
    }

    return { ok: false, reason: 'invalid_code' };
  }

  close() {
    if (this.sweepTimer) {
      clearInterval(this.sweepTimer);
    }
    this.store.clear();
  }
}

const defaultEmailVerificationService = new EmailVerificationService();

module.exports = {
  PURPOSES,
  resolveMailFrom,
  SimpleSmtpClient,
  EmailVerificationService,
  defaultEmailVerificationService,
};

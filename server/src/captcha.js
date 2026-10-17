'use strict';

const crypto = require('node:crypto');

const CAPTCHA_TTL_MS = 120_000; // 2 minutes
const MAX_VERIFY_ATTEMPTS = 3;
const SWEEP_INTERVAL_MS = 60_000;

class CaptchaService {
  constructor() {
    /** @type {Map<string, { answerHash: string, expiresAt: number, attempts: number }>} */
    this.store = new Map();
    this.sweepTimer = setInterval(() => this.sweep(), SWEEP_INTERVAL_MS);
    if (this.sweepTimer.unref) {
      this.sweepTimer.unref();
    }
  }

  sweep() {
    const now = Date.now();
    for (const [id, item] of this.store.entries()) {
      if (item.expiresAt <= now) {
        this.store.delete(id);
      }
    }
  }

  hashAnswer(answer) {
    // 过滤所有空白字符并转小写，防止用户输入带空格或验证码渲染带空格导致的误判
    const normalized = String(answer || '').replace(/\s+/g, '').toLowerCase();
    return crypto.createHash('sha256').update(normalized).digest('hex');
  }

  /**
   * Generates a new SVG captcha.
   * @param {object} [options]
   * @returns {{ captchaId: string, image: string, text: string, answer: string }}
   */
  create(options = {}) {
    const width = options.width || 140;
    const height = options.height || 42;
    const id = `cap_${crypto.randomBytes(16).toString('base64url')}`;

    const isMath = Math.random() > 0.35;
    let displayText = '';
    let answer = '';

    if (isMath) {
      const a = crypto.randomInt(1, 20);
      const b = crypto.randomInt(1, 16);
      const isAdd = Math.random() > 0.3;
      if (isAdd) {
        displayText = `${a} + ${b} = ?`;
        answer = String(a + b);
      } else {
        const max = Math.max(a, b);
        const min = Math.min(a, b);
        displayText = `${max} - ${min} = ?`;
        answer = String(max - min);
      }
    } else {
      const chars = '23456789ABCDEFGHJKLMNPQRSTUVWXYZ';
      let code = '';
      for (let i = 0; i < 4; i++) {
        code += chars[crypto.randomInt(0, chars.length)];
      }
      displayText = code.split('').join(' ');
      answer = code;
    }

    const bgColors = ['#111827', '#0f172a', '#181f2b', '#1e293b'];
    const bg = bgColors[crypto.randomInt(0, bgColors.length)];

    // Noise lines
    let lines = '';
    for (let i = 0; i < 4; i++) {
      const x1 = crypto.randomInt(0, width);
      const y1 = crypto.randomInt(0, height);
      const x2 = crypto.randomInt(0, width);
      const y2 = crypto.randomInt(0, height);
      const color = `rgba(${crypto.randomInt(120, 220)}, ${crypto.randomInt(150, 240)}, 255, 0.3)`;
      lines += `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${color}" stroke-width="1.2" />`;
    }

    // Bezier curves
    const cx1 = crypto.randomInt(10, width - 10);
    const cy1 = crypto.randomInt(5, height - 5);
    const cx2 = crypto.randomInt(10, width - 10);
    const cy2 = crypto.randomInt(5, height - 5);
    const startY = crypto.randomInt(5, height - 5);
    const endY = crypto.randomInt(5, height - 5);
    const curve = `<path d="M 0,${startY} C ${cx1},${cy1} ${cx2},${cy2} ${width},${endY}" stroke="rgba(129, 140, 248, 0.45)" fill="none" stroke-width="1.8" />`;

    // Noise dots
    let dots = '';
    for (let i = 0; i < 16; i++) {
      const dx = crypto.randomInt(0, width);
      const dy = crypto.randomInt(0, height);
      const r = (crypto.randomInt(8, 16) / 10).toFixed(1);
      dots += `<circle cx="${dx}" cy="${dy}" r="${r}" fill="rgba(255,255,255,0.2)" />`;
    }

    const textColor = '#60a5fa'; // Tailored blue-violet accent for Solum
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" style="background-color:${bg};border-radius:6px;user-select:none;display:block;">
  ${dots}
  ${lines}
  ${curve}
  <text x="50%" y="58%" dominant-baseline="middle" text-anchor="middle" fill="${textColor}" font-size="18" font-weight="700" font-family="-apple-system,BlinkMacSystemFont,monospace,sans-serif" letter-spacing="2px">
    ${displayText}
  </text>
</svg>`.trim();

    const dataUrl = `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;

    this.store.set(id, {
      answerHash: this.hashAnswer(answer),
      expiresAt: Date.now() + CAPTCHA_TTL_MS,
      attempts: 0,
    });

    return {
      captchaId: id,
      image: dataUrl,
      text: displayText,
      answer,
    };
  }

  /**
   * Verifies a captcha answer. Consumes the captcha on success or too many attempts.
   * @param {string} captchaId
   * @param {string} answer
   * @returns {{ ok: boolean, reason?: string }}
   */
  verify(captchaId, answer) {
    if (!captchaId || typeof captchaId !== 'string') {
      return { ok: false, reason: 'missing_captcha_id' };
    }
    const item = this.store.get(captchaId);
    if (!item) {
      return { ok: false, reason: 'captcha_expired' };
    }
    if (item.expiresAt <= Date.now()) {
      this.store.delete(captchaId);
      return { ok: false, reason: 'captcha_expired' };
    }

    item.attempts += 1;
    if (item.attempts > MAX_VERIFY_ATTEMPTS) {
      this.store.delete(captchaId);
      return { ok: false, reason: 'too_many_attempts' };
    }

    const providedHash = this.hashAnswer(answer || '');
    let matches = false;
    try {
      matches = crypto.timingSafeEqual(
        Buffer.from(providedHash, 'hex'),
        Buffer.from(item.answerHash, 'hex')
      );
    } catch {
      matches = false;
    }

    if (matches) {
      // Single use
      this.store.delete(captchaId);
      return { ok: true };
    }

    if (item.attempts >= MAX_VERIFY_ATTEMPTS) {
      this.store.delete(captchaId);
      return { ok: false, reason: 'too_many_attempts' };
    }

    return { ok: false, reason: 'invalid_captcha' };
  }

  close() {
    if (this.sweepTimer) {
      clearInterval(this.sweepTimer);
    }
    this.store.clear();
  }
}

const defaultCaptchaService = new CaptchaService();

module.exports = {
  CaptchaService,
  defaultCaptchaService,
};

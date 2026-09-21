'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { createFixedWindowLimiter, secureEqualHex } = require('../src/http-security');

test('full limiter preserves unexpired windows and refuses new keys', () => {
  const limiter = createFixedWindowLimiter({ windowMs: 100, maxAttempts: 2, maxKeys: 2 });
  assert.equal(limiter.consume('one', 10), true);
  assert.equal(limiter.consume('one', 20), true);
  assert.equal(limiter.consume('two', 30), true);
  assert.equal(limiter.consume('three', 40), false);
  assert.equal(limiter.reserve('three', 40), null);
  assert.equal(limiter.record('three', 40), null);
  assert.equal(limiter.isLimited('one', 40), true);
  assert.equal(limiter.size, 2);
  assert.equal(limiter.consume('three', 110), true);
  assert.equal(limiter.size, 2);
  assert.equal(limiter.consume('two', 110), true);
  assert.equal(limiter.consume('two', 110), false);
});

test('limiter capacity cleanup preserves outstanding reservations', () => {
  const limiter = createFixedWindowLimiter({ windowMs: 100, maxAttempts: 1, maxKeys: 1 });
  const reservation = limiter.reserve('one', 10);
  assert.ok(reservation);
  assert.equal(limiter.consume('two', 110), false);
  reservation.release();
  assert.equal(limiter.consume('two', 110), true);
});

test('hex equality requires complete nonempty byte encodings', () => {
  assert.equal(secureEqualHex('aabb', 'AABB'), true);
  assert.equal(secureEqualHex('aabb', 'aabc'), false);
  assert.equal(secureEqualHex('', ''), false);
  assert.equal(secureEqualHex('abc', 'abc'), false);
  assert.equal(secureEqualHex('zz', 'zz'), false);
  assert.equal(secureEqualHex('aa??', 'aa!!'), false);
  assert.equal(secureEqualHex(null, null), false);
});

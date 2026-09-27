'use strict';

// Assembles the entry point's assembled source: `lib/*.js` concatenated in the
// order `src/postgres-server.js` declares. The contract tests assert on the raw
// text of this module -- they slice regions out of it with `indexOf` and check
// the code between them -- so after the split that text lives in the pieces,
// and this is the one place that knows how to put it back together.
//
// The order is read out of the entry point rather than listed here, so a piece
// added, removed or reordered shows up as a test failure rather than as source
// that was silently read in the wrong order.
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '..');
const listSource = fs.readFileSync(path.join(root, 'src', 'postgres-server.js'), 'utf8');
const list = /const LIB_PARTS = \[([\s\S]*?)\];/.exec(listSource);
if (!list) throw new Error('postgres-server.js no longer declares LIB_PARTS');

const parts = list[1]
  .split(',')
  .map((entry) => entry.replace(/\/\/.*$/, '').trim().replace(/^"|"$/g, ''))
  .filter(Boolean);

module.exports = function postgresServerSource() {
  return parts
    .map((name) => {
      if (!/^[\w.-]+\.js$/.test(name)) throw new Error(`bad LIB_PARTS entry: ${name}`);
      return fs.readFileSync(path.join(root, 'lib', name), 'utf8');
    })
    .join('');
};

'use strict';

// Solum Center's PostgreSQL entry point. The implementation lives in ../lib as
// an ordered set of pieces that used to be one 3160-line file; this file
// stitches them back into a single module scope and re-exports the public
// surface. Nothing here re-implements anything.
//
// The pieces are not CommonJS modules and are deliberately not turned into
// ones. They were cut out of a single scope, so they share declarations
// freely -- handle() reaches bearer(), verifyAccessToken() and every route
// helper by bare name. Hand-writing their import lists would be new code that
// can drift from the split, so instead the order is stated once, here.
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');

// Load order. 'preamble' is first because it carries the 'use strict'
// directive, the require()s of ./http-security, ./captcha and ./mailer, and
// the dashboard.html snapshot.
const LIB_PARTS = [
  "preamble.js",
  "http.js",
  "config.js",
  "tenant.js",
  "ai-usage.js",
  "ai-proxy.js",
  "server.js",
  "routes-root.js",
  "routes-identity.js",
  "routes-auth.js",
  "routes-captcha.js",
  "routes-session.js",
  "routes-account.js",
  "routes-ai.js",
  "routes-backups.js",
  "routes-sync.js",
  "routes-alerts.js",
  "routes-keys.js",
  "startup.js",
];

// The pieces are concatenated and evaluated inside a function so they run in a
// module-shaped scope: top-level const/function stay private instead of
// landing on the global object, and __dirname still points at src/ -- which is
// what dashboard.html, ../package.json and every ./ require are relative to.
// module and exports are the real ones, so the require.main check at the
// bottom of the last piece compares against this module, as it did before.
//
// Prepending the strict directive to the first line (rather than to its own
// line) keeps every line number in a stack trace pointing at the real file.
const source = LIB_PARTS.map((name) => {
  const file = path.join(__dirname, '..', 'lib', name);
  return fs.readFileSync(file, 'utf8');
}).join('');

const factory = vm.runInThisContext(
  "(function (__dirname, __filename, module, exports, require) {" +
  "'use strict';" + source + "\n})",
  { filename: path.join('lib', LIB_PARTS.join('+')) },
);

factory(__dirname, __filename, module, module.exports, require);

// The last piece assigns module.exports; keep only the documented surface, so a
// name the split starts or stops exporting is a loud failure here rather than
// a silent difference at some call site.
const EXPORTS = [
  "buildConfig",
  "createPostgresServer",
  "optionalBoundedNumber",
  "optionalHttpUrl",
  "parseBackupPlan",
  "parseIfMatch",
  "parseModelCatalog",
  "parseTakeover",
  "parseWriterClaim",
  "proxyCompletion",
  "readBodyLimited",
  "recoverTenantStaleAiRequests",
  "requiredOpaqueString",
  "safeDatabaseInteger",
  "usageProjection",
  "validateCompletionOptions",
  "verifyAccessToken",
  "withTenant",
];

const merged = {};
for (const name of EXPORTS) {
  if (!(name in module.exports)) {
    throw new Error('postgres-server: lib/ does not export ' + name);
  }
  merged[name] = module.exports[name];
}
for (const name of Object.keys(module.exports)) {
  if (!EXPORTS.includes(name)) {
    throw new Error('postgres-server: lib/ exports ' + name + ', which this entry point does not list');
  }
}
module.exports = merged;

# Changelog

All notable changes to Solum are documented in this file.

## [0.2.22] - 2026-10-17

### Fixed
- **ci**: Fixed GitHub Actions workflow failures:
  - Added `npm ci --ignore-scripts` to `web-and-cloud` job before running tests so `pg` dependency is properly installed.
  - Set `working-directory: .` for cargo metadata and settings generator steps in `android-manifest` job to resolve root scripts correctly.
  - Granted executable mode (`100755`) and added `chmod +x` to `crates/solum-app/gen/android/gradlew`.
  - Updated `scripts/gen-android-ci-settings.mjs` to also generate `app/tauri.build.gradle.kts` required by gradle build.
  - Added `01-schema.sql` provisioning in `web-and-cloud` job to materialize tables and role grants in test database.
  - Enabled `--test-reporter=tap` via `NODE_OPTIONS` and synchronized `postgres-integration.test.js` role table grants with `01-schema.sql`.
- **formatting**: Applied `cargo fmt` to `crates/solum-core/src/classify.rs` and `model.rs` to satisfy rust quality gate.
- **app**: Updated Android private storage path detection and clean app restart handling.
- **server**: Normalized captcha whitespace handling and added flexible SMTP configuration fallbacks.

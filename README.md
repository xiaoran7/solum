# Solum（息壤）

Local-first Personal Agent：把输入、日程、提醒、记忆、行为日志和建议保存在本机 SQLite，云端只承担账号、端到端加密同步、福利告警和经鉴权的可选 AI。当前工作区版本 `0.2.16`，Android `versionCode 1026`；以根 `Cargo.toml` 和 `crates/solum-app/tauri.conf.json` 为准。

本目录是 Android/桌面、共享 Rust core 与服务端的主仓。HarmonyOS 位于独立 Git 仓 `../harmony`，父目录 `Project/solum` 不是仓库；两仓必须分别检查、测试、提交和推送。

## Workspace

- `solum-core`：SQLite schema、ingest、提醒、记忆、Guard、同步/备份协议和领域规则。
- `solum-cli`：开发/自动化命令入口。
- `solum-app`：Tauri 2 应用与无框架静态前端。
- `solum-sync-server`：生产服务端代码；部署与 API 见 [`server/README.md`](server/README.md)。
- `solum-notif-access`、`solum-health-connect`、`solum-alarm`：Android 平台桥接。

## 核心边界

- 可查询的个人明文默认只在本机；同步只上传端到端加密 blob，服务端不能解密，恢复依赖用户妥善保存恢复码。
- 第三方通知标记为 `local_only`，不进入 recall 或云端 LLM 明文上下文。
- 已登录且启用云端 Reasoner 时，事件摄取优先做 LLM 语义裁决；网络、超时或服务异常时自动降级到本地确定性规则，不阻断离线 ingest 与提醒。
- 云端智能能力统一经 Solum Center 鉴权代理；TannerLab 通行证支持 `solum://oauth/callback` Deep Link 自动回跳换票。
- 危险操作必须经过 Guard：预览、人工确认、一次性令牌和 append-only 审计。
- Core 的时间由调用方注入；云端失败不能破坏本机数据路径。
- 前端保持 Vanilla JS + 静态 `crates/solum-app/dist/`（`index.html`、`app.js`、`i18n.js`），不引入 npm/前端框架，脚本遵循无 `unsafe-inline` 的 CSP。

## 验证

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
node scripts/check-frontend.mjs
node scripts/verify-android-release.mjs path/to/release.apk
```

APK 校验命令读取一个已构建的明确路径，不会构建、签名或安装产物。Android 构建和真机 QA 见 [`docs/ANDROID-RELEASE-QA.md`](docs/ANDROID-RELEASE-QA.md)；构建、主机 API、模拟器、真机和生产是不同证据层级，不能互相替代。前端交互变更还需完成 mock-IPC 与可见浏览器验证。

## 文档

从 [`docs/README.md`](docs/README.md) 进入。架构见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)，版本摘要见 [`docs/CHANGELOG.md`](docs/CHANGELOG.md)，当前待办见 [`docs/TODO.md`](docs/TODO.md)，Android 发布门禁见 [`docs/ANDROID-RELEASE-QA.md`](docs/ANDROID-RELEASE-QA.md)，长期坑点见 [`docs/PITFALLS.md`](docs/PITFALLS.md)。

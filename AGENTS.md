# Solum Agent 规则

## 仓库边界

- 本仓：`Project/solum/android`；HarmonyOS：`../harmony`，独立 Git；父目录不是仓库。
- 跨平台协议和产品语义以本仓 `docs/ARCHITECTURE.md` 与固定夹具为准；平台实现分别验证。
- 开工先读文档索引、与任务相关的架构/坑点，并检查 `git status --short`。

## 不变量

- 个人明文只落本机；云端只存账号元数据、福利告警与不可解密的同步/备份密文。
- `local_only` 第三方通知永不进入 recall 或云端 LLM 明文上下文。
- 云端失败降级到离线；`ingest` 和提醒触发不依赖网络。
- Core 不读系统时钟，`now` 由调用方传入。
- 危险操作只能由 Guard 签发一次性令牌并写审计，页面/模型不能绕过。
- 前端不引入框架或 npm 依赖；修改 `dist/index.html` 前读 `PRODUCT.md` 与 `DESIGN.md`。
- 凭据、密钥材料、生产 token、本机路径、设备个人数据和临时 SQLite 不入库。

## 完成条件

- 行为变化写 `docs/CHANGELOG.md`，可复用故障写 `docs/PITFALLS.md`，架构变化同步 `docs/ARCHITECTURE.md`；不要为每次会话制造流水。
- Rust 变更通过 workspace test、clippy `-D warnings` 和 fmt check；前端交互需 mock-IPC + 可见浏览器验证，断言最终渲染而不是属性意图。
- 只 stage 本次文件。提交/推送遵循用户当次要求和仓库规则；绝不跨仓混提，绝不自动部署。
- **打包发版铁律**：凡执行 Android APK、桌面打包或发版编译时，必须同步递增 `versionCode` 与 `versionName`（以及 `Cargo.toml` 版本），产物带版本后缀（如 `Solum-vX.Y.Z.apk`），严禁覆盖旧版本。

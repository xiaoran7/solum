# 项目约束

通用约束继承 [AGENT.md](../../../AGENT.md)。

- 本仓是 Android/shared core/server 主仓；`../harmony` 独立 Git，父目录仅作平台容器。
- 共享协议以 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) 与固定夹具为准；改前端读 `PRODUCT.md`、`DESIGN.md`。
- 个人明文只落本机；云端只存账号元数据、福利告警及不可解密的同步/备份密文。`local_only` 通知不进入 recall 或云端 LLM 明文上下文。
- 云失败降级离线，ingest/提醒不依赖网络；Core 不读系统时钟，调用方注入 `now`。
- 危险操作经 Guard 一次性令牌与审计，页面和模型不能绕过。
- 前端不引入框架或 npm 依赖；签名材料、设备数据与临时 SQLite 不入库。
- Rust 变更通过 `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`；前端交互做 mock-IPC 与可见浏览器验证。
- 记录用 `docs/CHANGELOG.md`、`docs/PITFALLS.md`；打包同步 Android 与 Cargo 版本，不自动部署。

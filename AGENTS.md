# 项目规则

## 工作约定

- 只在本项目内维护规则、记忆和产物；跨仓操作须有任务依据。先检查 Git 状态，保留其他会话改动；提交、推送、部署分别按用户授权执行。
- 按当前代码、清单和配置核对文档；测试、设备验收和生产状态分别报告，历史记录不能证明当前可用。
- 不提交凭据、私钥、Cookie、真实个人资料或运行数据库。删改前核对路径、归属和用途，不动他人改动、源素材、签名材料及私人日志。
- 发版或部署必须递增实际产品版本并同步相关清单；Android/Harmony 同步 versionCode/versionName。纯文档整理不虚构发版。
- README 放启动与导航；本文件放约束；docs 放契约和经验；CHANGELOG 放历史；MEMORY 放偏好。同一事实只维护一处，删改同步修复引用；Markdown 用 UTF-8 无 BOM。
- 临时产物放本项目 `_scratch/`，交付物用项目既定路径或 `Deliverables/`；验证新版后清理本项目过期产物，保留必要回滚及指定基线。收尾只停止本次启动的进程。

## 项目边界

- 本仓是 Android/shared core/server 主仓；`../harmony` 独立 Git，父目录仅作平台容器。
- 共享协议以 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) 与固定夹具为准；改前端读 `PRODUCT.md`、`DESIGN.md`。
- 个人明文只落本机；云端只存账号元数据、福利告警及不可解密的同步/备份密文。`local_only` 通知不进入 recall 或云端 LLM 明文上下文。
- 云失败降级离线，ingest/提醒不依赖网络；Core 不读系统时钟，调用方注入 `now`。
- 危险操作经 Guard 一次性令牌与审计，页面和模型不能绕过。
- 前端不引入框架或 npm 依赖；签名材料、设备数据与临时 SQLite 不入库。
- Rust 变更通过 `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`；前端交互做 mock-IPC 与可见浏览器验证。
- 记录用 `docs/CHANGELOG.md`、`docs/PITFALLS.md`；打包同步 Android 与 Cargo 版本，不自动部署。

长期偏好见 [MEMORY.md](MEMORY.md)。

# 息壤 Solum — Personal Agent

一个"真正了解你"的个性化 agent（不是无状态问答机器人）。设计目标、隐私边界与路线图见
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。**改动前请先读 [AGENTS.md](AGENTS.md)（智能体工作规范，
含强制文档留痕规则）；动前端前另需读 [docs/PRODUCT.md](docs/PRODUCT.md) 与 [docs/DESIGN.md](docs/DESIGN.md)。**

## 公开发布范围

本仓库只包含 **Solum 的桌面端与 Android 移动端**实现（Rust + Tauri 2 + Android 原生插件）。
鸿蒙端属于独立项目，明确不在本仓库、提交历史或发布物范围内。

本项目以本地优先为原则：个人数据留在设备本地；云端能力均为可选项，并受配置与用户操作约束。请勿提交
任何真实的 API key、OAuth token、同步密钥、邮箱授权码、签名 keystore、SQLite 数据库或导出的个人数据。

当前进度：**Phase 1–11 全部落地，Phase 12 已交付邮箱连接器（F21）**——核心闭环（离线、确定性、headless 可测）+ 图形前端（Tauri 2 桌面/Android 壳，常驻宿主：后台 ticker 自动触发提醒/问询/建议/同步/穿戴轮询，已真机/模拟器验证）+ 行为日志与定时问询（F3/F4）+ Suggestion Engine（F10 + F11 wellness 三信号）+ 云端 Reasoner（OpenAI 兼容网关，离线自动降级，闲聊纯文本 SSE 流式）+ 移动端通知监听与权限引导（F1/F2）+ Android 后台精确闹钟（`solum-alarm`，杀进程照样准点响）+ 聊天记录导入人格（F9 纯本地）+ 多设备端到端加密同步（F17）+ 穿戴数据接入（F5，Health Connect）+ 交互式生成 UI（F18 GenUI 协议）+ 语义记忆与检索（memory_facts + recall v1）+ 习惯闭环（routines，自动物化每日提醒 + 反向刹车 + 台账可编辑）+ 场景感知静默（F13）+ 真实 dangerous 工具过 HITL 护栏（`ledger_purge`）+ Daily Focus Brief（今日聚焦简报）+ **BloomXP 双向互通（Phase 8：只读拉取 + 受控日程推送）** + **通知上云开关（Phase 9，默认关闭、显式开启）** + **通知智能管线（Phase 10：App 白名单、双车道分诊、F12 回看与 Android 前台服务）** + **持久化自定义组件（Phase 11/F19：预览确认后保存、固定「组件」页离线记录 CRUD、可追加可空字段、form/list/table/stat 四视图、与日程双向快照互通、定义与记录随 F17 同步）** + **可还原的备份（导出 v2 + 过 Guard 的导入）** + **主流邮箱连接（F21：QQ / Gmail / Microsoft 365 / Outlook / 自定义 IMAP-SMTP，读取按需、发送逐次确认）**。

壳层采用四个稳定根入口「今天 / 记忆 / 计划 / 搜索」。切页只刷新当前视图依赖，重复幂等 IPC 仅在当前进程内短时合并；业务数据始终以 core / SQLite 为权威。最后视图、滚动位置和按会话聊天草稿会留在当前设备，搜索词、邮件内容、通知正文与凭据不会进入这份 UI 状态。

组件能力的范围与取舍见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) §3.12 与 §6：已完成 form/list/table/stat、组件同步与 schema 演进；顶层动态导航仍明确不做，组件保持在固定的「工作台 → 组件」入口。

> **归档说明**：本 README 曾包含一段面向 OpenAI Build Week 评委的英文章节（项目英文简介、Codex 实作范围声明、
> GPT-5.6 集成说明）。评委期已结束，该段连同当时"如实限定 Codex 使用范围"的决定一并存档到
> [docs/MISC.md](docs/MISC.md) 2026-07-19「README Build Week 章节归档」条目。

---

## 仓库结构

```
crates/
  solum-core/        # 核心库（UI 无关）：解析 / 抽取 / 分级 / 排程 / 护栏 / 存储 / 编排
    src/
      time_parse.rs    中英文自然语言时间解析
      extract.rs       意图路由 + 事件抽取（离线规则版）
      classify.rs      重要度分级 + 可编辑规则表
      schedule.rs      通知排程与到点查询
      proactivity.rs   分维度主动度（并编码 F7 高危硬约束）
      guard.rs         HITL 高危护栏（能力型一次性令牌 + append-only 审计）
      journal.rs       行为日志 + 定时问询策略（F3/F4）
      suggest.rs       Suggestion Engine：日程规则 + 习惯检测 + wellness 三信号（F10/F11）
      soulous.rs       BloomXP 互通边界：JWT 双 token 拉取、离线快照与经确认的最小日程推送
      email.rs         F21 邮箱连接器：IMAP 读取/搜索 + SMTP 发送 + OAuth2 PKCE；邮件与凭据均不入库
      llm.rs           云端 LLM 网关（OpenAI 兼容；闲聊/GenUI 回复 + 抽取兜底；闲聊纯文本 SSE 流式；最小化上下文）
      genui.rs         F18 交互式生成 UI：组件目录 + 校验器 + 动作白名单 + 离线模板
      widget.rs        F19 声明式组件 schema/记录校验：七类字段、严格拒绝与纯时刻格式
      memory.rs        语义记忆（memory_facts，F12 台账 fact 层）
      recall.rs        recall v1：词面检索（bigram × 时间衰减 × 层权重）注入 chat 上下文
      brief.rs         Daily Focus Brief 聚合核心
      stats.rs         D2 数据回看：活动聚类 / 应答率 / 穿戴基线（近 28 天中位数）
      routine.rs       习惯闭环：周期提醒物化为普通事件 + 反向暂停刹车
      scene.rs         F13 场景感知（睡眠中/日程中 → 问询建议静默）
      store.rs         本地 SQLite（迁移 / CRUD / 记忆台账 / 审计 / 同步 oplog 触发器）
      review.rs        自我复盘简报（F14，离线聚合 + 观察/记忆叙事段）
      persona.rs       数字人格设定与版本化（F9）
      persona_import.rs 聊天记录导入 → 人格初稿（F9，纯本地提取）
      export.rs        全量数据导出（含审计日志与人格版本，纯本地）
      sync.rs          多设备同步客户端（F17：端到端加密 + 行级 LWW 合并）
      wearable.rs      穿戴健康样本领域模型（F5：心率/步数/睡眠 + 去重键）
      notification_intelligence.rs  F20 通知白名单、双车道、确定性去重与安全 LLM 分诊协议
      orchestrator.rs  把上面装配成 ingest() 闭环 + 工具注册表
    tests/closed_loop.rs   文件落库的集成测试（关库再开验证持久化）
  solum-cli/         # headless 演示 CLI（二进制名 `solum`）
  solum-app/         # Tauri 2 桌面/移动壳（纯静态前端 dist/index.html，无 npm 依赖）
    src/lib.rs         106 个 tauri command + 常驻 ticker（系统时钟，OS 通知，自动同步，穿戴轮询、通知批处理）
    dist/index.html    20 视图（今天 / 记忆 / 计划 / 搜索四个一级根入口）：今天承载对话与当日状态；
                       记忆收纳台账·行为日志·穿戴·复盘·通知回看；计划收纳日程·提醒·建议；搜索在已加载
                       的本地数据中统一找回上下文。邮箱、设置（人格·主动度·隐私·数据入口·同步·云端·
                       重要度规则·护栏与审计）与工作台（资料·组件）归入低频工具入口，不占主导航位置
                       首启隐私门覆盖全窗口，未同意前不放行（solum-cli 不受此门影响）
  solum-sync-server/ # 自托管同步中转（单二进制 + SQLite；只存转加密 blob，不解密）
server/            # solum-cloud：自建账号 + 云端 AI 代理（Node 24 零依赖；与 Solum Harmony 共用同一契约，
                   # 登录后云端请求经它转发，第三方 API Key 只存服务端；与同步无关，不接收业务数据）
  solum-alarm/       # 本地 Tauri Android 插件：AlarmManager 精确闹钟镜像（进程被杀也准点提醒）
  solum-notif-access/ # 本地 Tauri Android 插件：通知监听权限、前台处理服务与电池/后台设置引导（桌面 no-op）
  solum-health-connect/ # 本地 Tauri Android 插件：Health Connect 只读适配（心率/步数/睡眠，桌面 no-op）
docs/             # ARCHITECTURE / CHANGELOG / PITFALLS / MISC / PRIVACY / PRODUCT / DESIGN / LLM-PROVIDERS
```

## 构建与测试

需要 Rust 稳定版（在 `x86_64-pc-windows-msvc` 上开发）。`rusqlite` 用 `bundled`，会从源码编译 SQLite。

```bash
cargo test --workspace      # 全量离线测试（LLM 用 FakeReasoner）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo build                 # 产出 target/debug/solum(.exe) 与 solum-app(.exe)
node scripts/check-frontend.mjs  # inline JS、HTML id 与高危 IPC 边界
(cd server && npm test)     # solum-cloud 鉴权、刷新与流式代理
```

## 许可证

本项目采用宽松的 [MIT License](LICENSE)：允许使用、修改、再发布及商用，但须保留版权与许可声明。

## 云端推理（可选）

不配置就是纯离线模式，一切核心功能照常。要启用云端（闲聊/GenUI 回复 + 抽取兜底 + 复盘改写），建
`solum-llm.json`（**已 gitignore，key 不入库**；位置见下方「数据放在哪」）或设 `SOLUM_LLM_BASE_URL`/`SOLUM_LLM_API_KEY`/`SOLUM_LLM_MODEL`，
也可以在壳层「设置 → 云端」可视化配置（8 家厂商预设 + 自定义端点 + 测试连接 + 保存热切换）：

```json
{ "base_url": "https://api.openai.com/v1", "api_key": "sk-…", "model": "gpt-5.6" }
```

`solum llm-status` 查看当前状态（只显示 key 尾 4 位）。任何 OpenAI 兼容端点均可（各家坑位见
[docs/LLM-PROVIDERS.md](docs/LLM-PROVIDERS.md)）。缺省模型为小米 MiMo `mimo-v2.5`。

## BloomXP 学习数据与受控出站（可选）

Solum 可读取你自有 BloomXP 服务的课表、考试、任务、当天打卡快照和专注会话。Solum → BloomXP 出站则**默认完全关闭**：当前只有「日程事件」一类可单独授权，且每次仍须在敏感 Tool 确认面中核对最小数据预览。
配置缺失时该数据源完全静默关闭，其他功能不受影响。将 BloomXP 既有登录所得的双 token 放到仓库根目录
应用数据目录中的 `solum-soulous.json`（**已 gitignore，凭据不入库、不进同步、不发给 LLM**；位置见下方「数据放在哪」）：

```json
{
  "server_url": "https://your-bloom-xp.example",
  "access_token": "…",
  "refresh_token": "…",
  "timeout_secs": 15,
  "push_schedule_events": false
}
```

```bash
solum soulous status                         # 脱敏配置、缓存计数与考试提前量
solum --now 2026-07-18T10:00:00 soulous pull # 手动拉取；401 时自动刷新并保存双 token
```

也可以在桌面壳「设置 → 云端」填写配置并点「立即拉取」。拉取只在该显式入口发生；服务不可达或响应异常时保留上次完整缓存，不影响录入、提醒或 ticker。缓存位于独立 `soulous_facts` 表（`source=soulous`，可跨设备同步），**不进入 `memory_facts` 或 recall**；当前 BloomXP API 只提供当天打卡快照而非历史列表。

若你明确开启「允许推送 Solum 日程事件」，日程页和 F12 台账会明确标识该类别正在流向 BloomXP；每条发送仍要点「推送」并通过一次性敏感确认。实际请求只含标题、类型、开始/结束时间和地点，来源为第三方通知的日程会被拒绝；参与人、原始输入、通知文本、人格和 Solum 记忆绝不出本机。BloomXP 接收端只写入当前 JWT 用户的 `external_context`，并且仅在该 BloomXP 用户开启 AI 记忆时才进入其 RAG。

## 邮箱连接器（F21，可选）

桌面壳侧栏的「邮箱」支持 QQ 邮箱、Gmail、Microsoft 365 / Outlook 及其他标准 IMAP/SMTP 邮箱：文件夹、最近邮件、发件人/主题搜索和纯文本正文均只在用户手动操作时读取。邮件正文与搜索结果只留在当前界面内存，**不写 SQLite、不同步、不导出、不进 LLM 或 recall**；附件上传、后台拉取、自动回复和自动发送尚未实现。

账户凭据只保存到本机 gitignore 的 `solum-email.json`（可用 `SOLUM_EMAIL_CONFIG` 改路径）：QQ 使用网页端开启 IMAP/SMTP 后生成的授权码；Gmail 和 Microsoft 推荐在各自平台登记桌面 OAuth 应用后，用本机 loopback + PKCE 授权。每封外发邮件都必须在完整预览后通过一次 `sensitive` Guard 人工确认；审计不记录收件地址、主题或正文。完整边界见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) §3.14 和 [docs/PRIVACY.md](docs/PRIVACY.md)。

## 隐私与通知上云

完整政策见 [docs/PRIVACY.md](docs/PRIVACY.md)，架构级不变量见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) §4。要点：

- **L1 硬约束（无开关）**：导入的聊天记录原文、人格全部版本、append-only 审计日志、穿戴逐条原始采样，任何情况下不发往云端。
- **「通知上云」开关（默认关闭，需主动开启）**：只控制通知文本能否作为**云端 AI 服务商**的上下文（recall 语料 + F20 分诊载荷）。关闭时新捕获的通知不外发，历史不回填。开关本身是设备本地偏好，不跨设备同步。
- **设备间同步始终开启，不受上述开关影响**：通知及派生数据无条件参与多设备同步——那条链路是端到端加密后发往**你自建**的中转服务器（服务器解不开、不留明文，§3.8），与"交给第三方厂商"不是同一风险面。捕获时刻的"是否允许上云"判定会随同步传播，因此同一条通知在你所有设备上的处理一致。

```bash
solum notif-cloud            # 查看通知上云开关
solum notif-cloud-set off    # 之后新捕获的通知仅留本机；on 可重新开启
```

桌面壳「设置 → 隐私」有同一开关的可视化入口。

## 通知智能管线（F20）

通知监听权限本身不等于读取授权：还要在「设置 → 隐私 → 通知智能管线」中主动把 App 加到**默认空的白名单**。不在名单内的通知在 Android 捕获端即被忽略，不进规则、不落库、不发 LLM。白名单和「通知上云」均是本机偏好，不会被其他设备同步改写。

- 重要通知由可编辑的包名范围子串/正则规则送入即时车道；普通通知以 15/20/30 分钟（默认 15）内部批处理。相同 App + 规范化内容在 10 分钟内只保留一条，所有判重、过滤和待处理结果都能在 F12「通知回看」恢复或提升为日程。
- 本地规则会先抽取；仅在「通知上云」开启且本地无法确定时才调用一次 LLM。关闭开关即纯本地、零 LLM/零外发。LLM 可提出新事件、待确认过滤规则，或「取消/改期某标题」的**无 id 意图**；后者必须由 Rust 在本机唯一匹配真实日程，再在 F12 点按确认，绝不自行执行，也不会扩张 `LLM_ACTIONS`。
- Android 在白名单非空时启动低优先级 `dataSync` 前台处理服务；设置页可请求忽略电池优化并跳往系统后台设置。国产 ROM 仍可能需要用户手动允许自启动/后台运行；服务停止或被系统杀死时 inbox 会保留，恢复后再处理。

```bash
solum notif-intelligence status                 # 白名单、规则、回看与待确认过滤提议
solum notif-intelligence allow com.tencent.mm   # 显式允许一个 App（会加入可编辑的预设即时规则）
solum notif-intelligence deny com.tencent.mm    # 停止后续捕获，不删除历史回看
solum notif-intelligence process                # 立即处理普通队列
```

## 多设备同步（可选，自托管）

新部署使用统一的 `solum-cloud + PostgreSQL`：账号、AI 代理和同步共用一个 HTTPS 地址，但客户端业务数据仍只以端到端加密 blob 进入中心库，服务端不能解密（ARCHITECTURE §3.8）。桌面/Android 继续使用本地 SQLite，鸿蒙继续使用 relationalStore；没有网络时录入和提醒不受影响。不配置同步则保持纯单机。

同一个服务还可承接自用状态提醒：`/v1/alerts` 接收和读取固定形状的渠道恢复事件，监控配置与界面不在 Solum 服务端，而在电脑上的独立 `benefit-monitor` 应用。中转不接收账户、余额、上游 API 密钥或任意通知正文；只保存渠道 ID/名称、固定状态码、数值指标和点击地址，保留 7 天。Android 配置同步后会复用现有 `dataSync` 前台服务每 20 秒拉取，并为首次连接后的每个新恢复事件发高优先级系统通知。电脑端同样默认每 20 秒轮询，通常在源站完成检测后的 0–40 秒到手机；需在手机系统中允许 Solum 后台运行、自启动并关闭电池优化。

```bash
# 1. 生产部署：统一账号、AI 与加密同步中心。
cd server
copy .env.example .env   # Windows；Linux/macOS 使用 cp
docker compose up -d --build

# 2. 推荐：在「设置 → 云端接入」登录 Solum 账号。客户端自动使用同一个
#    solum-cloud 地址并恢复账号同步密钥，无需另填同步地址或同步密码。磁盘只写：
#    { "url": "https://solum-cloud.example", "key": "64位hex设备端加密密钥" }
#    账号 access/refresh token 独立保存在 solum-account.json，401 自动刷新。

# 3.（旧 Rust/SQLite relay 兼容）solum-sync.json 仍可写原始 token/key，或 SOLUM_SYNC_URL/TOKEN/KEY：
#    { "url": "http://服务器:8787", "token": "你的token", "key": "64位hex" }
#    文件路径可用 SOLUM_SYNC_CONFIG 覆盖（同 SOLUM_LLM_CONFIG）；桌面/Android 均自动指向 app-data 目录，
#    Android 绑定：adb push solum-sync.json /data/local/tmp/ && adb shell run-as dev.solum.app cp /data/local/tmp/solum-sync.json ./solum-sync.json

solum sync           # 一轮推送 + 拉取合并（行级 LWW，幂等可重放）
solum sync-status    # 查看配置与本机设备标识
```

客户端现已启用“登录即恢复”：首台设备随机生成同步主密钥，并用账号密码与不可变 user UUID 在本地导出的包装密钥生成恢复信封；新设备登录同一账号后自动解开信封，无需再填写独立同步密码。服务器始终只保存包装后的密钥与业务密文，拿到数据库或 access token 也不能读取正文。旧 guest 数据不会自动归入登录账号。

Windows 独立监控应用位于 `D:\ClaudeSpace\benefit-monitor`，双击 `Start-BenefitMonitor.cmd` 后在浏览器打开 `http://127.0.0.1:17321/`。页面分别配置织境账号、Solum 账号服务器和同步中继；两套账号都会自动获取、保存、刷新 access/refresh token。密码与会话令牌由 Windows 当前用户 DPAPI 加密保存在 `%LOCALAPPDATA%\BenefitMonitor\secrets.dat`，页面/API 不回显。应用预置三个织境渠道，可分别启用检测和恢复通知，也可新增织境渠道 ID 或通用 HTTP JSON 检测。

应用持久化每路上次状态，首次启动只建立基线，只有从非正常恢复到正常才通知；Solum 暂时不可达时保留待发事件并在下一轮重试。配置和状态保存在 `%LOCALAPPDATA%\BenefitMonitor`。旧的 `D:\ClaudeSpace\check-gptplus.ps1` 仍保留为单渠道命令行兼容入口。

桌面壳配置好后顶栏出现同步按钮，ticker 每 5 分钟自动同步。审计日志按设计不同步（§4）。换掉主密钥即吊销所有旧设备。

## 跑桌面壳（GUI）

```bash
cargo run -p solum-app         # 打开 Solum 桌面窗口（需要 WebView2，Win11 自带）
# 默认使用 app-data 目录下的 solum.sqlite（与 CLI 共享同一份）；SOLUM_DB=... 可换库
```

### 数据放在哪

桌面端与 CLI **共用当前身份的同一份本地库和配置**，根目录是按系统用户划分的应用数据目录：

| 平台 | 目录 |
| --- | --- |
| Windows | `%APPDATA%\dev.solum.app\` |
| macOS | `~/Library/Application Support/dev.solum.app/` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/dev.solum.app/` |

未登录（guest）继续使用根目录里的 `solum.sqlite` 与 `solum-{llm,sync,soulous,email}.json`；登录后使用 `profiles/<账号 UUID>/` 下的同名文件。通知白名单也随 profile 保存。`solum-account.json`、隐私同意记录和 Android 当前系统闹钟镜像属于设备级状态，仍留在根目录。显式设置的 `SOLUM_DB`、`SOLUM_*_CONFIG` 或直连 LLM 环境变量继续优先于自动分区路径。

**从旧版本升上来不需要做任何事**：首次启动会把当前目录下的同名旧文件自动迁移过去，并在 stderr 打印一行 `[paths] 已接管 …`。迁移是移动而不是复制——留下两份会分叉，那正是这次要消除的问题。

之所以不再用「当前目录」：那会让「我在用哪个库」取决于程序是怎么被启动的（快捷方式的工作目录、从资源管理器双击、开机自启各不相同），静默打开甚至新建一个空库，而用户无法把它和数据丢失区分开。共享这条性质保留了，只是不再依赖你启动时站在哪。

单独指定：`SOLUM_DB=/path/to.sqlite`（桌面与 CLI 都认），或 CLI 的 `--db`。

侧栏页脚可切换「系统时钟 / 模拟时钟」，注入时钟贯穿全部视图——把时钟拨到提醒触发点即可确定性演示到点提醒。护栏页可完整演示：无确认直接执行（必被拦截）→ 请求执行 → 确认弹窗（后果预览）→ 一次性令牌执行 → append-only 审计。

桌面壳是**常驻宿主**：后台 ticker 每分钟按**系统时钟**（模拟时钟只影响手动操作，不驱动 ticker）自动触发到点提醒、按档位发起状态问询、自动生成建议、物化当日 routine、每日推送一次今日聚焦简报；问询在对话页出横幅，直接回一句（如「我在写代码」）即完成记录。闲聊回复逐 token 流式显示；含可执行动作的 GenUI 信封整包收完再渲染。

## 试跑闭环（CLI）

用 `--now` 注入时钟（保证可复现），`--db` 选本地库（默认跟随当前登录账号的 profile，未登录走 guest）：

```bash
solum --now 2026-07-06T10:00:00 add "明天下午3点在会议室和张伟开会"
solum --now 2026-07-06T10:00:00 add "下周五上午九点期末考试"
solum --now 2026-07-06T10:00:00 agenda      # 日程视图
solum --now 2026-07-06T10:00:00 daily-brief # 今日聚焦简报（日程 + 到点提醒 + 待处理建议）
solum rules                                 # 分级提前量规则表
solum --now 2026-07-07T14:30:00 fire        # 到点触发提醒（会议提前 30 分钟）
solum --now 2026-07-07T14:31:00 snooze 1 --minutes 10   # 稍后再响：已触发/待触发的提醒顺延（已取消的不复活）
solum add "把明天的会改到下午4点"             # 自然语言改期：只给时刻则保留日期，反之亦然；提醒自动重排
solum add "取消明天的会"                      # 自然语言取消：只出确认按钮，绝不一句话直删（GUI 点按确认）
solum add "每天早上八点提醒我吃药"             # 每日固定提醒 → 建 routine 并立即物化今明两天
solum reschedule 1 明天上午十点               # 按 id 直控改期（CLI 直控入口）
solum ledger                                # F12 记忆台账：可查看
solum forget event 2                        # 遗忘一条记忆（级联删除派生的提醒）
solum fact-edit 1 我对花生和腰果都过敏         # F12 可编辑：改写一条语义记忆（recall 立即生效）
solum export --out my-data.json             # 全量数据导出（含审计日志与人格版本，纯本地不上云）
solum --now 2026-07-06T10:00:00 review      # F14 自我复盘简报（默认近 7 天）
solum add "我在护肤"                         # 状态回答 → 自动落行为日志（F4）
solum add "记住我对花生过敏"                  # 语义记忆写入（M2，台账 fact 层可查可删）
solum recall 过敏                            # recall v1：看某个查询会带哪些背景上云（可审计）
solum notif-cloud-set off                    # 通知不再发往云端 AI（设备同步照常）；不改写历史行
solum log                                   # 行为日志：状态 / 问询 / 提醒触发
solum proactivity-set status_checkins butler && solum checkin   # 到点则主动问询（F3）
solum suggest --days 3                      # 生成建议（F10：备考/赶工/早休/冲突/习惯/wellness）
solum suggestions && solum suggest-set 1 accepted   # 采纳 habit 建议会自动建 routine
solum routines                              # 习惯闭环：周期提醒列表
solum stats                                 # D2 数据回看：活动聚类/应答率/穿戴基线
solum llm-status                            # 云端推理状态（未配置 = 离线模式）
solum persona-set --tone "干练、简洁" --nickname 老板   # 手动人格设定（F9，版本化）
solum persona                               # 当前人格 + 版本历史
solum persona-import chat.txt --me 我        # 聊天记录本地提取人格初稿（预览，加 --save 才落库）
```

高危护栏演示（任何主动模式下都拦得住，需人工确认）：

```bash
solum guard-run demo_delete /some/path      # 无确认 → 必被拦截
solum guard-demo demo_delete /some/path     # 完整 请求→确认→一次性令牌→执行 流程（模拟工具）
solum guard-demo ledger_purge '{"layer":"behavior","before":"2026-07-17T00:00:00"}'
                                         # 真实 dangerous 工具：预览真实条数 → 令牌 → 真删 → 审计
solum audit                                 # 只读、append-only 审计日志
```

## 设计要点（为什么这么写）

- **离线优先**：解析/分级/排程/护栏全部离线且确定性，云端不可达时提醒照常触发（F16、通知可靠性）。
- **注入时钟**：核心逻辑不读系统时间，`now` 由调用方传入 → 纯函数、可测。
- **护栏是编译期约束**：`Tool::execute` 需要 `Grant`，而 `Grant` 只能由 `guard.rs` 内部签发，模块外无法绕过 Guard 执行高危工具（架构 §3.3 落地）。
- **隐私**：核心个人数据落本地 SQLite（已 gitignore `*.sqlite*`）；本地会话历史、资料工作台文件和邮箱凭据/邮件内容均不参与同步或导出。云端网关每次调用只发「当前一句话 + 当前时刻 + 所选会话最近 ≤ 4 轮 + 可审计的 recall 片段」；行为日志/台账/人格/邮件永不出本机；第三方通知文本受「通知上云」开关门控（默认关闭，需主动开启，见 [docs/PRIVACY.md](docs/PRIVACY.md)）；云端失败一律降级离线，ingest 不失败。

更多取舍与踩坑见 [docs/MISC.md](docs/MISC.md) 与 [docs/PITFALLS.md](docs/PITFALLS.md)。

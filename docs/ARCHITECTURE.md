# 息壤 Solum（Personal Agent）架构文档

> 状态：本文档是设计的唯一权威（AGENTS.md 开工前必读）。**Phase 1–11 已全部落地，Phase 12 已交付首项 F21 邮箱连接器（2026-07-20）**；各阶段的完成时间、范围与验收见本文 §6 路线图与 [CHANGELOG.md](CHANGELOG.md)，实现相对设计稿的具体取舍记录在 [MISC.md](MISC.md)。（2026-07-27 文档整治：原状态段的逐阶段流水与 §6 重复且长期滞后于 §6，已删去，此处只维护一句总体状态。）模块划分、数据流、隐私边界与 MVP 范围如下。代码：`crates/solum-core`（核心库）+ `crates/solum-cli`（headless 演示 CLI）+ `crates/solum-app`（Tauri 2 桌面/Android 壳）+ `crates/solum-sync-server`（自托管同步中转）+ `crates/solum-notif-access`（Android 通知权限/前台处理本地插件）+ `crates/solum-health-connect`（Android Health Connect 只读插件）+ `crates/solum-alarm`（Android AlarmManager 系统级提醒插件，app 进程死了提醒也准点响），上手见根目录 `README.md`。

## 0. 项目定位

- 一个"真正了解你"的个性化 agent，而不是无状态问答机器人。
- 三个决策已定：
  - 项目位置：独立新仓库（2026-07-20 完成项目更名），与 lx-music 无关。
  - 客户端技术栈：Tauri（Rust 外壳 + Web 前端）。Tauri 2.x 原生支持 iOS/Android，desktop 与 mobile 可共用大部分前端代码，无障碍类系统权限通过 Tauri 自定义原生插件（Android 侧需要写 Kotlin plugin 调用 `AccessibilityService` / 通知监听）实现。
  - 使用范围：自己 + 多设备同步（手机/电脑/未来手表）；sync-server 按 Solum 账号隔离租户，同一账号的设备共享一个分区。

## 1. 需求汇总

### 1.1 功能性需求（原始 8 条 + 补充）

| # | 能力 | 说明 |
|---|------|------|
| F1 | 自然语言事件摄取 → 日程 | 用户随口说一件事(如学校通知)，agent 抽取结构化事件，自动排入日程 |
| F2 | 重要度分级 + 分级提前通知 | 考试类提前 3 天、会议类提前 30 分钟、课程类提前 30 分钟；策略需可配置/可学习 |
| F3 | 主动习惯学习 + 定时问询 | 主动问"现在在干嘛"，识别护肤等固定步骤并主动提醒 |
| F4 | 行为日志自动生成 | 上述交互沉淀为结构化日志，供后续推理使用 |
| F5 | 穿戴设备接入 | 手表/手环数据(心率、睡眠、步数)作为状态感知输入 |
| F6 | 云端推理 + 本地数据存储 | 只用云端做"思考"，原始数据不落云端 |
| F7 | 高危操作强制人工确认 | 删除文件、支付等 dangerous 操作，任何主动模式下都不可绕过确认 |
| F8 | 主动度可调 | 管家/秘书/被动，且应按功能维度独立可调，而非一个总开关 |
| F9 | 数字人格养成 | 导入聊天记录，提取语气/用词/价值观，形成可版本控制的人格画像 |
| F10 | 前瞻建议引擎 | 基于日程规律推测未来需求并建议(如周五休息→建议提前买周四车票) |
| F11 *(补充)* | 情绪/异常状态感知 | 结合穿戴设备识别心率异常/久坐/睡眠差，主动关怀而非仅通知 |
| F12 *(补充)* | 记忆可视化台账 | 用户可随时查看/编辑/删除 agent 记住的任何一条记忆，这是隐私信任的前提，非可选项 |
| F13 *(补充)* | 场景模式自动切换 | 识别工作/学习/休息/睡眠/开会等场景，动态调整打扰策略 |
| F14 *(补充)* | 周期性自我复盘简报 | 每周生成"我为你做了什么/观察到什么"的简报 |
| F15 *(补充)* | 人格漂移控制 | 人格画像版本化，可回滚，避免因导入数据污染导致人格突变 |
| F16 *(补充)* | 离线兜底 | 云端不可达时，已排好的本地提醒仍必须按时触发 |
| F17 *(补充)* | 多设备同步 | 手机/电脑（未来手表）之间的数据与状态同步，端到端加密 |
| F18 *(补充)* | 交互式生成（Generative UI） | agent 回应可携带动态生成的可交互 UI（卡片/表单/按钮），用户操作回流给 agent 形成闭环；操作就地发生在对话流里，见 §3.9 |
| F19 *(补充，Phase 11 第一、二步 ✅)* | 持久化自定义组件（Custom Widget） | 用户可用自然语言生成持久存在的功能小组件，确认预览后保存；固定「工作台 → 组件」二级入口提供组件记录的离线增删改查，并可追加可空字段。它与一次性对话内 GenUI（F18，对话即焚）分属两套渲染器。定义、字段与记录自 schema v13 起参与多设备同步（第一条竖切时曾不同步，该契约已推翻，见 §3.12）。 |
| F20 *(补充，✅ Phase 10 完成)* | 通知智能管线（Notification Intelligence） | F1 升级：默认空的 App 白名单在 Android 捕获端总阀 → `dataSync` 前台处理服务守住 inbox → 双车道（规则命中的重要即时 / 15–30 分钟普通批量）→ 本地优先、只对不确定项做受「通知上云」门控的 LLM 分诊 → LLM 待确认过滤提议 → F12 可见回看、恢复/提升。LLM 不产 id、不执行既有记录动作，`LLM_ACTIONS` 不拓宽。 |
| F21 *(补充，✅ Phase 12 首项完成)* | 邮箱连接器 | 用户可手动连接 QQ 邮箱、Gmail、Microsoft 365 / Outlook 及自定义 IMAP/SMTP 账户，在本地查看、搜索和撰写邮件；发送一律走 `sensitive` Guard 预览与确认。 |

### 1.2 非功能性需求

- **隐私优先**：原始行为日志/聊天记录/人格数据只落本地盘，云端只经手完成单次任务所需的最小上下文。
- **通知可靠性**：提醒是这个产品的信任基础，绝不能"因为 LLM 慢/挂了就不响"。
- **安全护栏是架构级的**：高危操作拦截不能靠 prompt 约束 LLM，必须是代码层面的强制关卡。
- **可扩展**：功能以"能力插件"形式接入，方便后续加穿戴设备、加新的通知渠道。
- **多设备一致性**：本地优先(local-first)，设备间通过同步层最终一致，而不是强依赖中心服务器在线。

## 2. 总体架构

```
┌───────────────────────────────────────────────────────────┐
│ 展示层                                                        │
│ Desktop(Tauri) ── Mobile(Tauri 2 + 原生插件) ── (Web demo 阶段1) │
│  Chat UI / 日程视图 / 通知中心 / 人格与主动度设置面板              │
└───────────────────────────┬─────────────────────────────────┘
                            │ Tauri IPC / 本地 HTTP
┌───────────────────────────▼─────────────────────────────────┐
│ Agent Orchestrator（每台设备本地长驻，Rust 核心 + 可嵌入脚本层）  │
│                                                               │
│  Intent Router ─→ Planner ─→ Importance Classifier            │
│  Proactivity Scheduler（按维度分开关：日程/建议/问询/复盘）        │
│  HITL Guard（risk_level 强制拦截，任何主动模式下都无法绕过）        │
│  Persona Manager（人格状态机 + 版本控制 + 回滚）                  │
│  Suggestion Engine（规则触发时机 + LLM 生成建议内容）              │
└───┬─────────────┬─────────────┬─────────────┬────────────────┘
    │             │             │             │
┌───▼──────┐ ┌────▼───────┐ ┌───▼────────┐ ┌──▼─────────────┐
│Cloud LLM │ │Local Memory│ │Tool Plugins│ │Wearable Adapter │
│Gateway   │ │Store       │ │(标注risk_  │ │(手表/手环 SDK/  │
│(脱敏/最小 │ │SQLite +    │ │level)      │ │蓝牙/健康平台API)│
│化上下文)  │ │本地向量库   │ │            │ │                 │
└──────────┘ └─────┬──────┘ └────────────┘ └─────────────────┘
                    │ 端到端加密增量同步
              ┌─────▼──────┐
              │ Sync Server │  （只转发加密 blob，不解密、不推理、不留明文）
              │ (轻量,自托管)│
              └────────────┘
```

## 3. 关键模块设计

### 3.1 Agent Orchestrator

- 常驻本地进程（Tauri sidecar 或内嵌 Rust 服务），是唯一有权限调度全部模块的"大脑"。
- **Intent Router**：区分输入类型——闲聊 / 事件登记 / 状态回答 / 高危指令。
- **Planner**：把一句话拆成结构化任务（时间、地点、类型、关联人）。
- **Importance Classifier**：规则表 + LLM 兜底。规则表示例：

  ```
  exam      -> lead_time: 3d,  channel: push+banner
  meeting   -> lead_time: 30m, channel: push
  class     -> lead_time: 30m, channel: push
  deadline  -> lead_time: 1d,  channel: push+banner
  ```

  规则表用户可编辑；LLM 用于处理规则表里没覆盖的新类型，并把分类结果反哺规则表（人工确认后才固化，避免自我强化出错误策略）。

### 3.2 Proactivity Scheduler（主动度引擎）

- 不做成一个全局开关，而是每个能力域一个独立等级：`schedule_reminders` / `life_suggestions` / `status_checkins` / `weekly_review` 各自可设 `passive / secretary / butler`。
- "管家模式"提高触发频率和主动询问的深度，但**不改变 F7 的高危护栏**——这条是硬约束，写在下面单独强调。

### 3.3 HITL Guard（高危操作护栏）

- 每个 Tool 定义时必须声明 `risk_level: safe | sensitive | dangerous`。
- `dangerous`（删除文件、支付、发送不可撤回消息等）：
  - 无论 Proactivity 等级如何，强制弹出人工确认，展示"将要执行的具体操作 + 后果预览"。
  - 确认前 Orchestrator 不持有执行该 Tool 的能力（不是"LLM 说了算再拦截"，而是执行层本身没有直接调用权限，必须经过 Guard 签发的一次性 token）。
  - 所有 dangerous 操作写入本地 append-only 审计日志，不可编辑删除。
- `sensitive`（比如"帮我回复这条消息"）：默认需确认，但用户可在秘书/管家模式下选择"仅提醒不阻塞"。

### 3.4 Persona Manager（数字人格）

- 输入：用户手动设定的风格描述 + （后期）导入的聊天记录批量离线处理提取的语气/常用词/态度倾向。
- **F9 导入管道强制纯本地（2026-07-07 决定）**：聊天记录的解析、人格特征提取全程不经过云端 LLM——记录里包含对话另一方的内容，对方未同意上云。提取用本地规则/统计（高频词、语气助词、句长分布、常用表达），产出仅作人格"初稿"供用户手动修改确认。原始聊天记录本身**不进同步管道**，只同步用户确认后的人格版本。
- 输出：`persona_profile_vN.json`，包含可读的特征摘要（不是黑盒向量），用户能看懂自己"人格画像"里写了什么。
- **版本化**：每次重大更新生成新版本，旧版本保留可回滚，防止一次错误的聊天记录导入污染整个人格。
- 人格画像是本地文件，同步时随 Local Memory Store 一起加密同步，不单独处理。
- *（Phase 2 v1 已落地：手动风格设定（称呼/语气/口头禅/风格备注），版本化存 SQLite `persona_versions` 表（内容为可读 JSON），活动版本用指针标记、回滚即移动指针（历史全部保留）。未按本稿原文使用独立 `persona_profile_vN.json` 文件——单一权威存储更简单且随库同步，取舍记录见 MISC.md。注意：人格风格文本会作为系统提示的一部分随云端调用发出（这是"完成单次任务所需的最小上下文"的一部分），行为日志/台账仍不出本地。）*
- *（Phase 3 导入管道已落地，2026-07-07：`solum-core::persona_import`，微信/QQ txt 与「昵称: 消息」两种格式，规则/统计提取（语气词表 + 中文 3–4 字 n-gram 去重 + 标点/句长比例推断语气），强制"先预览再确认"（CLI `persona-import` 预览默认 / `--save` 落库；壳层导入卡片初稿填表单可改再存）。原始记录读后即弃、不落任何库，保存的版本 `source = "import"`。）*

### 3.5 Local Memory Store（记忆分层）

| 层 | 内容 | 存储 | 生命周期 |
|---|------|------|---------|
| 短期上下文 | 当前会话最近 ≤ 4 轮已完成对话 | 壳层本地会话存储 / core 内存窗口 | 本机持久，不同步、不导出 |
| 情景记忆 | 行为日志(如"07:20 完成护肤三步") | SQLite | 长期，可按策略归档/压缩 |
| 语义记忆 | 习惯规则、偏好("周五晚上通常休息") | SQLite + 向量库(本地embedding，如 sqlite-vec) | 持续更新 |
| 人格记忆 | Persona Profile | 本地文件 | 版本化保留 |

- **F12 记忆台账**：一个独立 UI 面板，按上表分层展示，每条记忆可查看来源(哪次对话/哪次导入)、可编辑、可删除，删除后不可通过对话恢复。

### 3.6 Cloud LLM Gateway

- 所有云端调用的唯一出口，职责：
  1. 从 Local Memory Store 按需检索相关片段，组装"够用就好"的上下文，而不是整段行为日志打包发出去。
  2. 支持切换 provider——接口按 OpenAI 兼容 `/chat/completions` 抽象，**当前接小米 MiMo token-plan（用户 2026-07-06 决定，见 MISC.md），Claude API 等可通过配置切换**。
  3. 云端返回结果落地本地后即视为本地数据，网关本身不持久化对话内容。
  4. *（已落地：`solum-core::llm`，凭据走 env/`solum-llm.json`（gitignored），每次调用只发当前一句话+当前时刻+所选会话最近 ≤ 4 轮。）*
  5. 各厂商（小米 MiMo / DeepSeek / GLM / Kimi / Qwen / OpenAI / Gemini）的端点、模型名与参数怪癖调研见 **docs/LLM-PROVIDERS.md**（2026-07-15）；配置增加选填 `temperature`（可置 null 不发送，OpenAI gpt-5 系必需）、`max_tokens`、`timeout_secs`（思考类模型需 >30s）。
  6. *（已落地：壳层「设置 → 云端」视图，厂商预设下拉 + 测试连接 + 保存热切换 reasoner；`solum-app` 命令 `llm_config_get/save/test`，get 永不回传完整密钥，save 写 `SOLUM_LLM_CONFIG` 指向的 JSON。）*
  7. **流式输出（2026-07-19，聊天纯文本先行）**：网关抽象新增 `Reasoner::complete_streaming(system, user, on_token)`——OpenAI 兼容 `stream:true` + SSE 逐 token 回调，只读 `delta.content`（`reasoning_content` 天然不碰，前导 `<think>` 块过滤），返回同 `complete` 的 think-stripped 全文；默认实现回退到 `complete`（离线/测试 reasoner 无需改动）。聊天走 `llm::chat_reply_ui_streaming`：**输出以纯文本开头就逐 token 可见流式，以 JSON 信封开头就抑制可见流式、整包收完再走既有严格 `parse_envelope`**（§3.9 信封整包渲染不变）。上行内容不变（§3.6 第 1 条最小上下文照旧），流式只影响下行呈现方式。
  8. **统一账号身份（2026-08-12，由账号代理扩展；2026-08-13 收紧访客边界）**：完整产品的云端唯一通路是登录统一 `server/`（solum-cloud）后请求 `{server}/v1/ai/chat/completions`，第三方 API Key 只存在服务端环境变量。`solum-core::account` 会话落 `solum-account.json`（gitignored，密码只用于登录请求与本地恢复信封解密、不落盘），访问令牌 15 分钟过期、401 时刷新一次并轮换刷新令牌。该访问令牌同时是 §3.8 同源同步接口的租户身份证明：服务端验签后只以令牌 `sub` 选择租户，拒绝客户端自报用户名。**身份关联不等于解密权**：同步主密钥随机生成并只在客户端解封/保存，账号服务与同步服务均拿不到同步明文。无账号会话时壳层不得安装直连 API Key reasoner，也不得从旧 token 配置启动同步或福利告警；已有 `solum-llm.json` 仅保留为迁移材料，不再构成访客云端旁路。退出登录后回到纯本地 guest profile。

### 3.7 Wearable Adapter

- 抽象接口，具体实现按设备类型（Apple Health / 小米运动 / 华为运动健康等开放 API 或蓝牙直连）适配。
- 只产出结构化状态事件（心率异常、久坐提醒、睡眠质量）喂给 Orchestrator，不直接触发用户可见动作——是否提醒、怎么提醒仍由 Proactivity Scheduler + Importance Classifier 决定。
- **接入路线（2026-07-12 决定，Phase 4 F5 v1）**：用户当前设备是三星运动健康，后续可能再接小米等平台。不走各厂商私有 SDK——三星健康 Data SDK 已改为合作伙伴审批制（2025-07-31 起，旧版 Android SDK 废弃），个人开发者拿不到批准；改走 **Android 官方 Health Connect**（`androidx.health.connect:connect-client`，稳定版 1.1.0）：三星健康自 2022-10 起就把心率/步数/睡眠数据同步写入 Health Connect，我们只需请求 Health Connect 的读权限，不关心数据最初来自哪个厂商 app——这也是"后续接小米"时唯一不用改适配层代码的路径（前提是对方 app 也同步进 Health Connect；国内版小米运动健康是否同步未验证，届时重新评估）。
  - v1 范围严格限定为 F5 本身：只读三类数据（心率/步数/睡眠时长），落本地存储，**不写回、不接 F11 情绪感知/主动关怀、不接 F13 场景切换**——那是需要另外评估触发策略的后续工作，架构上先把"数据能进得来"这层打通。
  - Health Connect 权限是特殊类别（`android.permission.health.*`），不是标准运行时权限弹窗，走 Health Connect 自己的授权页（`PermissionController` 签发的 Intent）；只声明 READ 三项，不声明任何 WRITE 权限（v1 只读，不写健康数据）。
  - 读取实现必须遍历 Health Connect 的分页结果；步数使用时间窗聚合值而不是累加原始 `StepsRecord`，避免多来源/重叠区间重复计数。只有样本成功写入本地 SQLite 后才推进拉取水位，失败会在下一轮重试。
  - *（实现见 §6 Phase 4 与 CHANGELOG：新 crate `solum-health-connect`，Tauri Android 插件，结构与 `solum-notif-access` 一致。）*

### 3.8 Sync Server（多设备同步）

- 定位：只做**加密数据中转**，不解密、不推理、不留明文，服务端被攻破也拿不到用户数据。
- 同步模型：本地优先(local-first)，各设备本地 SQLite 是权威数据源，增量变更加密后上传，其他设备拉取解密合并。
- 冲突解决：建议用 CRDT 或 last-write-wins + 字段级合并（你在 lx-music-improve 项目的桌面统计模块已经用过"CRDT store"的模式，这里可以复用同样的思路，不必重新设计一套）。
- 部署：轻量自托管（可参考你已有的 1Panel 部署经验），单二进制 + SQLite/轻量 KV 即可，不需要重型消息队列。
- **中心库收口（2026-08-12 决定）**：生产部署改为一个 `solum-cloud` API 容器 + 一个 PostgreSQL 中心库；客户端仍然 local-first，桌面/Android 保留 SQLite，鸿蒙保留 relationalStore，任何客户端都不得直连 PostgreSQL。中心库只保存账号/会话、设备、密钥信封、账号级加密设置和不可读的同步密文，不保存可供服务端查询的行为日志、人格、健康采样或聊天明文。账号、AI 代理与同步共用同一个 HTTPS origin，现有 `/v1/push|pull|stats|alerts` 协议保持兼容；`solum-sync-server` 的 SQLite 实现降为旧部署和迁移期兼容入口，不再作为新部署的第二套权威数据源。
- **PostgreSQL 租户护栏**：所有用户数据表都以不可变 `tenant_id UUID` 为首列，并同时启用、强制 RLS；策略只读取 API 在单个事务内设置的 `app.current_tenant_id`，客户端请求体没有提交租户 id 的位置。运行账号不是表 owner、没有 superuser/BYPASSRLS，且只获所需 schema/table 权限；RLS 列与主要游标组成 `(tenant_id, seq)` 等复合索引。连接由进程池复用，不按请求新建连接。大附件后续进对象存储，PostgreSQL 只留密文索引。
- **登录即恢复的密钥模型（2026-08-12 定案）**：用户同步主密钥由首台已登录设备用系统随机源生成；中心库只保存 `recovery-xchacha20poly1305` 密钥信封。客户端以账号密码和不可变 `user_id` 经 PBKDF2-HMAC-SHA256（600,000 轮）导出恢复包装密钥，再以绑定协议版本、`user_id` 与 key version 的 AAD 包装同步主密钥；密码、恢复包装密钥和同步主密钥均不上传。新设备登录成功后用本次输入的密码解开信封，自动恢复同账号设置、画像和其他同步数据。若本机密钥与信封解出的密钥冲突，必须停止并显式报错，绝不静默覆盖；若账号密码将来支持修改，改密事务必须先重包信封。access token 与数据库权限只决定能读取哪个租户的信封，不能解密它。设备公钥批准作为后续无密码换机通道，不是当前登录恢复的前置条件。未来跨用户共享使用独立 `workspace + membership + workspace key envelope`，不得通过关闭 tenant RLS 实现。
- **身份与密钥分离（2026-08-12 修订并加固）**：传输走 TLS，租户身份来自 solum-cloud 签发的短期账号 access token；token `sub` 是账号创建时生成且永不随用户名改变的 UUID，用户名只用于登录/展示，relay 拒绝非 UUID 的账号 `sub`。同步内容仍以设备本地 32 字节主密钥做 XChaCha20-Poly1305 加密。账号 token 只决定“能访问哪个租户分区”，同步密钥只决定“能否解开该分区内的 blob”，两者不可互相替代。设备吊销可撤销账号会话；同步密钥疑似泄露时仍须换密钥并重新播种。旧 `SOLUM_SYNC_SERVER_TOKEN` 只作为 `legacy` 租户迁移入口，不得访问任何账号租户。
- **客户端账号隔离（2026-08-12）**：设备级 `solum-account.json` 只保存当前会话；登录账号的数据根为 `profiles/<user_uuid>/`，其中 SQLite、直连 LLM、同步、邮箱、Soulous 与通知白名单配置分别独立，桌面壳和 CLI 使用同一 active-profile 解析器。WebView 的聊天历史、UI 状态与资料 IndexedDB 也以同一 UUID 加后缀分区；Android 后台告警读取该 UUID 下的同步配置并以 UUID 隔离游标，原生通知监听器读取该 UUID 下的白名单，不再旁路回 guest 根目录。未登录使用 `guest` 分区。登录/退出不会热换一部分状态，而是重启进程，先关闭旧 SQLite 连接、缓存、后台同步与内存对话，再打开目标 profile。旧会话缺 UUID 时可继续刷新 AI 会话，但不得启用账号同步/账号数据分区；重新登录新版 solum-cloud 后补齐。现有未登录本地库保持为 guest，不自动迁入账号，避免在没有用户确认时改变数据归属。账号会话、隐私同意与 Android 系统闹钟镜像是设备级控制状态，刻意不随账号复制；运维显式提供的路径/凭据环境变量仍优先。
- **通知捕获同步（schema v10，Phase 9/10 + 2026-07-19 解耦）**：第三方通知的 raw input 及其派生事件、提醒**无条件参与多设备同步**，不受任何开关控制。理由见下方「为什么同步与上云解耦」。Phase 10 的 `notification_captures`、过滤提议和 Rust 已解析但仍待确认的动作提议只是本机 F12 分诊回看元数据，**不新增同步 payload**；远端设备只获得 raw input 与派生数据。App 白名单和 `notif_cloud` 都是设备本地设置，不走 meta LWW 同步；重要路由规则复用既有 `RuleTable` 配置。
- **为什么同步与上云解耦（2026-07-19 用户拍板）**：`local_only` 此前同时是「不发云端 LLM」和「不参与同步」两件事的条件，但两者风险量级完全不同——发 LLM 是**明文**交给用户自配的第三方厂商（小米/DeepSeek/OpenAI…），同步是**端到端加密 blob** 发往用户**自建**的 sync-server（服务器解不开、不留明文，本节第一条）。捆在一个开关上导致「想要设备间同步通知、但别喂给 LLM」这个合理诉求无法表达。现拆开：**同步永远开，`notif_cloud` 只管 LLM**。
- **`local_only` 语义收窄（schema v10）**：该列现在**只表示「不得作为云端 LLM 上下文」**，与同步无关（旧语义"本地专属"已不准确，保留列名是为避免大范围迁移，读代码时以本条为准）。捕获时按开关盖戳，此后不可变（PRIVACY.md §2）。
- **戳随 payload 跨设备传播**：`raw_inputs`/`events` 的同步 payload 新增 `local_only` 字段，接收端直接采用捕获设备的原始判定，使同一条记录在所有设备上的 LLM 资格一致。**缺失该字段的旧版本 payload 保守回退**为「按 `[通知·` 前缀判定为排除」。接收端的 recall 仍额外 AND 上**本机**的 `notif_cloud` 开关（双重门：捕获时同意 + 本机当前设置都放行才可上行）。
- **前向兼容：读不懂的 op 进隔离区（schema v11，2026-07-19）**——接收端遇到**本版本不认识的表**（版本更新的对端新增的）或**无法解释的 payload**时，把该 op 原样暂存进设备本地表 `sync_quarantine`（不参与同步、无 guid），**游标照常前进**；升级后 `migrate()` 末尾按 `(hlc, origin)` 顺序重放，LWW 语义与首次到达时一致。此前是直接抛错，会连带整批回滚且游标不前进，导致该设备**同步永久卡死**（push 仍工作，故伪装成健康，机制见 PITFALLS 2026-07-19）。**不选"直接跳过"**是因为跳过会让游标越过该 op，日后升级也永远补不回来——静默丢数据比卡死更坏。隔离区有上限（5000 条）且溢出可见（`meta.sync_quarantine_dropped` + CLI 告警）。**服务端版本协商在本架构下不可能**：relay 只过密文，不知道 blob 里有哪些表，无法按能力过滤（取舍见 MISC 2026-07-19）。此后新增同步表不再打断落后版本的设备。
- **v9 → v10 迁移**：此前因 `local_only=1` 而被排除在同步之外的历史通知行，迁移时补 guid 并借回填 UPDATE 进入 oplog，从而首次同步到其他设备；它们的 `local_only` 戳**保持不变**（仍不给 LLM）。已上传的历史密文仍只能由用户轮换同步密钥并清理服务器后消除。
- **KDF 从 Argon2id 换成 PBKDF2-HMAC-SHA256（2026-07-22，当天第三次修订同一段）**：起因是想让 sync-server 运维仪表盘也能直接输用户名+密码，而 Argon2id 没有浏览器原生实现——要么违反仪表盘"不引入依赖"的原则塞一份 JS 库，要么手搓 Argon2id（安全代码不该冒这个险）。PBKDF2 与 HKDF 都是 `SubtleCrypto` 原生算法，浏览器和 Rust 零依赖打平结果（已用 Node `crypto.webcrypto.subtle` 逐字节核对一致）。**代价**：PBKDF2 不是内存困难型，抗 GPU/ASIC 并行暴力破解弱于 Argon2id——对自托管单用户 relay，这个权衡换来"仪表盘也能登录"值得。HKDF 的 salt 从隐式 `None` 改成显式全零 32 字节，因为 `SubtleCrypto` 的 HKDF 接口没有"省略 salt"的选项，必须两边显式对齐同一个值。
- **同步绑定改为账号会话 + 自动密钥恢复（2026-08-12，替代独立同步密码）**：设置页只需登录统一 Solum 账号，客户端强制同步与登录使用同一 origin，以 `solum-account.json` 的短期 access token 鉴权，并在 401 时通过 refresh token 轮换。首设备随机主密钥及其登录恢复信封按本节前述模型创建，新设备无需再填写同步地址或同步密码；磁盘只保存 `{url,key}`，不保存账号密码。旧 `{url,username,password}` 和 `{url,token,key}` 仍可读取并访问 `legacy` 租户，迁移完成前不破坏现有单用户部署；旧账号 profile 若已有本地主密钥，会在首次新版登录时建立恢复信封，若远端已有不同密钥则停止切换并要求人工处理。
- **移动端同步配置补齐（2026-07-22；2026-08-12 profile 化）**：`SyncConfig::load()` 支持 `SOLUM_SYNC_CONFIG` 路径覆盖（同 `SOLUM_LLM_CONFIG`）；未显式覆盖时，桌面、CLI、Android 与原生后台服务现在统一读取 active profile，guest 才使用 app-data 根目录。此前 Android 无 cwd 且原生服务硬编码根目录，两条旁路都已收口。
- **运维仪表盘（2026-08-12 多租户修订）**：`GET /v1/stats` 只聚合已认证账号自己的 blob/设备/告警，不返回跨租户总量；`GET /` 静态页面本身无数据，无鉴权即可加载，页面通过账号服务器的 `/v1/auth/login|refresh` 获取短期令牌，再请求 `/v1/health`/`/v1/stats`。旧静态 token 登录只查看 `legacy` 租户。页面保持 vanilla JS、无第三方依赖。
- **账号隔离的状态告警中转（2026-08-12）**：sync-server 提供 `POST /v1/alerts` 与 `GET /v1/alerts?since=N[&source=...]`，所有写入、读取、游标和 `(tenant_id,event_id)` 幂等约束均限定在令牌账号内。电脑端独立 `benefit-monitor` 只监听 `127.0.0.1`，织境密码与会话由 Windows DPAPI 加密；Solum 侧同样保存账号会话并自动刷新，向自己的租户发布恢复事件。告警表与加密同步 blob 分表，只允许固定状态字段，不接收账户、余额、上游 API 密钥、聊天或任意正文。告警保留 7 天；Android 以同一 Solum 账号会话拉取自己的事件，账号切换时使用按不可变 UUID 区分的本地游标，避免改名或切账号沿用错误进度。
- **告警不是同步数据，也不是第三方推送**：这是单用户自建 relay 上的最小状态中转，服务端能看到“某渠道恢复”这一事实，因此不宣称端到端加密；作为交换，Windows PowerShell 5.1 应用零额外依赖，Android 不引入 Firebase。Android 已有 `dataSync` 前台服务在配置同步后每 20 秒拉取；首次成功连接只建立游标，此后对批次内每个新 `operational`/测试事件分别发高优先级通知，超过 10 分钟的不补发。电脑端同样默认每 20 秒轮询，因此源站检测完成后的典型手机通知延迟为 0–40 秒；ROM 杀后台时不保证实时，用户需允许后台运行/自启动并关闭电池优化。
- *（v1 已落地，2026-07-12：`crates/solum-sync-server`（tiny_http + SQLite 单二进制，POST /v1/push、GET /v1/pull，Bearer token）+ `solum-core::sync`。变更捕获用 **SQLite 触发器**写 `sync_oplog`（含级联删除，Rust 写路径漏不掉；应用远端操作时 `sync_applying` meta 标志抑制回声）；行标识为随机 guid（schema v2 迁移回填，老数据借回填自动进 oplog 完成首次全量），FK 在捕获时翻译成 guid。合并为行级 LWW（毫秒 UTC hlc + 设备 id 决平局），幂等可重放；人格版本号设备本地化、冲突自动重编号，活动指针按 guid 同步。加密 XChaCha20-Poly1305，密钥 64 位 hex 配 `solum-sync.json`/`SOLUM_SYNC_*`。`audit_log` 按 §4 明确不同步。CLI `pa sync`/`sync-status`；壳层顶栏手动同步 + ticker 每 5 分钟自动。未做 CRDT 字段级合并（LWW 整行足够 v1，取舍见 MISC）。）*

### 3.9 Generative UI（交互式生成，F18）

> 立项讨论与参考项目调研见 MISC.md「2026-07-14 交互式生成理念立项讨论」。理念借鉴 [AGenUI](https://github.com/AGenUI/AGenUI)（高德+千问，A2UI v0.9 协议三端原生渲染）与 [AG-UI](https://github.com/ag-ui-protocol/ag-ui)（事件式 agent-前端交互协议）；**只借协议理念，不引入其实现**（AGenUI 是移动原生 SDK，与 Tauri + vanilla JS 栈不匹配）。

**定位**：agent 的回应从「纯文本 + 预置视图」升级为「文本 + 按语境动态组装的可交互 UI」。用户在生成 UI 上的操作（点按钮、提交表单）回流给 Orchestrator 形成闭环，结构化操作就地发生在对话流里，不再要求用户切页。这是"个人 agent"区别于"带聊天框的工具箱"的关键体验。

**四条硬原则（与既有护栏同级，实现不得放松）**：
1. **UI 描述是数据，不是代码**。LLM 只输出受目录约束的 JSON 组件树，渲染器只认目录内组件；不生成/不执行任何脚本。
2. **生成的 UI 只能"请求"动作，无权执行**。按钮/表单只能绑定 command 白名单（见下），Rust 侧校验动作名 + 参数 schema 后才执行；`dangerous` 动作照走 §3.3 Guard 完整确认流程——生成的确认按钮只是入口，不是令牌。
3. **校验不过就降级纯文本，对话永不失败**（F16 精神）。解析复用 §3.6 抽取兜底的防御模式：剥代码围栏 → serde 严格反序列化 → 目录/白名单/参数校验，任何一步不合格整包丢弃、退回纯文本回复。
4. **UI 描述不持久化**（对话即焚）。它是持久数据在当下的一次性渲染，回放即过期且有重复执行动作的风险；留痕的是动作结果——业务数据落既有表、动作走既有审计/台账，不新增同步表。取舍论证见 MISC.md 当日条目。

**分层**：
- **协议层（`solum-core::genui`）**：定义组件目录（Rust 类型 + serde schema）、动作白名单、校验器。产出 `UiEnvelope { version, components }`。离线时由规则引擎按意图选预置模板（事件卡片、建议卡片本来就是模板的雏形），云端在线时由 LLM 自由组装——**渲染器只认协议不认来源**，两条路共用。
- **渲染层（`solum-app` 前端）**：vanilla JS 目录内渲染器（JSON → DOM），随对话气泡内联渲染；不引入前端框架/npm。
- **回传层（既有 Tauri command）**：组件动作 = `{ command, args }`，前端只是把「用户点了」这一事实传回，与 `guard_confirm` 的"传声筒"设计同构。

**组件目录 v1（8 种，从小做起，宁缺毋滥）**：

| 组件 | 用途 | 关键字段 | 可绑动作 |
|------|------|---------|---------|
| `text` | 段落文本（回复正文/说明） | `content`（纯文本，无富文本/HTML） | 无 |
| `event_card` | 事件预览/确认（F1 抽取结果） | `title / kind / start / location?`（展示用快照） | 卡片级按钮见 `button_group` |
| `reminder_card` | 提醒条目（到点/即将到点） | `event_title / fire_at / status` | 同上 |
| `routine_card` | 每日固定提醒创建后的可见确认 | `title / time_of_day / active`（展示用快照） | 同上；离线模板只提供可逆的暂停入口 |
| `suggestion_card` | 建议条目（F10） | `content / dedup_key` | 同上 |
| `button_group` | 1–4 个按钮的动作行 | `buttons: [{ label, action, style: primary/normal/danger }]` | 白名单动作 |
| `form` | 少量字段的就地编辑（如人格初稿修改、事件改时间） | `fields: [{ name, label, type: text/select/datetime/toggle, value?, options? }]` + `submit: { label, action }` | 白名单动作 |
| `choice` | 单选快捷项（问询答案/档位选择），点即提交 | `options: [{ label, action }]` | 白名单动作 |

**动作白名单 v1**（全部映射到既有 Orchestrator 能力，不为 GenUI 发明新写路径）：`ingest`（录入一句话）、`reminder_dismiss` / `reminder_fire`、`suggestion_set`（accepted/dismissed）、`checkin_answer`（状态回答）、`proactivity_set`、`persona_import_save`（携用户改过的草稿）、`event_reschedule`、`event_cancel`（只打开 Guard 预览，不存在同名直调 IPC）、`routine_set_active`（仅持久化 routine id + active 布尔值；每日提醒创建卡仅发出 `active=false` 的可逆暂停）、`guard_request`（发起高危工具确认流程，后续照走 §3.3）。参数一律在 Rust 侧按各 command 既有校验走一遍——白名单只是入口收窄，不替代参数校验。

**明确不做（v1）**：~~流式增量渲染~~（**2026-07-19 部分落地**：聊天纯文本回复已逐 token 流式，见 §3.6 第 7 条；但**信封仍整包渲染**——组件级增量渲染（类 A2UI `updateComponents`/`updateDataModel`）需重构 `parse_envelope` 的半截 JSON 容错，明确延后）；自由布局与样式（组件外观由前端既有 CSS 统一决定，LLM 不能指定颜色/尺寸——防"UI 注入"式误导，如把危险按钮画成主按钮）；跨对话引用旧 UI（不持久化的直接推论）。

> **聊天 prompt 与流式的关系（2026-07-19）**：`chat_reply_ui` 的 prompt 已从「永远输出信封」放松为「默认纯文本回复，仅当确实要给出可执行的后续动作（`ingest`/`checkin_answer`）时才输出信封 JSON」。纯闲聊因此以纯文本开头、可被 sniff-router 逐 token 流式；渲染结果与旧「纯文本信封」完全一致（前端对纯文本信封本就渲染成纯文本）。这与 F18 既有的「散文即消息」兜底路径同构，校验不过仍降级纯文本（原则 3 不变）。

**隐私边界**：不新增由 UI 本身造成的上行数据——UI 描述是云端**返回**的内容；发往云端的仍只有当前输入 + 必要上下文（§3.6 不变）。通知文本若被 recall 选中，只能依 §4 第 6 条的开关作为**上下文**进入既有聊天调用；这不增加 GenUI 动作，也不改变 `LLM_ACTIONS`、参数校验或 Guard。

### 3.10 记忆检索与语义记忆（Memory Recall，F6 补全 + F3 地基）

> 2026-07-15 缺口评估立项（讨论记录见 MISC.md 当日条目）。动机：目前云端 chat 调用只发「当前一句话 + 时钟 + 人格风格」（`llm::chat_reply/chat_reply_ui`），对话层面仍是无状态问答——这是与 §0 定位（"真正了解你"）差距最大的一块。§3.6 第 1 条本来就写明"从 Local Memory Store 按需检索相关片段"，本节把它设计落地；§3.5 的"语义记忆"层（至今未建）也在此补全。

**四条硬原则（与既有护栏同级）**：
1. **检索纯本地**。评分/选取不经过云端；上行内容 = 当前输入 + top-k 片段 + 既有系统提示，片段数 ≤ 5、总长 ≤ 500 字（硬上限在代码里，不靠 prompt 自觉）。
2. **通知捕获来源的文本受「通知上云」开关控制（双重门）**：本机开关关闭时全部排除；开启时**仅**纳入捕获时已标 `local_only=0` 的行。故历史本地行不会因后来开启而回填进语料或云端上下文。同一判定适用于 F20 分诊载荷（Phase 10 验收修复，见 CHANGELOG 2026-07-19）——**任何通往云端 LLM 的路径都必须查逐行戳，全局开关只能是附加条件，不能单独作数**。跨设备场景下戳随 payload 传播（§3.8），因此远端同步进来的通知在本机同样受其捕获时的原始判定约束。
3. **F12 优先**：每个片段可溯源（来自哪条记忆/哪条日志）；检索直接读权威存储、无副本缓存，台账删除即从语料消失。
4. 云端不可用时 recall 不运行也无害（离线路径完全不变，F16）。

**两级演进（v1 刻意不引入向量库）**：
- **v1 词面检索**：记忆条目量级小（数百条），全量拉取 + 内存打分即可：字符 bigram 重合度（中文免分词）× 时间近因衰减 × 层权重（语义记忆 > 行为日志 > 历史事件）。不建 FTS 索引——数据量配不上索引成本，且 FTS5 unicode61 对中文分词不友好。
- **v2 向量检索**：sqlite-vec + **本地** embedding 模型（必须本地——把记忆句子发云端做 embedding 等于内容出本地，违反 §4 第 1 条）。模型体积与移动端可行性是主要风险，待 v1 跑出真实命中率数据后再评估，不预设结论。

**新实体：语义记忆表 `memory_facts`**（schema v3→v4，guid + 同步触发器复用 §3.8 机制）：
`{ id, guid, content（一句话事实）, source（manual/chat/habit）, created_at, last_used_at }`
- 写入来源：① 用户直说（"记住我不吃辣"——Intent Router 新增 `MemoryWrite` 意图，规则先行：句首"记住/别忘了"类模式；LLM 兜底判定的须经用户确认才落库，同 Importance 反哺原则）；② 习惯检测固化（Phase 7 D4 采纳 habit 建议时顺带落 fact）。自动提炼问询回答 v1 不做——未经确认的自动写记忆是人格污染（F15）的同类风险。
- 纳入 F12 台账新层 `fact`：可查看来源、可编辑、可删除，删除后不可通过对话恢复。

**会话短期上下文与历史（独立可先行）**：壳层维护可新建、切换、删除的本机会话历史；完整转录只存在 WebView 的本地会话存储中，不进入 core、SQLite、同步或导出。每次选择会话及发出新消息前，壳层通过 `chat_context_set` 把该会话最近 ≤ 4 轮已完成对话装入 core 内存窗口；`Orchestrator::replace_chat_history` 再次截断。于是历史会话不会把整份转录送上云，chat 上行仍只有当前输入、时刻、最多四轮上下文与可审计的 recall 片段。

**接线面**：新模块 `solum-core::recall`（纯函数 `recall(store, query, now) -> Vec<Snippet>`，单元测试不需要 I/O）；`llm::chat_reply/chat_reply_ui` 增加上下文参数（片段 + 近轮对话），系统提示新增「已知背景（来自本地记忆检索，可能不全）」区块，并保留"不要假装知道其他背景"的既有约束（背景以外仍不许编造）；CLI 新命令 `pa recall <query>` 打印检索结果与评分——让"发给云端之前它检索到了什么"可肉眼审计（F12 精神延伸到上行内容本身）。

### 3.11 BloomXP 互通（Phase 8；8.1/8.2 均于 2026-07-18 完成）

> 背景：用户同时运营 BloomXP（学习打卡全栈 App，Spring Boot + React，自有 VPS，多用户产品；仓库 `D:\ClaudeSpace\Project\Desktop_export_2026-07-13\BloomXP\`）。双向打通的价值已确认：Solum 拿 BloomXP 的课表/任务/打卡数据能更懂用户；BloomXP 拿 Solum 的日程/状态派生事实能更好地辅助学习规划。为此把 §4 的隐私原则从"一刀切不出本地"细化为三级分级（见 §4 第 1/5 条），这是一次**有意的原则调整**，不是违规。

**数据分级（本节是分级的唯一权威定义）**：

- **L1 永不出本地（红线不变）**：导入聊天记录原文（含对话另一方内容，2026-07-07 决定的理由与推送目标无关）、人格画像全部版本、append-only 审计日志、穿戴设备原始采样（逐条心率等）。第三方通知捕获链改由 §4 第 6 条的开关控制；`local_only` 通知来源仍一律拒绝 Solum → BloomXP 推送。
- **L2 派生结构化事实，白名单内可推给自有 BloomXP 服务**：日程事件（时间/标题/类型/重要度）、任务及状态、场景模式结论（"在上课/在休息"）、穿戴派生结论（"昨晚睡眠不足"而非心率曲线）。特征：结构化、粒度粗、单条泄露损失有限。**8.2 v1 实际只开放日程的标题、类型、开始/结束时间与地点；重要度等其余候选字段尚未获独立授权，不能随意加入。**
- **L3 BloomXP → Solum 只读拉取**：课表、考试、任务、打卡、专注时长，进 Solum 本地 SQLite 当只读事实源。

**硬约束（实现时不得妥协）**：

1. **白名单默认全关、逐类打开**，配置面与 F8 主动度同级呈现；F12 台账能看到"哪些类别正在流向 BloomXP"，随时可关。
2. **Solum→BloomXP 推送走 Tool 通道，`risk_level: sensitive`**——初期每次推送可见可确认，跑顺后再评估降级为后台同步（降级本身要过用户拍板）。
3. **来源打标防回声**：跨系统数据一律带 `source=pa` / `source=soulous`；接收方不得把对方来源的数据再回流（否则 Solum 推的事实经 BloomXP RAG 又被 Solum 学回来，两边互相喂造成漂移——BloomXP 曾因双向量库踩过同类坑）。
4. **两侧 AI 记忆各自独立**：只交换事实，不同步记忆/向量库。
5. BloomXP 侧需要一个接收端（`external-context` 模块 + 单表 `user_id + source + type + payload + ts`，只归属用户本人账号；喂 AI 链路当上下文，进 RAG 必须过 `aiMemoryEnabled` 闸且写入走 Spring `RetrievalService`）——实现在 BloomXP 仓库，本节只记录契约。

**8.2 v1 已交付的可审计收口**：Solum 的本地 `push_schedule_events` 白名单默认 `false`，桌面「设置 → 云端」与 F12 台账都持续显示开关状态；只有非 `local_only` 的 Solum 日程才会显示逐条「推送」入口。入口生成 `soulous_push_event` Sensitive Tool，请求和预览共用同一最小投影，确认后才向 `/api/external-context` 发出 `source=pa`、稳定 `externalId` 的幂等请求。BloomXP 只接受 `schedule_event` 和严格字段白名单，按当前 JWT 用户隔离并拒绝未列字段；没有供 Solum 拉回这些行的读 API，避免回声；仅在该 BloomXP 用户显式 `aiMemoryEnabled` 时经 `RetrievalService` 进入 RAG。

**实施顺序**：8.1 先做 L3（BloomXP→Solum 只读，零隐私风险、见效快，认证复用 BloomXP 现有 JWT）；8.2 再做 L2（Solum→BloomXP 推送）。详见 §6 Phase 8。

### 3.12 持久化自定义组件（F19，Phase 11 第一条竖切）

**边界**：这是一个可用的最小闭环，不是通用低代码框架。`widget_defs` 保存名称、受控图标与 list 视图的排序字段；**schema 以一字段一行存在 `widget_fields`**（v13 起，此前是 `widget_defs.schema_json` 单列）；`widget_records` 保存 `widget_id + data JSON + created_at`。三张表**都参与多设备同步**（v13 起，第一条竖切时曾明确不同步，该契约已被推翻）。`widget_schema_rejections` 仍为**设备本地**：它是"缺哪些类型/视图"的产品证据，同 `audit_log` 不出本机。

**视图槽位与字段序必须进同步 payload（v15，2026-07-20 修）**：`widget_fields` 的四个槽位（`form_ord` / `list_ord` / `table_ord` / `stat_ord`）与规范序 `ord` **全部是同步属性**，`SYNC_PAYLOADS` 少带任何一列，对端就落到列默认值——表现为 `table` / `stat` 视图跨设备消失、两台设备字段序不一致，且本机毫无异常。`_restore` 共用这份定义，因此同一处遗漏会同时让备份还原不回来。**新增视图槽位 = 改四处**：建表、迁移 `ALTER`、`SYNC_PAYLOADS`、`apply_one`。存量由 v15 迁移重播修复，且只重播偏离默认值的行（纯接收方保持沉默，否则会把坏数据盖回好的）。

**为什么是行不是 JSON 列（v13，MISC 2026-07-20 定稿）**：整份 schema 存一列时，两台设备并发加字段会走行级 LWW 整行覆盖，晚写方**永久抹掉**先写方的字段，且本机已填该字段的记录会被 schema 孤儿化。拆成行之后合并即求并集；配合下面「只允许加可空字段」的约束，字段集合是**只增集合（G-Set）**，天然收敛，无需任何新增合并逻辑。视图归属折叠为字段自身的 `form_ord` / `list_ord`（NULL = 不在该视图），避免"视图字段数组"再次成为并发冲突点；另有 `ord` 列作为规范字段序，因为同批插入的 `created_at` 相同、靠它排序会落到随机 guid 上而使两台设备顺序不一致。

**schema 演进（v13）**：唯一允许的操作是**追加可空字段**。强制可空是因为已有记录无法追溯填写必填值；其回报是**记录零迁移**（老记录只是少一个键，`validate_record` 对缺失的非必填字段本就放行）。删字段与改类型仍不提供——它们会丢数据，想改走"新建组件 + 导入"。加字段风险级 `safe`。**合并可能使字段数或组件数超过上限，该状态合法但只读增长**：照常渲染与写记录，仅拒绝再加，绝不截断；因此上限只在新增路径强制，`validate_record` 不再重跑 `validate()`。同名字段并发创建按 guid 字典序确定性取舍，落败者进拒绝日志而非静默丢弃。

**v12 → v13 的拆分是一个事务**：先把 `schema_json` 展开成行、最后 `DROP` 掉那一列——这一步只有在删列成功后才是幂等的，中途死掉会留下「行已提交 + 列还在」的状态，重跑即撞 `UNIQUE(widget_id, name)` 而让数据库**永久打不开**。整段包事务，失败回滚回 v12 形状。同类形状（先派生、再删源）今后一律照此处理。

**删除组件必须显式删子行**：SQLite 未开 `recursive_triggers`，FK 的 `ON DELETE CASCADE` **不会**触发子表的同步触发器，靠级联会让其他设备永远留着孤儿字段与记录。故 `delete_widget_definition` 逐表显式删除，每行各自产生 delete op。

**声明式 schema**：字段只允许 `text` / `number` / `date` / `datetime` / `time` / `bool` / `enum(options)` 七类；`time` 是以 `%H:%M` 严格解析的纯时刻。字段数最多 12、视图数最多 4。未知字段或类型、额外 JSON 键、超限、重复/不存在的字段引用、缺 form/list 任一视图，全部拒绝整份定义，绝不修补或部分落库。LLM 只可返回这个 JSON，不能返回代码、HTML 或 CSS。

**写入链路与安全性**：主动输入中明确的「创建组件」规则先于 F1 事件摄取；较模糊的组件请求可由云端 reasoner 判定。reasoner 不可用时只说明新建暂不可用，绝不退化为建立日程。生成结果先在对话中以独立的 DOM 渲染器预览，只有用户确认才写入 `widget_defs`；F18 的 `genui.rs` 与信封协议不参与此路径。记录增删改是 `safe`，创建组件是 `safe`；删除组件（级联全部记录）只能通过 Guard 的预览、人工确认、一次性 token 与 append-only 审计。

**呈现与离线性**：壳层以固定「工作台」抽屉承载「资料 / 组件」两块（2026-07-20 IA 收口后入口在对话输入框「+」浮窗菜单，不再是顶层导航组，见 DESIGN.md），不根据定义动态注册导航。组件页读取已建定义，按 schema 的 `form` 视图生成增改表单、按 `list` 视图显示可选字段排序；每次记录操作后重新从本地存储读取并渲染。已建组件 CRUD 完全离线，只有生成一个新组件需要云端。资料页的 PDF/文档归档同样只留在浏览器本地存储；文本可由用户手动带入输入框，PDF 当前仅归档而不声称已解析。

**硬上限**：字段数 ≤ 12、视图数 ≤ 4（均由 schema 校验拒绝整份定义），本机组件总数 ≤ 8（`MAX_WIDGETS`，无法从单份 schema 判断，改在 `insert_widget_definition` 存储边界强制，任何调用方绕不过）。撞组件上限的请求照常生成预览、在确认时被拒，并**照样写入拒绝日志**——「用户想建第 9 个」与「用户想要 grid」同属第二步要看的产品信号。上限是并发容量不是终身配额，删掉一个即腾出位置。

**明确留后（第一条竖切时的范围声明）**：`table` / `stat` 视图、grid/chart、动态导航、同步、从 `events` 导入或记录提升为日程、以及 schema 演进都不在这条竖切内。**其中同步与 schema 演进已在第二步（v13）补齐，`table`/`stat` 视图与日程互通已在第三步（v14）补齐，见 §6 Phase 11；动态导航已定论不做；仍留后的只有 grid/chart 与删字段/改类型式迁移。**

**CLI 缺席是有意的，但留下一个可测性缺口（2026-07-20 验收补记）**：`solum-cli` 没有任何 widget 命令，这是本仓唯一只能从图形壳层访问的子系统（routines / soulous / notif-intelligence / stats / recall / sync / persona-import 都有）。**记录 CRUD 不进 CLI 是刻意的**——schema 驱动的表单交互在命令行里没有合理形态，硬做就是把渲染逻辑复制一份。**但由此产生的后果要如实记账**：组件的端到端行为目前**进不了 `cargo test`**，只能靠会话临时的 mock-IPC harness 验证，这与本项目"核心闭环 headless 可测"的一贯姿态不一致。**已于第三步补上（2026-07-20）**：只读 `solum widgets [--id N]`，用于排障与导出核对；只读不写因而不必复制表单语义，组件数据自此在 headless 下可观测。写操作仍只在图形壳层。

### 3.13 数据导出与恢复（备份必须能还原才叫备份）

**导出（v1 起）**：本机每一层用户可见数据聚合成一份可读 JSON，纯本地不上云，兑现 §4 第 2/4 条的知情权与可导出承诺。

**为什么 v2 加了 `_restore` 段（2026-07-20）**：v1 的各层是给人看的，**行上不带 guid**。后果是它根本无法被还原——重复导入会让所有内容翻倍，事件与其原始输入的关联也接不回来。**一份不能还原的文件不该被叫作备份**。v2 因此在可读各层之外增加 `_restore`：按表分组的、带 guid 的**行级线上形状**，其字段表达式与同步捕获触发器**共用同一份 `SYNC_PAYLOADS` 定义**，所以导出的行天然就是合并路径认识的行。代价是内容在文件里出现两次；对个人备份而言，这比在"可读"与"可还原"之间二选一便宜。

**导入 = 走普通合并路径**：`_restore` 的行被还原成同步 op，交给既有的 `apply_remote_ops`，因此白拿 LWW、FK 按 guid 翻译、幂等与隔离区，**没有第二套导入器**。

**时间戳用导出时刻而非"现在"**，这是本设计的承重决定：LWW 于是把恢复的行当作"从另一台设备迟到的行"处理，从而**恢复旧备份不会覆盖你之后做的修改**，同一份文件导入两次是 no-op。**恢复相对于用户此后的操作没有特权**。

**导入过 Guard**：它跨所有层写入，风险级同删除组件——预览显示来源设备、导出时间与各表条数，人工确认后才执行，留 append-only 审计。导入**从不删除**任何本机数据。

### 3.14 邮箱连接器（F21，✅ Phase 12 首项完成）

- **协议与账户覆盖**：统一以标准 IMAP（读取、文件夹、服务端搜索）+ SMTP（发送）实现。QQ 邮箱使用其开通 IMAP/SMTP 后生成的授权码；Gmail 与 Microsoft 365 / Outlook 优先使用 OAuth 2.0 Authorization Code + PKCE，使用 IMAP OAuth / SMTP OAuth 的短期 access token 并以本地 refresh token 续期。界面同时保留「自定义 IMAP/SMTP」入口，供其他标准邮箱及明确允许的应用专用密码使用。任何厂商没有开放 IMAP/SMTP 或被组织策略禁用时，必须如实报错，不以网页抓取伪装为已接入。
- **本地凭据边界**：邮箱账户配置、应用专用密码、OAuth client 配置与 refresh token 只保存在 gitignore 的本机 `solum-email.json`（移动端位于 app-data），不进 SQLite、同步 payload、导出文件或诊断日志；设置页和 IPC 永不回传完整秘密，只显示掩码尾部。OAuth 回调使用本机 loopback listener，授权 code、PKCE verifier 和 state 仅在内存中保留到本次授权结束或超时。
- **邮件数据边界**：邮件列表、正文、附件元信息和搜索结果仅保留在本次进程 / 界面内存；v1 不建立本地邮件镜像、不做后台轮询、不触发通知，也不自动将邮件内容写入日程、记忆或台账。它们默认绝不进入 LLM、recall 或「通知上云」语料，避免第三方邮件内容成为 prompt 或执行指令。日后若要把某封邮件转为日程或记忆，必须另设逐项、可见的最小投影确认。
- **发送是敏感外发**：`email_send` 定义为 `risk_level: sensitive`，每次都展示发件账户、收件人、抄送、主题、正文与附件摘要，确认 token 严格绑定该序列化草稿；没有 token 不可发送，不能被主动模式、LLM 或后台 ticker 调用。审计只记录账户 id、收件人数量与执行结果，绝不记录地址、主题、正文、附件名或 OAuth / SMTP 秘密；确认预览不会持久化。
- **v1 范围**：支持纯文本与 HTML 正文、To/Cc/Bcc、多账户切换、文件夹浏览、最近邮件、邮件详情、服务端主题/发件人搜索与无附件发送。附件上传、后台增量同步、邮件规则、自动回复、LLM 代拟或自动发送均不在本期范围；后续每一项都需重新评估数据留存与风险级别。

### 3.15 壳层缓存与 UI 状态（2026-07-29）

- **权威业务数据不复制**：SQLite / core 仍是日程、提醒、记忆、人格、规则与组件的唯一权威。壳层缓存只合并同一轮渲染中重复的幂等 IPC，TTL 为 0.7–1.5 秒、仅存在于当前进程；任意写命令成功后用 generation 整代失效，旧的在途读取也不能重新填回缓存。memory recall 仍按 §3.10 直接读权威存储，删除或改写立即生效。
- **禁止进入缓存的内容**：邮件列表、正文与搜索结果继续遵守 §3.14 的“仅当前界面内存”边界；隐私同意状态每次直读设备记录；密码、token、API key 和完整邮箱配置永不进入通用缓存。短时 IPC 去重不是持久化层，也不写 `localStorage` / IndexedDB。
- **允许持久化的 UI 状态**：`solum.ui-state.v1` 只保存最后视图、各根最后子视图、各视图滚动位置和按聊天会话分隔的未发送草稿；明确不保存搜索词、邮件表单、通知正文或账号字段。对话完整历史继续沿用兼容键 `pa.chat-sessions.v1`，不进 SQLite、同步或导出；序列化移出当前交互帧，退出/切后台时强制刷盘，并设置 2.4M 字符全局预算，超限优先保留最近会话和最近消息。
- **刷新模型**：导航只刷新目标视图依赖的数据，启动只做一次全量 hydrate；业务写入仍可触发全量或相关域刷新。刷新反馈不推动布局，只动画 `transform` / `opacity`，并尊重 `prefers-reduced-motion`。

## 4. 隐私与安全边界（这是整个项目的信任基础，优先级最高）

1. 原始数据（聊天记录、行为日志、人格文件、穿戴设备数据）**只存本地和自建 sync-server（加密态）**，任何第三方云端 LLM 只接收单次任务所需的最小上下文，且不做云端持久化。*（2026-07-18 细化：经用户白名单授权的**派生结构化事实**（L2 级，定义见 §3.11）可推送到用户自有的 BloomXP 服务；原始数据与人格仍属 L1 永不出本地。）* *（2026-07-19 例外：**第三方通知捕获链**被移出本条的绝对本地约束，改由第 6 条的开关控制——见下。聊天记录原文、人格、审计、穿戴逐条采样仍严格 L1 永不出本地，不受此例外影响。）* *（2026-07-20 补充：邮箱连接器的凭据与邮件内容不进入 SQLite / 同步 / 导出 / LLM，边界见 §3.14。）*
2. 用户对"agent 记住了什么"有完全的知情权和删除权（F12），这个面板要在 MVP 阶段就有雏形，不能是三期才做的"锦上添花"。
3. 高危操作的确认流程是代码层强制的，不受 Proactivity 等级、Persona 设定、或任何 prompt 影响（3.3）。
4. 审计日志 append-only，本地保存，用户可导出但不可从 UI 内直接删除历史审计记录。
5. 与自有 BloomXP 服务的数据交换遵守 §3.11 三级分级与四条硬约束（白名单默认全关、推送过 Tool+sensitive 确认、来源打标防回声、两侧记忆不同步）。注意 BloomXP 是多用户系统，数据进它的 MySQL 后暴露在整个应用的 bug 面上——这是 L2 只放"粒度粗、单条泄露损失有限"事实的原因。
6. **通知上云（2026-07-19 重划红线，2026-08-11 收紧默认值）**：第三方通知文本只有在「设置 → 隐私 → 通知上云」被用户主动开启后，才可作为上下文发往云端 LLM；新安装**默认关闭**（opt-in）。设置只决定**之后新捕获**行的 `local_only`；关闭后新行不发 LLM、不进 recall、不进 F20 分诊载荷，历史 `local_only=1` 行在再次开启时仍不回填。已保存的显式选择在升级时保持不变。**该开关与多设备同步无关**——通知的同步无条件常开，因为同步是端到端加密发往用户自建服务器，与"交给第三方厂商"不是同一风险面（解耦理由见 §3.8）。因 API 由用户自行配置，云端处理责任归所选厂商，隐私政策（`docs/PRIVACY.md`）明示后果。**两条护栏不随此放宽**：① 上传内容**可能包含第三方（联系人/机构）信息**，隐私政策必须如实披露；② **注入线不松**——通知内容进 LLM 仅作**上下文**，永不成为可执行动作来源（F18 硬原则、`LLM_ACTIONS` 窄白名单不变）。

## 5. 技术选型与理由

| 领域 | 选择 | 理由 |
|------|------|------|
| 客户端框架 | Tauri 2.x | Desktop+Mobile 共用前端，体积小，系统权限（通知/无障碍）通过原生插件可控；Android 端仍需单独 Kotlin 插件对接 AccessibilityService/通知监听 |
| 本地结构化存储 | SQLite | 成熟、零运维、Tauri 生态支持好 |
| 本地语义检索 | sqlite-vec 或本地向量库 | 避免额外起一个向量数据库服务，保持"本地优先"的轻量原则 |
| 云端推理 | OpenAI 兼容网关（当前小米 MiMo；可配置切换 Claude API 等） | 用户提供 MiMo token-plan；接口不绑厂商，换 provider 只改配置（2026-07-06 变更，原定默认 Claude API） |
| 多设备同步 | `solum-cloud` + PostgreSQL 中心库（旧 Rust/SQLite relay 仅迁移兼容） | 单一账号与同步入口；中心库只保存密文，客户端继续离线优先；RLS 在数据库层二次保证租户隔离 |
| 冲突解决 | 行级 LWW（原选型 CRDT，v1 落地时改用 LWW，见 §3.8 与 MISC 2026-07-12） | LWW 整行合并对 v1 数据形状已足够、幂等可重放；唯一需要集合语义的 widget 字段用"拆行 + 只增集合"解决（§3.12），未引入 CRDT 库 |

## 6. MVP 路线图

**Phase 1（Web/Desktop demo，验证核心闭环）** — ✅ 核心已实现（headless CLI，2026-07-06）
- ✅ 聊天输入 → 事件抽取 → 日程写入 → 分级提前通知（离线规则版；✅ OS 系统通知渠道已接，2026-07-06 桌面壳常驻 ticker）
- ✅ 本地 SQLite；✅ 云端推理已接真实网关（2026-07-06，`solum-core::llm`，OpenAI 兼容/当前 MiMo：闲聊回复 + 抽取兜底，云端失败自动降级离线）
- ✅ HITL Guard 已搭好并端到端测试（用模拟高危工具 `demo_delete` 演示，尚无真实 dangerous 操作接入）
- ✅ 记忆台账雏形（`ledger`：分层展示原始输入/事件/通知，可级联删除）
- ✅ 图形前端 / Tauri 壳（2026-07-06，`crates/solum-app`：Tauri 2 + 静态前端，8 视图覆盖对话/日程/提醒/台账/复盘/规则/主动度/护栏，已真机屏幕自动化验证）

**Phase 2（本地智能增强）**
- ✅ 行为日志 + 定时问询（2026-07-06，`solum-core::journal`：状态/问询/提醒触发自动落账，问询频率按 status_checkins 档位；桌面壳常驻 ticker 承载，OS 通知 + 对话页横幅）；习惯学习雏形见下
- ✅ Suggestion Engine v1（2026-07-06，`solum-core::suggest`：考试/截止/早会/冲突四条日程规则 + 行为日志习惯检测，dedup 防刷屏，life_suggestions 档位门控）
- ✅ Persona Manager v1（2026-07-06，`solum-core::persona`：手动风格设定，版本化 + 指针回滚，生效于闲聊回复与复盘措辞；聊天记录导入仍属 Phase 3）
- ✅ 周报复盘（离线切片：`solum-core::review` + CLI `review`；✅ 云端按人格改写已接，2026-07-06，`llm::rewrite_digest`，本地校验数字、失败降级离线原文）

**Phase 3（移动端 + 生态接入）** — 推进顺序 2026-07-07 定，四项全部完成：
1. ✅ 移动端基础壳（Tauri 2 Android + 通知渠道）+ 通知监听原生插件（F1/F2，2026-07-07 模拟器验证）+ 应用内权限引导入口（2026-07-12，新 crate `solum-notif-access`，模拟器端到端验证：跳系统设置页 + 自动检测已授权）；无障碍（AccessibilityService）暂未做，侧载阶段搁置见 §7
2. ✅ 聊天记录导入 → 人格提取 + 版本控制（2026-07-07，`solum-core::persona_import` 纯本地管道，见 §3.4；CLI `persona-import` + 壳层导入卡片，先预览确认再保存，`source=import`）
3. ✅ 多设备同步上线（2026-07-12，`solum-sync-server` 加密中转 + `solum-core::sync` 触发器 oplog / 行级 LWW 合并，见 §3.8；CLI `pa sync` + 壳层自动/手动同步，双设备端到端验证）
4. ✅ 穿戴设备接入 F5 v1（2026-07-12，见下方 Phase 4）——原计划"优先级最低可顺延"，因用户已有可用设备（三星运动健康）故提前完成，未等到 Phase 4 单独启动

**Phase 4（穿戴数据深化，✅ 已完成）** — 2026-07-12 开工，范围由用户拍板限定为 F5 本身：
- ✅ F5 v1：Health Connect 只读适配（心率/步数/睡眠时长），本地落库 + 纳入 F12 台账 + 随 §3.8 同步，见 §3.7 决定与 CHANGELOG。
- 当时暂不做（"看到数据"和"基于数据做主动决策"是两层不同复杂度的工作，v1 先把前者做扎实）：F11 情绪/异常状态感知、F13 场景模式自动切换。**两者后已于 Phase 7 D5 落地（2026-07-16，F11 三信号 + F13 两场景，见下）**。

**Phase 5（交互式生成 F18，2026-07-14 立项，设计见 §3.9）** — v1 当日完成：
- ✅ `solum-core::genui`：组件目录 v1 类型 + 校验器 + 动作白名单；四个离线模板构建器（事件确认卡/问询快捷答/建议采纳/人格草稿表单）
- ✅ Reasoner 接入（`llm::chat_reply_ui`）：Chat 意图单次调用请求信封 JSON，LLM 可用动作收窄为 `ingest`/`checkin_answer`（无真实行 id 的动作不给 LLM），防御解析失败降级纯文本
- ✅ `solum-app` 前端：对话气泡内联渲染器（JSON → DOM，textContent 不注入 HTML）+ 壳层 dispatch 白名单二次校验 + 按钮一次性禁用
- ✅ 端到端验证：四条链路（事件卡取消提醒/问询 choice 快捷答/建议采纳/人格 form 就地编辑）经 mock-IPC harness 浏览器点通 + 白名单外动作被拒反向用例；130 个 Rust 测试全绿、clippy 零告警、桌面壳启动冒烟通过。详见 CHANGELOG 2026-07-14 条目。
- ✅ **云端路径真机验证**（2026-07-14，Android 模拟器 + 真实 MiMo）：闲聊「忘记喝水」→ MiMo 返回带「设个喝水提醒」按钮的信封 → 点击 → 按钮携带的 `ingest` 动作发出 → 云端兜底解析 → 提醒落库 → 第二代事件卡片渲染，全闭环成立；移动端 `solum-llm.json` 从 app-data 目录加载（同日补上，此前移动端云端恒离线）。

**增补（2026-07-14，F2/F16 硬化，用户拍板）**：
- ✅ **OS 通知降噪**：系统通知只保留「事件提醒」（开会/考试这类要到点响的）；问询/建议/通知捕获属信息获取，只走窗口内呈现（横幅/toast/对话流卡片），不再打系统通知。
- ✅ **Android 后台提醒**：新 crate `solum-alarm`——ticker 把 pending 提醒集合镜像成 AlarmManager 精确闹钟（exact 降级 inexact 兜底；开机重挂；已触发即时摘除），app 进程被杀系统也准点投递；Android 上 OS 提醒 toast 由 alarm receiver 独占、提醒状态仍由 `fire_due` 独占（防双响、单写者不变）。模拟器已验证「杀进程 → 准点响 → 重开收敛」全链路，见 CHANGELOG 当日条目。

**Phase 6（记忆与数据地基，2026-07-15 规划，✅ 2026-07-16 全部完成）** — 缺口评估的第一优先级：现状对话层面仍是无状态问答，与 §0 定位差距最大；同时把"等真实数据"的两条待决项收口。设计见 §3.10 与 MISC 当日条目：
1. ✅ **M1 会话短期上下文**：聊天壳层提供本地历史会话与切换；chat 上行仍仅携带所选会话最近 ≤ 4 轮对话（完整转录本机存储，不同步、不导出）。独立于其余项、成本最低，可先行。
2. ✅ **M2 语义记忆表**：`memory_facts`（schema v4，guid+同步触发器）+ F12 台账新层 `fact` + `MemoryWrite` 意图（规则先行，LLM 兜底须确认）。
3. ✅ **M3 recall v1**：`solum-core::recall` 词面检索（bigram 重合 × 时间衰减 × 层权重，top-5/500 字硬上限，通知捕获来源排除）接入 chat 系统提示；CLI `pa recall` 让上行内容可审计。
4. ✅ **D2 数据回看**（规则表初值的**人工确认固化**留待真实数据积累后由用户执行，`pa stats` 已就绪）：一次性离线分析命令 `pa stats`——行为日志活动聚类、问询应答率、穿戴三类数据覆盖率与分布（含个人基线估计：近 28 天中位数）。产出两件事：人工确认后固化 ImportanceRule 初值（§7 第一条收口）；按"每类数据 ≥ 14 天覆盖"门槛判定 F11/F13 能否开工。纯只读、不上云。
5. ✅ **D3 第一个真实 dangerous 工具**：`ledger_purge { layer, before }`（对话指令批量删除台账条目，如"把上周的行为日志清掉"）。选它的理由：真实破坏性写路径、无外部依赖、审计留痕有实际意义。走 §3.3 完整流程——确认弹窗展示**将删除的条数与范围预览** → 一次性 token → 执行 → append-only 审计；GenUI `guard_request` 动作首次接真实载荷。Guard 从"演习"变实弹。

**Phase 7（主动智能完全体，依赖 Phase 6 产出，✅ 2026-07-16 全部完成）**：
1. ✅ **D4 F3 习惯闭环完全体**（偏差：采纳后直接按检测时间自动建 routine，未做 GenUI form 微调，取舍见 MISC 2026-07-16）：新实体 `routines` 固定提醒表（v1 仅每日频率；`{ title, time_of_day, source, active, scheduled_until }`，schema 升级 + 同步 + 台账层）；采纳 habit 类建议 → 自动建 routine；到点生成当日 notification（dedup 按日），复用既有提醒/AlarmManager 链路；**暂停或删除 routine 会撤销所有尚未触发的已物化 occurrence 与提醒**，已触发的历史仍保留；routine 触发后可一键"已完成"落行为日志，**连续 7 天未确认 → 反向建议暂停该 routine**（主动性自带刹车，防骚扰）。习惯事实同步落 `memory_facts`（接 M2）。
   - **明确日常意图也可创建 routine**：用户输入带明确时间的「每天/每日/每早/每晚……」时，离线解析器直接建立每日固定提醒，并立即物化今天/明天的 occurrence；来源保留为原始输入，用户可在 F12 台账暂停或删除。仅支持每日频率，`每周` / `每月` / `每小时` 等尚无正确调度模型的表达必须继续明确告知未支持，不能伪装成一次性日程。
2. ✅ **D5 F11/F13 v1**（数据门槛检查内置于每条 wellness 规则，门槛未开的信号自动静默——无需人工先跑 D2 报告再开工）（开工门槛 = D2 报告确认数据覆盖，阈值全部相对个人基线、不用绝对值）：F11 只做三个可解释信号——久坐（清醒时段步数窗口）、睡眠不足（低于基线 20%）、静息心率异常（连续 3 日高于基线 10%）；输出走 Suggestion 引擎（新 kind `wellness`），受 `life_suggestions` 档位门控、dedup 按日、**不发系统通知**（信息类，遵守 2026-07-14 降噪决定）。F13 只做两个场景——睡眠中（Health Connect 睡眠会话或 23:00–07:00 无活动）→ 问询静默；日程中（当前时刻在事件区间内）→ 问询/建议延后；实现为 `scene(now, store) -> Scene` 纯函数供 Proactivity Scheduler 查询，不做独立状态机、不做地理位置。
3. ✅ **D6 F14 叙事增强**：周报在数字切片外加「观察」段（top 习惯、routine 完成率、wellness 信号次数，离线统计产出结构化条目）与「本周我记住了什么」段（新增 `memory_facts` 列表，纯本地渲染不上云）；云端仍只做措辞改写，复用 `rewrite_digest` 的"数字/条目必须原样保留否则打回"校验模式。

**Phase 8（BloomXP 互通，2026-07-18 立项；8.1/8.2 ✅ 已完成，设计声明见 §3.11）**：
1. ✅ **8.1 BloomXP → Solum 只读拉取（L3，2026-07-18）**：`solum-core::soulous` 调用 BloomXP 现有 REST API，使用现有 JWT 双 token 并在 401 时自动刷新；服务器地址与 token 仅存 gitignore 的本地 `solum-soulous.json`，缺配置时整体静默关闭。课表/考试/任务/打卡/专注时长以原子快照进入 schema v6 独立 `soulous_facts` 表，强制 `source=soulous`、稳定 guid、同步触发器与行级 LWW；**不写入 `memory_facts`、不进 recall 语料**。考试数据接 Importance Classifier，考试/任务接 F10 Suggestion Engine，全部五类数据接 F14 周报素材；CLI 提供 `solum soulous pull/status`，桌面壳「设置 → 云端」提供本地配置和手动拉取。网络或解析失败绝不覆写上次完整缓存，也不进入 `ingest`/提醒/ticker 链路（F16）。**接口偏差**：当前 `GET /api/checkin` 只提供“今日是否打卡/连续天数/余额”，没有历史打卡列表；本实现仅于成功拉取时保存当日快照，不伪造历史记录。若日后需要完整历史，再由 BloomXP 另行增加只读聚合端点。
2. ✅ **8.2 Solum → BloomXP 受控推送（L2，2026-07-18）**：Solum 增加默认关闭的 `push_schedule_events` 单类白名单、F12 可见状态与逐日程 `soulous_push_event` Sensitive Tool；一次性 Guard 确认展示与实际 HTTP 请求同源的最小投影（标题、类型、开始/结束时间、地点），`local_only` 第三方通知来源一律拒绝。401 复用现有 refresh token 轮换，推送不经过 ingest、ticker 或同步。BloomXP 增加认证的单向 `/api/external-context` 接收端及 schema v23/v24，按 `user + source + type + externalId` 幂等更新、严格拒绝白名单外字段、固定 `source=pa`，没有反向读取端点；入库事实仅在接收用户的 `aiMemoryEnabled` 开启时由 `RetrievalService` 写入 RAG。

**下一步待做队列（2026-07-19 排期，按依赖顺序）**：

**Phase 9（通知上云地基，✅ 2026-07-19 完成；默认值 2026-08-11 收紧）**：设置开关「通知上云」已接入 core、CLI 与「设置 → 隐私」；最初按作者自用取舍为 opt-out，现改为新安装默认关闭、用户显式开启。`ingest_captured` 在捕获时读取开关，令 raw input/event/notification 共享 `local_only` scope，既有同步触发器列驱动自动生效（§3.8）。§3.10 recall 在开时只纳入 `local_only=0` 的通知派生语料、关时全部排除；历史 `local_only=1` 行不回填，开关本身保持设备本地。

**Phase 12（邮箱连接器，F21，✅ 2026-07-20 首项完成）**：已交付 QQ / Gmail / Microsoft 365 / Outlook / 自定义 IMAP-SMTP 的手动连接、收件箱/文件夹/详情/搜索和受 Guard 保护的发送；凭据、refresh token 与邮件正文均不落 SQLite，不同步、不导出、不进入 LLM。OAuth 使用本机 loopback + PKCE，QQ 使用邮箱授权码；v1 不做附件、后台拉取、自动转日程或自动发送，完整边界见 §3.14。

**Phase 10（F20 通知智能管线，✅ 2026-07-19 完成）**——依赖 Phase 9 的 capture-time scope，已按两部分落地：
- **10a（Rust 双车道分诊）**：App 白名单默认空且在 Android listener 读取 extras 前做总阀；白名单内通知写可逆的 F12 `notification_capture` 回看。`RuleTable` 的包范围子串/正则重要规则（允许 App 时填充可编辑预设）把通知路由到即时或普通车道；同 App + 规范化内容 hash 的 10 分钟确定性去重。普通队列以最多 24 条批量分诊，间隔 15/20/30 分钟；规则先抽取，只有不确定项且「通知上云」开启才调一次 LLM。过滤结果、判重与离线失败均可回看/恢复；对捕获时已标 `local_only` 的行，“恢复”只会重跑确定性本地抽取，绝不改变逐行戳或触发 LLM，并必须以不同的可读理由说明结果。LLM 过滤规则必须用户确认后才固化；已确认的既有日程动作把源通知标为独立的已处理终态，不复用“待回看”。
- **10b（Android 保活）**：白名单非空才启动低优先级 `dataSync` 前台服务，配置 `FOREGROUND_SERVICE` / `FOREGROUND_SERVICE_DATA_SYNC` / 电池优化豁免权限；普通批次由宿主内部定时器运行，刻意不引 WorkManager。设置页提供电池优化与应用后台设置入口，并如实提示国产 ROM 仍可能需要人工放行。
- 护栏未放宽：开关关闭即纯本地、零 LLM/零外发；LLM 可产生新事件、待确认过滤提议，或不含 id 的「取消/改期某标题」意图。对已有记录，Rust 必须先唯一匹配本机 event id，再在 F12 提供确认/忽略，确认前不执行；`LLM_ACTIONS` 不扩张，通知来源只作分诊信号而非授权。取舍见 MISC 2026-07-19「Phase 10 实现取舍」。

**Phase 11（F19 持久化自定义组件，✅ 第一、二步完成）**：
- **第一条竖切（2026-07-19）**：「一句主动输入 → LLM 声明式 schema → 严格校验/拒绝日志 → 预览确认 → 本地持久化 → 固定组件 tab 的 list + form CRUD」闭环，详见 §3.12。七类字段中 `time` 复用 routine 的 `%H:%M` 语义。删除组件是接入真实 `widget_delete` Dangerous Tool 的 Guard 链路，记录写入保持 safe。当时三张表明确不同步。验收结论见 MISC 2026-07-20。
- **第二步（schema v13，2026-07-20）**：**schema 演进 + 多设备同步**。schema 由单列 JSON 改为一字段一行，`widget_defs` / `widget_fields` / `widget_records` 进入 `SYNCED_TABLES`（**推翻第一条竖切"不同步"的契约**）；唯一的演进操作是追加可空字段，因而记录零迁移。合并语义、上限超限的处理与显式级联删除见 §3.12。拒绝日志仍不同步。
- **第三步（schema v14，2026-07-20）**：补齐 `table` / `stat` 视图（视图槽位扩到四个，由 `WidgetViewType::ALL` 统一索引，**合并语义未动**）；`stat` 的聚合算子**由字段类型推导而非写进 schema**，以免在声明式 schema 里开出表达式位；新增 `widget_import_events` / `widget_promote_record` 两条与日程互通的**快照**通路（拷值不建链接，理由见 §3.12）；新增只读 `pa widgets` CLI，补上组件此前进不了 headless 测试的缺口。
- **仍然留后**：grid / chart（各需一整套坐标刻度语义）、以及删字段/改类型式的 schema 迁移（想改走"新建组件 + 导入"）。**动态导航已给结论：不做**——`MAX_WIDGETS = 8`，按定义注册顶层导航会让主导航被用户数据挤爆，而组件页本身已是目录+详情两级；固定入口是终态不是欠账。

## 7. 待决问题 / 风险清单

- ~~Importance Classifier 的规则表初始值从哪来~~ → 收口路径已定（2026-07-15）：Phase 6 D2 `pa stats` 数据回看产出真实分布，人工确认后固化初值，见 §6。
- ~~聊天记录导入人格（F9）处理管道要不要强制本地跑~~ → 已决（2026-07-07）：强制纯本地，见 §3.4。
- Android AccessibilityService 权限申请对应用商店审核敏感——**自用侧载阶段显式搁置，上架前重评**，不阻塞 Phase 3。
- ~~Sync Server 的加密密钥管理目前还没设计~~ → 已决（2026-07-07）：自托管场景从简，预共享主密钥方案，见 §3.8。
- ~~国内穿戴设备 API 开放度不确定（个人开发者可能只能蓝牙直连）~~ → 已决（2026-07-12）：不走厂商私有 SDK，统一走 Android 官方 Health Connect，见 §3.7。小米等其他平台若不同步进 Health Connect，届时需要单独评估（可能确实要退回蓝牙直连）。
- ~~F11/F13 触发策略还没设计~~ → 已设计（2026-07-15，Phase 7 D5，见 §6）：三信号 + 两场景，阈值全部相对个人基线（近 28 天中位数，来自 D2 统计管道）而非绝对值；开工门槛 = D2 报告确认每类数据 ≥ 14 天覆盖，仍然不拍脑袋定阈值。
- **release keystore 仓库外无副本**（`solum-release.keystore` + `keystore.properties` 均 gitignored）：丢失则以后升级 app 只能卸载重装（丢本地数据）。用户需自行把这两个文件备份到仓库之外的安全位置——这是唯一一件代码解决不了的待办。
- recall v2（本地 embedding + sqlite-vec）的移动端可行性未评估（模型体积/内存），v1 词面检索先跑，见 §3.10。
- 用真实数据喂 Phase 6/7 的前提是日常真实使用（dogfooding）：release APK 已可安装到主力手机，多条待决项（规则表初值、F11/F13 门槛、recall 命中率）都只能靠真用积累。

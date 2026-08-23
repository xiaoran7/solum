# 息壤 Solum（Personal Agent）架构文档

> 状态：本文档是设计的唯一权威（AGENTS.md 开工前必读）。**Phase 1–12 的历史实现已落地；2026-08-22 用户收口产品为 Mobile-only、Android-first：移动端范围包括 Android、iOS、HarmonyOS，当前主开发和首发 Android，成熟后补齐 iOS 与鸿蒙；桌面和网页不再是产品。多设备双向同步由平台无关的端到端加密版本化云备份与换机恢复取代。** `solum-cloud + PostgreSQL` 继续承载账号、AI、必要元数据和不可读备份密文；现有桌面构建、Rust/SQLite relay 与 `/v1/push|pull` 只属迁移态。当前代码以 `solum-core` + Tauri Android 壳 + Android 原生插件为主；未来 iOS/HarmonyOS 通过各自平台适配层复用同一领域契约、服务端 API 与备份格式，不能分叉成三套不兼容产品。

## 0. 项目定位

- 一个"真正了解你"的个性化 agent，而不是无状态问答机器人。
- 三个决策已定：
  - 项目位置：独立新仓库（2026-07-20 完成项目更名），与 lx-music 无关。
  - 客户端范围：**只做移动端，Android-first**。Android 当前使用 Tauri 2 + Kotlin 原生插件；iOS 与 HarmonyOS 在产品成熟后补齐，具体壳层可按平台能力选择，但领域模型、账号/AI API、备份格式与隐私语义必须一致。桌面构建只作为待清理历史入口，不发布、不扩展。
  - 使用范围：当前活跃手机是唯一业务权威；服务器提供账号、AI 与端到端加密云备份。新手机无论运行 Android、iOS 或 HarmonyOS，都通过账号登录和规范化快照恢复接管数据，不设计多个终端长期并发写入。

### 0.1 本地多平台工作区与权威边界

本地开发目录固定为 `Project/solum/android`、`Project/solum/harmony` 与预留的 `Project/solum/ios`。`android` 是当前 Gitea `solum` 主仓，除 Android 壳外还承载共享 Rust core、唯一服务端实现、平台无关导出/备份协议及本文档；`harmony` 是独立 ArkTS 仓库；`ios` 尚无实现。父级 `Project/solum` 只组织平台目录，不是合并后的 Git 仓库。

跨平台契约只在本主仓定义版本和固定夹具，各平台仓实现自己的存储、权限、后台与发布适配。目录相邻不改变仓库边界：跨仓修改必须分别审查、测试、提交，且不得覆盖另一仓已有未提交工作。HarmonyOS 当前主仓相对路径为 `../android`；历史文档中的旧目录名只保留当时语境，不应继续用于脚本或当前协作说明。

## 1. 需求汇总

### 1.1 功能性需求（原始 8 条 + 补充）

| # | 能力 | 说明 |
|---|------|------|
| F1 | 自然语言事件摄取 → 日程 | 用户随口说一件事(如学校通知)，agent 抽取结构化事件，自动排入日程 |
| F2 | 重要度分级 + 分级提前通知 | 考试类提前 3 天、会议类提前 30 分钟、课程类提前 30 分钟；策略需可配置/可学习 |
| F3 | 主动习惯学习 + 定时问询 | 主动问"现在在干嘛"，识别护肤等固定步骤并主动提醒 |
| F4 | 行为日志自动生成 | 上述交互沉淀为结构化日志，供后续推理使用 |
| F5 | 穿戴设备接入 | 手表/手环数据(心率、睡眠、步数)作为状态感知输入 |
| F6 | 云端推理 + 本地明文权威 | 云端只处理本次任务所需的最小明文上下文；完整原始数据不以服务端可读形态落云，F17 只允许端侧加密、服务端不可读的备份密文 |
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
| F17 *(2026-08-22 重定义)* | 端到端加密云备份与换机恢复 | 活跃移动端定期生成完整、版本化、平台无关的加密快照；服务器只存密文，可恢复到 Android、iOS 或 HarmonyOS。原“手机/电脑多设备双向同步”退出目标架构。 |
| F18 *(补充)* | 交互式生成（Generative UI） | agent 回应可携带动态生成的可交互 UI（卡片/表单/按钮），用户操作回流给 agent 形成闭环；操作就地发生在对话流里，见 §3.9 |
| F19 *(补充，Phase 11 第一、二步 ✅)* | 持久化自定义组件（Custom Widget） | 用户可用自然语言生成持久存在的功能小组件，确认预览后保存；固定「工作台 → 组件」二级入口提供组件记录的离线增删改查，并可追加可空字段。它与一次性对话内 GenUI（F18，对话即焚）分属两套渲染器。定义、字段与记录自 schema v13 起参与当前旧多设备同步；该实现只属迁移态，目标是随其余可移植数据进入新 F17 加密备份。 |
| F20 *(补充，✅ Phase 10 完成)* | 通知智能管线（Notification Intelligence） | F1 升级：默认空的 App 白名单在 Android 捕获端总阀 → `dataSync` 前台处理服务守住 inbox → 双车道（规则命中的重要即时 / 15–30 分钟普通批量）→ 本地优先、只对不确定项做受「通知上云」门控的 LLM 分诊 → LLM 待确认过滤提议 → F12 可见回看、恢复/提升。LLM 不产 id、不执行既有记录动作，`LLM_ACTIONS` 不拓宽。 |
| F21 *(历史实现，Phase 12 首项已完成)* | 邮箱连接器 | 当前代码可手动连接 QQ、Gmail、Microsoft 365 / Outlook 及自定义 IMAP/SMTP，在本地查看、搜索、撰写并经 `sensitive` Guard 发送。2026-08-22 移动路线覆盖此方向：不再扩张完整邮件客户端，后续只建设高价值事件提取；现有桌面 loopback OAuth 必须经过移动原生回调与安全存储改造后才能算 Android 能力。 |

### 1.2 非功能性需求

- **隐私优先**：原始行为日志/聊天记录/人格数据的明文权威只在本地；云端 AI 只经手完成单次任务所需的最小上下文，备份服务只接收端侧加密且无法解开的密文。
- **通知可靠性**：提醒是这个产品的信任基础，绝不能"因为 LLM 慢/挂了就不响"。
- **安全护栏是架构级的**：高危操作拦截不能靠 prompt 约束 LLM，必须是代码层面的强制关卡。
- **可扩展**：功能以"能力插件"形式接入，方便后续加穿戴设备、加新的通知渠道。
- **可恢复性与可移植性**：活跃手机的本地数据存储仍是唯一业务权威；云端备份不能成为在线依赖。规范化快照必须与 SQLite、Core Data、relationalStore 等具体实现解耦，支持未来跨 Android/iOS/HarmonyOS 恢复；坏包、旧包或服务端故障不得覆盖最后一个可验证版本。

## 2. 总体架构

```
┌───────────────────────────────────────────────────────────┐
│ Mobile Client（Android first；未来 iOS / HarmonyOS）             │
│ Chat / 日程 / 通知 / 人格 / 主动度 / 信息源与权限管理               │
└───────────────────────────┬─────────────────────────────────┘
                            │ Tauri IPC / 本地 HTTP
┌───────────────────────────▼─────────────────────────────────┐
│ Agent Orchestrator（活跃手机本地运行，核心 + 平台适配层）           │
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
                    │ 版本化逻辑快照 → 压缩 → 端到端加密
              ┌─────▼──────────┐
              │ Solum Cloud     │  账号 / AI / 加密备份 / 恢复信封
              │ PostgreSQL      │  只保存备份密文，不持有解密主密钥
              └────────────────┘
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
- **F9 导入管道强制纯本地（2026-07-07 决定）**：聊天记录的解析、人格特征提取全程不经过云端 LLM——记录里包含对话另一方的内容，对方未同意上云。提取用本地规则/统计（高频词、语气助词、句长分布、常用表达），产出仅作人格"初稿"供用户手动修改确认。当前发布实现中，原始导入文件不进旧同步，只同步用户确认后的人格版本；目标备份是否包含完整对话以用户确认后的新隐私政策与 F17 规范为准，不能把“可加密备份”混同为“可交给 LLM”。
- 输出：`persona_profile_vN.json`，包含可读的特征摘要（不是黑盒向量），用户能看懂自己"人格画像"里写了什么。
- **版本化**：每次重大更新生成新版本，旧版本保留可回滚，防止一次错误的聊天记录导入污染整个人格。
- 人格画像的当前权威落在本地 SQLite；迁移期随旧 Local Memory 同步链路加密传播，目标架构改为纳入版本化加密备份，不单独建立服务端明文画像。
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
  8. **统一账号身份（2026-08-12，由账号代理扩展；2026-08-14 会话加固）**：完整产品的云端唯一通路是登录统一 `server/`（solum-cloud）后请求 `{server}/v1/ai/chat/completions`，第三方 API Key 只存在服务端环境变量。`solum-core::account` 会话落 `solum-account.json`（gitignored，密码只用于登录请求与本地恢复信封解密、不落盘），访问令牌 15 分钟过期；PostgreSQL 对每个受保护请求重新检查账号 active 状态，因此管理员禁用账号后无需等待 token 自然过期。401 时客户端刷新并轮换 refresh token；后继 token 由服务端 HMAC 密钥和当前 token 哈希确定性派生，数据库仍只存哈希。服务端提交后若响应丢失，上一代 token 在 5 秒 grace 内可幂等取回同一个后继；跨代重放撤销整族，桌面和 Android 只对传输/JSON 异常重试一次。该访问令牌同时是 §3.8 同源同步接口的租户身份证明：服务端验签后只以令牌 `sub` 选择租户，拒绝客户端自报用户名。**身份关联不等于解密权**：同步主密钥随机生成并只在客户端解封/保存，账号服务与同步服务均拿不到同步明文。无账号会话时壳层不得安装直连 API Key reasoner，也不得从旧 token 配置启动同步或福利告警；已有 `solum-llm.json` 仅保留为迁移材料，不再构成访客云端旁路。退出登录后回到纯本地 guest profile。
  9. **账号默认模型与受控选择（2026-08-13）**：登录用户无需另配第三方密钥即可使用中心提供的默认模型。模型目录由中心的 `SOLUM_DEFAULT_MODEL` 与 `SOLUM_ALLOWED_MODELS` 决定；默认模型必须始终包含在目录中且不可被用户删除。客户端只能选择目录内的模型标识，不接受用户提交任意上游 URL 或 API Key。请求的非默认模型在上游响应尚未提交时失败，中心会再尝试一次默认模型，并在响应头和用量事件中同时记录 requested/effective model、回退标志与原因；默认模型自身失败则直接返回错误。流式响应一旦写出响应头或首个字节就不得重启另一条流，避免把两个模型的输出拼接成一条回复。
  10. **订阅权益与用量权威（2026-08-13）**：调用链固定为 `客户端 → Solum Center → 上游 Agent/LLM → Solum Center → 客户端`，回程不能绕过中心，否则无法可靠完成鉴权、额度控制、回退、流式转发与统计。中心在联系上游前，以 `(tenant_id, period_start)` 为边界在 PostgreSQL 事务内原子预留一次请求额度；并发请求不能超额。一次已预留的上游尝试无论成功、失败或发生回退都计入请求次数，同时单独记录成功/失败、回退次数、请求/实际模型、上游延迟和可获得的 token 数。`GET /v1/ai/models` 返回中心允许的模型与默认项，`GET /v1/ai/usage` 返回当前套餐、周期上限、已用/剩余和重置时间；客户端展示这些服务端权威值，不在本机自行计数。当前只交付 entitlement/quota 基础，不等同于已接入支付渠道；以后支付回调只能更新权益表，不能直接改用量事件。
  11. **代理资源与中断边界（solum-cloud 0.6.1，2026-08-13）**：流式上游先读取首块再向客户端提交 200；非默认模型若在首块前返回空流或断开，仍只回退一次默认模型。响应开始后逐块转发并等待 Node `drain`，慢客户端不会无限堆积发送缓冲；客户端断开会取消仍在执行的上游请求。非流式响应由 `SOLUM_AI_MAX_RESPONSE_BYTES` 约束，`stream` / `temperature` / `max_tokens` 严格校验，不把歧义值静默透传。进程崩溃遗留的 `pending` 用量事件按 `SOLUM_AI_PENDING_TIMEOUT_SECONDS` 在该租户下一次读取/调用时幂等结算为 `interrupted`；恢复继续使用租户事务和强制 RLS，不在启动或后台面向全部账号做线性扫描。

### 3.7 Wearable Adapter

- 抽象接口，具体实现按设备类型（Apple Health / 小米运动 / 华为运动健康等开放 API 或蓝牙直连）适配。
- 只产出结构化状态事件（心率异常、久坐提醒、睡眠质量）喂给 Orchestrator，不直接触发用户可见动作——是否提醒、怎么提醒仍由 Proactivity Scheduler + Importance Classifier 决定。
- **接入路线（2026-07-12 决定，Phase 4 F5 v1）**：用户当前设备是三星运动健康，后续可能再接小米等平台。不走各厂商私有 SDK——三星健康 Data SDK 已改为合作伙伴审批制（2025-07-31 起，旧版 Android SDK 废弃），个人开发者拿不到批准；改走 **Android 官方 Health Connect**（`androidx.health.connect:connect-client`，稳定版 1.1.0）：三星健康自 2022-10 起就把心率/步数/睡眠数据同步写入 Health Connect，我们只需请求 Health Connect 的读权限，不关心数据最初来自哪个厂商 app——这也是"后续接小米"时唯一不用改适配层代码的路径（前提是对方 app 也同步进 Health Connect；国内版小米运动健康是否同步未验证，届时重新评估）。
  - v1 范围严格限定为 F5 本身：只读三类数据（心率/步数/睡眠时长），落本地存储，**不写回、不接 F11 情绪感知/主动关怀、不接 F13 场景切换**——那是需要另外评估触发策略的后续工作，架构上先把"数据能进得来"这层打通。
  - Health Connect 权限是特殊类别（`android.permission.health.*`），不是标准运行时权限弹窗，走 Health Connect 自己的授权页（`PermissionController` 签发的 Intent）；只声明 READ 三项，不声明任何 WRITE 权限（v1 只读，不写健康数据）。
  - 读取实现必须遍历 Health Connect 的分页结果；步数使用时间窗聚合值而不是累加原始 `StepsRecord`，避免多来源/重叠区间重复计数。只有样本成功写入本地 SQLite 后才推进拉取水位，失败会在下一轮重试。
  - *（实现见 §6 Phase 4 与 CHANGELOG：新 crate `solum-health-connect`，Tauri Android 插件，结构与 `solum-notif-access` 一致。）*

### 3.8 端到端加密云备份与换机恢复（F17，取代多设备同步）

> **2026-08-22 覆盖性决定**：下方 `/v1/push|pull`、oplog、LWW、设备游标等内容记录的是当前已部署历史实现，只为安全迁移保留，不再是目标架构。新功能不得继续扩建多主同步；先完成可验证备份/恢复，再按 TODO 顺序退役旧链路。

- **单写者模型**：只有当前活跃手机写业务数据，不再解决电脑/手机或多台设备并发修改。换机是“旧手机最后备份 → 新手机恢复并接管”，操作系统可以不同，但不是两个设备长期最终一致。
- **备份内容**：所有可移植用户数据默认纳入平台无关的逻辑快照，包括对话、日程、提醒、记忆、人格、行为、通知处理结果、健康、组件与 Agent 设置。各系统权限、硬件密钥句柄、系统闹钟注册、失效 OAuth 会话和本机路径不是可移植数据，只保存恢复意图并在目标系统重新授权/绑定。
- **加密格式**：客户端在本地生成带 schema/protocol 版本的规范化完整快照，覆盖全部可移植用户数据与提供商允许迁移的凭据，不能直接以任一平台数据库文件作为协议；压缩后使用随机主密钥进行 AEAD 加密，AAD 至少绑定用户 UUID、设备代次、备份版本、创建时间和格式版本。服务端不接触快照明文或主密钥；外部凭据恢复后仍要验证是否已过期或撤销。
- **恢复安全**：登录仅决定可访问哪个租户；密码派生包装密钥只在新手机本地解开恢复信封。下载后必须先验认证标签、哈希、容量和迁移兼容性，在临时库完成导入/完整性检查后原子切换；本机已有数据时禁止静默覆盖。
- **保留策略与接管**：服务端保存多个完整版本而非不可重建的无限增量历史；上传采用幂等、原子提交，失败或半包不能替换最近可恢复版本。账号维护单调递增的 active generation，新手机经用户确认接管后提升代次，旧手机后续上传必须被拒绝。容量、单包大小、版本数量、用户删除和灾备恢复都必须有可测试的明确规则。

#### 备份数据覆盖清单（2026-08-23 审计基线）

“存在于某个本地文件”不等于“应原样复制到新手机”。本清单按**可移植数据**、**仅恢复意图**、
**永不进入普通备份载荷**三类逐源定案；其中“待补”表示数据属于备份目标，但当前 `solum-export` v2
还不能安全、幂等地恢复。任何新增 SQLite 表或新的持久化文件/浏览器存储，都必须先加入该清单并选择
恢复语义；core 测试会把未分类的新 SQLite 表视为失败。只有所有“待补”项都有稳定跨平台 schema、
尺寸规则和恢复测试后，才可把 TODO 的“完整逻辑载荷”划完。

| 持久化源 | 分类 | 当前状态与恢复语义 |
|---|---|---|
| SQLite `raw_inputs`、`events`、`notifications`、`behavior_log`、`suggestions`、`persona_versions`、`health_samples`、`memory_facts`、`routines`、`soulous_facts`、`widget_defs` / `widget_fields` / `widget_records` | 可移植 | 已在 `_restore` 中以稳定 guid/FK 形状可幂等恢复；可读层只是副本，不作为恢复输入。 |
| SQLite `meta.rule_table`、`meta.proactivity`、`meta.persona_active` | 可移植 | 已在 `_restore.meta` 中恢复；`persona_active` 以人格版本 guid 翻译，不能复制设备本地行号。 |
| SQLite `notification_captures`、`notification_filter_proposals` | 可移植 | v3 已生成/验证稳定内容身份，并把 capture 的 raw input / event 引用转为 guid；平台恢复事务尚未落库。 |
| SQLite `notification_action_proposals` | 可移植历史 | v3 保存源状态并把 `pending` 的恢复状态强制为 `stale`；平台恢复事务尚未落库。 |
| SQLite `audit_log` | 可移植 | v3 已生成/验证稳定内容身份；后续事务只能按 id append 缺失项，仍不得 UPDATE/DELETE。 |
| WebView `pa.chat-sessions.v1[.<profile>]` | 可移植 | Android v3 适配器强制落盘当前输入并提交同 profile 的有界一致切片；`contextTurns` 明确不作权威。现有 v2 手动导入恢复能力未被替换。 |
| `solum.ui-state.v1[.<profile>].chatDrafts` | 可移植 | Android v3 适配器只投影引用现存 session 的草稿；last view、滚动位置等 UI 状态不迁移。 |
| IndexedDB `pa-local-documents-v1[.<profile>]/documents` | 可移植附件，待分块 | Android 适配器已用单个 readonly transaction 读取 manifest 并按 Blob 原始字节计算 SHA-256；blob 仍需独立分块密文，不能塞入当前 64 MiB 容器或以内存 base64 冒充完成。 |
| `notif-spool/*.json`、`notif-spool/failed/*.json` 与升级遗留 notification inbox | 可移植 | Android 适配器在 Orchestrator 锁内冻结完整文件名集合、限制预读尺寸并只读提交；恢复仍只进待处理区，快照不消费源文件。 |
| LLM 的非秘密 provider/base URL/model 选择 | 可移植 Agent 设置 | Android 配置适配器已严格读取并只投影非秘密字段；存在但损坏的配置使整次快照失败，`api_key` 不在目标类型中。 |
| `notification_intelligence.filter_rules` 与批处理间隔 | 可移植设置 | v3 builder 已从 Store 拆出规则/间隔，并把 allowed/auto-event package 只放恢复意图，避免顺带授予权限。 |
| `notif_cloud`、通知 app 白名单、auto-event app 白名单、邮箱/Soulous 连接、健康数据源与后台提醒 | 仅恢复意图 | Android v3 适配器已投影非秘密事实；新设备默认仍关闭，核对已安装 app 与提供商后由用户重新授权，OS 权限和系统闹钟依据已恢复日程重新注册。 |
| 邮箱/Soulous 的非秘密账户标识与 provider 配置 | 条件可移植，待提供商审计 | 只有提供商条款允许且新隐私政策明确覆盖时才进入版本化 credential 区；否则只恢复重新连接意图。恢复后必须验证凭据是否过期或撤销。 |
| `meta.device_id` / HLC / overflow 与 drop 计数、`sync_oplog`、`sync_quarantine`、`sync_bad_blobs`、`sync_push_holds`、`capture_loss_receipts`、overflow marker、Android alert cursor | 永不备份 | 设备身份、迁移态传输队列、游标和故障收据不是用户业务状态；新设备重新建立，不能重放旧同步内部状态。 |
| `notif-policy.json` | 永不备份 | 由已确认的通知配置投影给原生 listener，恢复设置并重新授权后再生成，不能作为第二份权威。 |
| `widget_schema_rejections` | 永不备份 | 仅用于本机产品能力诊断，既不影响用户组件，也不作为恢复输入。 |
| 账号 access/refresh token 与密码、备份主密钥/恢复码/包装密钥、旧同步 key/token、LLM API key、邮箱密码/OAuth token、Soulous token | 永不进入普通逻辑载荷 | 密钥不得被自己加密，账号会话不得克隆。将来若某提供商明确允许凭据迁移，也只能进入独立、版本化、可撤销的 credential 区，不能混入通用 Agent 设置。 |
| 隐私同意记录、OS 权限、硬件密钥句柄、系统闹钟镜像、本机路径、UI 导航/滚动、guest 选择 | 永不备份 | 都绑定设备、安装或当次法律文本；新设备重新同意、授权、注册或选择。 |

#### `solum-export` v3 逻辑载荷契约（core 阶段）

v3 保留 v2 的全部可读层与 `_restore`，新增必填 `_portable`；`_restore` 仍只负责已经有 guid 的 SQLite
领域行，`_portable` 负责平台存储、无 guid 的本地台账、非秘密 Agent 设置和恢复意图。v3 builder 必须
由平台适配器显式提交当前 profile 的 WebView/IndexedDB/配置快照，core 不得把“适配器没传”猜成
“用户没有数据”。现有只持有 `Store` 的 `build_export` 继续写 v2；新 builder 才写 v3。

`_portable` 固定为 `{format:"solum-portable", version:1, ...}`，拒绝未知字段；下列数组即使为空也必须
出现。SQLite 台账时间沿用领域层无时区墙钟 `YYYY-MM-DDTHH:MM:SS`（不能把历史本地时间伪标成 UTC），
WebView 会话/待入库通知时间使用带 offset 的 RFC 3339；SHA-256 与 v3 内容身份均为 64 个小写 hex
字符，capture 原有的非安全去重 `content_hash` 仍是 16 个小写 hex（不得把它误当完整性摘要）：

| 字段 | v1 形状与语义 |
|---|---|
| `notification_journal.captures` | 通知处理结果；以 `raw_input_guid` / 可选 `event_guid` 关联 `_restore`，不复制 SQLite id。同时保存 `source_state` 与 `restore_state`，源 `queued` 恢复为 `needs_review`，绝不静默重新分诊。 |
| `notification_journal.filter_proposals` | 过滤提议及状态；稳定 id 不含可变 `state`。 |
| `notification_journal.action_proposals` | 动作提议历史；以 capture 稳定 id 与可选 `event_guid` 关联（目标已删除或旧行没有 guid 时允许为空）。源 `pending` 的 `restore_state` 固定为 `stale`，换机后不能继续执行旧确认卡。 |
| `audit_log` | append-only 审计；稳定 id 覆盖整条不可变内容。恢复只能按 id 追加缺失项，禁止 UPDATE/DELETE。 |
| `chat.sessions` / `chat.drafts` | 最多 40 个 session、每个最多 240 条消息、标题 80 字、消息/草稿 12,000 字、聊天总字符预算 2,400,000；消息 role 仅 `user/assistant`，不保存可重建的 `contextTurns`。 |
| `documents` | 最多 30 条 manifest；每项含平台中立 id、名称、media type、明文字节数与 SHA-256，单件最多 20 MiB、合计最多 600 MiB。blob 不 base64 塞入主 JSON；分块密文协议落地前，manifest 只能用于计划与完整性核对，不能宣称附件已恢复。 |
| `pending_notifications` | 尚未入 SQLite 的 spool/failed 原始通知；恢复只回到待人工/本地规则处理区，不直接送 LLM 或执行动作。 |
| `agent_settings` | LLM 仅保存 base URL、model、temperature、max tokens、timeout；通知仅保存 filter rules 与 batch interval。API key 与通知 app 授权不在此对象。 |
| `restore_intents` | 保存“此前开启/连接”的非秘密事实：通知上云、capture/auto-event package、邮箱账户摘要、Soulous endpoint、健康来源和后台提醒；目标设备默认仍关闭，重新授权后才应用。 |

无 guid 行的稳定 id 按以下跨语言规则生成：

```text
id = lowercase_hex(SHA-256(
  UTF8("solum-export:v3:" || domain) || 0x00 || canonical_identity_json ||
  0x00 || ASCII(decimal_duplicate_ordinal)
))
```

`canonical_identity_json` 采用 backup-v1 相同的递归对象键排序/紧凑 UTF-8 规则；ordinal 从 0 开始，
只在相同 domain + identity 的行之间按源端稳定顺序递增。capture 的 identity 是
`raw_input_guid`；filter/action 排除可变状态；audit 覆盖全部不可变字段；pending notification 排除
队列重试次数。读取端必须重算并拒绝不匹配 id、重复 id、悬空 capture/event/raw-input 引用、超限数组、
重复 session/document id、非法 role/digest/time/URL。AEAD 认证并不替代这些领域校验：认证只能证明包未被
服务端篡改，不能证明一个有 bug 的客户端写出了可恢复形状。`raw_input_guid` 必须指向 `_restore`；
可选 event guid 允许指向已删除的历史目标，不作为恢复动作授权。

`plan_import` 可同时验证 v2/v3，并把 `_portable` 各域计数加入预览；旧 `import_document` 只实现 v2
Store 合并，对 v3 必须在任何写入前返回“需要平台恢复事务”，禁止只恢复 `_restore` 后报成功。后续
Android 临时库事务消费同一个已验证计划，并在 SQLite、会话存储、附件与设置全部成功后原子切换；
在那之前完整逻辑载荷 TODO 仍保持未完成。

#### Android v3 主载荷快照适配 v1（2026-08-23）

本阶段只把 Android 当前 profile 的平台存储接入 `solum-export` v3 主 JSON；不生成附件分块密文，
不上传服务端，也不实现恢复写入。WebView 调用 Rust 时提交严格的
`{profile_id, chat, documents}`：`chat` 直接使用上节的 sessions/drafts 形状；`documents` 只含从一个
IndexedDB readonly transaction 读出的 manifest。每个 blob 必须按原始字节计算 SHA-256，且记录的
`size` 必须等于 `Blob.size`；blob 本身不得 base64 或经 IPC 塞进主 JSON。

快照顺序与失败语义固定如下：

1. WebView 在同一事件循环内先强制保存当前输入草稿和会话，再深拷贝聊天切片；随后用单个 readonly
   transaction 读取当前 profile 的 `pa-local-documents-v1[.<uuid>]/documents`，逐件校验并摘要。
2. IPC 必须携带 `app_info.local_profile_id`。Rust 在采集前后都重新读取 active profile；任一端不是同一
   个 `guest`/小写 UUID，或期间发生登录、退出、切号，就拒绝整次导出，不能把两个账号拼进一个包。
3. Rust 持有 Orchestrator 锁时读取 SQLite 与通知 spool，因此 resident ticker 不能同时把同一条通知从
   spool 搬进 SQLite。原生 listener 只以 `.tmp` + fsync + atomic rename 发布 `.json`；适配器先冻结并
   排序一次文件名集合，只读其中完整文件。集合冻结后才发布的新文件属于下一份快照，不删除、不改名、
   不消费；`failed/` 与升级遗留 inbox 同样纳入，恢复标为待处理而不是直接分诊。
4. LLM、邮箱与 Soulous 从同 profile 的原子 JSON 配置读取；文件存在但损坏必须失败，不能伪装成“未配置”。
   只投影 LLM 非秘密参数、邮箱 id/label/provider/address、Soulous endpoint/出站选择；API key、邮箱密码、
   OAuth/token、旧同步凭据、本机路径和 key tail 都不得进入 IPC 或载荷。Health Connect 与后台提醒只记录
   “此前可用且已授权/启用”的恢复意图，新设备仍需重新授权。
5. 适配器在读文件前限制数量与累计字节，再交给 core v3 上限/引用/时间/摘要校验；任一 IndexedDB 错误、
   spool 截断/字段错误、配置损坏、profile 漂移或 core 校验失败，都不写导出文件。错误只指出来源类别与
   文件名，不回显通知正文、凭据或完整个人数据。

这个切片只解决 v3 **主逻辑载荷的 Android 采集**。manifest 能证明资料在采集时的身份和尺寸，但没有
blob 分块密文就不能恢复资料；没有临时库事务也不能导入 v3。因此顶层 F17 仍未完成，现有 v2 Store-only
builder/固定夹具与旧同步迁移态都继续保留。当前产品里的手动“导出/导入”也继续使用可恢复的 v2 配对；
v3 IPC 命令先作为备份协调器的内部 seam，不能在恢复事务完成前替换按钮并制造不可恢复的用户文件。

#### `solum-backup` protocol v1（F17 加密备份地基，2026-08-23）

本节只定义**平台无关容器和端侧 codec**，不是完整云备份闭环。protocol v1 可认证承载
`solum-export` v2 或 v3：v2 是现有 Store-only 导出，v3 增加 `_portable` 逻辑载荷与纯恢复计划；固定
v2 夹具继续逐字节兼容。本节自身不承诺附件、临时库或接管；它们已由下文后续切片分别补齐。整个产品仍不能
声称“全量备份”或“换机恢复”已经完成：恢复码包装、受保护删除、600 MiB 真机压力、生产对象存储/灾备及
Android↔iOS↔HarmonyOS 互通仍未验收。

**算法与密钥边界**：调用方传入恰好 32 字节的随机主密钥。codec 先把内层 JSON 规范化为紧凑
UTF-8 字节，再用 RFC 1950 zlib 容器内的 DEFLATE、固定 level 6 压缩；AEAD 明文为
`SHA-256(规范化 JSON) || zlib stream`，最后用 XChaCha20-Poly1305 加密（24 字节 nonce、16 字节
tag）。摘要位于加密体内，服务端看不到，也不能用它判断两份备份是否包含相同内容。Rust 实现使用 `flate2` 的纯 Rust
`miniz_oxide` 后端，不需要 C/汇编或系统 zlib，满足 Android 交叉编译约束。生产 nonce 必须由 OS
CSPRNG 每包随机生成；仅仓库内公开测试夹具可注入固定 nonce。密钥、明文和未脱敏完整数据不得写入
日志、错误或夹具；服务端只需把整个容器作为不透明字节保存，不持有主密钥，因而不能解开正文。

**二进制编码（所有多字节整数均为 unsigned big-endian）**：

| 偏移 | 长度 | 内容 |
|---|---:|---|
| 0 | 8 | ASCII magic `SOLUMBAK`（hex `53 4f 4c 55 4d 42 41 4b`） |
| 8 | 4 | `header_len`，JSON 头的字节数，范围 `1..=16384` |
| 12 | `header_len` | UTF-8、无 BOM、紧凑 JSON 头；v1 拒绝未知字段 |
| 12 + `header_len` | 余下全部 | 原始 AEAD ciphertext，末尾 16 字节是 Poly1305 tag |

Rust v1 写入器按下表顺序、无空白序列化 JSON 头；读取器允许字段顺序不同，因为认证使用的是容器中
**原始头字节**而不是重新序列化结果。字符串均按 JSON/UTF-8 编码，整数不得为负数或浮点数：

| 字段 | v1 规则 |
|---|---|
| `format` | 固定 `solum-backup` |
| `protocol_version` | 整数 `1` |
| `user_id` | 小写、带连字符的 UUID；绑定账号，不是用户名 |
| `device_generation` | `u64` 且大于 0 |
| `backup_id` | 每包新建的小写、带连字符 UUID |
| `created_at` | UTC RFC 3339，精确到毫秒，固定形状 `YYYY-MM-DDTHH:MM:SS.sssZ` |
| `payload_format` | 固定 `solum-export` |
| `payload_version` | 整数 `2` 或 `3`；必须与解密后文档的 `version` 一致；其他值拒绝，不能降级猜测 |
| `payload_digest` | 固定 `sha256`；32 字节摘要位于 AEAD 明文开头，不出现在头中 |
| `compression` | 固定 `zlib-deflate`；写入 level 6，读取接受任何合法 RFC 1950/1951 流 |
| `encryption` | 固定 `xchacha20-poly1305` |
| `nonce` | 24 字节 nonce 的 48 个小写 hex 字符 |
| `plaintext_size` | 规范化内层 JSON 的精确字节数 |
| `compressed_size` | zlib 压缩流的精确字节数，不含 32 字节摘要 |
| `ciphertext_size` | 密文含 tag 的精确字节数，必须等于 `32 + compressed_size + 16` 和实际余长 |

**内层 JSON 规范化**：对象键在每一层按 Unicode 标量字典序排列，数组顺序保留，不写 BOM 或无意义
空白，字符串与数字使用 `serde_json` 的紧凑 JSON 表达。`Value` 输入与 JSON 字节输入都先解析并按此
规则重写，所以同一逻辑文档、同一 metadata/key/nonce 会产生固定夹具字节；其他平台只要能读取合法
JSON 即可解包，但写 fixture 时必须遵守该规范化规则。

**AAD 精确字节规则**：

```text
AAD = UTF8("solum-backup:aad:v1") || 0x00 || raw_header_bytes
```

`raw_header_bytes` 是上表中从偏移 12 开始、长度恰为 `header_len` 的原始字节。因此 AAD 确定性绑定
用户 UUID、device generation、backup id、创建时间、协议版本、内层格式及版本，同时也绑定算法、
nonce 和三个尺寸；任何一个头字段的有效值被改动都会在认证标签校验时失败。magic 与 `header_len`
由 framing 严格校验，不进入 AAD。

**硬尺寸上限**：头 `16 KiB`；ciphertext（含 tag）`64 MiB`；解压后规范化 JSON `256 MiB`；
容器总长不得超过 `12 + 16 KiB + 64 MiB`。读取器不得依赖头内自报尺寸分配无界内存：先由总长与
`header_len` 算出实际密文余长并执行 64 MiB 上限，解压则最多读取 `256 MiB + 1` 字节以识别压缩
炸弹。三个尺寸字段必须与实测完全一致；不能用调低上限或截断输出“修复”坏包。

**版本与错误分类**：v1 只接受上述精确格式、版本和算法；`protocol_version != 1` 分为
`unsupported_version`，任一算法标识未知分为 `unsupported_algorithm`，不能猜测或降级。其余失败
分为 `truncated`（前导/头/密文不完整）、`invalid_container`（magic、JSON、字段或尺寸矛盾）、
`size_limit`、`authentication`（错误 key、tag、密文或有效 AAD 字段被改）、`decompression` 和
`invalid_payload`（JSON 或 `solum-export`/`plan_import` 校验失败）。诊断只报告类别与非敏感字段名，
不得回显 key、nonce 以外的密钥材料、明文片段或完整个人数据。

**恢复前校验顺序（全部完成前不得写 Store）**：① 根据容器总长和 `header_len` 先限制实际密文；
② 校验 framing、头 JSON、版本、算法、字段形状及三个尺寸；③ 以原始头字节构造 AAD 并验证 AEAD
tag；④ 拆出加密体内的 32 字节摘要并核对压缩流长度；⑤ 受限解压，核对 `plaintext_size` 与
`SHA-256(规范化 JSON)`；⑥ 解析内层 JSON，交给现有 `plan_import`，并交叉核对内外
`payload_format/version`；v3 还要逐项验证 `_portable` 身份、引用与上限。codec 只在全部成功后返回
验证过的文档和导入计划，本身从不接收或写入 `Store`；后续恢复事务只能消费这个验证结果。仓库固定
夹具位于 `crates/solum-core/tests/fixtures/backup-v1/`，同时保留 v2 与 v3 的合成 payload、公开测试
key/nonce 和精确容器字节，供 Android/iOS/HarmonyOS 实现做逐字节互通测试。

#### `solum-attachment` protocol v1（F17 附件分块密文，2026-08-23）

`solum-export` v3 主载荷只保存资料 manifest；每个资料 blob 单独编码成一个 `solum-attachment` 容器。
读取端必须先解开并验证所属 `solum-backup` v1，取得 v3 manifest 与外层账号/代次/backup id，再把这些
已认证值作为 binding 传给附件 codec。附件不压缩（任意媒体通常已压缩，且这里不引入解压炸弹面），也
不把名称、media type、账号 UUID、backup id、attachment id 或 SHA-256 写入明文容器头。chunk framing
与 ciphertext 长度会暴露精确字节长度（协议不伪装尺寸隐藏），但服务端不能从容器判断资料名称、内容
摘要或把不同备份中的相同资料关联起来。

**binding 与子密钥**：调用方仍传入 backup-v1 的 32 字节主密钥。binding 按下列字段顺序编码为无空白
UTF-8 JSON；字符串使用 JSON 转义，整数是无符号十进制。`user_id` / `backup_id` 必须是小写 UUID，
`device_generation > 0`，`attachment_id` 必须是 1..128 字节的 ASCII 字母、数字或 `-._:`，摘要必须
是 64 个小写 hex，单件 `plaintext_size <= 20 MiB`。这些字节只在端侧内存中构造，不写入容器：

```text
{"user_id":...,"device_generation":...,"backup_id":...,"attachment_id":...,"plaintext_size":...,"plaintext_sha256":...}

salt = UTF8("solum-attachment:hkdf-salt:v1")
info = UTF8("solum-attachment:key:v1") || 0x00 || binding_bytes
attachment_key = HKDF-SHA256(master_key, salt, info, output_length=32)
```

每件资料派生独立 XChaCha20-Poly1305 子密钥；这不是密码派生或恢复信封，只是主密钥的协议域隔离。
摘要与尺寸必须来自已认证 v3 manifest，不能相信下载对象附带的服务端 metadata。`backup_id` 已由主容器
绑定创建时间和 payload version，因此附件通过该 id 传递绑定整份备份，不另复制一套可漂移时间字段。

**二进制 framing（所有整数均为 unsigned big-endian）**：

| 偏移/重复记录 | 长度 | 内容 |
|---|---:|---|
| 0 | 8 | ASCII magic `SOLUMATT`（hex `53 4f 4c 55 4d 41 54 54`） |
| 8 | 4 | `header_len`，范围 `1..=1024` |
| 12 | `header_len` | 顶层 UTF-8 紧凑 JSON 头，v1 拒绝未知字段 |
| 每个 chunk | 4 | `chunk_header_len`，范围 `1..=512` |
| 其后 | 4 | `ciphertext_len` |
| 其后 | `chunk_header_len` | chunk UTF-8 紧凑 JSON 头，v1 拒绝未知字段 |
| 其后 | `ciphertext_len` | XChaCha20-Poly1305 ciphertext，末尾 16 字节为 tag |

顶层写入字段顺序固定为 `format`、`protocol_version`、`kdf`、`encryption`、
`chunk_plaintext_size`、`chunk_count`：格式/版本固定 `solum-attachment` / `1`，算法固定
`hkdf-sha256` / `xchacha20-poly1305`。生产写入器固定 `chunk_plaintext_size=1048576`（1 MiB）；
读取器接受 `1..=1048576`，以便固定夹具用小块覆盖多块互通，但 `chunk_count <= 4096` 且必须精确等于
`max(1, ceil(binding.plaintext_size / chunk_plaintext_size))`。零字节资料仍有一个空明文 chunk，从而
拥有认证 tag，不存在“无 tag 的合法容器”。

每块头的写入字段顺序固定为 `index`、`plaintext_offset`、`plaintext_size`、`ciphertext_size`、
`nonce`。`index` 从 0 连续递增；offset 必须等于此前明文字节累计；除末块外明文长度必须等于顶层块长，
末块必须补齐 manifest 的精确总长；`ciphertext_size = plaintext_size + 16` 并等于 framing 中的实测值；
`nonce` 是 24 字节的 48 个小写 hex。同一附件内 nonce 不得重复。生产写入器为每块调用 OS CSPRNG；
只有公开测试夹具可注入固定 nonce 列表。

**每块 AAD 精确规则**：

```text
AAD = UTF8("solum-attachment:aad:v1") || 0x00 ||
      U32BE(len(binding_bytes)) || binding_bytes ||
      U32BE(len(raw_header_bytes)) || raw_header_bytes ||
      U32BE(len(raw_chunk_header_bytes)) || raw_chunk_header_bytes
```

因此每个 tag 同时认证账号、device generation、backup id、attachment id、manifest 明文尺寸/摘要、
协议/算法/块长/块数，以及本块序号、offset、尺寸和 nonce。顶层/块头使用容器中的**原始 JSON 字节**，
读取器不靠重序列化做认证；magic 和所有 framing 长度由结构校验覆盖。换 key、换 manifest binding、把
附件挂到另一备份、交换/复制/删除块、篡改头/密文/tag 都必须在返回明文前失败。

**上限与恢复顺序**：单件明文 20 MiB、最多 4096 块、顶层头 1 KiB、块头 512 B；容器硬上限按
`20 MiB + 4096 * (8 B framing + 512 B chunk header + 16 B tag) + 12 B + 1 KiB`
计算。读取器先限制实际容器总长，再校验 binding；随后解析顶层头并据 manifest 计算唯一合法块数，完整
扫描所有记录，限制每段长度、拒绝截断/尾随字节/乱序/offset 缺口/重复 nonce；结构全部通过后才派生
子密钥并逐块认证解密到私有有界缓冲，最后核对总字节数与 `SHA-256(plaintext)`。任一步失败都不返回
部分明文，平台恢复层只能把成功结果写入临时存储；跨多个附件的协调器还必须沿用 v3 的最多 30 件、
合计 600 MiB 上限，全部成功后才允许原子切换。

错误分为 `truncated`、`invalid_container`、`size_limit`、`unsupported_version`、
`unsupported_algorithm`、`authentication`、`digest_mismatch` 与 `random_source`；不得在错误或日志中
包含 key、binding JSON、附件名、明文片段或完整摘要。仓库固定夹具位于
`crates/solum-core/tests/fixtures/attachment-v1/`，使用公开标注的合成 bytes、测试 key 和固定 nonce，
故意采用 16 字节块覆盖多块 framing；生产 API 不暴露固定 nonce 入口。本阶段仍不包含服务端 API、
Android UI、临时库原子恢复、恢复码/信封或三端实机互通，顶层 F17 继续保持未完成。

#### v3 临时恢复镜像 core 契约 v1（2026-08-23）

这是 Android 原子恢复的第一段提交边界，不是 live profile 切换。调用方必须先用 `solum-backup` v1
认证并解出 `solum-export` v3，再用同一外层 binding 认证并解出全部 `solum-attachment` v1；core 接收
v3 文档与 `{attachment_id, plaintext}` 集合后，按以下固定顺序工作：

1. 调用 `plan_import` 重跑格式、版本、字段、上限、稳定 id 与引用校验，并反序列化同一份 `_portable`；
2. 要求附件集合与 `documents` manifest **精确相等**：不得缺件、多件或重复 id，逐件重算明文字节数与
   SHA-256，并再次执行 30 件 / 单件 20 MiB / 合计 600 MiB 上限；
3. 只允许写入由恢复协调器新建的空 Store。先检查所有用户域为空，再用一个最外层 SQLite 事务导入
   `_restore`，随后按 guid 将通知台账引用翻译为临时库 row id，append 审计历史，并恢复非权限设置；
4. `queued` capture 只能按 v3 已验证的 `restore_state=needs_review` 落库，旧 `pending` 动作只能按
   `restore_state=stale` 落库；`notif_cloud`、通知白名单、auto-event 白名单和外部连接只进入恢复意图，
   不得在新设备静默授权；
5. 全部 SQLite 写入成功后才返回平台交接包（会话/草稿、资料明文、待处理通知、非秘密 LLM 设置和
   重新授权意图）。任一 SQL、guid 引用、附件摘要或设置错误回滚整个事务，不返回部分交接包。

错误必须只说明失败类别/域/id，不包含密钥、附件明文、通知正文、聊天正文或完整个人数据。该 API
不合并已有 Store，也不删除、覆盖或重命名 live profile；现有数据场景须由后续 UI 预览并明确选择。
平台层下一步必须把临时 SQLite、WebView inactive namespace、附件与 spool 一并持久化，完成哈希/行数
复核后以单一 durable generation 指针发布；在该指针与崩溃恢复状态机完成前，不能把本阶段称作
“原子恢复完成”或把现有 v2 导入入口替换为 v3。

#### Android profile generation 发布协议 v1（2026-08-23）

每个 `guest` 或账号 UUID profile 根目录拥有独立 generation 命名空间。没有
`active-generation.json` 时继续使用历史 profile 根目录，保证升级不会搬动或遗失现有数据；指针存在后，
数据面路径固定为 `generations/<32-lower-hex>/`。generation id 由端侧 OS CSPRNG 生成，不接受调用方路径；
指针与 manifest 均为 UTF-8 紧凑 JSON、拒绝未知字段和符号链接。

`restore-staging/<id>/` 只写尚未发布的候选镜像。seal 前必须关闭临时 Store、完成 WAL checkpoint，且目录中
必须有独立 `solum.sqlite`，不得残留 `solum.sqlite-wal` / `solum.sqlite-shm`。seal 递归登记全部普通文件的
相对 POSIX 路径、字节数与 SHA-256，限制文件数与总字节，拒绝绝对路径、`..`、链接、重复路径和未登记
文件；manifest 自身不计入 artifact。publish 必须重算精确文件集合/尺寸/摘要，随后按以下顺序执行：

指针上限 4 KiB，字段精确为 `{format:"solum-profile-generation",version:1,generation}`；manifest 上限
2 MiB，字段精确为 `{format:"solum-profile-image",version:1,generation,artifacts}`，每个 artifact 精确为
`{path,size,sha256}`。artifact 最多 12,000 个、普通文件合计最多 1 GiB；`path` 使用 `/` 分隔的非空
UTF-8 相对路径，`sha256` 是 64 位小写 hex。generation id、manifest generation 与目录名必须完全一致。

1. 将 sealed staging 目录原子 rename 为同一 profile 文件系统中的 `generations/<id>/`；
2. fsync generation 与 staging 两个父目录，使新目录项及旧目录项删除均先持久化；
3. 用同目录临时文件写入并 fsync `active-generation.json`，再原子 rename 并 fsync profile 根目录；
4. 仅在指针提交成功后返回发布成功；旧 generation 不在此事务删除。

因此进程在 seal/rename 前崩溃只留下 staging，在目录 rename 后、指针提交前崩溃只留下未引用 generation，
两者启动时都被忽略，旧数据仍是唯一 active；指针提交后新 generation 已完整且可验证。启动时指针缺失
明确选择 legacy，指针损坏、id 非法、目标/manifest/SQLite 缺失则**停止启动且不回退 legacy/空库**，
避免把损坏伪装成“数据没了”。清理 staging、未引用 generation 与旧 generation 是后续显式维护动作，
不得在启动解析中顺手删除。

generation 只接管用户数据面：SQLite、通知 spool/policy，以及后续的附件和 WebView inactive namespace。
设备级账号会话、隐私同意、主密钥/旧同步 key、LLM API key、邮箱/OAuth/Soulous 凭据仍留在 profile 根，
不进入 generation，不因恢复被覆盖。Android 原生 listener 必须冻结一次“账号 profile 根 → active
generation”解析结果，再用同一目录读 policy、写 spool/overflow；指针非法时 fail closed，不得退回根目录。
profile 根固定 `active-generation.lock` 不随 generation rename：Kotlin `FileChannel.lock()` 持有 POSIX 写锁并
覆盖 pointer 解析、policy 读取和 spool 原子发布；Android Rust 使用与其同进程也会冲突的 Linux OFD 锁，
桌面测试路径使用平台文件锁。最终恢复协调器还必须在该锁内停止 intake、把锁前已完成的旧代 late spool
纳入新代或明确留在设备级待处理区，再提交 pointer；仅有锁能禁止“写到切换一半”，不能自动搬运锁前事件。
本阶段只建立文件系统发布/启动解析地基；WebView inactive namespace 的 prepare/ack、附件实际落盘、恢复
协调命令、已有数据选择与 live 连接重启尚未接通，因此顶层原子恢复仍未完成。

#### Portable generation staging image v1（2026-08-23）

恢复协调器在调用 generation publish 前，必须先把 core 已验证的 `PreparedPortableRestore` 一次性组装为
**sealed、仍未激活**的 generation。组装 API 自己创建随机 staging id 和全新 `solum.sqlite`，在该 Store 的
单一恢复事务成功后才取得 `PlatformRestoreBundle`；调用方不能把任意未校验 JSON 或附件路径塞进镜像。
固定布局如下，所有路径都位于 `restore-staging/<generation>/`：

```text
solum.sqlite
notif-policy.json
notif-spool/restore-<stable-id>.json
notif-spool/failed/restore-<stable-id>.json
webview/bootstrap-v1.json
webview/documents/<sha256(document-id)>.blob
generation-manifest.json                 # seal 最后生成
```

`notif-policy.json` 必须精确写 `{\"allowed_packages\":[]}`，恢复包中的 app 白名单与 auto-event 白名单只保留
为重新授权意图，不得借恢复重新开启原生通知正文采集。pending notification 按 `received_at` 转 epoch
milliseconds，单文件为 listener/ticker 现用的 `{ts,pkg,title,text}` 紧凑 JSON 加换行；源 `failed=true`
直接进入 `failed/`，其余进入待处理目录。文件名只使用 v3 已验证的 stable id，不包含通知正文或 app 标题。

`webview/bootstrap-v1.json` 是 UTF-8 紧凑 JSON，根字段精确为
`{format,version,generation,chat,documents,llm,restore_intents}`，格式/版本固定
`solum-webview-bootstrap` / `1`。`documents` 每项精确为 `{id,name,media_type,plaintext_size,sha256,blob}`；
`blob` 固定指向 `documents/<sha256(UTF8(id))>.blob`，不直接把跨平台 id 当文件名。blob 字节在写入前已经
被 backup/attachment AEAD 和 v3 manifest 双重认证，组装时仍重新核对文件字节数与 SHA-256；bootstrap
包含必要的用户数据，故只能留在 app 私有 profile generation，不能记录到日志或错误文本。

顺序固定为：创建 staging → 恢复全新 SQLite 事务 → 写附件/Bootstrap/spool/空 policy → 对刚写文件逐项
重读复核 → checkpoint SQLite 并切换 DELETE journal → 关闭 Store → generation seal 重算全目录 manifest。
任何一步失败均不返回 sealed handle、不写 active pointer、不删除或覆盖 live/旧 generation；残留 staging
启动时保持 inert。成功只表示磁盘镜像完整，**不表示已恢复**：WebView 尚需在下一阶段把 bootstrap 写入
generation 绑定的 localStorage/IndexedDB inactive namespace，确认成功后才能在 profile 锁内发布并重启。

#### WebView inactive generation hydrate v1（2026-08-23）

WebView 数据不通过猜测 Android Chromium 私有目录来“搬库”。Rust 根据当前账号 profile 与 generation
生成唯一命名空间，并把**精确名称**交给前端：legacy guest 沿用无后缀旧 key，legacy 账号沿用 `.<uuid>`；
generation 一律使用 `.<profile-id>.generation.<32-lower-hex>` 后缀。该后缀同时作用于
`pa.chat-sessions.v1`、`solum.ui-state.v1` 与 `pa-local-documents-v1`，所以 hydrate 可以在当前 live
WebView 中写入另一个 generation，而不会覆盖用户正在使用的会话、草稿或 IndexedDB。

Rust reader 只接受 `restore-staging/<id>/` 中已 seal 的 generation；打开时先重跑 generation manifest
文件集合/尺寸/SHA-256 校验，再严格解析 `webview/bootstrap-v1.json`，验证 format/version/generation、
聊天/恢复意图上限、唯一资料 id、blob 固定摘要路径、单件/累计尺寸及每件明文摘要。附件 IPC 固定每块最多
512 KiB，返回 `{generation,document_id,offset,total_size,data_base64,done}`；reader 会限制 id/offset、拒绝
符号链接和文件尺寸漂移，错误不含资料名、正文、blob 内容或完整摘要。一次 begin 只建立一个进程内 reader
session，后续 chunk/finish 必须绑定同一 profile 与 generation，不能借接口读取任意 app 私有文件。

前端 hydrate 顺序固定为：begin 取得 bootstrap 与精确 namespace → 写入目标 chat/UI-state key → 清空目标
inactive IndexedDB → 每件资料按顺序拉取块、重组 Blob、核对尺寸/SHA-256 后写入 → 在 readonly transaction
回读**精确资料集合**并再次核对 metadata/Blob 摘要 → 回读两项 localStorage 与预期 JSON 相等 → finish
提交 `{profile_id,generation,documents:[{id,plaintext_size,sha256}]}`。Rust 只在 receipt 与已打开 bootstrap
精确相等时把该 generation 标为本进程 `ready`；不接受缺件、多件、重复或摘要变化。

hydrate 允许对同一 inactive namespace 幂等重做：每次先清空目标 IndexedDB，且 active namespace 在整个
过程中不变。ready 目前故意只在内存中，进程崩溃或重启后必须重新 open/校验/hydrate，不能拿陈旧 ack
发布；本阶段不提供 pointer publish 命令。下一阶段的恢复协调器必须同时持有 profile generation 锁、处理
旧 generation 的 late spool、再次确认 WebView ready 与用户的现有数据选择，才可提交 pointer 并重启。

#### Android restore publication coordinator v1（2026-08-23）

publication 是恢复的唯一破坏性提交点，只允许 Android 移动壳内部调用，不成为普通网页/桌面导入入口。
调用方必须先完成上节 hydrate ready，再取得一次性 replacement preview：WebView 先强制刷盘当前 active
chat/UI-state，按资料 Blob 原始字节生成精确摘要与计数；Rust 同时在旧 Orchestrator 与 sync Store 都被冻结
时生成当前 SQLite portable snapshot 的计数和 SHA-256。preview 返回随机 32-byte token，只在本进程内保存，
绑定 profile、目标 generation、SQLite fingerprint 与 WebView fingerprint；错误、日志和 UI 预览只显示各域
计数，不回显聊天、通知、资料名或正文。

即使目标 profile 看似新建，publish 也不接受裸 generation id：未来恢复 UI 必须展示 preview，并由用户明确
选择“用此备份替换本机数据”后原样交回 token。confirm 会再次刷盘/摘要 WebView、重算 SQLite fingerprint，
并要求 profile、generation、ready、token、两端摘要与 preview 全部相等；任一期间写入、切号、重复确认或
陈旧 token 都在 pointer 前失败，重新 preview。token 一次性消费，publish 成败都不能把它当长期恢复凭据。

提交锁顺序固定为 `Orchestrator -> sync Store -> active-generation.lock`，与 ticker/sync 的现有顺序一致；这样
确认后旧 SQLite 不再接受写入，Rust ticker 也不能同时把 spool 搬入 SQLite，原生 listener 则被 profile 锁
挡在 pointer 边界外。profile 锁内顺序固定如下：

1. 重验 sealed staging 与当前 active generation；把当前 active 数据目录里仍待处理的 `notif-spool/*.json`、
   `failed/*.json`、升级遗留 inbox，以及 overflow marker/旧计数转换为严格、有界、内容寻址的副本，写入
   `restore-handoff/<target-generation>/` 并逐文件/目录 fsync。handoff 位于 profile 私有根，不进入备份，
   文件名不含通知正文；单文件 64 KiB、总项 10,000，未知字段、符号链接、超限或半包在 pointer 前拒绝。
2. 重跑 candidate manifest；将 staging 原子 rename 到 `generations/<id>`，随后按 generation v1 协议 durable
   替换唯一 pointer。pointer 是唯一恢复提交记录，提交前任何失败都保持旧 profile active。
3. pointer 提交后，在同一 profile 锁内把 handoff 幂等写入新 generation 的 live spool/overflow 目录，再
   fsync 并删除已完成 handoff。旧 generation 的原文件不删除；重复内容最多由现有 capture dedup 吸收，
   不能以去重为名冒险丢一条通知。
4. 将本进程永久置为 profile-transition 状态并整进程重启；Android 必须先创建新的 launch task 再退出，
   不能热换 SQLite/WebView。若 Activity 重启调用异常，旧进程仍拒绝任何业务/同步写入，并明确要求用户
   重新打开应用，不能在 pointer 已提交后继续把数据写进旧 Store。

崩溃恢复由 handoff 保证：pointer 前崩溃时旧 active 和原 spool 均未动，已写 handoff 只是 inert 副本；
pointer 后、handoff 合并前崩溃时，新进程在打开 SQLite、启动 ticker/listener 前，必须按 active generation id
续完对应 handoff。合并为 create-or-verify，同名目标只有逐字节相等才算幂等；损坏 handoff 或无法落盘必须
停止启动，不能静默丢弃后继续。pointer 提交后 generation manifest 只代表发布时 sealed 基线；live spool、
overflow 与 SQLite 此后按既有运行时规则可变，不重新冒充 sealed snapshot。

本节仍是内部发布 seam：没有备份下载/解密 UI、服务端 `/v1/backups`、恢复码/信封、隐私政策迁移或实机
跨平台互通。未来 UI 负责把 preview 计数和不可撤销替换后果呈现为有重量的确认，但不得绕开这里的一次性
token、重读校验与进程重启边界。

#### `solum-cloud` opaque backup API v1（2026-08-23）

这一层只负责把已经由端侧 `solum-backup` / `solum-attachment` 加密的对象按完整版本保存；服务端既不取得
32 字节主密钥，也不解析容器头、AAD、内层 export、资料名称或正文。唯一租户身份是已验证 access token 的
不可变 UUID `sub`，请求路径、query 和 JSON 都没有 `tenant_id` / `user_id` 字段。所有 SQL 都在
`withTenant` 事务及强制 RLS 下执行；运行角色不是 owner、superuser 或 `BYPASSRLS`。TLS 仍是传输要求，
但 TLS、账号 token 与 RLS 只能限制谁能取得密文，不能替代端到端加密。

**对象与上传状态**：一个完整版本由恰好一个 `main` 对象和 0 至 63 个 `attachment` 对象组成。`main` 是
完整 `solum-backup` v1 字节，attachment 是 manifest 所列的独立加密容器；main 使用随机 UUID v4，attachment
使用下述 keyed UUID v8，`object_id` 不得直接复用 attachment id、文件名、明文或明文摘要。服务端只公开版本时间、generation、对象数量、种类、
密文字节数和密文 SHA-256；这些是有意泄漏的流量 metadata，密文摘要不是明文摘要。

创建上传使用 `POST /v1/backups`，`Content-Type: application/json`，body 必须只有以下字段：

```json
{"backup_id":"11111111-1111-4111-8111-111111111111","generation":1,"created_at":"2026-01-02T03:04:05.000Z","protocol_version":1,"objects":[{"object_id":"22222222-2222-4222-8222-222222222222","kind":"main","bytes":123,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}]}
```

UUID 必须是小写带连字符形式且不是 nil；`generation` 是 1..2^53-1；时间必须是可往返的 UTC RFC 3339
毫秒格式 `YYYY-MM-DDTHH:mm:ss.sssZ`；版本固定为 1；对象 id 互异、尺寸为正安全整数、摘要为 64 位小写
hex，并且恰有一个 main。未知/缺失字段一律 400。服务端把对象按 `object_id` 字节序排序，再以示例相同的
键序、紧凑 UTF-8 JSON 编码整个已验证 plan，并取 SHA-256 作为 `plan_etag`；因此同一组对象的数组顺序
不影响幂等性。上方示例本身的 canonical SHA-256 是
`3ae4b633307d12c2f82fac4ddb36539f5ed8d6c5a18e5e9a09a68d2b96979528`。相同租户、`backup_id` 和完全相同 canonical plan 返回既有 pending/complete 版本；同 id
不同 plan 返回 409，不能覆盖已提交版本。

对象上传使用 `PUT /v1/backups/{backup_id}/objects/{object_id}`，只接受
`application/octet-stream`。读取前先按声明尺寸与服务端单对象上限约束 `Content-Length`，流读取仍独立
执行硬上限；实际长度和服务端计算的 SHA-256 必须与 plan 完全相等。第一次上传 create-or-verify，重试
逐字节相同的对象成功，不同字节返回 409。API 不接受 base64/JSON 密文，也不在错误或日志中回显 body、
摘要之外的攻击者内容。上传任一 mutation 时，plan generation 必须等于租户当前 active generation。

完成上传使用 `POST /v1/backups/{backup_id}/commit`，只接受零长度 body。在同一数据库事务和租户 advisory
lock 内重验 generation、plan、每个对象已到齐及字节数/摘要，然后把 pending 原子改为 complete。缺件返回
409，pending 永不出现在列表/下载中，也绝不能替换 latest；重复提交相同 complete 版本幂等成功。提交成功
后在同一事务内按 `committed_at DESC, backup_id DESC` 保留最新 N 个完整版本并删除更老版本；响应明确给出
`pruned_versions`，不能静默让客户端以为所有历史仍在。清理失败则整个 commit 回滚。

读取与删除接口如下：

- `GET /v1/backups?limit=1..100` 仅列本租户 complete 版本，默认 20，顺序同保留排序；每项返回
  `backup_id/generation/created_at/committed_at/protocol_version/total_bytes/object_count/plan_etag/objects`，
  object 只含 `object_id/kind/bytes/sha256`。响应同时返回当前 `active_generation`。
- `GET /v1/backups/{backup_id}/objects/{object_id}` 只读取 complete 版本，返回原始
  `application/octet-stream`、精确 `Content-Length`、`Cache-Control: no-store` 与带引号的密文摘要 ETag。
- `DELETE /v1/backups/{backup_id}` 硬删除一个 complete 或 pending 版本及其对象。必须携带
  `If-Match: "{plan_etag}"`；缺失为 428、陈旧/不符为 412，成功返回被删 id。用户可以删除最后一个版本；
  未来 UI 必须对此给强确认，但服务端不能假装用户没有删除权。

**单写者 generation**：`GET /v1/backups/generation` 返回当前值（新租户惰性初始化为 1），但 generation
本身不构成写权限。服务端另存一个不在目录、响应或日志中公开的随机 writer UUID；首台设备必须先以
`POST /v1/backups/writer` 原子 claim。claim 仅在 generation 仍为 1、尚无 writer 且租户没有任何
pending/complete 备份时成功，同一 writer 重试幂等，其他 writer 一律 409。因此两台新设备即使同时看到
generation 1，也只有一台能成为写者；已有任何备份时，缺少本地 writer 状态的新设备必须先走恢复接管，
不能把空 profile 认作首台设备。创建 plan、PUT 对象与 commit 都必须携带
`X-Solum-Backup-Writer`，服务端在租户 advisory lock 内同时核对 generation 与 writer，不能只靠客户端
本地文件自律。

换机确认接管后，客户端用 `POST /v1/backups/generation` 提交严格 JSON
`{"expected_generation":1,"takeover_id":"non-nil lowercase UUID","writer_id":"new writer UUID"}`。服务端在 tenant advisory lock 下执行
compare-and-increment：expected 不等于当前值返回 409；首次成功只加一并记录 takeover id；同 takeover id
且 writer id 相同的丢响应重试返回同一新 generation，其他重放或换 writer 不能再次推进。推进时原子轮换
writer 并删除旧 generation 的 pending 上传，
保留已 complete 历史；旧设备此后的 create/仍存在的 commit 以 409 失败，已随接管删除的 pending object
路径按不存在返回 404。这防的是正常旧客户端误写，
不是把仍持有有效账号 token 的恶意设备变成不可信方；真正撤销仍需撤销账号会话/设备授权。

**默认硬上限与配置下界**：单对象 68 MiB（覆盖 core 的 64 MiB backup 容器上限及 framing）、单版本
700 MiB（覆盖当前 600 MiB attachment 明文预算和主容器）、每版本 64 对象、每租户同时 4 个 pending、
每租户已存 ciphertext 共 2 GiB、保留 10 个 complete 版本（生产不得配置低于 2）。pending 同样占配额，
超过 24 小时由逐租户清理任务删除；创建、对象上传和 commit 都在 advisory lock 内重算数据库实际用量，
不能信任客户端计数。超限统一 413 或 429，不靠降低 core 上限、截断或静默淘汰做绿。PostgreSQL `bytea`
只是本阶段有界后端；公开对象 API 故意与存储实现分离，生产规模的大附件迁入对象存储仍是后续门槛，
本节不宣称 600 MiB 真机上传与灾备已经验收。

错误只返回稳定 code：`invalid_backup_plan` / `unsupported_backup_version`（400）、
`backup_not_found` / `backup_object_not_found`（404）、`backup_plan_conflict` /
`backup_object_mismatch` / `backup_incomplete` / `backup_generation_stale` /
`backup_writer_stale` / `backup_writer_required`（409）、
`backup_object_too_large` / `backup_package_too_large` / `backup_quota_exceeded`（413）、
`backup_pending_limit`（429）、`precondition_required`（428）、`precondition_failed`（412）；对象上传缺少精确
`Content-Length` 返回 `content_length_required`（411）。恢复端必须先完整下载
main、由端侧 backup codec 验证认证标签/AAD/export plan，再按已认证 manifest 派生对象 id、下载并验证 attachment，最后
才进入前述 sealed generation 与 publication；列表 metadata 和服务端 SHA-256 只用于传输完整性与重试，
不能代替 AEAD、manifest 或写库前校验。

本节交付的是唯一中心服务的不可读密文存储协议与 PostgreSQL 隔离地基，不包含 Android 上传/下载 UI、
恢复码/恢复信封、对象存储、生产灾备演练、跨平台实机互通或旧 `/v1/push|pull` 的安全退役。

#### Android encrypted backup uploader v1（2026-08-23，内部 seam）

Android 上传器复用账号恢复信封已经保护的 32 字节端侧主密钥，不另造或上传备份密钥。设备在账号
profile 根保存严格 `solum-backup-device.json` v2：
`{"format":"solum-backup-device","version":2,"user_id":"lowercase UUID","device_generation":1,"writer_id":"lowercase UUID","phase":"claiming|active"}`。
它是设备接管资格而非用户数据，不进入 portable generation 或云备份。文件缺失时只有服务器
`active_generation == 1` 仍不够：客户端先 durable 写入带随机 writer UUID 的 `claiming`，再调用服务端
原子 claim；响应丢失只可用同一 writer 重试，成功后才 durable 改为 `active`。服务端已有 writer 或任一
备份记录时 claim 失败，文件保持不可写状态并要求走显式恢复接管。每次备份先读取服务器 generation 并与
本地 active 文件精确相等，写请求再由服务端逐次核对 writer；任一不一致在上传前或服务端事务内失败。
下文接管流程只有在用户确认替换后才能调用 generation compare-and-increment、原子轮换 writer，并把新值
durable 写入新设备；上传 seam 本身不能执行接管。

一次上传 session 固定绑定当前账号 UUID、profile、device generation、随机 backup UUID、UTC 毫秒创建时间、
当前 access/refresh 会话和主密钥；任一期间切号、退出或 profile 改变都取消 session。WebView 先强制保存
会话/草稿，并在一个 IndexedDB readonly transaction 取得资料 manifest；Rust 随后在 Orchestrator 锁内合并
SQLite、通知 spool 与非秘密配置，生成 v3 payload，再立即创建 `solum-backup` main 密文。main 使用随机
UUID v4；每件附件使用下述 keyed UUID v8，使恢复端可在 main 认证后精确定位对象而不向服务端暴露 manifest id。

**attachment object UUID 的精确派生**：只使用成熟 HKDF-SHA256。IKM 是调用方传入的 32 字节主密钥，salt
是 ASCII `solum-backup-object-id:hkdf-salt:v1`。HKDF info 是 ASCII
`solum-backup-object-id:info:v1`，随后依次追加 `u32_be(len(backup_id UTF-8)) || backup_id UTF-8 ||
u32_be(len(attachment_id UTF-8)) || attachment_id UTF-8`；这里 backup id 已是 36 字节小写 UUID，attachment
id 已通过 v3 的 1..128 字节 ASCII id 校验。HKDF expand 输出恰好 16 字节，然后令 byte[6] 的高四位为
`1000`（UUID v8）、byte[8] 的高两位为 `10`（RFC 4122 variant），按小写 8-4-4-4-12 UUID 输出。未来三端
必须逐字节复现；不得使用语言原生字符串长度、UTF-16、JSON、平台字节序或未声明的零分隔。backup id 每次
随机，因此相同资料跨备份不可由服务端链接；没有主密钥也不能由 object id 反推 attachment id。客户端还
必须拒绝与 main id 或其他附件派生 id 碰撞的计划。此规则修正了最初全随机附件 object id 无法从服务端排序
列表映射回加密 manifest 的缺口，不改变任何服务端字段或明文可见性。

资料 blob 不进入主 JSON，也不写明文临时文件。WebView 只按 manifest 顺序把同一 Blob 以最多 512 KiB
base64 IPC 块传给 Rust；Rust 要求 attachment id、offset、累计尺寸和最终 SHA-256 与已冻结 manifest 精确
相等，单件最多 20 MiB。单件完整后立即用 `solum-attachment` v1 和生产随机 nonce 加密，主动清零明文
buffer，只把密文原子写入 profile 私有 `backup-upload-staging/<random-session>/`。该目录不含资料名、media
type、账号或 attachment id，文件名只使用随机 object UUID；正常成功/取消时删除，进程崩溃残留仍只有
不可读密文，下一次 begin 只清理该固定 staging 根下严格随机命名的孤儿。

全部 attachment 封装完成后才得到稳定 plan；上传顺序固定为 create plan → main → manifest 顺序附件 →
commit。每个请求绑定同一账号 server origin，只带 access token；401 最多刷新一次并先 durable 保存轮换
会话，传输失败最多重试一次。plan、对象 PUT 和 commit 都依赖服务端幂等语义，重试不得重加密或更换
backup/object id；因此丢响应不会制造两个版本。客户端在 PUT 前再次核对 staging 密文的字节数/SHA-256，
commit 响应必须回显同 backup id、generation 与 `status=complete` 才可记为成功。错误、状态与诊断只显示
阶段和稳定分类，不记录 access/refresh token、主密钥、明文、资料 id/名称、完整摘要或服务器响应 body。

本节只提供未来 Android 设置页/后台调度可调用的 begin → attachment chunks → commit 内部 seam；不自动
定时上传，不新增可见按钮，也不把网络失败接进提醒/ingest。下载、历史选择、恢复发布 UI、恢复码、后台
充电/Wi-Fi 策略、对象存储与真机大包验收仍属后续，旧同步继续保持迁移态。

#### Android encrypted backup downloader v1（2026-08-23，内部 seam）

下载协调器是平台中立的严格读取路径，只消费 `GET /v1/backups` 的 complete 版本，不读取 pending，不自动
接管 generation，也不写 active pointer。列表必须是严格 JSON：拒绝未知字段、非法 UUID/UTC 毫秒时间、
非法小写 SHA-256/plan etag、零尺寸、对象重复、计数或总尺寸不一致、非唯一 main、超过 64 对象/68 MiB
单对象/700 MiB 整版、generation 高于 active、created_at 晚于 committed_at 及非递减
`(committed_at, backup_id)` 排序；客户端还按服务端 canonical plan 规则重算 plan etag。列表中的服务端摘要
只作传输校验，不能替代容器认证。

选择一个版本后顺序固定为：① 在发请求前按声明尺寸与上述上限拒绝超限；② 只下载唯一 main，流读取最多
`declared bytes + 1`，要求 HTTP `Content-Type=application/octet-stream`、`Cache-Control=no-store`、精确
`Content-Length`，并复核实际字节数和密文 SHA-256；③ 以当前账号恢复出的 32 字节主密钥调用 backup-v1
codec，先完成格式/版本/算法/AAD/AEAD/受限解压与 `plan_import`；④ 要求认证头中的 user UUID、generation、
backup id、created_at、protocol version 与所选列表及当前账号逐项相等，并要求内层为可组装的 export v3；
⑤ 从已认证 manifest 按上节规则派生全部 attachment object UUID，要求与列表的 attachment 对象构成严格
一一对应集合，无缺失、额外或碰撞；⑥ 按 manifest 顺序逐件有界下载、复核传输字节/摘要，再以完整 binding
完成 `solum-attachment` AEAD、块序、明文尺寸和 SHA-256 校验；⑦ 全部对象验证成功后才调用现有 v3 prepare，
并只向新建 inactive staging 组装、重读校验和 seal generation。任何一步失败都不得打开 live Store、写 active
pointer、推进 generation 或修改现有 generation；成功返回的也只是 sealed generation id 与无敏感计数，
必须交给下文预览/确认协调器才能发布。

GET 使用相同账号 origin 与 bearer token；401 最多刷新并 durable 保存一次，瞬时传输失败最多重试一次。
重试必须保持相同路径与所选版本，响应 body、token、主密钥、明文、attachment id/名称、完整摘要和本机路径
不得进入错误或日志。可见入口、已有数据预览、WebView hydrate 与 generation takeover 由下节提供；恢复码、
崩溃后下载暂存清理与 600 MiB Android 真机内存/磁盘压力验收仍是后续产品门槛。

#### Android 备份与恢复中心 v1（2026-08-23，手动入口与接管事务）

设置新增登录账号专属的“备份与恢复”二级页，不扩张四个根入口。页面只显示用户能判断的事实：最近成功
时间、密文总量、版本数量、设备代次，以及 complete 历史的创建时间/大小；object id、plan etag、密文摘要、
主密钥和内部 generation id 不进入可见文案。v1 只有用户主动点击“立即备份”，不做后台定时、充电/Wi-Fi
策略或失败通知；上传仍完整复用前述冻结 → 分块 → 密文 staging → commit seam。历史空状态明确引导先创建
备份，网络失败保持本机数据不变。

选择历史版本后先执行 downloader 的完整验证并组装 sealed inactive generation，再 hydrate generation 隔离的
WebView namespace；这两步都不是恢复提交。随后必须对当前 live SQLite 与 WebView 做精确 replacement preview，
显示将被替换的本机行、会话/消息/草稿/资料计数，以及将恢复的版本时间、附件数和逻辑行数。普通预览使用
中性面；只有最后一步使用红色模态，明确说明“本机当前内容将由所选备份替换、旧手机停止成为备份写入者、
应用会重启”。按钮名称始终为“替换本机并接管”，不能用“继续/确定”弱化后果。预览 token 仍一次性绑定
live SQLite/WebView 摘要，任何变化都要求重新预览。

**generation takeover 的持久事务**：云端恢复不能直接调用裸 pointer publish。最终确认前先在 profile 私有根
原子写严格 `solum-backup-takeover.json`，只含 format/version、账号 UUID、server origin SHA-256、所选
backup UUID、sealed generation id、expected active generation、随机 takeover UUID、随机新 writer UUID、phase 与可选 target
generation；不含 token、主密钥、明文、资料 id 或服务器响应 body。顺序固定为：

1. 在不持有 Orchestrator/SQLite/profile 锁时先重读 live preview，仍精确匹配才建立 `planned` journal；
2. 用 journal 中同一 takeover id 与 writer id 调用服务端 compare-and-increment；传输结果不明时保留 journal 并只允许原值
   重试，不能生成新 id。响应必须是 `expected + 1`；durable 记录 `remote_advanced` 后，才把本机严格
   `solum-backup-device.json` 从缺失/expected 幂等推进到 target，再记录 `device_ready`；
3. 重新取得 Orchestrator → sync Store → profile generation 锁，第二次重读并消费同一 preview token；仍一致才
   发布 sealed generation pointer、完成 notification handoff。pointer durable 后删除 journal并 fsync profile 根，
   再按既有规则整进程重启；
4. 若远端已推进而最终 preview 因本机变化失效，不能回滚 generation，也不能发布陈旧 preview；页面要求重新
   预览并以同一 journal 续作。进程重启后，`device_ready` journal 若目标 generation 尚未 active，则恢复中心
   只允许继续该候选；若 pointer 已是目标则安全清理 journal。不同账号、origin、backup 或 generation 不能覆盖
   既有 journal。

服务端推进成功会使旧 generation 后续上传失败，但不会删除任何 complete 历史；它不是远程擦除旧手机。
本轮仍不开放 complete 版本删除，因为删除属于另一项不可逆操作，必须另接 Guard + `If-Match` 预览，不能
借恢复确认顺带授权。恢复码、后台备份、生产对象存储、600 MiB 真机压力和跨平台实机互通仍未完成。

**隐私同意**：手动入口可把完整对话、日程/提醒、记忆/人格/行为/审计、健康、通知处理结果、组件、导入
资料及非秘密设置/重新授权意图作为端侧加密密文保存到 Solum Center；账号会话、密码、主密钥、恢复码、
LLM/邮箱/Soulous token、OS 权限和硬件密钥永不进入普通备份。服务端仍会看到账号归属、版本时间、对象数量
和密文尺寸这些流量 metadata。由于这是新增外发场景，隐私政策版本必须递增，旧同意不得自动沿用；再次
同意前上传/下载/接管命令与入口均不可用。通知是否允许作为 AI 明文上行仍由原“通知上云”开关独立控制，
不能把端到端加密备份混写成 AI 授权。

#### 历史多设备同步实现（迁移期，只读理解）

- 定位：同步数据面只做**加密数据中转**，不解密、不推理、不保存同步明文；服务端数据库泄露不能直接解开同步正文。这不等于统一中心看不到账号元数据、结构化告警或正在代理的 AI 请求，三者按各自边界单独约束。
- 同步模型：本地优先(local-first)，各设备本地 SQLite 是权威数据源，增量变更加密后上传，其他设备拉取解密合并。
- 冲突解决：建议用 CRDT 或 last-write-wins + 字段级合并（你在 lx-music-improve 项目的桌面统计模块已经用过"CRDT store"的模式，这里可以复用同样的思路，不必重新设计一套）。
- **旧部署兼容**：`solum-sync-server` 的单二进制 + SQLite 只服务既有 relay 和迁移，不是新生产拓扑；它不得与中心库同时接受同一租户的新写入。
- **中心库收口（2026-08-12 决定）**：生产部署改为一个 `solum-cloud` API 容器 + 一个 PostgreSQL 中心库；客户端仍然 local-first，桌面/Android 保留 SQLite，鸿蒙保留 relationalStore，任何客户端都不得直连 PostgreSQL。中心库按域保存账号/会话、设备、密钥信封、账号级加密设置、AI 权益/用量元数据、结构化告警和不可读的同步密文；不保存可供服务端查询的行为日志、人格、健康采样或聊天明文。账号、AI 代理与同步共用同一个 HTTPS origin，现有 `/v1/push|pull|stats|alerts` 协议保持兼容；`solum-sync-server` 的 SQLite 实现降为旧部署和迁移期兼容入口，不再作为新部署的第二套权威数据源。
- **PostgreSQL 租户护栏**：所有用户数据表都以不可变 `tenant_id UUID` 为首列，并同时启用、强制 RLS；策略只读取 API 在单个事务内设置的 `app.current_tenant_id`，客户端请求体没有提交租户 id 的位置。运行账号不是表 owner、没有 superuser/BYPASSRLS，且只获所需 schema/table 权限；RLS 列与主要游标组成 `(tenant_id, seq)` 等复合索引。连接由进程池复用，不按请求新建连接。大附件后续进对象存储，PostgreSQL 只留密文索引。
- **登录即恢复的密钥模型（2026-08-12 定案；2026-08-14 删去伪能力）**：用户同步主密钥由首台已登录设备用系统随机源生成；中心库只保存 `recovery-xchacha20poly1305` 密钥信封。客户端以账号密码和不可变 `user_id` 经 PBKDF2-HMAC-SHA256（600,000 轮）导出恢复包装密钥，再以绑定协议版本、`user_id` 与 key version 的 AAD 包装同步主密钥；密码、恢复包装密钥和同步主密钥均不上传。新设备登录成功后用本次输入的密码解开信封，自动恢复同账号设置、画像和其他同步数据。新注册账号的密码从服务端校验到端侧 KDF 都保留精确输入；旧账号统一标为 `password_version=1`，因为已发布注册路径曾保存裁剪哈希，而旧 bootstrap 管理员又可能保存精确哈希。服务端因此先校验精确输入，只在输入含首尾空白且精确校验失败时回退裁剪校验；成功登录仍保持 v1，直到显式改密流程能同时重包恢复信封，不能凭一次登录猜测并升级密码语义。若本机密钥与信封解出的密钥冲突，必须停止并显式报错，绝不静默覆盖；若账号密码将来支持修改，改密事务必须先重包信封。access token 与数据库权限只决定能读取哪个租户的信封，不能解密它。设备公钥批准/通用设备信封尚无完成的信任、撤销和客户端协议，相关公开 API 已删除，不能把数据库预留列写成已实现能力。未来跨用户共享使用独立 `workspace + membership + workspace key envelope`，不得通过关闭 tenant RLS 实现。
- **身份与密钥分离（2026-08-12 修订并加固）**：传输走 TLS，租户身份来自 solum-cloud 签发的短期账号 access token；token `sub` 是账号创建时生成且永不随用户名改变的 UUID，用户名只用于登录/展示，relay 拒绝非 UUID 的账号 `sub`。同步内容仍以设备本地 32 字节主密钥做 XChaCha20-Poly1305 加密。账号 token 只决定“能访问哪个租户分区”，同步密钥只决定“能否解开该分区内的 blob”，两者不可互相替代。设备撤销只阻断该设备 ID 的 push/pull，不等于撤销账号 refresh 会话；账号退出使用当前 refresh token 单独撤销。同步密钥疑似泄露时仍须换密钥并重新播种。旧 `SOLUM_SYNC_SERVER_TOKEN` 只作为 `legacy` 租户迁移入口，不得访问任何账号租户。
- **客户端账号隔离（2026-08-12；2026-08-14 并发加固；2026-08-19 移动端重启修复）**：设备级 `solum-account.json` 只保存当前会话；登录账号的数据根为 `profiles/<user_uuid>/`，其中 SQLite、直连 LLM、同步、邮箱、Soulous 与通知白名单配置分别独立，桌面壳和 CLI 使用同一 active-profile 解析器。WebView 的聊天历史、UI 状态与资料 IndexedDB 也以同一 UUID 加后缀分区；Android 后台告警读取该 UUID 下的同步配置并以 UUID 隔离游标，原生通知监听器读取该 UUID 下的白名单，不再旁路回 guest 根目录，也不得把账号 bearer token 发往账号中心 origin 之外的 URL。未登录使用 `guest` 分区。登录/退出不会热换一部分状态，而是重启进程，先关闭旧 SQLite 连接、缓存、后台同步与内存对话，再打开目标 profile；桌面沿用 Tauri 可执行文件重启，Android 由 `MainActivity` 创建新的启动任务后退出当前 Activity 进程。旧会话缺 UUID 时必须重新登录，不能刷新或解锁账号能力。GUI/CLI 对会话的发布、模型更新、refresh 轮换和删除共用独立 `.lock` 文件的 OS 排他锁；Android Kotlin 用 Java `FileChannel.lock()` 的 POSIX 记录锁，Rust Android 用 Linux OFD 锁，使同一进程内的两套运行时也会真实互斥。网络刷新不持锁，返回后再比较当前身份/令牌并提交，已退出或已切换身份的旧请求只能失败，不能复活会话。现有未登录本地库保持为 guest，不自动迁入账号，避免在没有用户确认时改变数据归属。账号会话、隐私同意与 Android 系统闹钟镜像是设备级控制状态，刻意不随账号复制；运维显式提供的路径/凭据环境变量仍优先。

- **同步历史与容量（2026-08-14）**：中心 `sync.blobs` 保存的是不可从剩余行重建的增量操作，不是缓存；在没有端侧快照/服务端压缩协议前禁止按时间删除。此前 30 天清理会让长期离线设备静默漏数据，且全局 identity 序列无法表达单租户“已删除到哪里”。当前改为永久保留历史并以每租户 1 GiB / 100,000 blob / 32 设备的默认硬上限拒绝新写入；运维可显式调高，但不能用静默清理换容量。告警仍按 7 天清理。
- **通知捕获同步（schema v10，Phase 9/10 + 2026-07-19 解耦）**：第三方通知的 raw input 及其派生事件、提醒**无条件参与多设备同步**，不受任何开关控制。理由见下方「为什么同步与上云解耦」。Phase 10 的 `notification_captures`、过滤提议和 Rust 已解析但仍待确认的动作提议只是本机 F12 分诊回看元数据，**不新增同步 payload**；远端设备只获得 raw input 与派生数据。App 白名单和 `notif_cloud` 都是设备本地设置，不走 meta LWW 同步；重要路由规则复用既有 `RuleTable` 配置。
- **为什么同步与上云解耦（2026-07-19 用户拍板）**：`local_only` 此前同时是「不发云端 LLM」和「不参与同步」两件事的条件，但两者风险量级完全不同——发 LLM 是**明文**交给用户自配的第三方厂商（小米/DeepSeek/OpenAI…），同步是**端到端加密 blob** 发往统一中心或旧版自建 relay（服务端解不开同步正文，本节第一条）。捆在一个开关上导致「想要设备间同步通知、但别喂给 LLM」这个合理诉求无法表达。现拆开：**同步永远开，`notif_cloud` 只管 LLM**。
- **`local_only` 语义收窄（schema v10）**：该列现在**只表示「不得作为云端 LLM 上下文」**，与同步无关（旧语义"本地专属"已不准确，保留列名是为避免大范围迁移，读代码时以本条为准）。捕获时按开关盖戳，此后不可变（PRIVACY.md §2）。
- **戳随 payload 跨设备传播**：`raw_inputs`/`events` 的同步 payload 新增 `local_only` 字段，接收端直接采用捕获设备的原始判定，使同一条记录在所有设备上的 LLM 资格一致。**缺失该字段的旧版本 payload 保守回退**为「按 `[通知·` 前缀判定为排除」。接收端的 recall 仍额外 AND 上**本机**的 `notif_cloud` 开关（双重门：捕获时同意 + 本机当前设置都放行才可上行）。
- **前向兼容：读不懂的 op 进隔离区（schema v11，2026-07-19）**——接收端遇到**本版本不认识的表**（版本更新的对端新增的）或**无法解释的 payload**时，把该 op 原样暂存进设备本地表 `sync_quarantine`（不参与同步、无 guid），**游标照常前进**；升级后 `migrate()` 末尾按 `(hlc, origin)` 顺序重放，LWW 语义与首次到达时一致。此前是直接抛错，会连带整批回滚且游标不前进，导致该设备**同步永久卡死**（push 仍工作，故伪装成健康，机制见 PITFALLS 2026-07-19）。**不选"直接跳过"**是因为跳过会让游标越过该 op，日后升级也永远补不回来——静默丢数据比卡死更坏。隔离区有上限（5000 条）且溢出可见（`meta.sync_quarantine_dropped` + CLI 告警）。**服务端版本协商在本架构下不可能**：relay 只过密文，不知道 blob 里有哪些表，无法按能力过滤（取舍见 MISC 2026-07-19）。此后新增同步表不再打断落后版本的设备。
- **push 半轮的毒丸防护（2026-08-21）**——上条的对称面：push 在 pull **之前**执行，因此服务端**永久拒绝**（507 配额满 / 403 设备撤销 / 429 设备数上限 / 400、413 请求不可接受）或单条 op 超过 7 MiB 打包上限，曾让整轮以 Err 结束且游标不动——设备永远重试同一批注定被拒的上传，并且**再也执行不到 pull 半轮**，一次拒绝等于既推不出去也收不进来。现在：永久拒绝只作废本轮 push 半轮，pull 照常；拒绝原因持久化 `meta.sync_push_blocked` 并在**下一次成功推送时自动清除**（与 history_gap 的人工确认不同：配额会恢复/设备会解封，成功推送即证明）；被拒时未上传的积压数量计入当轮 outcome。单条超限的 op 暂存 `sync_push_holds`（`migrate_sync` 内 `CREATE TABLE IF NOT EXISTS` 纯增量新表，不递增 schema 版本、无数据迁移；只存 oplog 指针与 hlc 不存正文——oplog 永不例行清理，未来放宽上限可按指针回放重推），游标照常前进。瞬时失败（网络/超时/5xx）同样只跳过 push 半轮继续 pull，不留持久标记。取舍细节见 MISC 2026-08-21。
- **v9 → v10 迁移**：此前因 `local_only=1` 而被排除在同步之外的历史通知行，迁移时补 guid 并借回填 UPDATE 进入 oplog，从而首次同步到其他设备；它们的 `local_only` 戳**保持不变**（仍不给 LLM）。已上传的历史密文仍只能由用户轮换同步密钥并清理服务器后消除。
- **KDF 从 Argon2id 换成 PBKDF2-HMAC-SHA256（2026-07-22，当天第三次修订同一段）**：起因是想让 sync-server 运维仪表盘也能直接输用户名+密码，而 Argon2id 没有浏览器原生实现——要么违反仪表盘"不引入依赖"的原则塞一份 JS 库，要么手搓 Argon2id（安全代码不该冒这个险）。PBKDF2 与 HKDF 都是 `SubtleCrypto` 原生算法，浏览器和 Rust 零依赖打平结果（已用 Node `crypto.webcrypto.subtle` 逐字节核对一致）。**代价**：PBKDF2 不是内存困难型，抗 GPU/ASIC 并行暴力破解弱于 Argon2id——对自托管单用户 relay，这个权衡换来"仪表盘也能登录"值得。HKDF 的 salt 从隐式 `None` 改成显式全零 32 字节，因为 `SubtleCrypto` 的 HKDF 接口没有"省略 salt"的选项，必须两边显式对齐同一个值。
- **同步绑定改为账号会话 + 自动密钥恢复（2026-08-12，替代独立同步密码）**：设置页只需登录统一 Solum 账号，客户端强制同步与登录使用同一 origin，以 `solum-account.json` 的短期 access token 鉴权，并在 401 时通过 refresh token 轮换。首设备随机主密钥及其登录恢复信封按本节前述模型创建，新设备无需再填写同步地址或同步密码；磁盘只保存 `{url,key}`，不保存账号密码。旧 `{url,username,password}` 和 `{url,token,key}` 仍可读取并访问 `legacy` 租户，迁移完成前不破坏现有单用户部署；旧账号 profile 若已有本地主密钥，会在首次新版登录时建立恢复信封，若远端已有不同密钥则停止切换并要求人工处理。
- **移动端同步配置补齐（2026-07-22；2026-08-12 profile 化）**：`SyncConfig::load()` 支持 `SOLUM_SYNC_CONFIG` 路径覆盖（同 `SOLUM_LLM_CONFIG`）；未显式覆盖时，桌面、CLI、Android 与原生后台服务现在统一读取 active profile，guest 才使用 app-data 根目录。此前 Android 无 cwd 且原生服务硬编码根目录，两条旁路都已收口。
- **运维仪表盘（2026-08-12 多租户修订）**：`GET /v1/stats` 只聚合已认证账号自己的 blob/设备/告警，不返回跨租户总量；`GET /` 静态页面本身无数据，无鉴权即可加载，页面通过统一中心的 `/v1/auth/login|refresh` 获取短期令牌，再请求 `/v1/health`/`/v1/stats`。旧静态 token 登录只查看 `legacy` 租户。页面保持 vanilla JS、无第三方依赖。
- **账号隔离的状态告警中转（2026-08-12）**：统一中心（旧 relay 兼容路径另行维护）提供 `POST /v1/alerts` 与 `GET /v1/alerts?since=N[&source=...]`，所有写入、读取、游标和 `(tenant_id,event_id)` 幂等约束均限定在令牌账号内。电脑端独立 `benefit-monitor` 只监听 `127.0.0.1`，织境密码与会话由 Windows DPAPI 加密；Solum 侧同样保存账号会话并自动刷新，向自己的租户发布恢复事件。告警表与加密同步 blob 分表，只允许固定状态字段，不接收账户、余额、上游 API 密钥、聊天或任意正文。告警保留 7 天；Android 以同一 Solum 账号会话拉取自己的事件，账号切换时使用按不可变 UUID 区分的本地游标，避免改名或切账号沿用错误进度。
- **告警不是同步数据，也不是第三方推送**：这是统一中心告警域上的最小状态中转，服务端能看到“某渠道恢复”这一事实，因此不宣称端到端加密；作为交换，Windows PowerShell 5.1 应用零额外依赖，Android 不引入 Firebase。Android 已有 `dataSync` 前台服务在配置同步后每 20 秒拉取；首次成功连接只建立游标，此后对批次内每个新 `operational`/测试事件分别发高优先级通知，超过 10 分钟的不补发。电脑端同样默认每 20 秒轮询，因此源站检测完成后的典型手机通知延迟为 0–40 秒；ROM 杀后台时不保证实时，用户需允许后台运行/自启动并关闭电池优化。
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

> 背景：用户同时运营 BloomXP（学习打卡全栈 App，Spring Boot + React，自有 VPS，多用户产品；仓库通常位于本仓同级 `../bloom-xp/`）。双向打通的价值已确认：Solum 拿 BloomXP 的课表/任务/打卡数据能更懂用户；BloomXP 拿 Solum 的日程/状态派生事实能更好地辅助学习规划。为此把 §4 的隐私原则从"一刀切不出本地"细化为三级分级（见 §4 第 1/5 条），这是一次**有意的原则调整**，不是违规。

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

**边界**：这是一个可用的最小闭环，不是通用低代码框架。`widget_defs` 保存名称、受控图标与 list 视图的排序字段；**schema 以一字段一行存在 `widget_fields`**（v13 起，此前是 `widget_defs.schema_json` 单列）；`widget_records` 保存 `widget_id + data JSON + created_at`。三张表当前都参与迁移态多设备同步（v13 起，第一条竖切时曾明确不同步，该契约已被推翻），目标 F17 改为纳入完整备份。`widget_schema_rejections` 当前仍只在设备本地：它是“缺哪些类型/视图”的产品证据；是否进入未来备份需在 schema 清单中显式决定，不能沿用“审计永不出本机”的旧口号替代评估。

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

> **2026-08-22 覆盖性决定**：本节首先记录已存在的桌面来源实现及其安全边界，不代表继续建设完整邮件客户端。移动产品只保留用户主动连接后的高价值事件提取；Android/iOS/HarmonyOS 必须各自使用平台认可的系统浏览器与原生 OAuth redirect，并重新验收凭据安全存储、进程恢复和后台限制。

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

1. 原始数据明文（聊天记录、行为日志、人格、穿戴设备数据）以手机本地存储为权威；当前旧同步和未来版本化备份都只能上传设备端生成的端到端加密密文，服务端不持有解密主密钥。任何第三方云端 LLM 只接收本次任务所需的最小明文上下文，且 Solum Center 不把提示词或回复正文写入用量记录。*（2026-07-18 细化：经用户白名单授权的**派生结构化事实**（L2 级，定义见 §3.11）可推送到用户自有的 BloomXP 服务。）* *（2026-07-19 例外：第三方通知是否能作为 LLM 上下文由第 6 条的 opt-in 开关控制。）* *（2026-07-20 当前行为：邮箱凭据与邮件内容不进入 SQLite / 旧同步 / 导出 / LLM；未来只有提供商允许迁移且新隐私政策明确覆盖的配置，才可进入 F17 密文备份。）*
2. 用户对"agent 记住了什么"有完全的知情权和删除权（F12），这个面板要在 MVP 阶段就有雏形，不能是三期才做的"锦上添花"。
3. 高危操作的确认流程是代码层强制的，不受 Proactivity 等级、Persona 设定、或任何 prompt 影响（3.3）。
4. 审计日志 append-only，当前版本只在本地保存，用户可导出但不可从 UI 内直接删除历史审计记录；F17 上线后可作为完整备份的一部分加密保存，但恢复后仍不得破坏 append-only 语义。
5. 与自有 BloomXP 服务的数据交换遵守 §3.11 三级分级与四条硬约束（白名单默认全关、推送过 Tool+sensitive 确认、来源打标防回声、两侧记忆不同步）。注意 BloomXP 是多用户系统，数据进它的 MySQL 后暴露在整个应用的 bug 面上——这是 L2 只放"粒度粗、单条泄露损失有限"事实的原因。
6. **通知上云（2026-07-19 重划红线，2026-08-11 收紧默认值）**：第三方通知文本只有在「设置 → 隐私 → 通知上云」被用户主动开启后，才可作为上下文发往云端 LLM；新安装**默认关闭**（opt-in）。设置只决定**之后新捕获**行的 `local_only`；关闭后新行不发 LLM、不进 recall、不进 F20 分诊载荷，历史 `local_only=1` 行在再次开启时仍不回填。已保存的显式选择在升级时保持不变。**该开关只控制 AI 明文上行，不控制端到端加密的数据保护链路**：当前迁移期旧同步仍会无条件传播通知密文，目标 F17 则把可移植通知数据纳入版本化密文备份；两者的服务端都不能解密正文。因 API 由用户自行配置，云端 AI 处理责任归所选厂商，隐私政策（`docs/PRIVACY.md`）明示后果。**两条护栏不随此放宽**：① AI 上行内容可能包含第三方（联系人/机构）信息，隐私政策必须如实披露；② **注入线不松**——通知内容进 LLM 仅作上下文，永不成为可执行动作来源（F18 硬原则、`LLM_ACTIONS` 窄白名单不变）。

## 5. 技术选型与理由

| 领域 | 选择 | 理由 |
|------|------|------|
| 客户端框架 | Mobile-only，当前 Tauri 2 Android | Android 先复用现有 Rust 核心与静态 UI；iOS/HarmonyOS 后续通过平台适配层接入，同享领域协议和备份夹具，不要求三端必须使用相同 UI 壳技术 |
| 本地结构化存储 | SQLite | 成熟、零运维、Tauri 生态支持好 |
| 本地语义检索 | sqlite-vec 或本地向量库 | 避免额外起一个向量数据库服务，保持"本地优先"的轻量原则 |
| 云端推理 | OpenAI 兼容网关（当前小米 MiMo；可配置切换 Claude API 等） | 用户提供 MiMo token-plan；接口不绑厂商，换 provider 只改配置（2026-07-06 变更，原定默认 Claude API） |
| 加密云备份 | `solum-cloud` + PostgreSQL 中心库 | Android 生成版本化逻辑快照并端到端加密；中心库只保存密文与恢复元数据，RLS 隔离租户 |
| 换机恢复 | 临时库导入 + 完整性检查 + 原子切换 | 单写者不需要 CRDT/LWW；跨 schema 恢复失败不得破坏本机或最后一个可用备份 |

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

**Phase 9–12（2026-07-19 排期，✅ 已全部完成，按依赖顺序）**：

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
- ~~**release keystore 仓库外无副本**~~ → 已解决（2026-08-22）：`solum-release.keystore` 已在仓库外做加密备份并完成解密回环验证，密码保存在独立密码管理器；`keystore.properties` 仍不得入库，恢复发布环境时需按手册重建配置并再次核对签名证书。
- recall v2（本地 embedding + sqlite-vec）的移动端可行性未评估（模型体积/内存），v1 词面检索先跑，见 §3.10。
- 用真实数据喂 Phase 6/7 的前提是日常真实使用（dogfooding）：release APK 已可安装到主力手机，多条待决项（规则表初值、F11/F13 门槛、recall 命中率）都只能靠真用积累。

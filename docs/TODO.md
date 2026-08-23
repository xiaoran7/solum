# TODO 待办

> 状态约定：`待验证` 尚未在目标环境跑通；`已通过` 已有可复查证据；`已复现` 已稳定观察到缺陷；`受阻` 受设备、权限或外部环境限制。Android 真机测试默认使用已安装的生产版客户端与 `https://solumsync.tannerlab.xyz`，不得卸载应用、清空数据或删除用户内容。

## 2026-08-22 Mobile-only、Android-first + 加密云备份架构收口（用户拍板）

> 最终定案：Solum 只做移动端，不再把桌面端或网页端作为产品；移动平台范围包括 Android、iOS、HarmonyOS，当前主开发和首发 Android，项目成熟后补齐 iOS 与鸿蒙。现有多设备双向同步退出目标架构，改为“当前活跃手机作为单一权威数据源 → 端到端加密版本化云备份 → 同平台或跨平台换机恢复”。服务端仍只有一个 `solum-cloud + PostgreSQL`。当前桌面与 push/pull 代码只是迁移态，在备份/恢复闭环验收前不得先删到无法保护现有数据。

- [ ] **先设计并实现跨平台加密备份格式。** 复用现有逻辑导出能力生成与 SQLite/relationalStore 等具体数据库无关、带 schema/protocol 版本的规范化快照，覆盖所有可移植用户数据（含日程、提醒、记忆、人格、行为、通知结果、健康、组件、完整对话、Agent 设置，以及提供商允许迁移的邮箱账户配置/凭据）；压缩后以客户端主密钥做 AEAD 加密，AAD 绑定用户 UUID、设备代次、备份版本、创建时间和格式版本。必须支持未来 Android ↔ iOS ↔ HarmonyOS 恢复，禁止直接把 Android SQLite 文件当作长期备份协议。
  - [x] **备份协议与 core codec 地基（2026-08-23）**：`solum-backup` protocol v1 已定义精确 framing/header/AAD 字节，使用纯 Rust zlib-DEFLATE level 6 后再做 XChaCha20-Poly1305；core 支持 `Value`/JSON 字节规范化、生产随机 nonce、严格字段/算法/版本/尺寸/认证校验、受限解压和 `plan_import`，且 codec 本身不写 Store。公开固定 key/nonce 的合成夹具供未来三端做逐字节互通。
  - [x] **数据覆盖审计（2026-08-23）**：已逐项登记 24 张 SQLite 表、WebView 会话/草稿、IndexedDB 资料、尚未入库的通知 spool、端侧配置/会话/凭据与原生能力，明确可移植、仅恢复意图、按字段拆分和永不备份；core 测试会在新增 SQLite 表未选择备份语义时失败。审计确认不能靠给 v2 多塞字段冒充完成：无 guid 台账、append-only 审计、会话一致快照和最大 600 MiB 的附件都需要独立恢复设计。
  - [x] **`solum-export` v3 core 契约与恢复计划（2026-08-23）**：新增严格 `_portable`，覆盖通知台账/审计稳定身份、完整会话/草稿 schema、资料 manifest、待入库通知、非秘密 Agent 设置和重新授权意图；排队 capture 恢复为 `needs_review`，待确认动作强制 `stale`。`plan_import` 可验证 v2/v3，旧 Store importer 对 v3 在任何写入前拒绝部分恢复；backup-v1 同时支持 v2/v3 并保留两套固定字节夹具。
  - [x] **Android v3 主载荷快照适配（2026-08-23，内部 seam）**：WebView 强制保存并冻结当前 profile 的会话/草稿，以单个 IndexedDB readonly transaction 读取资料并按 Blob 原始字节计算 manifest SHA-256；Rust 在 Orchestrator 锁内只读采集 active/failed/legacy notification spool、严格读取非秘密配置与恢复意图，并在前后校验 profile。损坏/超限/切号均不产出文件，通知正文和凭据不进错误；现有 v2 手动导出/导入保持可恢复配对，v3 尚不替换 UI 入口。
  - [x] **附件分块密文协议与 core codec（2026-08-23）**：新增 `solum-attachment` protocol v1；每件资料以 HKDF-SHA256 从主密钥和隐藏 binding 派生独立子密钥，使用 1 MiB 生产分块与逐块随机 XChaCha20-Poly1305 nonce。AAD 精确绑定账号、device generation、backup、附件 id、manifest 尺寸/摘要、块序号/offset/尺寸及原始协议头；服务端可见容器不含账号、backup/附件身份或内容摘要，但 framing/密文长度仍会暴露字节长度。读取端先完整校验 framing、块数/顺序/nonce 唯一性和全部 tag，再核对总长/SHA-256，失败不返回部分明文；公开三块固定夹具供未来三端逐字节互通。
  - [x] **v3 临时恢复事务 core 地基（2026-08-23）**：新增两段式 prepare/restore API；先重跑 v3 全部 schema/稳定 id/引用校验并要求附件明文集合与 manifest 的 id、数量、尺寸、SHA-256 精确相等，再只向全新空 Store 写入。`_restore`、通知台账、append-only 审计和非权限通知设置共用一个最外层 SQLite 事务，任何漏行、隔离、重复 guid、引用/隐私 scope 或 SQL 失败均整体回滚；成功后才返回会话、资料、待处理通知、非秘密 LLM 设置与重新授权意图的平台交接包。
  - [x] **Android profile generation 发布地基（2026-08-23）**：core 新增 staging → seal → generation rename → durable 单一指针的崩溃安全状态机，seal 对 SQLite 做 checkpoint/quick-check 并拒绝 WAL/SHM，manifest 严格约束路径、文件数、总尺寸与 SHA-256；启动时指针缺失才使用旧 profile，指针存在但损坏则 fail closed，不回退到空库。Android Rust 与原生通知 listener 共用 profile 根跨进程锁，保证 pointer → policy → spool 在同一 generation；账号会话、主密钥和 OAuth/API token 等设备凭据仍留在 profile 根目录，不进入可恢复 generation。
  - [x] **sealed portable generation 组装（2026-08-23）**：core 直接消费不可伪造的 `PreparedPortableRestore`，在随机 staging 中恢复全新 SQLite，并落盘 WebView bootstrap、按 id 摘要命名的附件、live-compatible pending/failed notification spool 与强制空白名单；逐项重读复核后 checkpoint/关闭 Store，再由 generation seal 对全目录做最终 manifest。任一步失败都不返回 sealed handle、不写 active pointer、不覆盖现有 generation；本阶段保持平台中立，便于未来三端复用同一镜像组装规则。
  - [x] **Android WebView inactive generation hydrate seam（2026-08-23）**：core 只从 sealed staging 重验 generation manifest/bootstrap，并以最多 512 KiB 的有界块读取 manifest 声明的附件；Android IPC 为单一 generation 建立内存 session。WebView 按账号 + generation 精确隔离 chat/UI localStorage 与 IndexedDB，清空 inactive DB 后逐件写入，按 Blob 原始字节重算 SHA-256、完整重读 metadata/localStorage，并提交精确有序 receipt；未通过前不写 Store、不发布 active pointer，崩溃后必须重新 hydrate。
  - [ ] **仍缺 Android 恢复发布协调器**：需处理 profile 锁前已落入旧 generation 的迟到通知，完成本机已有数据预览/选择、hydrate ready 复核、active pointer 发布、live WebView/双 SQLite 连接关闭与进程重启。完成前顶层项保持未完成；服务端 API、恢复码/信封和 Android↔iOS↔HarmonyOS 实机互通仍是后续 TODO。
- [ ] **实现唯一服务端备份 API。** 在同一 `solum-cloud` origin 增加账号鉴权的上传、列表、下载和删除接口；服务端只从 access token 的不可变 `sub` 选择租户。设置明确的单用户容量、单包大小、版本数量和保留规则，至少保留多个历史版本，上传必须幂等且完整提交，不能用半包覆盖最近可恢复版本。账号维护单调递增的 active generation；新手机经用户确认接管后提升 generation，旧手机的后续上传必须被拒绝，防止旧状态重新成为“最新”。
- [ ] **实现安全换机与跨平台恢复闭环。** 新手机登录后在本地解开恢复信封，校验认证标签、哈希、格式版本、容量和 schema 迁移可行性，再导入平台自己的临时数据存储并做完整性检查，最后原子切换；本机已有数据时必须预览并明确选择。除密码包装外提供用户自行保存的恢复码包装，改密必须重包；密码与恢复码都丢失时如实告知服务器无法解密。Android 首先完成闭环，随后 iOS/HarmonyOS 必须用同一套规范化夹具做交叉恢复测试。
- [ ] **区分可移植数据与平台绑定能力。** 用户数据原则上全部进入加密备份；Android/iOS/HarmonyOS 的权限授予、系统闹钟注册、硬件密钥句柄、失效 OAuth 会话和本机路径不能伪装成可直接恢复，只备份可移植配置/意图并在目标系统重新授权或绑定。各平台适配器负责把同一领域配置映射到通知、健康、后台与安全存储能力，并如实展示平台差异。
- [ ] **停止桌面产品开发，建立移动平台路线。** 不再发布 Windows/macOS/Linux 安装包，也不为桌面端新增功能或修复非共享核心问题。Android 是当前唯一开发/验收目标；核心协议、领域模型、备份夹具和 UI 语义从一开始保持移动平台中立，成熟后依次补 iOS 与 HarmonyOS。Tauri 可继续承载 Android/iOS；鸿蒙若需独立原生壳，也必须复用同一服务端契约与备份规范。CLI 仅作核心回归/迁移工具。
- [ ] **备份验收后退役多主同步。** 先盘点生产账号与 `legacy` 租户，生成最后一个可验证备份并保留旧库只读；随后删除客户端 `/v1/push|pull|stats` 调度、`sync_oplog`/LWW/游标/隔离区/毒丸等只为多主同步存在的复杂度，以及服务端 `sync.blobs`/设备同步路由。不得在用户仍只有旧增量历史、尚无可恢复快照时关闭或删除远端数据。
- [ ] **退役独立 Rust/SQLite 中继与服务端 SQLite 入口。** 从 Cargo workspace、构建/测试/部署链和文档删除 `crates/solum-sync-server`；迁移完成后删除 `server/src/server.js`、`npm run start:sqlite` 及双实现契约分支。一次性只读迁移器和归档说明可以保留，不得继续暴露第二套可写服务器。
- [ ] **移除网页端与网页登录入口。** 删除 PostgreSQL 入口 `server/src/postgres-server.js` 当前使用的 `server/src/dashboard.html` 及其 `GET /` HTML；旧 SQLite 入口 `server/src/server.js` 本来就没有 Dashboard 或 `GET /`，无需虚构删除项。根路径改为 404、204 或无用户功能的机器响应。注册、登录、备份和恢复只在移动客户端 UI 提供，并收紧 Origin/CORS。若未来要求证明官方安装包身份，另立设备密钥、mTLS 或平台 App Attestation 设计，不能靠 User-Agent。
- [ ] **统一域名、接口与产品术语。** 所有移动端只保存一个 `server_url`，供 `/v1/auth/*`、`/v1/ai/*`、`/v1/backups/*`、`/v1/keys/*` 共用。`solumsync.tannerlab.xyz` 只作迁移期兼容别名，最终不再代表独立同步系统；把产品和文档中的“多设备同步 / Sync Server”改为“端到端加密云备份 / 换机恢复”。

### 备份地基之后的移动端核心优化

- [ ] **信息源注册表 + 平台能力矩阵。** 统一声明通知、分享、日历、健康、邮箱、位置等来源在 Android/iOS/HarmonyOS 的支持状态、授权、最后成功时间、错误、AI/记忆/备份资格与删除入口；UI 如实显示平台差异，不伪造三端能力一致。
- [ ] **统一候选信息与来源追溯。** 定义平台中立的 source/external id、发生/采集时间、原始证据引用、敏感级别、可信度、有效期、用途许可和候选动作；原始证据、派生事实与执行动作分层，AI 推测不得冒充来源事实。
- [ ] **跨来源去重与关联。** 用稳定 ID、内容指纹、时间邻近和实体匹配合并日历、邮件、通知和用户输入的同一事件；默认可信度为用户确认 > 权威结构化来源 > 原始邮件/订单 > 通知 > AI 推测，冲突必须可见。
- [ ] **优先补系统分享与日历，并重做移动邮箱授权边界。** Android 先交付“分享给 Solum”和系统日历只读，再做经确认的日历写入；随后以同一领域契约适配 iOS/HarmonyOS。邮箱方向收窄为高价值事件提取，不继续扩张完整邮件客户端；现有 F21 来自桌面壳的 loopback + PKCE，不能直接视为移动端已成熟，Android/iOS/HarmonyOS 必须分别完成系统浏览器返回、原生应用 OAuth redirect、凭据安全存储和后台限制验收。
- [ ] **记忆质量闭环。** 长期记忆增加来源、可信度、最后验证时间、有效期和纠错入口；区分确认事实、来源信息、Agent 推测、建议和待确认动作。用户纠错可形成待确认来源规则，短生命周期信息自动过期，健康原始采样优先转本地趋势摘要。
- [ ] **移动首页与状态文案收口。** 保持“今天 / 记忆 / 计划 / 搜索”四根入口；今天页按“现在 → 需要确认 → 接下来 → 今日观察 → 快速输入”组织，信息源进入设置，正常备份只显示最近成功时间。逐步删除 push/pull/blob/seq/relay 等实现词和桌面表格/侧栏心智。
- [ ] **冻结非核心扩张。** 在 Android 备份恢复、信息源和 Agent 质量达到 `MOBILE-ROADMAP.md` 成熟门槛前，不新增桌面功能、实时同步、福利监控、BloomXP 双向能力、复杂组件/GenUI 类型、完整邮件客户端或新的高敏感来源，也不提前展开 iOS/HarmonyOS 实现。

## 2026-08-21 全仓优化审计行动清单

> 2026-08-21 会话对全仓做了七维度并行审计（Rust 核心 / 前端 / Node 云服务 / 同步加密 / Android / CI 发布 / 文档流程），每条发现经独立核查员对照代码与已拍板红线复核（证据不实杀掉、打分不当修正），同步维度由会话逐条抽查源码确认。以下为收敛后的行动清单，均未开工（标注 ✅ 的为当批已完成部分）；完整证据行号见当次会话记录。

### P0 立即项

- [x] **已解决（2026-08-22）：真机同步 TLS `unexpected EOF`——服务器侧链路最终恢复。** 期间曾发生“文档称橙云、Cloudflare 权威 DNS 实际仍灰云指向阿里云 `8.148.11.79`”的误判；随后又短暂灰云直指海外 VPS `34.104.162.153`，导致公网 `:443` 落到 TannerLab Authorization Server、`/v1/health` 返回 401，而正确 Solum Center 只在 `:8443` 返回 200。用户最终启用橙云后，Cloudflare 与 Google DoH 均返回边缘 IP `104.21.64.111` / `172.67.182.236`，独立公网 HTTPS trace 命中 CF 边缘，`/v1/health` 返回 `0.6.4 / 11c1e246…` 200。现链路才真正是客户端 → CF 边缘 :443 → VPS openresty :8443 → solum-cloud；阿里云 TLS 指纹过滤已退出主链路。详见 PITFALLS 同日最新条目。
- [ ] **待验证：vivo 上用现有生产数据复测 CF 后的 Rust 同步。** 服务器侧前提已满足，但不能用浏览器、远端抓取或主机 curl 代替 rustls 真机结果。设备可用后等待 DNS TTL、重连网络/VPN，再执行一次登录态 push/pull 并保存状态码/错误链；不得卸载、清数据或删除用户内容。若仍失败，先比较设备解析结果与 CF 边缘响应，避免把本地 DNS/SNI 缓存误判为生产回退灰云。
- [x] **已做：push 侧毒丸防护（2026-08-21）。** 永久拒绝（507/403/429/400/413）只作废 push 半轮、继续 pull，原因持久化 `meta.sync_push_blocked`（下次成功推送自动清除）；单条超 7 MiB 的 op 暂存 `sync_push_holds`（只存 oplog 指针），游标照常前进；瞬时失败同样只跳过 push 半轮。设计取舍见 MISC 2026-08-21，ARCHITECTURE §3.8 已补条目。**遗留**：pull 侧 403（设备撤销同样阻断 pull）是否也要跳过半轮+持久标记，见下方新增条目。
- [x] **已做：SQLite 双连接补 `busy_timeout=5000ms`（2026-08-21）。** `Store::open` 一处覆盖 orch 与 sync_store；PRAGMA 断言测试钉住。CoreError::Storage 抹掉 rusqlite 错误码仍是小项（见小项打包）。
- [x] **已处理：公开仓泄露本机路径（仓库内容侧）。** 根 README 与后续文档复核发现的本机盘符路径已改为相对描述或脱敏占位，tauri command 计数改为「100+」。**收尾（2026-08-22 用户拍板）：GitHub 公开仓暂停维护、专心做 gitea**；AGENTS.md 远端政策已更新，本地 remote `origin` 更名 `github-paused`，公开边界清单条款保留备将来恢复。注意 remote 改名本身不会改分支级上游，裸 `git push` 的剩余缺口见下一项。
- [ ] **防误推：把本机 `main` 上游改为 `gitea/main`。** 当前 `branch.main.remote=github-paused`，所以在 `main` 裸跑 `git push` 仍会指向已暂停的 GitHub；执行并核对 `git branch --set-upstream-to=gitea/main main`，随后用 `git status -sb` / `git branch -vv` 确认。该项只改变本机 Git 配置，不代表授权执行 push。
- [x] **已做（仓库侧）：CI 补到真实开发路径（2026-08-21）。** 新增 `.gitea/workflows/ci.yml`（rust 三门含 Tauri Linux 依赖 + web-and-cloud 挂 postgres service，集成测试防静默 skip 有显式断言）；`.github/workflows/ci.yml` 加 android-manifest 门禁（gradle 仅跑到 manifest 合并 + aapt 断言三组件）并升级 web-and-cloud；配套 `scripts/gen-android-ci-settings.mjs` 解决 `tauri.settings.gradle` 为 gitignore 绝对路径生成文件、裸 checkout 无法跑 gradle 的问题（已验证与 tauri CLI 产物一致）。**边界**：Gitea 侧需服务器安装并注册 act_runner（ubuntu-latest 标签、docker 模式）才实际生效；Android job 依赖 GitHub 托管 runner 预装 SDK，自建 runner 需先补 sdkmanager 步骤。

### 结构性（排期）

- [ ] **拆分 `store.rs` / `orchestrator.rs` 两个超大模块。** 2026-08-22 复核分别为 6104 / 5747 行；数字会继续变化，不再把旧行数当稳定锚点。分节注释完整即天然拆分线；Rust 多 impl 块机制下零 API 破坏，测试耦合已验证极低。每步独立提交三绿再进下一步；拆到哪节补哪节测试（email/soulous/notification_intelligence/store-sync 段测试密度与风险不成比例）。
- [x] **已被 2026-08-22“单服务器架构收口”取代：不再收敛双实现。** PostgreSQL `postgres-server.js` 保留为唯一服务实现；SQLite `server.js` 在迁移完成后直接退役，不再为两入口抽公共层或继续投入功能建设。
- [ ] **发布核对脚本化**：`scripts/verify-android-release.mjs`（aapt badging / ZIP 条目 / apksigner / manifest 三组件断言，末尾输出 CHANGELOG 可粘贴证据块）+ 版本三元组一致性检查（tauri.conf.json version / versionCode / Cargo.toml workspace version）+ Android 启动时 AlarmReceiver 自检绊线（PackageManager.getReceiverInfo，缺失时设置页显式告警）。0.2.6 生产 P1 正是从手工核对网缝隙漏出的。
- [x] **已被 2026-08-22“加密云备份取代多主同步”取消：不再设计 sync_oplog 压实。** oplog 在新备份/恢复闭环验收前仅维持现状以保护迁移；随后整体退役，不再为其增加长期维护机制。
- [x] **已被 2026-08-22“退役独立中继”取代：不再扩建 Rust 中继清扫。** PostgreSQL 同步历史在快照/压缩协议出现前继续永久保留并以租户硬配额拒绝新写入；旧 Rust 中继只完成迁移和下线，不再增加定时清扫、统计或水位线能力。
- [ ] **memory_ledger 分页**：全表加载无 LIMIT，作为 F12 日常界面在长寿命设备上内存与 JSON 载荷线性无界；加 (layer, limit, cursor) 按层懒加载，导出继续走 export.rs 独立路径。
- [ ] **CSP 去 unsafe-inline 收尾**（MISC 2026-07-21 明记「别忘了」）：tauri.conf.json script-src 仍带 unsafe-inline。2026-08-22 实测 `index.html` 为 394,377 UTF-8 字节、其中唯一内联脚本 256,535 字节；相对 07-21 的 268,890 / 184,401 字节分别增长约 46.7% / 39.1%，不能把整文件增幅误写成脚本增幅。拆 `dist/app.js` 时 `check-frontend.mjs` 必须同步改为校验 `app.js`——否则 `<script src>` 被捕获为空内容、`vm.Script("")` 静默通过，静态检查空转。
- [ ] **云服务小加固打包**：/v1/auth/refresh|logout 补 IP 限流（认证面唯一没节流的未鉴权入口，垃圾 token 也打满一次 DB 事务 + advisory lock）；ai.usage_events 增加保留期清理（全库唯一无清理表）；~~compose 增加 pg_dump 备份编排 + 恢复演练~~ ✅ **已做（2026-08-22）**：每日 UTC 03:23 自动 pg_dump（保留 14 份，`/opt/1panel/backup/solum-cloud/`），全新容器恢复演练通过（10 表行数比对一致、0 错误，手册 RESTORE.md 同目录）；recovery envelope 与 /v1/devices 补运行时集成测试（现仅 contract 正则断言，create-only 语义测不出来）。
- [x] **已完成：文档检索层**（本批）：PITFALLS/MISC 尾部时序错乱修正 + 头部主题索引与插入规则；CHANGELOG 版本速查索引；docs/README.md 导航升级；ARCHITECTURE 状态句与 §6 标题更正。详见 CHANGELOG 当日条目。

### 小项打包

- [x] **已被 2026-08-22“加密云备份取代多主同步”取消：不再扩展 pull 侧 403 语义。** 当前错误保持显式可见直到迁移；资源投入转向备份坏包防护、设备代次接管和换机恢复。
- [ ] `orchestrator.rs` 三处多步写漏包 `with_transaction`（`persist_event_with_scope` / `fire_due` / `set_importance_rule`；方法已支持嵌套 savepoint，机械缩进调整 + 「insert_notification 故障时事件回滚」回归测试）。
- [ ] CoreError::Storage 保留 rusqlite 错误码（BUSY/LOCKED/CONSTRAINT 现被 to_string 抹掉）；row mapper 对损坏 JSON 不再 `unwrap_or_default()` 静默吞（至少 eprintln 带 id 或可见计数）。
- [ ] push 装箱循环 O(n²) 重复加密改估算式装箱（每加一条 op 就整批重新序列化+ChaCha 加密；积压越大浪费越多，恰是离线回连场景，手机 CPU 上可感知）。
- [ ] 根目录约 35MB 一次性产物（output/ 旧 APK、演示视频、playwright 日志）移仓库外归档目录。**注意：根目录 `solum.sqlite*` 是「桌面与 CLI 共库」的刻意设计（MISC 有记录），勿清。**
- [ ] 通知渠道 ID `pa_notification_pipeline` 改名残留：与 2026-07-20 改名决议矛盾（同批改了 pa_reminders 渠道），改名（带 deleteChannel 迁移）或补进刻意保留清单，二选一显式拍板。
- [ ] TODO 受阻项代码侧推进：debug-only 进程自杀入口（前台服务随进程死、alarm 由系统持有，正好复现「后台回收后 alarm 存活」场景，替代被禁用的 force-stop）；福利告警测试无需新代码——POST `/v1/alerts` status 含 "test" 即可触发客户端「福利监控测试」通知，把配方写死即可从受阻转待验证。
- [ ] CHANGELOG 全史切版：大量粗体顶层条目仍堆在唯一 `[Unreleased]` 下，并存在千字级单行；条目数和最长行会随文档提交变化，不再用易过时数字描述。需逐条定版本归属，属专门批次；当前以头部版本速查索引缓解。

## 2026-08-20 验收回修

- [x] **已修复/P1：通知、捕获、息壤和邮件页面的 raw exception 展示。** 生产可见状态、空状态和错误卡统一经过 `safeUserError`；前端语法检查拒绝高风险 `String(e)` 文本渲染。
- [ ] **待验证：新 0.2.8/1018 APK 的 vivo 覆盖安装与提醒 E2E。** 新包的 manifest、签名、ABI 和 `AlarmReceiver` 已核对；设备当前 ADB `device not found`，不能把旧包安装证据或 dumpsys alarm 当作本轮通过。

## Android 生产链路审计（2026-08-19）

### 已复现缺陷与 UX 问题

- [x] **P1 / 已修复：网络失败时账号注册不再暴露传输层内部错误。** Android UI 统一映射为可重试提示，详细错误只进控制台诊断日志。
- [x] **P1 / 已修复：破坏性确认弹窗改用用户可理解的操作名称。** Guard 的一次性令牌、人工确认、审计和返回键取消保持不变。
- [x] **P2 / 已修复：记忆台账来源改为用户语言并保留可追溯分类。** 不再渲染 `intent=ingest_event`、`event#1` 等实现标识。
- [x] **P2 / 已修复：guest 首页示例统一显示“示例”标记。** 离线演示输入仍可用且不会写入真实数据。
- [x] **P1 / 已修复构建输入：Android Release 版本、构建号、签名与 ABI 在构建前后统一核对。** 新包版本为 0.2.8 / versionCode 1018，实际 APK 元数据仍需构建环境复核。
- [x] **P1 / 已修复源码：`app_info` 由 setup 管理的 `AppState` 提供。** 新包安装后仍需在 vivo V2405A 冷启动复核无 `state not managed`。
- [x] **P1 / 已修复：24 小时数字时间使用数字邻接保护。** `今天22:08开会` 不再触发 09:00 默认值，并新增中文、英文、日期后缀和无效时间回归测试。
- [x] **P1 / 已修复构建输入：Android 项目重新生成后合并 manifest 必须包含 `dev.solum.alarm.AlarmReceiver`。** 设备进程外到点通知仍待新包真机复测。

### 真机补测矩阵

- [ ] **待验证：生产账号会话。** 确认当前 `0.2.6 (1016)` 是 guest、已登录或会话过期；只通过应用 UI 登录/注册，不读取或输出用户密码、token。
- [ ] **待验证：登录后 Activity 重启。** 验证成功鉴权会重启到新 profile，而不是窗口消失、白屏或继续使用旧 guest 数据。
- [ ] **待验证：生产 AI。** 验证模型目录、用量显示和至少一条最小化测试问答；记录模型/HTTP 结果但不记录账号秘密或私人对话。
- [ ] **待验证：端到端加密同步。** 检查同步状态、设备标识和手动同步结果，确认账号数据与 guest profile 不混用；不删除或覆盖现有远端数据。
- [ ] **待验证：本地事件与系统提醒。** 创建可识别的合成 QA 日程，检查本地入库、提前量与精确 `RTC_WAKEUP` alarm；测试后不删除用户既有事件。
- [ ] **待验证：后台进程回收后的提醒。** 仅用 `am kill` 模拟系统回收，确认 alarm 保留并在到点时通知；不得用“强行停止”替代该测试，因为 Android 会主动取消/阻断 alarm。
- [ ] **待验证：通知监听权限与前台处理服务。** 检查系统授权、应用白名单、`dataSync` 前台服务和 OEM 后台限制；仅在现有授权允许时用另一应用发送可控测试通知。
- [ ] **待验证：电池与自启动策略。** 检查电池优化白名单、后台高耗电、自启动/关联启动设置是否会中断 Solum；不擅自改变用户的 OEM 系统策略。
- [ ] **待验证：生产福利告警。** 若生产环境存在安全、幂等、可清理的测试告警机制，验证 `/v1/alerts` 到 Android 通知；否则标记受阻并说明缺少测试入口。
- [ ] **待验证：Health Connect。** 检查系统可用性与当前授权状态；未经用户明确操作不新增健康权限，不读取或记录真实健康明细。
- [ ] **待验证：退出登录隔离。** 确认退出后回到 guest profile，账号数据不出现在 guest；不以清数据方式验证。
- [x] **已通过：同机生产 AI 最小问答。** 真机现有 `xiaorana` 会话读取到模型 `mimo-v2.5`、权益 `standard 1000`，发送 `OK` 后收到“收到！有什么我能帮你的吗？”。默认输入法临时切换到已安装 ADB 键盘仅用于注入，随后已恢复讯飞输入法。
- [x] **历史对照已保留、当时归因已被后续探针推翻。** 真机 Chrome 曾打开 `https://solumsync.tannerlab.xyz/v1/health` 得到 `status=ok`、`0.6.4 / 11c1e246`，同机 Solum Rust `/v1/pull` 报 TLS `unexpected end of file`。后续 TLS 指纹矩阵证明当时是阿里云侧按 SNI + ClientHello 过滤，并非客户端 fake-IP；2026-08-22 橙云最终启用后服务器侧路径已更换，仍需按上方新项做 vivo rustls 复测。
- [x] **已通过：通知监听授权状态。** `enabled_notification_listeners` 与 `dumpsys notification` 均包含 `dev.solum.app.SolumNotificationListener`；当前通知白名单仍为空，应用页正确显示“还没有已捕获通知”。
- [x] **已通过：Health Connect 权限页可达，未授权真实数据。** “穿戴 → 去授权”打开系统 Health Connect 权限页，列出步数、睡眠、心率；按隐私边界未点击“允许”，所以真实健康读取仍未验证。
- [ ] **受阻：通知监听端到端捕获。** 权限已存在，但当前应用白名单为空；为了不改变用户隐私设置，未擅自加入 QQ/其他应用，也未读取个人通知正文。

### 已有验证基线

- [x] **已通过：模拟器本地核心闭环。** `0.2.7 (1017)` x86_64 Release 已验证隐私门、同意后通知权限请求、guest、英文事件摄取、30 分钟提醒、精确 AlarmManager 注册、进程重启后的事件/alarm 保留、计划/记忆/搜索/设置入口和 Guard 返回键取消。
- [x] **已通过：生产服务主机侧最小链路。** `https://solumsync.tannerlab.xyz` 的注册、登录、模型目录、用量、AI completion、pull、alerts 与测试会话 logout 均返回成功；该结果只证明服务端链路，不替代 Android 真机验证。
- [x] **已通过：真机安装状态。** vivo V2405A（Android 16 / API 36）当前安装 `dev.solum.app` `0.2.6 (1016)`，通知、网络、精确 alarm、开机恢复、前台 data sync 与 wake lock 权限已授予；应用数据保持原样。
- [x] **已通过：干净 target 的 workspace 测试基线（2026-08-21 解决）。** 根因是整仓搬迁时把 23.8 GiB 旧 `target/` 原样拷来，3000 个文件引用旧绝对路径，tauri build script 读不存在的权限文件即挂；`cargo clean` 全删重建后基线 **353 项全绿**（现 361 项，见 CHANGELOG 当日条目）。教训记入 PITFALLS 当日条目：target 永不随仓库迁移。
- [x] **已通过：真机 QA 事件入库与 alarm 注册证据。** “今天晚上十点十分开会，QA真机提醒测试2” 写入事件 `2026-08-19 22:10`，生成 `21:40` 提前 30 分钟提醒；系统显示 exact `RTC_WAKEUP`、`exactAllowReason=policy_permission`。到点未产生通知，原因见上方生产 APK receiver 缺失。
- [ ] **受阻：完整后台回收提醒。** `am kill dev.solum.app` 未终止进程，因为通知监听 `dataSync` 前台服务仍在运行；不能用 force-stop 代替（会由系统取消 alarm）。需后续提供不改用户配置的进程级故障注入/构建版测试入口。

### 证据

- 审计记录：工作区外 `_scratch/solum-android-prod-audit-20260819.md`
- 截图：工作区外 `_scratch/solum-027-*.png`
- 当前代码基线：`088f184`（`main`，本地分支；生产服务版本需以实时只读检查为准）

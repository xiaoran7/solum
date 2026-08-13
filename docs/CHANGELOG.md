# Changelog

> 强制记录：每次新增/完善/修改功能都要在 `Unreleased` 下加一条。格式参考 [Keep a Changelog](https://keepachangelog.com/)。
>
> 版本纪律：Solum 与鸿蒙版每次功能更新或问题修复都必须分别递增各自的应用版本和平台构建号，并把新版本、构建号和产物信息写入各自的变更文档。同一改动涉及两端时，两端都要更新；不得只改代码、不改版本。

## [Unreleased]

- **Solum 0.2.1（Android versionCode 1011）+ solum-cloud 0.5.1（2026-08-13）——中心化账号入口。** `solum-cloud` 根路径新增面向用户的注册/登录与账号空间，账号空间只展示设备、加密同步包统计和结构化福利提醒；新增 `/v1/meta` 中心发现接口。公开部署默认开放注册，账号、同步、设备和提醒收敛到同一 origin；Compose 健康检查修正为真实的 `/v1/health`。Solum 客户端默认使用公开中心并隐藏服务器地址，福利监控器也只需同一套账号密码，不再要求用户理解账号服务器与 relay 两个地址。
  - 独立福利监控器改为单实例后台服务与页面心跳分离：重复启动时，已有控制台标签仍活跃则不再开页，控制台已关闭才重新打开；福利恢复不再主动打开详情网页，只保留本机与 Solum 通知，用户点击通知后进入详情。
  - 验证：Rust 工作区全部测试通过（solum-core 311 项、通知队列 16 项、relay 7 项、闭环 4 项等），Clippy `-D warnings`、fmt、中心服务 11 项 Node 测试、两处前端脚本检查与 PowerShell 语法检查通过；中心页在 1280px 与 390px 视口无横向溢出、控制台零错误。arm64 Release APK 为 27,482,789 字节，SHA-256 `64A8B9964A57B0351A92C8A4CB402E48DCC7C4E9390FEB8D530514164E341EC2`，APK Signature Scheme v2 有效。真实 Android 手机通知仍需安装后验证。

- **2026-08-12 登录恢复同步验证补记**：recovery envelope 密码/账号绑定测试、两设备 create/recover 状态机、账号 token 同源限制、服务端恢复信封固定范围/create-only 与撤销设备同步入口契约均通过；`cargo test --workspace` 全绿（solum-core 311 项、闭环 4 项、relay 7 项等），Clippy `-D warnings`、fmt、前端静态检查（1 段脚本、284 个唯一 id）、中心服务 Node 10 项测试和 Node 语法检查通过；鸿蒙纯逻辑与 SQLite 对拍 198/198、diff 检查通过。当前机器没有 Docker 和完整 DevEco/Hvigor SDK，真实 PostgreSQL/RLS 容器集成与鸿蒙 HAP/真机跨端走查仍无法执行；旧 guest 数据没有自动认领或迁移。

### Changed
- **统一注册入口**：桌面与 Android 的账号页新增“注册”，直接调用中心 `solum-cloud /v1/auth/register`，成功后复用登录的密钥建立、profile 隔离和整进程切换流程；注册是否开放仍由服务端 `SOLUM_REGISTRATION_MODE` 控制，密码至少 12 个字符。Harmony 同步提供相同入口，因此账号只在中心库创建一次，不会向其他容器复制注册。
- **Solum 0.2.0（Android versionCode 1010）+ solum-cloud 0.5.0（2026-08-12）——账号登录即恢复端到端同步主密钥。** 首台已登录设备改用系统随机源生成 32 字节同步主密钥；客户端以账号密码和不可变 user UUID 经 PBKDF2-HMAC-SHA256 600,000 轮导出恢复包装密钥，再以绑定协议、UUID 与 key version 的 AAD 生成 XChaCha20-Poly1305 信封。中心 PostgreSQL 只保存不可读信封；固定 `GET|PUT /v1/keys/recovery` 的 PUT 采用 create-if-absent 并返回当前权威值，双首设备竞态不会互相覆盖，通用设备信封端点不能访问保留 recipient。本机已有密钥会先与远端解包结果核对，冲突即停止账号切换，不静默覆盖。桌面/Android 登录在发布新 session 前完成目标 profile 的建钥/恢复，再沿用既有同步 barrier 整进程重启；独立“同步密码”和可变 relay origin 退役，账号 access token 只能发送到登录服务同源地址。同步页改为展示自动管理状态。架构与服务部署文档同步更新；Harmony 0.3.0 同轮接入相同 envelope/wire 契约并恢复自动密文同步。
- **solum-cloud 0.4.0（2026-08-12）——统一 PostgreSQL 中心库第一阶段。** 新生产编排收口为一个 `solum-cloud` API + PostgreSQL 17：账号、可选开放注册、AI 代理、加密同步 blob、设备、福利告警和同步 key envelope 共用同一 HTTPS origin；端侧 SQLite/relationalStore 与既有行级 LWW 不变，服务端仍不持有业务明文或同步主密钥。中心 schema 按 `auth/sync/vault` 分域，全部租户数据表带 `tenant_id UUID`、启用并强制 RLS，策略只接受 API 从已验签 token `sub` 在单事务内设置的租户上下文；运行账号非 owner/superuser/BYPASSRLS，权限最小化并使用有限连接池，PostgreSQL 不发布宿主端口。Compose 增加幂等迁移容器、私有数据库网络和只绑定回环的 API 端口；旧 Node/SQLite 账号服务与 Rust/SQLite relay 仅保留为测试和迁移兼容，不自动认领 `legacy` 数据。主 README 和中心服务部署说明同步改为统一地址；当前客户端仍需相同同步加密密码，key envelope 只是后续恢复码/旧设备批准流程的服务端地基。
  - 验证：新增 PostgreSQL schema/RLS、事务租户上下文、UUID token 与 Compose 暴露面契约测试；Node 语法检查和原 SQLite 账号回归测试通过。当前开发机未安装 Docker，因此尚未在真实 PostgreSQL 容器上执行集成测试或迁移任何生产数据。
- **Solum 0.1.9（Android versionCode 1009，2026-08-12）——账号 UUID 与本机用户数据、配置全面隔离。** solum-cloud 0.3.0 为用户生成不可变 UUID，access token `sub`、刷新会话外键和 sync relay 租户全部改用 UUID，用户名只做登录/展示；0.2.x 账号库启动时事务迁移 UUID 与现有刷新会话，relay 明确拒绝旧用户名形态的账号 token。客户端按 `profiles/<user_uuid>/` 隔离 SQLite、直连 LLM、同步、邮箱、Soulous 与通知白名单，聊天历史、UI 状态与资料 IndexedDB 同样按 UUID 分区；桌面壳、CLI、Android 后台告警和原生通知监听全部使用同一 active profile，告警游标改用 UUID。未登录数据保留在 guest。登录/退出后整进程重启，确保旧账号的数据库连接、配置、缓存、后台同步和内存上下文全部关闭再切换。端到端加密 key 暂时保留既有用户名 KDF 盐，避免已绑定设备与新版本产生两把不兼容密钥；用户名改名功能上线前另做显式协议迁移。旧账号服务不返回 UUID 时新版客户端拒绝建立隔离账号会话和账号同步。
  - 验证：solum-cloud 两份同源服务各 5 项集成测试通过（含完整/中断数据库迁移、token `sub` UUID 和刷新保持身份）；sync relay 7 项集成测试通过（Alice/Bob blob、告警、统计隔离，非 UUID `sub` 返回 401）；solum-core 308 项单元测试 + 4 项闭环测试通过；完整 workspace、Clippy `-D warnings`、fmt、前端静态检查与桌面/390×844 浏览器交互通过，Android `:app:compileArm64DebugKotlin` 构建成功。
- **Solum 0.1.8（Android versionCode 1008，2026-08-12）——同步与福利通知按 Solum 账号隔离。** `solum-cloud` 继续签发 HMAC access token，`solum-sync-server` 使用同一 `SOLUM_AUTH_SECRET` 验签并以 token `sub` 选择租户；客户端不能提交租户 ID。blob、告警、游标、幂等事件和统计均限定在账号内，旧数据自动进入 `legacy`，旧 `SOLUM_SYNC_SERVER_TOKEN` 只能访问该迁移租户。桌面同步设置只保存中继 URL 和设备端加密 key，账号 access/refresh token 独立存于 `solum-account.json`，401 时统一刷新、原子保存并重试；旧三种同步配置继续可读。
  - Android 福利告警改读账号会话，按用户名隔离游标并在 401 后轮换会话；独立 Benefit Monitor 改为分别配置账号服务器和同步中继，Solum 密码与 access/refresh token 和织境凭据一起由 Windows DPAPI 加密保存。relay 仪表盘用账号登录并只展示该账号统计，账号服务新增 `SOLUM_ALLOWED_ORIGINS` 精确 CORS 白名单；Harmony 的同源 `server/` 契约同步更新。
  - 验证：`cargo test --workspace` 331 个非零测试全绿，Clippy `-D warnings`、fmt、前端静态检查和两份 solum-cloud Node 测试（各 3 项，含 CORS）通过；PowerShell 5.1 语法与 UTF-8 BOM、Android debug/release Kotlin 编译通过。真实浏览器在 1280×720 与 390×844 验证 relay 账号登录后只返回 `tenant=alice`、密码清空且账号 token 不进 localStorage；Benefit Monitor 以同一账号登录并发送测试告警，relay 反查 `alice` 租户内收到 1 条 `benefit-monitor/test`，本机 `/api/config` 不回显密码/access/refresh token；两页均无横向溢出，干净会话控制台零错误。最终 0.1.8 / 1008 arm64 Debug APK 478,846,504 字节（SHA-256 `64858550D5A4B95985BED80C5F34A828A6DF9BDDD3FE47B3F3179F5562CE1A42`）；arm64 Release APK 237,629,285 字节（SHA-256 `BAD9FA38DED0075C065FACBC99050B55B1B59619B642FCD1C3490F96BE8FE118`），包内含 `lib/arm64-v8a/libsolum_app_lib.so`，APK Signature Scheme v2 验证通过。当前 `adb devices` 无在线设备，因此未做手机安装、后台刷新和系统通知运行验证；生产服务也未部署。
- **Solum 0.1.7（Android versionCode 1007，2026-08-12）——独立福利监控与手机恢复提醒链路。** `D:\ClaudeSpace\benefit-monitor` 是只监听本机回环地址的零额外依赖监控应用，带紧凑浏览器前端、20 秒轮询、状态基线、恢复去重、Windows 通知、Solum 失败重试，以及织境三项预设和可新增的织境/HTTP JSON 渠道。`solum-sync-server` 只保留 Bearer 鉴权的 `/v1/alerts` 固定字段中转、7 天留存和幂等事件 ID，不托管监控页；Android 复用 `dataSync` 和 `solum-sync.json`，首次连接只建立游标，此后每 20 秒增量拉取并逐条通知新的恢复/测试事件，点击打开对应 HTTPS 地址。该状态表与端到端加密同步 blob 分离，服务端不接收账户、余额、上游密钥或任意正文。
  - 织境鉴权由手动粘贴访问令牌改为邮箱/密码自动登录；应用保存并自动轮换 access/refresh token，刷新失效时用账号密码重新登录。密码与会话由 Windows 当前用户 DPAPI 加密写入 `secrets.dat`，保存新凭据前先真实验证，失败不会覆盖原有会话；页面、本机 API 与 `config.json` 均不回显秘密。
  - 验证：PowerShell 5.1 语法/BOM 与前端 JS 语法通过；临时 HTTP JSON 源模拟“降级→正常→正常”只产生 1 条恢复事件，首次状态不通知，三类令牌均不从本机 API 回显，重启后配置和状态保持。真实 Chrome 在桌面与 390×844 显示正常，无横向溢出。workspace 329 项测试、Clippy `-D warnings`、fmt 和 diff 检查全绿，Android arm64 debug/release Kotlin 均 `BUILD SUCCESSFUL`。0.1.7 / 1007 arm64 Release APK 为 27,369,613 字节，SHA-256 `135D24F06964EBD73AF0EF37B11B7A775C234F72360C09FAE8583807ECAC3B69`，APK Signature Scheme v2 验证通过。生产 relay 未部署，手机未安装新版，因此真实手机通知链路尚未运行验证。
- **Android 通知权限顺序收紧（2026-08-11）**——不再在 Tauri `setup` 阶段抢先弹出 `POST_NOTIFICATIONS`；只有用户在应用内完成隐私政策同意后，前端才调用一次原生权限命令。拒绝通知权限不阻断主界面，通知监听权限仍保持独立的系统设置开关。
  - 验证：清空 `Medium_Phone_API_36.1` 模拟器应用数据后冷启动，隐私门出现时前台仍为 `MainActivity`；点击同意并确认记录落盘后，系统通知权限弹窗才出现；拒绝权限后主界面继续正常运行。新增静态回归检查，禁止 `setup` 直接申请权限或前端把申请放到隐私同意之前。修复版尚待 vivo 真机重新连接后覆盖复验。
- **Solum 0.1.5（Android versionCode 1005，2026-08-11）——安全与隐私默认值收口。** 恢复 SQLite、云端/同步/邮箱/账号凭据、Android SDK 路径与 release keystore 的 Git 隔离，同时允许 README、docs 与演示材料正常进入版本管理；通知上云从新安装默认开启改为默认关闭，已有显式选择保持不变，隐私政策升级到 v2 并同步应用内正文；`event_cancel` 删除公开直调 IPC，改走 Guard 的后端预览、人工确认、效果摘要绑定与一次性令牌，事件在预览后发生变化会自动拒绝执行。新增零依赖前端静态检查与 GitHub CI，持续执行 Rust test/clippy/fmt、前端脚本/id/高危 IPC 边界检查和 solum-cloud 测试。
  - 验证：`cargo test --workspace` 327 个非零测试全绿，Clippy `-D warnings` 与 fmt 零漂移；前端静态检查通过（1 段 inline JS、284 个唯一 id、无 `event_cancel` 直调及权限顺序回归），solum-cloud 2 项集成测试通过，Android Universal Kotlin 编译通过。当前 arm64 debug APK 为 243,768,670 字节，SHA-256 `8868A11E19A48DEE250F1815A1028BEFA767CA7ED2D79B687E2E66539CF2B37B`，`aapt` 确认 versionName 0.1.5 / versionCode 1005，包内含 `lib/arm64-v8a/libsolum_app_lib.so`。已在 `Medium_Phone_API_36.1` 模拟器和 vivo V2405A（Android 16 / API 36）真机安装并启动基础版，系统确认 `MainActivity` 位于前台且进程持续存活；首启隐私政策 v2 正常渲染，“不同意并退出”会结束应用，同意后进入主界面，拒绝通知权限时仍可正常运行，真机四根主导航与输入框回归通过。通知监听权限保持未开启，未读取其他应用通知。未生成 release 签名包。
- **Solum 0.1.4（Android versionCode 1004，2026-07-29）——排版、交互、缓存与持久化基础体验收口。** 中文标题降低过紧字距，正文统一 16px / 1.72 行高与严格 CJK 换行，表格、时间和统计数字统一等宽数字；移动端底栏标签不再跌破 12px。切页从无条件 `refreshAll` 改为按当前视图定向刷新，启动去掉重复全量刷新，加入延迟出现且只动画 transform / opacity 的顶端进度反馈，`prefers-reduced-motion` 下退为静态状态；新增滚动位置恢复、`/` 聚焦本地搜索与 `Alt+1…4` 四根导航快捷键。幂等 IPC 读取仅在进程内做 0.7–1.5 秒请求合并，任何写入成功后整代失效；邮件正文/搜索、隐私同意状态与秘密配置明确不缓存，memory recall 继续直读权威存储。导航位置、每视图滚动和按会话聊天草稿落本机 `solum.ui-state.v1`，不包含邮件、搜索词、通知正文或凭据；聊天历史异步批量写入并在 `pagehide` 强制刷盘，同时按 2.4M 字符总预算优先保留最近会话，避免长期使用顶满 WebView `localStorage` 配额。
  - 验证：inline JS 语法、284 个 HTML id 唯一性与 `git diff --check` 通过；实页 1280×720 验证四根导航、计划二级入口、无横向溢出、控制台零错误，重载后能恢复「提醒」视图与按会话草稿，`/` 与 `Alt+1` 快捷键点通。`cargo test --workspace` 327 个非零测试全绿，Clippy `-D warnings`、fmt 与 `cargo build -p solum-app` 全通过。通用 Release APK（arm64-v8a + x86_64）为 51,596,021 字节，SHA-256 `2AF989034683640243D9BD4473BBB22A7B3CA119FFC44C83B9325925CA799885`，versionName 0.1.4 / versionCode 1004，v2 签名有效，证书 SHA-256 `F3467B14C3CA39A88D74B85F10800565D0A81BF67FCF392147EC6B4F6AEFCA87`。当前 `adb devices` 无在线设备，因此本轮未做 Android 运行时安装/启动验证。
- **Solum 0.1.3（Android versionCode 1003，2026-07-29）——按参考图重做导航心智模型，而不只复刻外观。** 一级导航从 6 个功能组收敛为「今天 / 记忆 / 计划 / 搜索」四个稳定根：今天承载对话与当日状态，记忆聚合台账/行为/穿戴/复盘/通知回看，计划聚合日程/提醒/建议；邮箱、设置、资料与组件移入桌面侧栏工具区和移动端右上信号环工具面板。新增零云端、零新数据库的统一本地搜索页，检索当前已加载的计划、记忆和行为数据，结果点击后回到所属视图继续处理。真实浏览器 1440×900 与 390×844 验证主导航严格为四项、工具路径闭环、搜索结果返回记忆上下文、无横向溢出。
  - 验证：inline JS 语法与 283 个 HTML id 唯一性检查通过；浏览器控制台零错误；`cargo test --workspace` 327 个非零测试全绿，Clippy `-D warnings`、fmt 与 `cargo build -p solum-app` 全通过。通用 Release APK（arm64-v8a + x86_64）为 51,591,889 字节，SHA-256 `D703658DFF870CB2FEC4CD9279CA9A9FD9A7641CB990A5510F4E632240690F1D`，v2 签名有效且证书未变。已在 Android 模拟器覆盖安装并启动，系统确认 versionCode 1003 / versionName 0.1.3、`MainActivity` 位于前台，WebView 可访问性树中四个主导航入口齐全；未代用户点击首启隐私同意，因此设备内交互走查止于隐私门。
- **Solum 0.1.2（Android versionCode 1002，2026-07-29）——壳层 UI 全面重构为“黑白编辑部 + 微粉信号”体系。** 参考用户提供的四张移动端界面提炼高留白、强字级、极轻轮廓、超大圆角、黑色实心主动作与微粉状态点，但保留 Solum 真实的 6 组一级导航、对话主界面、日程/提醒/建议、行为记录、邮箱、记忆台账、设置多级目录、工作台抽屉、隐私门与 Guard 流程。桌面端改为 248px 圆角侧栏 + 独立主画布 + 宽屏今日面板；移动端改为品牌顶栏 + 悬浮胶囊底栏，390×844 无横向溢出。新增零依赖动效系统：按 GSAP core/timeline/performance 原则用 Web Animations API 实现可中断的“标题 → 操作 → 内容”短时间线、异步卡片错峰升入和导航反馈，只动画 transform/opacity，`prefers-reduced-motion` 下即时切换。深色主题、键盘焦点、≥44px 触控区、高危红色语义和全部既有 DOM/IPC 契约保持不变。验证：inline JS 语法与 269 个 HTML id 唯一性检查通过；真实浏览器 1440×900 与 390×844 走查无控制台错误、无页面横向溢出，日程组切换、设置目录与设置二级面包屑均可用；`cargo test --workspace` 全绿、Clippy 零告警、fmt 零漂移、`cargo build -p solum-app` 成功。通用 Release APK（arm64-v8a + x86_64）为 51,584,537 字节，SHA-256 `3C404B60C779DB6754C9AEEC374E00D1BB72ACE6EEA43B823305CC8DFCB81B9A`，v2 签名验证通过，沿用原证书，可覆盖升级；产物位于 `crates/solum-app/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk`。
- **Solum 0.1.1（Android versionCode 1001，2026-07-29）**——首启隐私说明改为口语化表达，去除数据库、加密算法、内部模块名等实现术语；摘要与全文统一使用安全的 Markdown 渲染，不再直接显示 `**`、反引号等标记；“同意并开始使用”和“不同意并退出”点击后立即显示进度，失败或超时会在当前页面给出明确提示，Android 拒绝路径直接结束应用进程。设置页同步使用同一份可读政策，不再向用户展示同意记录的内部文件路径。重新打包的通用 Release APK 为 51,568,233 字节，包含 `arm64-v8a` 与 `x86_64`，SHA-256 为 `1A6E21570E2AD795497EDEBF7C63D85D772186FB451DC8A652F911C32FF43EF9`，沿用原签名，可覆盖安装升级。

### Fixed
- **Android 首启隐私说明的同意/不同意按钮无可见反馈（2026-07-29）**——移动端现在将 `SOLUM_PRIVACY_CONSENT` 固定指向应用私有数据目录，避免把设备级同意记录写到不存在或不可写的工作目录；同意写入失败会直接显示在隐私门内，不再被隐私门遮住的 toast 吞没；不同意改为调用原生退出命令，确保 Android 也会结束应用而不是只尝试关闭 WebView 窗口。

### Added
- **首启隐私门 + 应用内完整隐私政策（2026-07-27，自 PA-harmony `privacyConsent*/privacyPolicy.ets` 回移，用户点名要这一项）**——`solum-core::privacy` 新模块：`PRIVACY_POLICY_VERSION`（实质变化才递增，措辞润色不算）、`has_current_consent`（**只认与当前版本完全一致的记录**——"比当前更高"同样不放行，那只可能来自坏文件/篡改/别的分支构建）、`PrivacyConsent` 落 `solum-privacy-consent.json`（`SOLUM_PRIVACY_CONSENT` 可覆盖，走 `fsatomic::write_atomic`）。壳层 2 个命令 `privacy_consent_status` / `privacy_consent_accept`，前端加全窗口门（`z-index` 高于 toast，无遮罩关闭/无 Esc/无取消路径，门内就地展开全文而不跳视图，不同意＝关窗退出），「设置 → 隐私」新增「完整隐私政策」折叠区并显示同意时间与记录文件路径。
  - **政策正文是重写的，不是搬运的。** 鸿蒙 0.2.0 的正文写着「本版本不提供多设备同步」「不使用第三方通知监听」——这两句对本仓都是**假的**（本仓有端到端加密同步、有 Android 通知捕获、还有邮箱连接器）。照抄等于让对外声明与代码事实脱节。正文按 `docs/PRIVACY.md` 与实际代码重写，只沿用鸿蒙的结构。**有一条测试专门钉死这两句不许出现**，防止以后有人图省事贴回来。
  - **同意记录刻意不进 SQLite、不进同步**：「这台设备上有人点过同意」是设备级事实，不该由另一台设备的同意状态代答（判据同鸿蒙）。**`solum-cli` 不过这道门**——它是本机自动化入口，加交互式同意只会卡住脚本，且不新增任何数据出境路径。
  - 验证：Rust 5 条（版本判定含未来版本、JSON 往返、落盘生命周期、坏文件/旧版本重新征求、正文红线）；真实浏览器 mock-IPC 走查断言落在渲染上——门 `display:flex`、矩形 1280×720 **恰好铺满视口**、`elementFromPoint(中心)` 命中门内、「同意」按钮在视口内、全文/摘要双向切换、同意后 `display:none` 且 body 滚动锁解除。
- **多入口采集领域层 + 「设置 → 数据入口」页（2026-07-27，自 PA-harmony `core/capture.ets` 回移，用户点名要这一项）**——`solum-core::capture`：入口目录、待确认草稿队列 `CaptureInbox`、给人核对用的线索抽取（事项/时间/地点/金额）。壳层 4 个命令（`capture_entry_points` / `capture_inbox_list` / `capture_inbox_add` / `capture_inbox_discard`），新增设置二级页展示各通道真实状态与待确认区。
  - **入口清单按本仓真实能力给状态，没有照抄鸿蒙**：鸿蒙把「第三方通知」标成「鸿蒙未开放」、把系统分享与截图 OCR 标成「可用」，这三条对本仓全都反着——本仓有 Android 通知捕获（桌面没有，故按平台算出「可用」/「桌面无此机制」，**不是**「待接入」，后者会暗示以后会有），却没有系统分享目标（manifest 里没有 `ACTION_SEND`）也没有任何 OCR 实现。另有一条测试禁止把未实现的入口标成 `Ready`。鸿蒙清单里的「桌面 Agent」本仓不列——本仓自己就是那个桌面端。
  - **收到不等于保存**：外部输入只进进程内存队列，「放入输入框」也只把正文送进对话框、仍由用户自己发送，落库照走既有 ingest 管道，没有任何自动写库路径。队列有条数与单条长度上限（超长截断而非拒绝，拒绝会让用户以为分享失败）。
  - **范围如实说明**：桌面端目前**没有**系统分享/OCR 生产者，页内的粘贴框是手动等价入口；`CaptureInbox` 在有真正生产者接入前只有这一条来路。
  - 验证：Rust 12 条（平台差异、未实现入口不得标可用、字符串往返、四类线索、显式地点优先于自然语言地点、裸 URL 不当事项、中文按**字符**截断不 panic、队列上限挤出最旧、空标题兜底）；浏览器走查断言 8 行入口渲染与各自状态标签、粘贴→待确认（线索摘要「事项：缴纳体检费 · 时间：明天 · 金额：128 元」）→采用→行消失全链路。

### Fixed
- **采集内容「放入输入框」会静默丢掉换行（2026-07-27，浏览器走查发现，未进过发布）**——对话输入框是单行 `<input type="text">`，把多行采集正文直接赋值给它，浏览器会**静默吃掉换行**：`缴纳体检费\n明天 上午10点…` 变成 `缴纳体检费明天 上午10点…`，词边界消失，后续抽取会解错。改为显式把换行折成空格再填入，而不是交给浏览器去丢。这类缺陷编译与单测都测不到——它只在真实 DOM 里发生。
- **`server/.dockerignore` 回移时漏拷，同源两份拷贝出现漂移（2026-07-27，复核回移完整性时发现）**——对 `PA-harmony/server/` 与本仓 `server/` 做逐文件比对，除刻意不同的 README 首段措辞外，唯一差异是本仓缺这个文件。影响有限（`Dockerfile` 是白名单式 `COPY package.json ./` + `COPY src ./src`，本就不会把 `data/`、`.env`、运行时 `*.db` 打进镜像），但它会让整个 `server/` 目录连同本地运行数据一起进入 docker 构建上下文，且**同源拷贝出现任何差异本身就是该违反同步义务的信号**。已补齐为与鸿蒙侧逐字节一致。复核同时确认：鸿蒙 `core/` 全部 39 个模块中，属"鸿蒙首创且主仓缺失"的只有当日已移植的四项，其余要么是主仓移植过去的对照实现，要么在 MISC 当日条目里有明确的不移植裁定。集成测试 2/2 仍绿。

### Added
- **solum-cloud 账号云代理服务端移植进主仓（2026-07-27，`server/`，自 PA-harmony 0.2.0 回移）**——Node 24 零依赖单进程：`/v1/auth/login|refresh|logout`（scrypt 密码哈希、HMAC 15 分钟访问令牌、30 天刷新令牌哈希存储+每次刷新轮换、登录限流）+ `/v1/ai/chat/completions` 固定转发 MiMo Token Plan（`stream:true` 时 SSE 逐块透传、中断硬断连接不伪装成功）+ `/v1/health`。SQLite 用户库、Docker 部署、`.env.example` 占位符为过不了启动校验的 `CHANGE-ME`。**与 `PA-harmony/server/` 同源同契约，改动要两边同步**（两处 README 均已注明）。集成测试 2/2（Node 24 本机跑通）。它与 `solum-sync-server` 职责完全不同：sync-server 中转加密业务数据，solum-cloud 只鉴权云端 AI、绝不接收业务数据。
- **客户端「账号登录」云端模式（2026-07-27，同上批次）**——直连厂商 API Key 之外的第二条云端通路，语义对齐鸿蒙 0.2.0 客户端（accountRules/accountClient）：
  - `solum-core::account`（新模块）：`AccountSession` 落 `solum-account.json`（gitignored；与鸿蒙四字段文件形状兼容，本仓额外存 `model` 字段，读旧形状按默认模型补齐）；服务器地址复用 `net::validate_endpoint`（HTTPS 强制、仅回环放行 http）；模型名校验逐条对齐服务端 `validateModel`；登录/刷新/退出（退出是本机优先、服务端吊销尽力而为）；`AccountReasoner` 实现 `Reasoner`（非流式+流式，think-stripping 复用 `llm.rs` 同一套助手），401 刷新一次并重试，**轮换后的令牌对立即落盘（重试再失败也不丢新令牌）**。
  - 壳层 4 个新命令 `account_status_get/login/logout/model_save`（登录/退出为 async，网络往返不冻结 UI 线程；密码只存在于登录调用内）；启动装配与 `llm_config_save` 遵守裁决规则「登录期间账号代理优先于直连」；退出登录回落直连配置或全离线（`Orchestrator::clear_reasoner` 新增）。移动端补第五个 `#[cfg(mobile)]` 配置路径注入块（`SOLUM_ACCOUNT_CONFIG`）。
  - 前端「设置 → 云端接入」新增「账号登录」折叠区：状态行/登录表单/模型保存/退出，页脚云端状态联动；登录成功后密码框立即清空。
  - 验证：`cargo test --workspace` 309 全绿（含 account 6 条：会话往返与鸿蒙形状兼容、缺字段/明文 http 拒绝、模型校验、畸形登录响应不落成"看似已登录"的坏会话、401→刷新→重试的调用序列与轮换持久化、刷新失败映射"请重新登录"）；clippy `-D warnings` 零告警；fmt 零漂移；mock-IPC 真实浏览器走查登录/改模型/退出全状态机（渲染断言：状态行文案、表单/按钮显隐、密码清空、页脚文案与 title）。**验证缺口如实记录**：Rust 客户端 HTTP 层未与真实 solum-cloud 联调（服务端契约由其自身集成测试覆盖，客户端刷新/重试逻辑由注入 fake 的单测覆盖，但两者没有拼在一起跑过一次真实登录+对话）；浏览器走查中 toast 可见性断言受限于量具（见 PITFALLS 当日条）。
- **对话 Markdown 渲染（2026-07-27，同上批次）**——助手回复不再是纯文本一坨：自研无依赖块级渲染器，解析语义逐条对齐鸿蒙 `core/markdown.ets`（标题 1-4 级/段落聚合/引用/无序列表/任务列表 ☐☑/有序列表/分隔线/围栏代码/GFM 表格）；行内标记剥成可读文本，**链接渲染为「文字（URL）」、图片渲染为「图片：alt」——不可点击、不加载远程资源**；全部走 `textContent` 构建 DOM，绝不 `innerHTML`。表格复用既有 `dataTable`（`.dt` 样式）并包 `.tscroll` 横向滚动；代码块等宽横向滚动。应用于三处：流式定稿（流式期间保持纯文本、权威终值到达才换渲染形态，分工同鸿蒙 B4）、非流式整包、会话历史恢复；纯单段落回复仍走原来的轻量 span，观感零变化。验证：mock-IPC 浏览器断言全部块类型渲染、代码块里的 `<script>` 保持惰性文本、单段落回退、气泡集成（heading 700 字重、列表项计数）。
- **`scripts/gen-icon.mjs` 主仓版图标生成器（2026-07-27）**——见下方 Changed 图标条。

### Changed
- **应用图标弃用蓝底「PA」占位图，换「深夜书房的琥珀台灯」（2026-07-27，与鸿蒙 C1 同一构图同一着色函数）**——纯 Node 零依赖光栅（径向渐变+圆弧、4× 超采样），一次生成全套 51 个文件：Tauri 桌面 png 全清单、PNG 条目 `.ico`（16-256 七档）、PNG 块 `.icns`（ic07-ic14）、iOS AppIcon（尺寸从既有文件名解析）、Android launcher（legacy 方图+round+**adaptive 前景**——108dp 画布可见区只有中央 72dp，靠着色函数对 216 设计系界外坐标的自然外推画 1.5× 画布，完整构图落进可见区且被遮罩裁掉的部分无硬边；adaptive 背景色 `#fff`→`#131316` 对齐夜色底）。`.ico`/`.icns` 容器结构做了字节级校验，512 主图与 adaptive 前景成像已目视核对。旧「PA」蓝底图标彻底退役（产品 2026-07-20 已更名息壤，图标是最后一处旧名残留）。

### Changed
- **文档整治（2026-07-27，纯文档，零代码）**——核对全部 markdown 文档的真实性与时效性，对照代码抽查事实断言后修正过时描述：
  - 删除根目录 `Solum_demo_script_v2.md`（本地演示素材，已 gitignore）：v3 脚本与 MISC 2026-07-22 条目已明确其重剪方案过时，其独有信息（TTS 管线 SAPI Zira + ffmpeg adelay/amix、重剪分段）已在 MISC 2026-07-17/2026-07-22 条目留档；且其旁白稿含 v3 已按诚实红线撤回的「运行时模型为 GPT-5.6」口径，留着反而误导。
  - `ARCHITECTURE.md`：标题去掉「讨论文档 v0.1」；头部状态段的逐阶段流水与 §6 重复且滞后（只写到 Phase 9/10，漏 11/12），压缩为一句总体状态（Phase 1–11 落地 + Phase 12 首项 F21）并指向 §6/CHANGELOG；§6 Phase 4 由「进行中」改为已完成，并注明当时暂缓的 F11/F13 已于 Phase 7 D5（2026-07-16）落地；§3.12 「工作台主导航」改为 2026-07-20 IA 收口后的「对话输入框『+』→ 工作台抽屉」；§3.12 「明确留后」清单补注其中同步/schema 演进/table·stat/日程互通已在第二、三步补齐；§5 选型表「冲突解决 CRDT」改为实际落地的行级 LWW。
  - `README.md`：仓库结构里 `dist/index.html` 的「17 视图（7 组导航）」改为实测的 18 视图（6 组一级导航 + 设置多级目录，工作台为抽屉），设置组补漏「同步」页；「80+ 个 tauri command」更新为 90+（实测 96）。
  - `DESIGN.md`：两处「7 组导航」改为 6 组（工作台并入对话后的实际值，与同文件 2026-07-20 IA 条目自洽）。
  - 核查后**未改**：CHANGELOG/PITFALLS 历史条目（记录性质，不回改）；MISC 全部条目（均为带日期的决策留痕，被 ARCHITECTURE/CHANGELOG 按日期引用，经查无与 PITFALLS/ARCHITECTURE 的字面重复，无确定失效项）；PRODUCT/PRIVACY/LLM-PROVIDERS（与代码抽查一致：默认模型 `mimo-v2.5`、8 家厂商预设、DESIGN 色板 token 与 `:root` 一致、Phase 8.2 推送代码 `push_schedule_events` 在 `soulous.rs` 实在）。

### Changed
- **登录 KDF 从 Argon2id 换成 PBKDF2-HMAC-SHA256（2026-07-22，当天第二次修订）**——用户想让 sync-server 的运维仪表盘也能直接输用户名+密码（而不是粘贴派生好的 token），但 Argon2id 在浏览器里没有原生实现：要么塞一份第三方 JS Argon2（违反仪表盘"不引入依赖"的既定原则），要么手搓 Argon2id（安全代码不该这么冒险）。PBKDF2 和 HKDF 都是 `SubtleCrypto` 原生算法，换成它之后浏览器和 Rust 端零依赖打平算出完全一致的结果（`crates/solum-core/src/sync.rs` 用 `pbkdf2::pbkdf2_hmac::<Sha256>`，600,000 轮；HKDF 的 salt 从隐式 `None` 改成显式全零 32 字节，因为 `SubtleCrypto` 的 HKDF 没有"省略 salt"这个选项，两边都得显式声明同一个值）。
  - 用 Node 的 `crypto.webcrypto.subtle` 跑了一遍和 Rust CLI 完全相同的入参，逐字节核对派生结果一致，才接着往仪表盘里接。
  - **代价接受**：PBKDF2 不是内存困难型 KDF，离线暴力破解的抗性弱于 Argon2id；对自托管单用户 relay 而言，比起"仪表盘也能登录"的可用性，这个权衡定下来了。
  - `solum-sync-server/src/dashboard.html` 新增「或用用户名+密码登录」折叠区：浏览器本地跑同一套 PBKDF2+HKDF 算出 token，只存派生结果到 localStorage，密码本身不落地、不出这台浏览器。
  - **这次改动使旧的 Argon2id 派生结果全部作废**：relay 的 `SOLUM_SYNC_SERVER_TOKEN` 和所有设备的 `solum-sync.json`（用户名+密码格式的，会在下次启动自动用新算法重新派生，无需改文件；仍是旧 `{token,key}` 原始格式的文件不受影响）都要用新算法重新核对。已重新计算并更新生产 relay 的 token。

### Added
- **同步登录从「复制 64 位十六进制主密钥」换成「用户名+密码」（2026-07-22，用户体验反馈）**——原方案要求把 `solum-sync.json` 里的原始 token/key 手动搬到每台设备，在手机上尤其痛苦（还得靠 adb）。现在：
  - `solum-core::sync::derive_credentials(username, password)`：Argon2id（慢哈希，抗离线暴力破解，密码越弱越依赖它）把密码拉伸成一份中间密钥，再用 HKDF-SHA256（快）按不同 `info` 标签展开出两个互相独立的 32 字节输出——一个当 relay 的 Bearer token，一个当 §3.8 的端到端加密密钥；用户名只做加盐（不当作秘密）。同一账户在任何设备上算出来的结果都一样，不用再抄密钥。
  - `SyncConfig::load()` 现在识别 `solum-sync.json` 的两种形态：新的 `{url,username,password}`（登录时派生 token/key）和原有的 `{url,token,key}`（继续按原样工作，不强制迁移）。
  - relay 服务器完全不知道这次改动——它一直只是比较一个静态 token；「派生」只是把「用户手抄一个 token」换成「每台设备各自算出同一个 token」。一次性把新算出的 token 写进 `SOLUM_SYNC_SERVER_TOKEN` 即可（`solum sync-derive <用户名> <密码>` 打印出该填的两个值）。
  - `solum-cli` 新增 `sync-derive` 子命令，一次性打印派生结果，供服务器配置或手写配置文件时使用。
  - `solum-app` 新增「设置 → 同步」页面（Tauri command `sync_config_get`/`sync_config_save`）：url/用户名/密码三个字段，保存后立即可用既有的「立即同步」按钮验证；密码留空保存＝沿用已存的那份（同 LLM Key 的沿用逻辑）。旧的原始 token/key 文件会被状态行标注为"旧版"，保存表单即可升级。
  - 单测 4 条（`derive_credentials` 确定性/随输入发散/输出形状，`SyncConfigFile` 两种 JSON 形态解析），真实浏览器 + 注入 `window.__TAURI__` mock 走查了「设置 → 同步」的保存全链路（渲染断言：状态行文案、密码框 placeholder 变化、toast、侧栏"立即同步"按钮联动出现）。

### Fixed
- **移动端多设备同步此前无法配置（2026-07-22，用户尝试绑定手机时发现）**——`SyncConfig::load()` 只认 `SOLUM_SYNC_URL/TOKEN/KEY` 三个独立环境变量或一个 cwd 相对的 `solum-sync.json`，唯独没有像 `LlmConfig`/Soulous/邮箱三个兄弟配置那样支持 `SOLUM_SYNC_CONFIG` 路径覆盖——而 Android 既没有有意义的 cwd，也没法在已装应用上设进程环境变量，三条路都走不通，`solum-app` 的 `#[cfg(mobile)]` 启动注入里也确实漏了这一块（另外三个都有）。现已补上：`SyncConfig::load()` 新增 `SOLUM_SYNC_CONFIG` 路径覆盖（`crates/solum-core/src/sync.rs`），`solum-app` 启动时补上第四个 `#[cfg(mobile)]` 注入块，指向与 DB 同一个 app-data 目录（`crates/solum-app/src/lib.rs`）。绑定手机走法同 `solum-llm.json`：`adb push solum-sync.json /data/local/tmp/ && adb shell run-as dev.solum.app cp /data/local/tmp/solum-sync.json ./solum-sync.json`。桌面端不受影响（一直有 app-data 路径可用）。

### Added
- **`solum-sync-server` 运维仪表盘（2026-07-22）**——sync-server 只中转密文、不解密的定位不变，但此前完全没有可视化状态的方式。新增：
  - `GET /v1/stats`（沿用既有 Bearer token 鉴权，同 push/pull）：总 blob 数/字节数、oldest/newest seq、`retention_days`、DB 文件大小、按设备分组的 `blob_count`/`bytes`/`last_seq`/`last_received_at`。只聚合元数据，不接触 blob 内容。
  - `GET /`：静态运维仪表盘（`dashboard.html`，`include_str!` 编译进二进制，部署产物仍是单文件），vanilla HTML/CSS/JS，配色沿用 DESIGN.md 琥珀台灯色板。页面本身无数据、故不鉴权即可加载；页面内输入 token 后前端 `fetch` 带 `Authorization` header 调用 `/v1/health`/`/v1/stats`，与设备同步用同一把 token。健康点、总量卡片、设备表，10s 自动刷新（可关）。
  - 集成测试 `crates/solum-sync-server/tests/stats.rs`（3 条）：spawn 真实二进制验证 stats 聚合正确、无 token 返回 401、`/` 无需鉴权即返回仪表盘 HTML。
- **仪表盘视觉与易用性第二轮打磨（2026-07-22，用户反馈"太简陋"）**：卡片阴影/层级、在线状态改脈动动效、stat 区加图标与更大数字、设备表 zebra hover、超过 24h 未推送的设备行变灰提示陈旧、手动刷新按钮、显示"更新于 HH:MM:SS"、token 输入框加显隐眼睛图标。**新增内置帮助气泡**（点 token 标签旁的 `?`）：直接在页面里说明 token 就是部署时的 `SOLUM_SYNC_SERVER_TOKEN`，忘记了去部署机器的 1Panel → 容器 → 编排 → `solum-sync` → 编辑 →「环境变量」框里看——解决"这 token 我怎么获取"的重复提问。样式改动不影响 `/v1/stats` 响应结构，测试无需改。

### Security
- **通知授权拆成两种，捕获许可不再等于写入许可（2026-07-21，用户拍板）**——此前只有一个开关：界面说的是「选择后才允许 息壤 读取该应用通知」，而实际效果包含了「该应用的通知可以直接往你的日历里写东西」。**这是用户从未给过的授权**——两件事被一个开关捆在一起了。
  - `NotificationIntelligenceConfig` 新增独立的 `auto_event_packages`，**默认空**，逐应用单独授予，界面上是另一个按钮 + 一次明确的确认弹窗（弹窗里写清楚这与「读取」的区别）。撤销捕获会连带撤销这条更窄的授权（`normalized()` 里强制，不信任调用方）。
  - **确定性解析不能替代授权**：它让*内容*可追溯到原文（这挡住了模型凭空造日程），但它不说明*来源*配不配拥有写权限。这两件事上一轮被我混为一谈了。
  - **每应用每日 20 条上限**（`MAX_AUTO_EVENTS_PER_APP_PER_DAY`）。定位是**容纳损害而非授权**：授权由上面那条给，这条只保证被授权的应用出问题时填不满日历。超限的转为待确认，不丢弃。
  - 「近 7 天创建 N 条」按应用显示在授权列表里——事后发现机制，让用户看出某个应用写得比预期多。**它不构成授权**，也不替代授权。
  - **刻意没有用金额/验证码之类的关键词做安全边界**：关键词匹配是可绕过的启发式，把它摆在授权位置上会给出错误的安全感。
  - 回归测试 3 条：捕获授权单独存在时不建日程、自动建日程授权必须以捕获授权为前提（且随其撤销）、超过日上限后转待确认。真浏览器走查断言：两种状态各自的按钮与文案、确认弹窗出现且**确认前后端零调用**、确认后状态翻转。

### Fixed
- **receipt 改为「按文件确认删除后释放」，去掉 30 天按龄清理（2026-07-21 复审第七轮 P2，用户拒签收）**——上一轮给溢出计数补了 receipt，但 drain 每轮仍执行 `prune_capture_loss_receipts(now - 30d)`。**这把刚堵上的重复计数又开了回来**：receipt 要守的崩溃窗口**没有上界**，marker 删除持续失败（只读卷、被占用、权限变更）时它会无限期留在盘上，于是任何 cutoff 都会让 receipt 先过期、marker 后重扫，总数再次高于真实值——正是「至少 N 条」这个下界不允许的方向。**源码注释当时已经写明这个风险，却仍然执行了清理**（上一条 `undeliverable` 的说明里也点了同样的机制，但只对 `failed/` 改成实况清点，marker 这条漏了）。
  - `prune_capture_loss_receipts(before)` → `release_capture_loss_receipts(&[receipt])`：**按名删除，不按时间**。退休 receipt 的唯一理由是它命名的文件确实消失了。
  - 调用方顺序固定为「**先删文件、确认不存在、再释放 receipt**」。`remove_counted_file` 把删除和结果配对返回（`NotFound` 也算已消失——重点是之后没人能再扫到它，不是谁删的）。删除失败的 marker 保留 receipt，直到它真的消失为止。崩在两步之间只泄漏一行，不失真。
  - 旧共享计数文件的迁移走同一条路径（`note → 删除成功 → release`），不再按时间统一清理。
  - **顺带把表变小了**：以前稳态下要压着 30 天的 receipt，现在正常路径当轮就释放，只有卡住的文件才留行。
  - `overflow_receipt()` 抽成单一函数：记账和释放必须用同一套命名，否则 release 静默匹配不到任何行，行为悄悄退化回按龄清理。

- **溢出计数补幂等 receipt；`undeliverable` 改为目录实况而非计数器（2026-07-21 复审第六轮 P2 + 交付前自查）**
  - **记账与删 marker 之间崩溃会重复计数。** 数据库提交成功、删文件之前崩溃，下次启动重扫同名 marker 会再加一次。**更糟的是三处说法互相矛盾**：实现注释写「never counted twice」、测试却断言 `>= 1`（等于承认会重复）、界面说「至少 N 条」（下界）。重复计数意味着 N 可能大于真实丢失数，那就不是下界——我用一个宽松断言把已知缺陷合理化了。
    - 新增 `capture_loss_receipts` 表：**同一事务内**「插入唯一 receipt（按 marker 文件名）+ 累加计数」。重扫同名 marker 不再累加。测试改为断言**恰好一次**，并实测「去掉 receipt 的退化实现」会让 3 条测试全挂，确认测试真的钉住了它。
    - 旧共享计数文件的迁移是同一个窗口，同样受 receipt 保护（按 `<文件名>#<序号>` 命名，位置编号使得中途增长也能只计新增部分）。
  - **交付前自查又发现两处**（复审未提，我自己读出来的）：
    - `record_and_remove_overflow_markers` 用 `filter_map` 建名字列表，**删除循环却遍历原始路径列表**——文件名转换失败的 marker 会被删掉却从未计数。改为 path 与 name 配对，结构上不可能删掉没数过的东西。
    - **`undeliverable` 用「事件计数器」建模本身就是错的。** `failed/` 里的文件是永久保留的，而 drain 的 `read_dir` 不递归、根本扫不到它们——rename 之后崩溃那笔永远不会被计数；反过来 receipt 30 天后被清理又会重复计数。改为**在查询时现场清点 `failed/` 目录**：它不是事件，是当前状态，从事物本身导出的答案不会和事物漂移。界面文案也随之改成现在时。
  - Kotlin 侧本轮把两个 `now` 局部变量改名消歧时，`noteOverflow` 里的 `stem` 仍引用旧名——**这是个编译错误，靠逐行读发现**（本机没有 Android 工具链，Kotlin 编译不了）。另写了一个小脚本核对每个函数里的字符串插值标识符都能在该作用域解析。

- **溢出计数改为「一次丢弃一个不可变 marker」，彻底去掉共享可变计数文件（2026-07-21 复审第五轮 P2）**——上一轮的「append 一字节 + rename 认领」消掉了读-改-写的过期写，**但没消掉在途文件描述符的窗口**：原生端已打开旧 live → Rust 把它 rename 成 `.taking` → Rust 读长度、记账、unlink → 原生端此时才执行 `write(1)`。这一字节落在一个已被 unlink 的 inode 上，既不在 `.taking` 也不在新 live 文件里，仍然少报。`fsync` 救不了——描述符在 rename 之前就解析完了，写发生在读之后。
  - **共享可变计数文件这条路本身就是错的**，每一版「修复」都只是把竞态挪个位置。改成和 spool 本身同一个模式：一次溢出写一个 `<stem>.tmp` → fsync → rename 成 `<stem>.mark`，**没有任何共享可变状态可争**。
  - Rust 侧 `collect_overflow_markers` 普查 → `record_and_remove_overflow_markers` **只删它数过的那些路径**。按目录重新列举来删会把这个设计的意义直接抵消掉——普查与删除之间新出现的 marker 会被无声抹掉。
  - marker 也有上限（原生侧 2 000）。到顶之后是欠计数，界面据此说「**至少** N 条」而不是给一个拿不到的精度。
  - 旧版共享计数文件（`notif-spool.overflow` / `.taking`）在升级后首次运行一次性折入，不丢历史计数。
  - **测试重写**：上一轮那条「drain 期间到达」名不副实——它是「drain 完 → 写 → 再 drain」，根本没构造出窗口。新测试显式分四步走：普查 → **此刻**新增 marker → 只删普查到的 → 断言新 marker 仍在盘上且下一轮被计入。另用一段独立脚本验证这条测试能打死「按目录重新列举来删」的错误实现（错误实现下新 marker 被删且永不计数）。

- **spool 的两处跨线程/跨进程竞态（2026-07-21 复审第四轮 P2，均为上一轮引入）**——上一轮把「溢出计数」和「队列上限」补上了，但两者的**并发正确性**都没做对：
  - **溢出计数会错报。** 原生端的 `synchronized` 只保护 Kotlin 自己的写者，Rust 在另一个进程独立执行「读计数 → 入库 → 删除」。Rust 读到 `n` 之后原生写出 `n+1`，Rust 记完 `n` 就把文件删了——新增的那次溢出没了；反向时序则会重复计数。
    - **只加「rename 后再读」是不够的**：Kotlin 那边是**读-改-写**，一次在途的 RMW 会把过期的总数写回新 live 文件，把 Rust 刚认领走的计数又算一遍。
    - 所以计数器改成**追加式**：每次丢弃 append 一个字节，计数就是文件长度。**没有读的步骤，就没有过期可言。** 一字节的写还顺带让撕裂写不成立（要么 0 字节要么 1 字节）。
    - Rust 侧改为**原子 rename 认领**（`notif-spool.overflow` → `.taking`）后再读：认领之后原生的下一次 append 开一个新 live 文件，在途的 append 落在我们即将读的那个 inode 里，两种时序都恰好计一次。上次崩溃遗留的 `.taking` **先折入再认领新的**，否则 rename 会覆盖它、丢掉那笔计数；记账失败则保留文件下轮重试。
  - **5 000 上限不是严格上限。** 多个 `onNotificationPosted` 可以同时在 4 999 通过检查然后都写入——锁只包住了「已经满了之后的计数」，没包住「清理 → 计数 → 建临时文件 → rename」这个**配额决策**。改为用 `spoolLock` 覆盖完整临界区：**检查和它所授权的动作必须是同一个临界区，否则"上限"只是个建议。**
  - 新增 5 个真实文件系统场景测试：满队列计数恰好一次且被清除、**drain 期间到达的丢弃不被吞掉**、崩溃遗留认领可恢复且不重复、正常件消费后删除、核心*拒绝*（未授权）的件不会卡住队列。
  - 写测试时踩到一个自己的坑：崩溃恢复那条我最初断言「遗留的先记、live 的下一轮再记」——那是在**测实现细节**。一轮里把两者都记上完全正确（不丢不重），该断言的是总数与幂等性。

- **spool 溢出与入库失败不再静默吞掉通知（2026-07-21 复审第三轮 P2，均为上一轮 spool 改造引入）**——上一轮把收件箱改成 spool 修掉了并发追加丢数据，但新路径自己带进来两个洞：
  - **原生端到达 5 000 文件上限直接 `return`**，没有任何计数或提示；而且 `list()` 把遗留的 `.tmp` 也算进额度，几个崩溃残留的临时文件就能永久占住配额，此后通知**静默全丢**且界面上看不出原因。现改为：先清理超过 5 分钟的陈旧 `.tmp` 再计数，且**只统计完整的 `.json`**；溢出时把计数写进 `notif-spool.overflow`（原子替换 + `synchronized`，`onNotificationPosted` 可能多线程送达，丢一次自增就等于少报一条）。
  - **Rust 侧 `capture_notification` 返回 `Err` 时被吞掉，随后无条件删除 spool 文件**——注释写的是「核心接收之后才删」，代码做的是「不管接没接到都删」。数据库忙、磁盘满这类**可恢复**的失败因此变成永久丢失。旧 JSONL 升级迁移路径同一个问题。
    - `ingest_inbox_lines` 改为返回 `IngestBatch { outcomes, failed }`，**严格区分两件事**：`Ok(None)` 是核心*决定*不留（未授权、空文本），属完成态，可以删；`Err` 是核心*没能*记下本该记下的东西，必须留。只有 `failed == 0` 才删文件。
    - 失败重试**有界**：文件名带 `.tryN` 计数（写在文件名里所以能扛住重启——而重启恰恰是瞬时故障最常见的"修复"），3 次后移入 `notif-spool/failed/` 并计入持久计数。**不能无界重试**：那正是我在同步那边修过的毒丸形状，一个永久失败的文件会每轮拖住整个队列。
  - 两类损失都进 `meta`（`capture_spool_overflow` / `capture_undeliverable`），经 `notif_intelligence_status` 暴露，在通知设置里常驻提示 + 「我知道了」显式清除。文案区分「彻底丢了（内容未保留）」与「留着但没进来（原始内容在 `failed/`）」——**后者可恢复，前者不可，不该用同一句话说**。
  - 回归测试钉住契约本身（`failed == 0` 才删、`Ok(None)` 不算失败、`.tryN` 计数往返且不累积后缀）；真浏览器断言提示 520×171 真实渲染、确认后消失、零控制台错误。

> 以下六条修的是**上一轮修复自己引入的缺陷**（2026-07-21 复审提出）。原漏洞确实堵上了，但补丁带进了新问题——记在这里，因为「修复引入回归」比原始漏洞更值得留痕。

- **桌面端没有消费同步缺口信号（复审第二轮 P1）**——核心已经检测并持久化了缺口，但桌面壳没接：手动同步仍然只弹「推送 N 条、合并 M 条」的成功提示，`sync_status` 也不暴露持久化的缺口，用户照样以为同步是完整的。**检测到但不告诉人，等于没检测。**
  - `sync_status` 增加 `history_gap` / `bad_blobs_held` / `bad_blobs_dropped`；新增常驻横幅 `#syncHealth`（挂全局 banners 区，切到哪一页都在），带「我已重新对齐」按钮调用新命令 `sync_gap_acknowledge` 显式清除——**没有任何东西会自动清它**，因为除了用户没人知道数据是否已经找回来。
  - 手动同步有缺口时，第一句话必须是「同步不完整」而不是成功计数。
  - CLI 的 `sync-status` 同样打印缺口与「无法解密批次」的暂存/丢弃数——只在同步那一轮打印一次，用户很可能根本没在看终端。
  - 真浏览器走查断言渲染结果：有缺口时横幅 1029×127 在视口内、文案与确认按钮齐全、零控制台错误；确认后缺口消失而「无法解密批次」提示保留（两者是独立问题）；健康状态下横幅不出现而同步按钮可见（反证这条路确实执行过）。
  - **走查过程中抓到两个我写的真 bug**：`refreshStatus()` 是我臆造的函数名（实际外层函数叫 `initClock`），以及顶层 `listen(...)` 调用——`listen` 只在文件末尾的 `if (window.__TAURI__.event)` 块内解构，顶层调用是 ReferenceError，会中断后续整段脚本。两个都只有真浏览器跑起来才会暴露。
- **`sync_bad_blobs` 的淘汰数只有存储层方法、没人展示（复审第二轮 P2）**——限额是对的，但被淘汰的是**恢复材料**，静默丢弃等于白限。现由 `sync_status` 与 CLI `sync` / `sync-status` 一并展示。
- **通知收件箱改为单通知单文件 spool（复审第二轮 P2）**——上一轮修掉了「遗留文件被覆盖」，但**共享 JSONL 的并发追加本身无法做安全**：drainer rename 之后，监听器已在途的追加仍写向旧 inode（那个文件随后被我们删掉），半刷新的追加还会留下半行。两者都静默丢通知。
  - 改为 spool 目录：监听器写 `<stem>.tmp` → `fd.sync()` → rename 成 `<stem>.json`；Rust 侧只读完整的 `.json`，逐个处理完再删。**没有任何路径既被写又被读**，协调问题被绕开而不是被"赢下来"。旧的 JSONL 在升级后首次运行时一次性并入，不遗留在途数据。
- **同步中继留存清理会造成静默漏同步（复审 P1，本轮引入）**——上一轮为了给中继加磁盘上限，加了「启动清理 30 天前批次」。**但没有配套的缺口检测**：离线超过 30 天的设备回来后，游标指向一个已经不存在的序号，它照常拉「游标之后的一切」——那已经是一批更晚的、无关的数据——然后报告同步成功，而中间被清掉的远端操作**永久丢失且毫无提示**。这比中继文件无限增长糟糕得多。
  - 中继在 pull 响应里带上 `oldest_seq`（仍持有的最小序号），清理天数改为可配（`SOLUM_SYNC_SERVER_RETENTION_DAYS`，`0` 关闭）。
  - 客户端比对游标与 `oldest_seq`：落在留存窗口之外时置 `SyncOutcome.history_gap`，**并持久化到 `meta.sync_history_gap`**——只在一次返回值里出现的警告等于没有警告。CLI 与桌面端都会明确告知「这轮不是完整同步，需要从另一台设备导出导入重新对齐」。
  - `oldest_seq` 必须与 blobs 取自**同一个响应**，否则两者可能描述中继的不同状态。教训：**加留存/清理机制时，必须同时给出让客户端发现缺口的手段，否则就是在制造静默数据丢失。**
- **同步 HLC 同毫秒碰撞仍会漏掉更新（复审 P2，上一轮没修透）**——上一轮把 HLC 改成 `MAX(墙钟, hlc_last)` 并声称「同毫秒并列由 origin 打破」。**这句是错的**：同一设备对同一行的两次写入，origin 也相同，于是 `(hlc, origin)` 完全相等，而 `apply_remote_ops` 要求严格更新 → 对端应用第一次、静默跳过第二次，两端永久分叉。实测 `strftime('now')` 在同一语句里取两次**必然相同**，所以这是必然而非偶发。
  - 改为墙钟没走过上次戳时 **+1ms**（HLC 真正的逻辑计数器部分），用 `julianday` 算，时间戳格式不变、线协议不变。`julianday` 在当前年代的分辨率约 10µs，远细于 1ms 步进。
  - 回归测试：连续 20 次改同一行，断言 20 个戳互不相同。
- **通知收件箱的遗留 processing 文件会被新 inbox 覆盖（复审 P2，本轮引入）**——上一轮的注释写着「把遗留内容并进来而不是覆盖」，**但代码是覆盖的**：先把遗留读进 String，然后 `rename` 新 inbox 到同一路径（Android/Linux 上静默替换），于是遗留内容只存在于内存里，处理途中崩溃就丢——丢的恰恰是已经躲过一次崩溃的那批通知。
  - 改为**先把遗留处理完并删除，再认领新的**：任何时刻每一行未处理数据都恰好在磁盘上的一个文件里。抽出 `ingest_inbox_lines` 让两条路径走同一段代码。
- **`sync_bad_blobs` 无上限（复审 P2，本轮引入）**——上一轮新增这张表时只想着「密文要留着以便将来恢复」，忘了加界。持有同步令牌的一端只要密钥配错，就能持续产生读不出的批次，把其他设备的磁盘撑满。
  - 上限 200 条 / 单条 1 MiB，超限丢最旧并计入 `meta.sync_bad_blobs_dropped`（沿用隔离区既有的「有界且可见」模式）；超大密文保留记录但丢弃字节——**事实比字节重要**。
- **导入的 64 MiB 上限只在前端文件选择器（复审补充，本轮遗漏）**——那是 UI 的礼貌，不是限制：IPC 命令收的是字符串、可被直接调用，而 `serde_json::from_str` 会在下游任何行数/字段限制生效之前就把整棵树建出来。改为在 `DataImportTool::document` **解析之前**先查参数字节数。

### Security
- **Guard 确认现在绑定「效果快照」并会过期（2026-07-21，安全审核 P1）**——此前令牌只绑定 `(工具名, 参数)` 指纹，**不绑定预览时给用户看的后果**。于是「预览：将删除 12 条」→（同步拉取/新捕获让数据涨到 500 条）→ 旧确认照样执行，删掉 500 条。确认的语义本该是「我批准删这 12 条」，实际只是「我批准调用 ledger_purge(before=X)」。
  - `PendingConfirmation` / `ExecutionToken` 各带一个 `effect_digest`（预览文本的哈希）；`Guard::run` 在执行前**重新跑一次 `preview`**（trait 契约本就禁止 preview 有副作用）并比对，不一致就拒绝并要求重新预览。审计记 `Refused`。
  - 待确认项新增 10 分钟 TTL（`PENDING_TTL_MINUTES`），`confirm` 拒绝陈旧预览，`run` 顺带清扫过期项；令牌本身的 5 分钟 TTL 不变。
  - 新增 3 个回归测试，其中 `confirmation_does_not_survive_a_change_in_effect` 用「预览时 12 条、执行前变 500 条」的假工具直接复现了原漏洞。
- **出口地址统一强制 HTTPS，仅豁免本机回环（2026-07-21，安全审核 P1）**——LLM、同步中继、Soulous 三处此前都接受任意 `http://`，而三者请求各自携带 API Key / Bearer token / access+refresh token 和个人内容，明文即等于把凭据挂在网上。
  - 新增 `solum-core::net::validate_endpoint`，一条规则一个地方：**必须 https://，除非主机是明确的本机地址**（`localhost` / `127.0.0.0/8` / `[::1]`）。回环豁免是为了本地模型服务器和本地中继——那里没有网络跳数，强制证书只会逼人去关校验，结果更糟。
  - **刻意不豁免内网段**（`192.168.*` / `10.*`）：它们有跳数，而且同一个 Wi-Fi 下的敌意设备正好住在那里。
  - 解析上注意了两个坑：`user:pass@host` 的主机是 `@` **之后**那段（`http://127.0.0.1@evil.com` 必须拒），`127.0.0.1.evil.com` 这种后缀骗术也必须拒；两者都有测试。

### Security
- **云端模型不能再凭通知文本直接写日程（2026-07-21，安全审核 P1）**——通知文本是**第三方应用可控的输入**，提示词里虽然写了「通知内容是不可信数据」，但模型只要回一个结构合法的 `event`，就会直接落库并生成提醒，全程没有人。白名单里任何一个应用（或被入侵的应用）都能靠提示注入凭空造出一条日程。
  - 改为**模型只当路由器、内容一律本地出**：`LlmTriageDecision::Event` 不再采用模型给的字段，而是用确定性抽取器重跑原文；抽出来才建，抽不出来就置 `NeedsReview` 交给用户判断。这样每个字段都按构造可追溯到捕获文本。
  - 回归测试 `a_model_cannot_write_a_calendar_entry_the_source_text_does_not_support`：假模型对一条**没有任何时间**的通知返回合法 `event`，断言日历为空且捕获进入回看。（此前该批量路径根本没有建日程的测试覆盖。）
- **通知取消/改期提议绑定日程快照并会过期（2026-07-21，安全审核 P1）**——提议原本只存 `event_id` 与当时标题，确认时直接对「现在这个 id」执行取消或改期。期间用户或同步端改了日程，旧卡片仍会作用在新状态上；`event_id` 还只是行号，行删掉后 id 可被复用。
  - 提议新增 `event_guid` + `event_start` 快照列，确认前重新比对身份与开始时间，不符则置 `Stale` 并要求重新确认；另加 12 小时 TTL（`ACTION_PROPOSAL_TTL_HOURS`）。
  - 升级前就在途的旧卡片快照为空 → 一律判为「无法校验」而拒绝。**故意 fail-closed**：宁可让升级瞬间的几张卡片作废，也不执行一张校验不了的卡片。
- **通知捕获与云端批次补齐体量上限（2026-07-21，安全审核 P2）**——原生端与核心都完整读写 title/text，随后最多 24 条原样拼进一次 LLM 请求；异常或恶意的超长通知可以撑爆磁盘/内存、放大云端费用，并扩大外发的个人内容量。
  - 单字段 2 000 字上限（`MAX_FIELD_CHARS`），**在原生入口就截断**——超长文本压根不落我们的盘；Rust 侧 `capture_notification` 入口再截一次（`NotificationCapture::truncated`），下游一律拿到有界文本。
  - 单批次 24 000 字上限（`fit_batch`）。刻意返回**前缀长度**而不是过滤后的列表：模型是按 index 回话的，前缀能让 index 与记录按构造对齐，被裁掉的留在队列等下一批。
  - Android inbox 文件 4 MiB 上限——到顶说明 Rust 侧没在消费（被强停/磁盘满），此时停止写入比无限增长正确。

### Security
- **三个不可恢复的删除命令改走 Guard，不再存在未经令牌的执行路径（2026-07-21，安全审核 P2 补完）**——上一批只给它们补了审计留痕，明说「只做到可见、没做到不可绕行」。本批做完：
  - 新增三个 Guard 工具 `memory_forget` / `persona_clear` / `widget_record_delete`（risk=dangerous），**并把原来的 `forget` / `persona_clear` / `widget_record_delete` 三个 Tauri 命令整个删掉**。留着它们等于留着后门——前端 modal 是渲染不是授权边界，命令本身可以被直接 `invoke`。
  - 预览由后端生成，因此能说出**真实的影响面**：`memory_forget` 删原始输入时会数出「连同由它派生的 2 条日程、3 条提醒」（新增 `Store::describe_memory_deletion` / `memory_summary`），而前端此前只能给一句笼统的「会级联删除」。这个数字随后被效果摘要绑进令牌，预览与执行不一致就自动作废。
  - `memory_forget` 覆写 `audit_summary`，只记 `layer#id` 不记内容——否则审计日志会变成用户刚要求删除的那些内容的第二份副本。
  - `MemoryLayer` 加 `FromStr` 并移进 core：层名现在是工具参数的一部分，而参数会被指纹绑定，两端必须对同一个字符串有同一个理解。
  - **`run_tool` 成功后调用 `reload_caches()`**：工具是通过 `ToolCtx.store` 写库的，绕过了 orchestrator 的内存缓存。`persona_clear` 是具体案例——不刷新的话被删掉的人格会继续留在内存里影响回复，直到下次重启。
  - 前端三处收口到统一的 `guardedDestructive()`（照 `widget_delete` 既有形态）。`askForget` 少了 `summary` 参数：预览文案改由后端出，前端再传一份摘要就是第二事实源。
  - **`event_cancel` 本批未改**：它挂在 GenUI 云端动作白名单里，旁边有一句明确的设计声明「危险按钮只是入口收窄后的确认面：按钮上写明了事件名与时间，点按即确认」。改它等于推翻一条写下来的决定，需单独议。
  - 验证：新增 2 个 Rust 测试（无令牌时三个工具全部拒绝且各留一条 refused 审计；完整流程下预览含级联、确认后派生日程与提醒一并删除）；mock-IPC 真浏览器走查断言落在渲染结果上——`guard_request` 携带正确 tool/args、后端预览文案实际渲染出「连同由它派生的 2 条日程、3 条提醒」、模态 421×155 在视口内、点「永久删除」发出 `guard_confirm`、**全程 IPC 记录里不存在三个旧命令的直调**、点「取消」不发 confirm 且模态关闭；390px 下模态 294×223 无横向溢出、按钮均在视口内、控制台零报错。
- **恢复 Tauri CSP（原为 `csp: null` 完全关闭）（2026-07-21，安全审核 P2）**——`default-src 'self'`，并显式收紧 `connect-src`（只留 `'self'` 与 IPC）、`object-src 'none'`、`frame-src 'none'`、`form-action 'none'`。`script-src` 仍需 `'unsafe-inline'`（前端按 §3.9 就是一份内联脚本的静态 HTML），但**外发路径已经封死**：注入者拿到的是一个不能加载远程脚本、也不能把数据 POST 出去的执行环境。彻底去掉 `'unsafe-inline'` 需要把脚本拆成独立 `.js` 并配真浏览器走查，记在 MISC 待下一次前端批次做。
- **不可恢复的本机删除命令补 append-only 审计留痕（2026-07-21，安全审核 P2 部分）**——`event_cancel` / `forget` / `persona_clear` / `widget_record_delete` 此前既不要 Guard 令牌、也**完全不进审计**。本轮先补留痕（`audit_irreversible`）：「删了但查得到」和「删了且毫无痕迹」是两个量级的问题。审计时间戳取数据库墙钟而非注入的 `now`，避免调试模拟时钟污染审计。
  - **注意这条只做到「可见」，没做到「不可绕行」**：前端 modal 是渲染不是授权边界，命令仍可被直接调用。按 §3.3 应把这四个命令注册成 Guard 工具走完整令牌流程；**刻意没有另造一套 nonce 机制**（那正是 AGENTS.md 禁止的「为图省事开后门」）。详见 MISC 当日条目。

### Fixed
- **桌面与 CLI 的库和配置改用 app-data 目录，不再依赖当前工作目录（2026-07-21，安全审核 P2；用户拍板采用方案 b）**——原先「我在用哪个库」取决于程序是怎么被启动的：快捷方式的工作目录、资源管理器双击、开机自启各不相同，会**静默打开甚至新建一个空库**，而用户无法把这个现象和数据丢失区分开。
  - 新增 `solum-core::paths`：Windows `%APPDATA%`、macOS `~/Library/Application Support`、Linux `$XDG_DATA_HOME`，统一挂在 bundle id `dev.solum.app` 下。**刻意不引 `dirs`/`directories` 依赖**——规则不过是每平台一两个环境变量，而这个项目刚为一个本可避免的原生依赖付过代价（PITFALLS 2026-07-21 native-tls）。
  - **关键前提：`solum-cli` 调用同一个函数。** 旧的 cwd 默认值存在的理由就是「桌面和 CLI 共用一份库」，这条性质原样保留，只是不再依赖启动目录。`--db` 与 `SOLUM_DB` 仍然优先。
  - 迁移是**移动而非复制**：首次启动把 cwd 下的旧文件搬进 app-data 并打印 `[paths] 已接管 …`。留下两份会分叉，而分叉正是这次要消除的东西。迁移失败**不是致命错**——保持旧文件原样并提示手动移动，好过在半途丢掉它。
  - 两次接管有先后：先跑改名迁移（`pa.sqlite` → `solum.sqlite`，仍在旧位置），再搬进 app-data。这样跳过了某个版本的用户最终只会得到一份库，而不是两份各装一半。
  - 覆盖 `solum.sqlite` 与 `solum-{llm,sync,soulous,email}.json`。移动端不受影响（本就用平台提供的 app-data）。README 补「数据放在哪」一节。
  - 实测：干净环境下 CLI 写入落在 `app-data/dev.solum.app/solum.sqlite`、cwd 无残留；先用旧行为在 cwd 建库写一条数据，再按新默认启动，接管日志正常、**数据读得回来**、cwd 已无库。
- **HLC 名副其实：本地单调 + 拒绝远端荒谬时间戳（2026-07-21，安全审核 P2）**——所谓 HLC 此前就是纯物理墙钟，恰好放弃了这个名字承诺的性质：时钟一旦回拨（夏令时、NTP 校正、用户改了错误日期），**后写的反而输给先写的**，用户的修正静默不生效且界面上无从解释。
  - 本地：`HLC` 表达式改为 `MAX(墙钟, hlc_last)`，并加 `trg_hlc_advance` 触发器把签发过的最大戳持久化到 `meta.hlc_last`（device-local，不参与同步）。同毫秒内的并列一如既往由 `origin` 打破。
  - 远端：**超过本机时钟 1 天的 op 一律进隔离区而不是合并**。时钟漂移不会差出一天；而在纯 LWW 下这样一条 op 会永久压过本机此后对该行的每一次修正，记录被冻住且看不出原因。
  - **吸收也必须有界**——这是最容易做错的地方：教科书 HLC 会吸收远端时钟，但照做就等于把攻击者的 2099 年继承成本机时钟，之后本机所有写入都带着那个年份。因为 `trg_hlc_advance` 挂在 oplog 插入上，而被拒的 op 根本不会进 oplog，所以本机时钟只会吸收通过了这道门的戳。回归测试同时断言「没合并」与「本机 hlc_last 没被带飞」。
- **Health Connect 轮询水位持久化，不再重启就重复累计（2026-07-21，安全审核 P2）**——水位只在内存里，每次重启都重置为 now-6h 并重读六小时历史。注释说这只是「效率细节、不影响正确性」，**对区间聚合指标（步数）并不成立**：同一批步数会以不同的窗口边界再次到达，也就是不同的 `dedup_key`，于是再存一遍，当天总数虚高，还会经同步扩散出去。水位改为同时写入 `meta.health_poll_since_ms`，启动时续读；从未轮询过的设备才回看 6 小时。
- **Health Connect 原生读取补齐上限（2026-07-21，安全审核 P2）**——分页记录、心率子样本、JS 响应数组此前全部无上限累积，而 `readRecent` 的起点是从桥接传入的任意值。现限制：单类型 20 000 条 / 50 页、回看最长 7 天（`sinceEpochMs` 为 0 或畸形值不再等于「1970 年至今」）、跨桥心率子样本 50 000 条。
- **健康样本的修正值不再被永久忽略（2026-07-21，安全审核 P3）**——`dedup_key` 刻意不含数值（同一来源同一区间的同类读数就是同一次测量，重复轮询不该堆副本），但存储用的是 `INSERT OR IGNORE`，于是平台事后修正同一次测量时改不进来，第一个数字被永久保留——正好与 `dedup_key` 自己的注释相反。改为 `ON CONFLICT(dedup_key) DO UPDATE`（值有变化才写）。
- **导入：拒绝自称来自未来的备份、拒绝未知表、补齐体量上限（2026-07-21，安全审核 P2）**
  - **未来时间戳直接拒绝，而不是悄悄夹回来。** `exported_at` 只是文件对自己的声明，却被直接当作 LWW 时间戳；伪造一个 2099 年的备份，它写进去的行会**永久压过用户此后的每一次修改**——记录被冻结在攻击者的版本上，界面上还完全看不出为什么改不动。**先试的是「把时间戳夹到 now」，不够**：那样内容是旧的、时间戳是新的，照样压过导入前的所有编辑。文件要么是诚实备份（时间戳本就在过去，什么都不用修），要么不是——不是就该说出来。比较用的是 **LWW 实际比较的那个时钟**（同步触发器的 `strftime('now')`，真实 UTC），注入的 `now` 可能是调试模拟时钟，故取两者较宽松的一个加一天容差——这道检查是为了抓伪造，不是去管时区和时钟漂移。
  - **未知表拒绝导入，不再流进隔离区。** 隔离区是给「对端跑着更新版本」的 op 准备的——那种数据值得留着等升级。备份文件里的表名不是那回事，它只是文件说了算。放行意味着构造一份备份就能塞满 5 000 条隔离区上限，把真正等升级的跨设备数据挤掉。白名单以**导出端实际写出的表**为准（`SYNC_PAYLOADS` + `meta`）——写测试时正好抓到 `meta` 是合法的，说明这个判据得从导出端取而不是照抄同步表清单。
  - 体量上限：单次导入 500 000 行、单行 1 MiB（核心侧），前端在 `file.text()` 之前先查 `file.size`（64 MiB）——**读取发生在 Guard 预览之前**，用户还没决定要不要导入，进程就可能已经被撑爆。
- **同步中继补齐单页字节预算与留存清理（2026-07-21，安全审核 P2）**——只有 500 条的行数上限根本不是上限：500 × 8 MiB ≈ 4 GiB 密文，base64 + JSON 之后超过 5 GiB，而且**两端都在内存里构建**，任何持有 token 的设备都能触发。改为按总字节预算（16 MiB）截断，至少发一条以免单个大批次卡死游标——`since` 本就让拉取可续传。另加 30 天留存清理：没有任何留存策略时中继的 SQLite 只会一直涨。
- **旧库接管改为「先 checkpoint 再单文件移动」，不再可能丢 WAL 中的已提交数据（2026-07-21，安全审核 P1）**——原实现按主库、`-wal`、`-shm` 逐个 rename。主库移动成功而 WAL 移动失败（或进程在两者之间死掉）时，**下次启动看到新路径已有库就不再接管**，直接打开一个缺了 WAL 里那批已提交事务的数据库——丢失既不可见也不可逆。
  - 改为先用 SQLite 打开旧库、`PRAGMA journal_mode=DELETE`（把 WAL 全部回写进主库并删除 WAL），此后**只有一个文件、只有一次 rename**。checkpoint 失败则**一个文件都不动**，旧库保持完整，下次启动可重试。
  - 回归测试重写为真实场景：造一个 WAL 里有未 checkpoint 提交的真库，接管后断言那行数据仍在（旧测试用的是假字节内容，压根测不到这件事）。
- **Android 闹钟持久化失败不再被吞掉（2026-07-21，安全审核 P1）**——`save` catch 住异常什么都不做，而调用方仍认为同步成功并更新 `alarm_sig`，于是**不会重试**；重启后按过期的计划重挂：新提醒缺失、已取消的旧提醒复活。
  - `save` 改为**原子写入**（临时文件 → `fd.sync()` → rename）并返回成功与否；`writeText` 会先截断，崩在中间会留下解析不了的半截 JSON，而 `load` 把解析失败当成「没有任何闹钟」。
  - `replaceAll` 顺序改为**先持久化再取消旧闹钟**（写失败时旧闹钟原样保留，不会落到「两套都没有」），失败抛异常给 Rust；Rust 侧本就只在 `Ok` 时更新签名，链路就此闭合。**修的时候自己踩了一次**：把 `save` 提到取消循环之前后，循环里的 `load(ctx)` 读到的已经是新集合，会取消错对象——必须先把旧集合读进局部变量。
- **配置、通知策略与导出全部改原子写入（2026-07-21，安全审核 P2）**——新增 `solum-core::fsatomic::write_atomic`（临时文件 → fsync → rename，临时文件与目标同目录，因为 rename 只在同一文件系统内原子）。接入 LLM 配置、邮箱配置、Soulous 配置、`notif-policy.json`、桌面导出与 CLI `--out`。此前一律 `fs::write`（先截断再写），断电/被杀会留下看着合法的截断 JSON，而后果各不相同却都不可自查：LLM/Soulous 静默退为「未配置」、邮箱变不可读、通知策略 fail-closed 停止捕获。
- **通知白名单：策略文件先落盘，数据库后提交（2026-07-21，安全审核 P2）**——原顺序反过来，文件写失败时用户已在界面上取消授权，listener 却仍按旧策略读取该应用通知并写入 inbox（核心之后会丢弃，但**敏感文本已经被读取并落盘**）。改为先写文件：失败则两边都没变；即使数据库随后提交失败，核心白名单也是更严的那个，捕获会被丢弃。
- **导出文件名不再可能互相覆盖（2026-07-21，安全审核 P2）**——桌面端文件名精确到秒，同一秒内两次导出（双击、或手动撞上定时）后者会静默覆盖前者。改为撞名时自动加序号；CLI `--out` 指向已存在文件时**拒绝覆盖**而不是照写。
- **Android 闹钟签名纳入标题与正文（2026-07-21，安全审核 P3）**——签名只含 `(id, at_ms)`，改了事件标题但没改时间时签名不变，OS 闹钟不会重挂，到点仍念旧标题。
- **OAuth 回调只接受 `GET /callback`（2026-07-21，安全审核 P3）**——原实现消费**收到的第一条**本机请求，浏览器预取、favicon 请求或任何本地探测都能提前结束 5 分钟授权会话，用户只会看到「缺少 code 或 state」且无从理解。改为对非 `GET /callback` 回 404 并**继续等待**，直到拿到真回调或超时。
- **FileProvider 收窄到专用导出目录（2026-07-21，安全审核 P3）**——`external-path path="."` 把整个外部文件目录列为可授权范围。当前没有任何分享调用，不构成利用链，但下一个加分享功能的人离「授权范围远超预期」只差一个笔误。
- **Android 通知收件箱不再有「读到删之间必丢」的窗口（2026-07-21，安全审核 P1）**——原顺序是整文件读取 → 删除 → 解析，而 listener 在另一个进程持续追加：**读与删之间写入的通知被直接丢弃**。代码注释承认了这点并称「提醒是辅助性的，可接受」——但这是喂给提醒链路的输入，不该按可丢处理。
  - 改为**原子 rename 认领**：把 `notif-inbox.jsonl` 改名为 `notif-inbox.processing.jsonl` 再读。rename 是原子的，所以没有窗口——之后 listener 的追加会新建收件箱，而已在途的写入落在我们即将读的那个文件里，两种情况都不丢。
  - 崩溃留下的 `.processing` 文件在下一轮先并入再处理；认领文件在**全部交给核心之后**才删除。故意选择「崩溃时重跑一遍」这个方向——重复由核心的内容哈希判重吸收，而另一个方向是丢通知。
- **例行提醒不再可能「永久漏掉某一天」（2026-07-21，安全审核 P1）**——原实现按 `event_exists(标题, 时刻)` 判断是否已排期，而且事件与通知是两次独立写入、高水位还照常推进。两种失败都会变成不可修复：① 事件写成功、通知写失败（崩溃/磁盘），此后永远被当成「已排」；② 同步过来的**同名同时刻**事件，会顶掉一个跟它毫无关系的 routine 的提醒。
  - 判据改为**按 routine 来源**（`events.routine_id`）而非标题：新增 `routine_occurrence_event` / `routine_occurrence_needs_work`。后者把「事件在、但一条通知都没有」也算作待办——这正是崩溃留下的半成品形状。
  - 事件 + 通知合并进 `materialize_one` 的单个事务。
  - **高水位与修复的边界是这次最需要小心的地方**：高水位存在的意义是「别把用户删掉的日程复活」，所以高水位以内**绝不重新创建**；但它不能顺带压掉*修复*。因此高水位以内只修「事件在、通知不在」这一种形状（`repair_routine_occurrence`），事件整条不见了则视为用户主动删除，尊重之。通知处于 `fired`/已取消等任何状态都算已排期，不会复活用户消掉的提醒。
  - 回归测试 `a_routine_occurrence_missing_its_reminder_gets_repaired` 直接删掉通知模拟崩溃，断言下一轮修复成功、事件不重复、且真的能再次 `fire_due`。
- **例行完成凭证改按来源判定，不再被普通状态句误判（2026-07-21，安全审核 P2）**——确认逻辑此前比 `content == routine.title`，暂停建议更宽松地用 `content.contains(title)`。于是 routine 叫「护肤」时，用户随口说一句「我在护肤」就被当成当天已完成；同一句话还会**压掉本该出现的暂停建议**——反噬的正是防打扰刹车最该生效的场景。
  - 新增 `routine::source_tag` / `is_completion_of`，两处统一只认 `source = routine#{id}` 的行为日志条目。回归测试 `mentioning_a_routine_is_not_confirming_it`。
- **已拒绝的建议不能再被旧界面改回采纳（2026-07-21，安全审核 P2）**——状态更新原本是无条件 `UPDATE`，旧卡片/第二个窗口/重放请求可以把 `dismissed` 改回 `accepted`，而采纳**是有副作用的**：会创建 routine 或暂停既有 routine。
  - 改为原子条件更新 `WHERE status = 'pending'`（`Store::decide_suggestion`），并区分「不存在」与「已处理过」两种结果，后者回一句提示而不是静默重跑副作用。回归测试 `a_dismissed_suggestion_cannot_be_accepted_later`。
- **清空人格改为事务，且启动能容忍悬挂指针（2026-07-21，安全审核 P2）**——原顺序是先删版本、再删活动指针，中断后 `persona_active` 指向不存在的版本，而启动会加载活动人格，于是**应用起不来**，用户还看不见也修不了。
  - `clear_persona` 进事务且先清指针后删版本；`active_persona` 对 `NotFound` 就地自愈（清掉悬挂指针、按「没有人格」处理）——已经踩过旧路径的库也能正常启动。
- **多表级联删除与改期重排全部纳入事务（2026-07-21，安全审核 P2）**——新增可嵌套的 `Store::with_transaction`（已在事务中则退化为 savepoint，便于组合），并接入：`delete_event`（事件+提醒）、`delete_raw_input_cascade`（原始输入→派生事件→提醒→捕获→提案，这是用户的删除权路径，删一半比不删更糟）、`delete_routine`、`delete_widget_definition`、`reschedule_event`（改时间+删旧提醒+建新提醒）。
- **同步：一个打不开的批次不再永久卡死拉取游标（2026-07-21，安全审核 P1）**——base64 解码、解密、批次反序列化任一步失败都会在 `sync_pulled_seq` 推进**之前**返回 Err，于是同一个 seq 每轮反复拉取反复失败，**排在它后面的所有正常数据永远收不到**；推送仍然正常，设备看着活着，实际再也收不到东西。最常见触发条件不是攻击，是某台设备的 `SOLUM_SYNC_KEY` 配错。
  - 新增 `sync_bad_blobs` 表：打不开的批次连**密文原样保留**（将来密钥配对了还能捞回来）、记原因、然后推进游标继续处理后面的批次。`SyncOutcome.bad_blobs` 计数，CLI 与桌面端都会提示「是否各设备密钥不一致」。
  - 这与既有的 `sync_quarantine`（op 级、读不懂的表）是同一个思路的上一层：**读不懂的东西要留下并让开路，而不是堵死队列**。
  - 回归测试 `one_unopenable_blob_does_not_wedge_the_pull_cursor` 构造「好—坏—好」三个批次，断言坏批次**后面**那个能到货，且游标真的动了。测试里的好批次由真实 peer store 生成而非手写 JSON，否则测试可能靠「全部隔离」假绿。
- **同步网络请求补齐超时，并移出全局 orchestrator 锁（2026-07-21，安全审核 P1）**——此前 ureq 调用无任何超时，且 `sync_now` 在持有 `orch` 锁的状态下发起网络请求：一个半死的中继能把提醒 ticker、通知分诊和所有 UI 命令一起挂住，而这些跟同步毫无关系。
  - 连接 10s / 整请求 60s 上限；pull 响应改为**限长读取**（64 MiB）而不是 `into_json()` 直接照单全收。
  - 桌面端新增 `AppState.sync_store`——一条**只给同步用的独立 SQLite 连接**（WAL 模式本就支持并发连接）。网络在锁外跑，只有合并真的改了东西时才短暂取 `orch` 锁调用新增的 `Orchestrator::reload_caches()` 刷新规则表/人格/主动性缓存。手动同步与 ticker 后台同步都走这条路径。

### Changed
- **邮箱连接器 TLS 由 native-tls 迁移到 rustls；imap 2.4.1 → 3.0.0-alpha.15（2026-07-21）**——为打 Android 包而做的必要迁移，非口味问题：`native-tls` 会拖进 `openssl-sys`，而 **Android 没有系统 OpenSSL**，交叉编译要从源码编整个 OpenSSL（在本机 Windows + Git 精简 perl 的组合下走不通，详见 PITFALLS 当日条目）。rustls 是纯 Rust，交叉编译零外部依赖。
  - `imap` 必须一并升到 3.x：2.4.1 只有 `native-tls` feature。API 迁移面很窄，共三处：① 连接从 `imap::connect(addr, domain, &TlsConnector)` 改为 `imap::ClientBuilder::new(host, port).connect()`，TLS 后端由 feature 编译期选定；② 会话类型 `imap::Session<TlsStream<TcpStream>>` → `imap::Session<imap::Connection>`（后端无关）；③ `fetch()` 返回自持有的 `Fetches`（内部 borrow 自 `Vec<u8>` 的自引用结构），不再 Deref 成 `&[Fetch]`，`summaries_from_fetches` 因此改为按容器收参；`Envelope`/`Address` 的字节字段由 `Option<&[u8]>` 变为 `Option<Cow<[u8]>>`，取值处补 `.as_deref()`。
  - `lettre` 只换 feature（`native-tls` → `rustls-tls` + `rustls-native-certs` + `ring`），**代码一行未改**——它的 `TlsParameters`/`Tls` 抽象本就与后端无关。
  - **行为不变**：仍是 IMAP over TLS（993）与 SMTP wrapper/STARTTLS，证书校验走系统根证书（`rustls-native-certs`），XOAUTH2 认证路径不变。质量门三绿（245 测试 / clippy 零告警 / fmt 干净）。
  - 附带收益：APK 不再需要 OpenSSL，体积与构建时间都省下来；iOS / 其他交叉目标将来同样受益。

### Added
- **Android release 包（2026-07-21）**：`app-universal-release.apk`，50,666,389 字节，`dev.solum.app` / versionCode 1000 / versionName 0.1.0 / minSdk 26，含 `arm64-v8a` + `x86_64` 两套 `libsolum_app_lib.so`，桌面图标名「息壤」。沿用原 release keystore 签名（`CN=PA Personal Agent`，SHA-256 `f3467b14…ca87`，与 7-20 那版一致，可覆盖安装升级），`apksigner verify` 通过 v2 方案。**这是改名 PA → Solum 之后的第一个 Android 包**，也是第一个带邮箱连接器与本轮 UI 重构的包。

### Fixed
- **真机走查修掉三处只在 Android 上暴露的问题（2026-07-21）**——桌面浏览器走查全绿，装到模拟器上立刻现形：
  - **首屏问候被自动滚没**：`renderActiveChatSession` 无条件把消息流滚到底，空会话时把欢迎屏（灯 + 「你好，我是息壤。」+ 顶部通知横幅）整个顶出视口，手机上一进来只剩半截示例卡片。改为**有消息才贴底，空会话回到顶部**。桌面视口高，这个 bug 看不出来。
  - **抽屉顶进状态栏**：`#drawer` 是 `top: 0` 的全屏浮层，但只有顶栏/底栏做了安全区处理，抽屉没有。补 `.drawer-head { padding-top: calc(12px + var(--sat)) }` 与 `.drawer-body` 的 `--sab`。
  - **Android 返回键直接退出应用**：浮层（抽屉/模态/输入框菜单）打开时按返回会退到桌面，而不是关掉浮层。宿主 `WryActivity` 的策略是 `webView.canGoBack() ? goBack() : 退出 Activity`，因此浮层打开时压一条 history 即可接管返回键。**踩了一个竞态**：最初按浮层个数计数，菜单项点开抽屉时是「关菜单（`history.back()`，异步）+ 开抽屉」两步，那条延迟的 popstate 会在抽屉开好之后才到、把刚打开的抽屉又关掉——真机上表现为「点组件库没反应」。改为**只维护「当前有没有浮层」一条 history，并把同步放进微任务**，等这一轮 DOM 改动全部落定再比对。实测：抽屉开着按返回 → 关抽屉回对话页；再按 → 退出应用。

- **修复改名遗留：Android 侧自 2026-07-20 更名起就无法构建（2026-07-21）**——改名当天没有跑过 Android 构建，四处 cargo-tauri 生成的胶水文件全部停在旧名，直到本次打包才暴露。**四处都在 `.gitignore` 内**（正常由 `cargo tauri android init/build` 重新生成），本机无 cargo-tauri，故手工修正：
  - `gen/android/app/src/main/java/dev/solum/app/generated/*.kt` 九个文件仍声明 `package dev.pa.app`，而目录已是 `dev/solum/app/` —— `MainActivity`（`dev.solum.app`）靠同包解析引用 `TauriActivity`，**直接编译不过**。
  - `Rust.kt` 仍 `System.loadLibrary("pa_app_lib")`，而 crate 的 `[lib] name` 已是 `solum_app_lib` —— 即使编过也会在启动时 `UnsatisfiedLinkError`。
  - `proguard-tauri.pro` / `proguard-wry.pro` 的 `-keep` 规则全指向 `dev.pa.app.*`，而 **release 开着 `isMinifyEnabled = true`** —— R8 会把 WebView/Ipc 那批类当无用代码剥掉，属于「构建成功但一跑就崩」的那类。
  - `tauri.settings.gradle` 与 `app/tauri.build.gradle.kts` 仍引用 `:pa-alarm` / `:pa-health-connect` / `:pa-notif-access`，而三个插件 crate 目录已改名为 `solum-*`，gradle 报 "No matching variant"。
- **UI 自查回归修复：组件创建死胡同、抽屉里的宽容器组件、剩余长表单页（2026-07-20）**——对上一轮 IA 重构做逐视图自查后修掉 6 处，其中 2 处是重构本身引入的回归：
  - **组件创建后无去处（回归）**：chat 里确认组件预览 → `refreshWidgets()` 渲染进**关着的**抽屉，用户只得到一句 toast。改为确认后自动打开组件抽屉，并在预览卡上留「打开组件」按钮兜底。
  - **抽屉内组件详情用了主视图大标题头（回归）**：`.vhead`（24px 标题 + 一排 4 按钮）在 460px 抽屉里撑到 171px、占可视高度 21–23% 且 sticky 常占。新增 `.drawer-detail-head`（返回图标 + 组件名一行，次要操作「从日程导入 / 加字段 / 删除组件」收进「⋯」浮窗），实测 44px、占 6%。
  - **邮箱页 1726px→616px（全站最长页）**：主内容「邮箱内容」提到最前，「撰写邮件」（602px）与「连接账户」（560px）折叠；**未配置账户时自动展开连接表单**，点账户 chip 编辑也自动展开——不能把首次配置流程一起折掉。
  - **隐私页 1394px→1017px**：「即时分诊规则」（435px 进阶配置）从通知智能管线里拆为平级折叠；管线本身保持展开（它是本页实质内容），Android 电池控件与「立即处理队列」归回管线。
  - **云端页 1583px→908px**：Soulous 互通折叠（次要功能）。**人格页 1232px→758px**：编辑表单折叠，未设人格或聊天记录导入填表时自动展开。**护栏页 894px→574px**：审计日志置顶，可用工具与演示执行折叠。
  - 复测结果（桌面 1280×800）：16 个视图仅 4 个仍需滚动（agenda 695 / notifs 873 / privacy 957 / cloud 802），其余固定；移动端 390×844 下配置页基本一屏或一屏半，长页只剩数据页（notifs 1225 / ledger 1030 / captures 900——数据多本就该滚）。全站可见叶子文本仅 1 处 <12px（库路径，装饰用，符合 DESIGN.md）。
  - 验证：node `--check` + 标签配对 + 双档逐视图断言（折叠自动展开分支、「⋯」浮窗开合与视口内、返回目录、面包屑往返、规则模态取消后列表完好、sticky 标题 delta=0、组件创建后抽屉自动打开），控制台零报错。踩坑记入 PITFALLS（折叠藏掉唯一入口 / 宽容器组件塞窄容器 / 滚动量具在视图切走后失效）。

### Changed
- **壳层 IA 重构：固定视图壳 + 多级导航 + 工作台并入对话（2026-07-20，密度重构第二轮）**——按「页面尽量固定、少整页滚动，多用浮窗与多级导航降密度」的方向重排信息架构：
  - **固定视图壳**：`.vhead`（标题 + 主操作）sticky 吸附在各视图滚动口顶部，任何滚动位置下标题与「触发到点提醒」这类主操作都在手边；短页面完全不滚。
  - **工作台并入对话**：顶层「工作台」组撤销；对话输入框新增「+」浮窗菜单（上传资料 / 资料库 / 组件库），资料与组件以右侧抽屉呈现（`#drawer`，桌面 460px、移动全宽，z 层级在模态之下以保证抽屉内的高危删除确认能盖住它；Esc/遮罩关闭、焦点归还）。组件快捷起点点选后自动收抽屉并填入输入框。
  - **设置改真正的多级导航**：一级目录页（图标 + 名称 + 一句说明，6 项），二级详情页顶部为「← 设置 / 当前页」面包屑；已在二级页再点「设置」入口回到目录。替代原先 6 项挤一条分段控件。
  - **记忆组拆两页**：「通知回看」从记忆台账页拆出为独立二级视图，台账页只剩台账本体，两页滚动高度各减近半。
  - **规则编辑改浮窗**：编辑表单进模态（琥珀主按钮），规则列表不再被就地整个替换。
  - 邮箱连接器（F21，同日并行落地）保持顶层「邮箱」组不变，其界面已按本轮组件体系（statusline / fieldrow / fold）实现，无需迁移。
  - 新增图标 back / chev / lock（同笔触手绘补进 `ICONS`）。
  - 验证：node `--check` + mock-IPC 真浏览器双档（1280 / 390）行为断言全绿：设置目录↔详情↔面包屑往返、组入口二次点击回目录、抽屉开合/换页/Esc/焦点、快捷起点收抽屉填输入框、规则模态取消后列表完好、sticky 标题在滚动 272px 后仍贴滚动口顶、移动端 6 标签与抽屉全宽、无横向溢出、控制台零报错。
- **壳层 UI 密度与层级全面重构（2026-07-20）**——针对「过密、层级扁平、状态噪音」三类痛点的一次系统性走查后重构（mock-IPC + 真浏览器逐视图量测：21/32 叶子文本 ≤13.5px、设置分段控件在 390px 溢出 2px 且无滚动暗示、提醒表格在窄屏把操作列横滚藏出屏幕、云端设置页滚动高度 1670px 的字段墙）。视觉体系「琥珀台灯」不动，动的是骨架：
  - **层级拉开**：视图标题 20px→24px；`.block` 间距 34→42px、标题 15→16px；行/表格内边距从贴边（`10px 2px`）改为有呼吸感（`12px 10px` / `11px 12px`）；全站须读小字下限从 11px 提到 12px（底栏标签、会话元信息），提示文字统一 13px（新增全局 `.hint`）。
  - **横幅语义收紧**：琥珀横幅只留给「需要用户注意/行动」的场景；陈述性稳态（Soulous 推送状态、桌面无 Health Connect、云端已配置、Android 服务正常）一律降级为新组件 `.statusline`（灰点+弱化文字+详情进 title），日程/台账/云端/隐私四处替换——「灯亮即有事」不再被状态噪音稀释。
  - **提醒列表弃表格改行**（`.notif-row`）：表格版 7 列在窄屏横滚会把「取消/稍后再响/已完成」藏出屏幕；行布局允许折行，操作任何宽度下可达，渠道文案顺带汉化（push→系统推送）。
  - **长表单分组**：新增 `.fieldrow`（短字段自适应并排）应用于云端 temperature/max_tokens/超时、Soulous 双 token、隐私「重要通知模式」（原先输入框+两个下拉+按钮挤一行 `align-items:end`，改为分层字段）。`formcol` 460→520px。
  - **低频内容折叠**：人格「从聊天记录导入」重表单收进默认关闭的 `details.fold`；侧栏「模拟时钟 + 库路径」调试项收进 `#devFold`，日常侧栏只剩云端/同步状态。
  - **结构重排**：记忆台账「导出/导入」上移进标题行、记忆主表提前、通知回看后置；护栏页审计日志提到演示区之前；复盘控件并入标题行。
  - **移动端**：分段控件窄屏允许换行（根治设置组 6 项溢出被藏）；触屏下 `.btn.sm` 补 ≥44px 命中区（`::after`，不改视觉尺寸）；说明问号 `.qmark` 同样扩到 44px；底栏标签字号 11→12px。
  - **今日面板断点 1360→1240px**：1280 宽的常见笔记本窗口此前完全看不到今日面板。
  - 验证：node `--check` 语法门 + mock-IPC 真浏览器逐视图断言（分段控件 `scrollWidth ≤ clientWidth`、提醒行操作按钮 `rect.right ≤ viewport`、折叠块 `innerText`/`elementFromPoint` 双证不可见、模态 Esc、问询/发送流程），390px 与 1280px 双档全绿。
- **项目更名：PA → 息壤 / Solum（2026-07-20）**——「PA」只是个占位缩写，且合并后产品外延早已超出"个人排程助手"。定名规则：**代码 / 仓库 / 域名 / bundleName 一律 `solum`，`息壤` 只出现在用户真正看到的界面上**（品牌标、Android 桌面图标名、通知渠道名、窗口标题、LLM 人格提示词）。
  - **全量改名**：7 个 crate（`pa-core` → `solum-core` 等）与 Rust ident、Android `applicationId` / namespace `dev.pa.*` → `dev.solum.*` 及 java 包目录、CLI 二进制 `pa` → `solum`、DB `pa.sqlite` → `solum.sqlite`、配置 `pa-{llm,sync,soulous}.json` → `solum-*.json`、环境变量 `PA_*` → `SOLUM_*`、事件名 `pa-chat-delta` → `solum-chat-delta`、`productName` → `Solum`。
  - **三处对外契约刻意不改**，改了就是破坏兼容：① `soulous::SOLUM_SOURCE` 的**值**仍是 `"pa"`——Soulous 端按 `source=pa` 识别，本次不动 Soulous 仓；② 导出格式标识写出改为 `solum-export`，但**读取端同时接受旧的 `pa-export`**（`export::LEGACY_FORMAT`），否则用户改名前导出的备份会被整份拒绝，正是 v2 刚修好的"不能还原就不叫备份"；③ release keystore 内部别名仍是 `pa-release`，改别名等于动签名身份（只改了文件名与 `keystore.properties` 指向）。
  - **数据迁移 `store::adopt_legacy_db`**：新路径无库、旧路径有库时，连同 `-wal`/`-shm` 一起接管；已有新库则**绝不覆盖**，旧文件原样留着供人工查阅。桌面 cwd 与 CLI 均接入，实测「旧库建日程 → 新默认路径启动 → 日程完好」。
  - **Android 侧数据无法在应用内迁移**：`applicationId` 变了就是另一个 App，旧包私有目录读不到。路径是**旧版导出 → 装新版 → 导入**，见 PITFALLS 当日条目。
  - **历史日志三件套（CHANGELOG / PITFALLS / MISC）不做替换**：里面记的是当时的事实（含 `crates/pa-app` 这类路径与 `PA_PREBUILT_JNILIBS` 这类命令），改了等于篡改历史。**本条目之前的记录一律沿用旧名**，读旧条目时按上面的映射表换算。

### Added
- **Phase 12 首项：邮箱连接器（F21，2026-07-20）**——新增 QQ 邮箱、Gmail、Microsoft 365 / Outlook 和自定义标准 IMAP/SMTP 的本地账户连接。用户可按需浏览文件夹、最近邮件和纯文本正文，或按发件人/主题做服务端搜索；邮件不建本地镜像、不后台拉取，读取内容只留在当前进程/界面内存。
  - QQ 走开通 IMAP/SMTP 后的授权码；Gmail 和 Microsoft 使用 Authorization Code + PKCE 的 OAuth2 本机 loopback 回调，短期 access token 在内存使用、refresh token 仅保存至 gitignore 的 `solum-email.json`。账户凭据和邮件数据不进 SQLite、同步、导出、LLM 或 recall；界面和 IPC 永不回显完整秘密。
  - 新增 `email_send` sensitive Tool：收件人、抄送、主题、正文由 Guard 完整预览并以一次性 token 绑定，未确认不可执行。审计有意只留账户 id、收件人数量与结果，不记录地址、主题、正文、附件名或秘密。
  - 桌面壳新增「邮箱」一级入口和账户/授权、阅读、搜索、撰写三段界面；mock-IPC + 真浏览器验证了 OAuth 显隐、按需读取、纯文本安全渲染以及发送确认弹层。另修复壳层重构后遗留的工作台空节点引用，避免其阻断其他视图初始化。
  - 文档同步标记 F21 为已完成的 Phase 12 首项；README、产品定位、隐私政策与设计导航数量均明确这一入口、外发边界和当前不做的自动化范围。
- **四块壳层功能：多会话对话、重要度规则编辑、通知源按应用名选择、资料工作台（2026-07-20）**——同一批工作区改动，按功能分述。
  - **多会话对话**：会话列表、切换与新建落在壳层本地存储（桌面侧栏 + 移动端下拉），**转录文本不进 core**。切换会话时壳层调用新命令 `chat_context_set`，core 侧 `Orchestrator::replace_chat_history` 只接收该会话最后几轮已完成对话并按 `MAX_HISTORY_TURNS` 截断——保持"云端每次调用只发最小上下文"这条隐私不变量（架构 §4），会话历史再长也不会整份进云端上下文。
  - **重要度规则可编辑**：`rules_save` 命令 + `Orchestrator::set_importance_rule`，保存后**立即重排该事件类型下未来日程尚未触发的提醒**（删掉 pending 计划按新 lead time / 渠道重建），返回受影响事件数供壳层反馈。**已触发和已忽略的历史不被改写**——重排只作用于 `upcoming_events` 且只删 pending，避免"改个规则把历史记录也篡改了"。
  - **通知源按应用名选择**：`pa-notif-access` 新增 `installed_apps`，Android 侧经 PackageManager 取可启动应用的**显示名 + 包名**，壳层搜索框只呈现应用名（`notif_intelligence_apps` 命令 + `InstalledApp` DTO）。包名仍是监听策略文件的实际键，只是不再要求用户知道 Android 的实现标识符。桌面无通知监听管线，`installed_apps` 诚实返回空列表而不是编一个假的选择器。
  - **资料工作台**：PDF / 文本文档上传归档，**仅存本机浏览器存储，不进同步、备份或云端**；文本文件可由用户主动加入对话输入。UI 明写「PDF 当前仅做本地资料归档，尚不会假装已解析其中内容」——不做未实现能力的暗示。
  - 新增 core 侧回归测试 2 条（会话切换只保留末 N 轮、改规则只重排未来 pending），随本批共 245 测试全绿。
- **备份变成可还原的：导出格式 v2 + 过 Guard 的导入（2026-07-20）**——此前的导出**行上不带 guid**，因此根本无法被还原：重复导入会让所有内容翻倍，事件与其原始输入的关联也接不回来。**不能还原的文件不该叫备份**。v2 在原有可读各层之外增加 `_restore` 段：按表分组、带 guid 的行级线上形状。
  - **与同步共用一份定义**：`_restore` 的字段表达式取自新提出的 `SYNC_PAYLOADS` 常量，同步捕获触发器用的是同一份。导出的行因此天然就是合并路径认识的行，**没有第二套并行的序列化**。
  - **导入 = 走普通合并路径**：还原成同步 op 后交给既有 `apply_remote_ops`，白拿 LWW、FK 按 guid 翻译、幂等与隔离区，不写第二套导入器。
  - **时间戳用导出时刻而不是"现在"**（承重决定）：LWW 于是把恢复的行当成"从另一台设备迟到的行"，**恢复旧备份不会覆盖你之后做的修改**，同一份文件导入两次是 no-op——恢复相对于用户此后的操作没有特权。
  - 导入跨所有层写入，故风险级同删除组件：Guard 预览来源设备、导出时间与各表条数 → 人工确认 → append-only 审计；**从不删除**任何本机数据。旧的 v1 文件仍可人工查阅，但会被明确拒绝自动导入而非静默半还原。CLI 与壳层各有入口。
- **Phase 11 第三步：`table` / `stat` 视图 + 与日程互通的两条快照桥 + 只读 CLI（schema v14，2026-07-20）**——补齐设计稿 ② 列出的四视图。`table` 与 `list` 共用数据绑定，只是排布成列（增量成本低，这正是当初把它列进候选的理由）；`stat` 每个字段出一块聚合磁贴。
  - **`stat` 的算子不进 schema，由字段类型推导**：number → 合计、bool → 计为是的条数、其余 → 已填条数。这是刻意的——让 LLM 指定算子等于给 schema 开一个表达式位，而"声明式而非可执行"是 F19 从第一天起的关键路径（同设计稿 ① 否决公式字段的理由）。算子要扩就改 Rust，不由生成内容决定。
  - **视图槽位从两个扩到四个**：`widget_fields` 增加 `table_ord` / `stat_ord`，与既有 `form_ord` / `list_ord` 同构（NULL = 不在该视图）；`widget_defs` 增加 `table_sort_by`。四个槽位由 `WidgetViewType::ALL` 统一索引，**加视图 = 加一个槽位，不改合并语义**——v13 那套并集合并原样继续生效，这正是当初拆行换来的复利。
  - **两条与 `events` 的通路，都是快照不是链接（设计稿 ⑦）**：`widget_import_events` 把已有日程一次性拷进记录，解决冷启动空表单；`widget_promote_record` 把一条组件记录提升为日程。两者都**拷贝值**，此后各走各的——被否的方案 B（实时只读查 events）会撞 F12 红线：台账删一条，组件里那行跟不跟着消失，两个答案都破。UI 文案明写"快照，改日程不影响本记录"。
  - **`pa widgets [--id N]` 只读 CLI**：补上 2026-07-20 验收记的可测性缺口——此前组件是全仓唯一只能从图形壳层访问的子系统，端到端行为进不了 headless。只读不写，因此不必把 schema 驱动的表单语义复制一份到命令行。
  - **动态导航仍然不做**（此前列在"留后"，现给结论）：`MAX_WIDGETS = 8`，按定义动态注册顶层导航会让主导航被用户数据挤爆，而组件页本身已是目录 + 详情两级。固定入口是终态，不是欠账。
- **Phase 11 第二步：组件 schema 演进 + 多设备同步（schema v13，2026-07-20）**——`widget_defs` / `widget_fields` / `widget_records` 进入 `SYNCED_TABLES`，组件定义、字段与记录随 §3.8 端到端加密同步。**schema 从一个 `schema_json` 列改为一字段一行**，这是本次的核心决定而非实现细节：并发加字段在单列上走行级 LWW 必然整份覆盖，晚写的一方**永久抹掉**另一方的字段，且本机已填该字段的记录会被 schema 孤儿化。拆行之后合并就是求并集——设计稿 ⑧「只允许加可空字段」让字段集合成为只增集合（G-Set），天然收敛，**不需要任何新的冲突解决代码**，现有触发器 / guid / LWW / 隔离区一行未改。取舍与被否方案（blob + 字段级合并）见 MISC 2026-07-20 定稿。
  - **唯一的演进操作是加字段，且强制可空**：已有记录无法追溯填写必填值。这条约束的回报是**记录零迁移**——老记录只是少一个键，而 `validate_record` 对缺失的非必填字段本来就放行。删字段/改类型仍然不提供（会丢数据），想改走"新建组件 + 导入"。风险级为 `safe`；删除组件仍是 `dangerous`。
  - **视图归属折叠进字段自身**：`form_ord` / `list_ord` 为 NULL 即不属于该视图，非 NULL 即其中的顺序。这样"哪些字段在哪个视图、按什么序"不再是一个会并发冲突的数组。两个视图保留各自独立的顺序（实测模型确实会给 form 和 list 不同的字段序）。另加 `ord` 列作为规范字段序——同一批插入的 `created_at` 完全相同，若靠它排序会落到随机 guid 上，**同一批行在两台设备上会得出不同顺序**。
  - **三条合并边界**：① 同名字段并发创建是 G-Set 唯一不自解的情况，按 guid 字典序确定性取舍，落败者写入拒绝日志而非静默丢弃；② 并集可能撑破 `MAX_FIELDS` / `MAX_WIDGETS`，**该状态合法但只读增长**——照常渲染和写记录，仅拒绝再加，绝不截断（截断＝丢用户亲手建的数据）；故 `validate_record` 不再重跑 `validate()`，上限只管"加"不管"用"。③ 删除组件改为在 Rust 里**显式逐表删子行**：SQLite 未开 `recursive_triggers`，FK 级联删除**不触发**子表的 AFTER DELETE 触发器，靠级联会让对端永远留着孤儿行。
  - v12 → v13 迁移把既有 `schema_json` 拆成字段行（保留必填标记、各视图成员与顺序、sort_by），随后 DROP 掉该列以杜绝继续写入不可合并的表示；新行在 guid 回填阶段进入 oplog，老组件因此能首次同步出去。壳层新增「加字段」入口（无"必填"控件，因为那是唯一不允许的选项）。测试覆盖并发并集、同名裁决、超限只读增长、必填拒绝、v12 迁移与删除写出子行 delete op。
- **Phase 11 / F19 第一条竖切：持久化自定义组件（schema v12，2026-07-19）**——主动输入可先被「创建组件」意图识别（明确措辞先于 F1 事件摄取；模糊请求可由云端 reasoner 判定），LLM 只生成受严格 serde 校验的声明式 JSON。字段封闭为 `text` / `number` / `date` / `datetime` / `time` / `bool` / `enum(options)`，`time` 复用 routine 的 `%H:%M` 解析；字段数 ≤ 12、视图数 ≤ 4，未知类型/键、超限或错误字段引用一律整体拒绝，并把原 schema 与理由写入设备本地拒绝日志。
  - 新增设备本地 `widget_defs`、`widget_records` 与 `widget_schema_rejections`：不加 guid、不进入 `SYNCED_TABLES`、不写同步触发器。创建定义必须先在对话内预览、由用户确认才落库；云端不可用会明确说明不能新建，绝不悄悄建立日程。已建组件的记录增删改查完全离线。
  - 壳层新增固定「组件」tab（不动态注册导航），独立渲染器只用 `createElement` / `textContent` 生成 schema 驱动的 `form` 与 `list`，每次记录变更后从本地数据重渲染并可按字段排序；`genui.rs` 与 F18 信封未改动。
  - 组件删除接入真实 `widget_delete` Dangerous Tool：预览级联记录数 → 人工确认 → 一次性 Guard token → append-only 审计；记录 CRUD 与创建组件维持 safe。`table` / `stat`、grid/chart、同步、动态导航、events 导入/反向提升及 schema 演进留待第二步。

### Added
- **Android release APK 重打（2026-07-20）**——含本轮全部改动：F19 第二/三步（组件同步、`table`/`stat` 视图、两条日程快照桥）、schema v15 的视图槽位 payload 修复与自愈迁移、v13 拆分事务化、「从日程导入」的跳过理由。**这是手机上第一个带组件功能的包**——此前装的 release 打于 2026-07-18 15:23，停在 schema v6（早于 F19），所以真机首启会一次性走完 v6 → v15 整条迁移链。
  - 沿用 2026-07-14 起的构建路径（本机未开开发者模式，走 `PA_PREBUILT_JNILIBS=1` 逃生门）：`cargo ndk -t arm64-v8a -t x86_64 --platform 26 -o gen/android/app/src/main/jniLibs build -p pa-app --lib --release --features tauri/custom-protocol` 出 .so，再 `gradlew assembleUniversalRelease`。产物 `app-universal-release.apk`（43,275,403 字节 / versionCode 1000 / versionName 0.1.0），同一 release keystore 签名（`CN=PA Personal Agent`，SHA-256 `f3467b14…ca87`），`apksigner verify` 通过 v2 方案，可覆盖安装升级。
  - **升级前务必先导出一份数据**：v6 → v15 是八段迁移，本轮只在构造的 v6/v12 库上验过（见 MISC 当日条目），真机数据量与形态都更复杂。

### Fixed
- **「从日程导入」的静默跳过与 `limit` 取错位置（2026-07-20，验收记账后补修）**——两条都不致命，但叠在一起会让用户对着「导入 0 条」完全无从下手。
  - **`limit` 现在限制的是「写入几条」而不是「往下看几条」**：原实现是 `list_events().take(limit)` 先截断再过滤，于是日程列表**开头**若有一串对不上字段的条目，就会把配额全吃掉，后面本可以导入的一条也进不来，整轮返回 0。现改为扫描全部日程、写满 `limit` 即停。
  - **跳过不再静默**：返回值从 `usize` 换成 `WidgetImportOutcome { imported, skipped, reasons }`，`reasons` 是**有界**样本（`MAX_SKIP_REASONS = 5`，长日程下不会退化成刷屏）。壳层 toast 相应改为「已导入 N 条，跳过 M 条：「某日程」缺少必填字段 "amount" 等」，从而分得清「没有日程」和「日程都对不上字段」——这正是本轮字段序 bug 表现为 7 条全跳时用户看不出原因的那个盲点。
  - 理由文本取 `CoreError::Invalid` 的内层消息而非 `to_string()`，否则 `invalid input:` 这个 Display 前缀会原样漏进 toast。
  - 回归测试 `the_import_limit_counts_records_written_and_skips_explain_themselves` 覆盖两条：全跳时 `skipped` 必须是 3（而不是被 `limit=2` 截断成 2），以及能导时 `limit` 照常封顶写入数。撤掉修复实测变红（2 vs 3）。四种结果形态（全导/无日程/全跳/混合）经真实浏览器 mock-IPC 走查确认 toast 文案与 390px 下的尺寸。
- **`table` / `stat` 视图和字段规范序根本没进同步 payload，跨设备与还原后双双丢失（schema v15，2026-07-20，真实双设备走查发现）**——`widget_fields` 的 `SYNC_PAYLOADS` 只带了 `form_ord` / `list_ord`，漏了第三步新增的 `table_ord` / `stat_ord`，以及 v13 专为跨设备顺序一致而加的 `ord`。接收端 `apply_one` 一直是读这三个键的，**缺的只有发送端**，于是到达的字段全部落到列默认值（`ord = 0`、成员为 NULL）。
  - **实测后果**：A 上 form/list/table/stat 四视图的组件，同步到 B 只剩 form/list——**当天刚交付的 v14 视图跨设备直接消失**；`ord` 全为 0 后并列顺序落到 `ORDER BY ord, guid` 的 guid 上，两台设备字段序不同（实测 A `move,feel,weight,notes` / B `notes,weight,move,feel`）。
  - **`_restore` 与同步共用同一份定义**，所以备份也一并丢——「备份必须能还原才叫备份」这条在 v14 视图上当时并不成立。
  - **还会连锁打挂「从日程导入」**：`event_mapping()` 取「第一个 text 字段」，字段序被打乱后取到的不再是必填的那个，于是 `validate_record` 判缺必填，**7 条日程全部静默跳过、返回 0**，而同一个组件在源设备上导入正常。这是本轮唯一一个「用户看得见但看不懂」的症状。
  - **修复**：payload 补三列；新增 `all_four_view_slots_and_the_canonical_order_survive_sync`（断言四个视图的字段列表与 sort_by 在对端一致，而不只是断言字段名到齐——旧测试正是只断言了名字才让这个洞绿着过）；导出测试补断言 `_restore.widget_fields` 五个槽位键在场。
  - **v15 迁移自愈存量**：`rebroadcast_widget_fields_view_slots()` 只碰**偏离默认值**的行（`ord <> 0` 或任一 `table_ord` / `stat_ord` 非空）——这恰好是本机权威的那批。只收到过损坏行的设备全是默认值，因此保持沉默，不会把自己那份坏数据反推回去覆盖好的。实测两台已损坏的库升级后一轮同步即收敛，四视图与字段序全部一致。
- **v12 → v13 迁移中途失败会让数据库永久打不开（2026-07-20）**——`migrate_widget_schema_to_rows` 不在事务里，而 `widget_fields` 上有 `UNIQUE(widget_id, name)`。进程在字段 INSERT 与 `DROP COLUMN schema_json` 之间被杀（手机 ROM 清后台、断电），已写的行提交、`schema_json` 还在；下次打开重跑整段，撞唯一索引 → `Store::open` 报错 → **每次打开都是同一个错，没有自愈**。实测复现：连开两次均为 `UNIQUE constraint failed: widget_fields.widget_id, widget_fields.name`。
  - 现把整段拆分包进一个事务，要么全部字段行加 `DROP` 一起落地，要么一行不留。回归测试 `a_failed_v13_split_rolls_back_instead_of_wedging_the_database` 钉死这一点——**注意它第一版写错了**：冲突落在第一个字段上，没事务时也是零提交，测试照样绿；改成让第一个字段先插入成功、第二个才冲突，去掉事务立刻红（实测 2 vs 1）。
  - 当前暴露面为零（手机 APK 停在 v6、桌面库已是 v14，全仓没有任何 v12 库），但 `migrate()` 整体仍非事务性，这条只是其中最尖锐的一处。——MISC 2026-07-19 设计稿 ⑥ 把它与「字段数 ≤ 12、视图数 ≤ 4」并列为硬上限，但竖切只落了后两条。漏掉的原因有迹可循：前两条能从单份 schema 判断，顺理成章写进 `WidgetSchema::validate()`；组件总数是**设备属性不是 schema 属性**，没有现成落点，于是掉出了校验器的心智模型。ARCHITECTURE §3.12 与 CHANGELOG 当时都只复述了前两条，所以第一轮按文档核对也发现不了——**只有回头核设计稿的原始约束清单才暴露**。
  - 现加 `widget::MAX_WIDGETS = 8`，在 `Store::insert_widget_definition` 存储边界强制，任何调用方（含未来的 CLI 或导入路径）绕不过。撞上限的请求照常生成预览、确认时被拒，并**照样写入拒绝日志**——「用户想建第 9 个」与「用户想要 grid」同属第二步该看的产品信号。上限是并发容量不是终身配额，删掉一个即腾出位置（有测试钉死这一点）。
- **导出漏了组件数据——不同步 + 不导出 = 那批数据只存在于一台设备上（2026-07-20，Phase 11 验收发现）**——`build_export` 的注释写着 "everything the user owns"，其测试注释更明确：「漏一层就是导出承诺打折」。但 F19 落地时新增的 `widget_defs` / `widget_records` 没有加进去。单独看只是一层遗漏，叠上本轮「三表刻意不同步」的决定才是真问题：**组件记录是用户一条条手输的原创数据，PA 里独此一份**，既不进同步 blob、又不进导出文档，设备丢了就是永久丢失，ARCHITECTURE §4 第 2 条的知情权/删除权也够不着它。现已把两张表加入导出，定义连同其 schema 一起导出（否则记录只是一堆没有列名的匿名 JSON）。
  - 原有测试只断言「每层 key 在场」，而空数组同样满足该断言——即便当时加了 key 也测不出内容缺失。新增 `export_carries_widget_definitions_and_their_records` 直接断言导出里的定义名、schema 字段名、记录归属 `widget_id` 与记录内容。
  - **共性前置**：今后再加任何设备本地表，`export.rs` 是和 `SYNCED_TABLES` 同等的必查项——不同步的表反而更依赖导出，因为它没有第二份副本。见 PITFALLS 当日条目。
- **同步前向兼容：读不懂的 op 不再让整条同步链永久卡死（schema v11，2026-07-19）**——`apply_one` 遇到本版本不认识的表时抛 `CoreError::Invalid`，该错误一路上抛：先让 `apply_remote_ops` 的事务整体回滚（**同一 blob 里合法的 events/routines 一并丢失**），再让 `sync_once` 提前返回，于是 `sync_pulled_seq` 游标不前进。下一轮从同一 seq 拉同一个 blob、同样报错，**无限重复**。而 push 在 pull 之前，旧设备仍在正常上传自己的改动——**表面存活、单向流动、分歧无声增长**，是最难察觉的坏法。已由复现测试钉死（`an_op_from_a_newer_peer_does_not_wedge_the_pull_cursor`，修复前实测 `Err(Invalid("unknown sync table widget_defs"))`）。
  - **改为隔离区（quarantine）而非跳过**：新增设备本地表 `sync_quarantine`（不参与同步、无 guid），本版本无法解释的 op 原样暂存，游标照常前进。选它而不选"直接跳过"是因为跳过会**静默丢数据**——游标越过该 op 后，即便日后升级也永远补不回来。
  - **升级即重放**：`migrate()` 末尾调 `replay_sync_quarantine()`，按 `(hlc, origin)` 排序走原有 `apply_remote_ops` 路径，LWW 语义与首次到达时一致。仍然不认识的表继续留存等更后面的版本；已认识的表无论结果如何都出队（结论已终局）。
  - **一并修掉「一条坏数据锁死整批」**：已知表但 payload 缺必需字段的 op 同样进隔离区而非中断合并。每条 op 包在 `SAVEPOINT` 里，隔离前回滚，确保不留半写状态。仅 `CoreError::Invalid`（数据形状问题）走此路径，存储/IO 错误照旧上抛。
  - **有界且可见**：隔离区上限 5000 条，溢出丢弃最旧的并累计到 `meta.sync_quarantine_dropped`；`pa sync` 在隔离数非零时提示「对端版本比本机新，数据没丢」，丢弃数非零时告警要求升级。**丢数据可以，静默丢不行。**
  - 影响面：这是 F19/每周重复 routine 等"未来新增同步表"的共性前置——修掉之后再加同步表不会再打断旧设备。取舍与被否方案见 MISC 2026-07-19「同步前向兼容」条目。

### Changed
- **「通知上云」与多设备同步解耦（schema v10，2026-07-19 用户拍板）**——此前 `local_only` 一个列同时是「不发云端 LLM」和「不参与同步」两件事的条件，导致「想要设备间同步通知、但别喂给 LLM」这个诉求无法表达；而两者风险量级完全不同（发 LLM 是**明文**给用户自配的第三方厂商，同步是**端到端加密 blob** 发往用户**自建**、解不开明文的中转服务器）。现拆开：
  - **同步无条件常开**：`raw_inputs`/`events`/`notifications` 三张表的同步触发器条件从 `NEW.local_only = 0` 改为恒真，通知及派生数据始终参与多设备同步，无开关可关。
  - **`local_only` 语义收窄为「不得作为云端 LLM 上下文」**，与同步无关。保留列名以避免大范围迁移，语义以 ARCHITECTURE §3.8 为准。
  - **捕获时的判定随 payload 跨设备传播**：`raw_inputs`/`events` 同步 payload 新增 `local_only` 字段，接收端直接采用捕获设备的原始判定；缺该字段的旧 payload 保守回退为「按 `[通知·` 前缀判定为排除」。此前接收端是靠前缀猜测重新判定的，会让同一条记录在不同设备上资格不一致——捕获时允许的在对端被误判为禁止（丢上下文），更危险的反向误判则靠开关侥幸挡着。接收端 recall 仍额外 AND 本机开关，形成「捕获时同意 + 本机当前设置」双重门。
  - **v9 → v10 迁移**：此前因 `local_only=1` 被挡在 oplog 外的历史通知行，经一次 no-op UPDATE 触发写入 oplog，从而首次同步到其他设备；它们的 `local_only` 戳保持不变，仍不给 LLM。
  - 同步更新 README、前端「设置 → 隐私」文案与 CLI `notif-cloud` 输出——旧文案说的「参与多设备同步」已不再由该开关控制，留着会误导。两条既有测试断言的是被推翻的旧契约（local_only 行不产生 oplog），改写为新不变量；新增跨设备回归测试覆盖两个方向：捕获时禁止的同步后在对端仍禁止、捕获时允许的同步后在对端仍允许（后者正是前缀猜测法必然猜错的方向）。

### Fixed
- **`hidden` 属性被类选择器的 `display` 压过，Android 专属按钮在桌面并未真正隐藏（2026-07-19，九项修复的复验发现）**——上一条声称「隐藏 Android 电池优化/后台设置三按钮」，JS 侧 `$("notifPipelineAndroidControls").hidden = !p.supported` 确实设上了，但该容器带 `class="btnrow"`，而 `.btnrow { display: flex }` 是**作者样式**、压过 `hidden` 属性所依赖的 UA `display:none`——复验实测三个按钮在桌面仍以 460×30 满尺寸渲染（`computedDisplay: "flex"`）。所以那句描述当时并不属实；后端拒绝直调那一半是好的，危害仅从「假报成功」降到「桌面上摆着点了会报错的按钮」。现加全局 `[hidden] { display: none !important; }` 一次兜死，并删掉此前为帮助浮窗单独打的 `.vhead .desc[hidden]` 特例（同一个坑的点状补丁）。复验：三按钮 `display:none`、0×0、`offsetParent` 为 null，同一 `.btnrow` 里的「立即处理普通队列」不受影响；全站 16 处 `hidden` 元素（12 个说明浮窗 + 隐藏文件输入 + 温度自定义输入等）全部走 `el.hidden` 属性开关、无一依赖 `style.display`，帮助浮窗展开/收起与 `descIn` 动画实测未回归。
- **Phase 10 F20 真人走查九项修复（2026-07-19）**——F12「通知回看」逐行同时展示标题、截断后的正文与统一格式的接收时间；`local_only` 行的「恢复处理」明确只重跑本机规则，重试失败会保留不同且可解释的原因，回归测试锁定其绝不触发云端调用。桌面壳从后端显式读取平台能力，隐藏 Android 电池优化/后台设置三按钮，桌面实现也会拒绝意外直调而非假报成功。重要规则有独立的包作用域控件，默认全局；微信/QQ/钉钉快捷项仅填包名、不再一次点击即授权。动作确认卡投影当前事件的原时间，改期/取消文案完整；确认动作后的源 capture 进入新终态 `resolved`（F12「已处理」且无恢复入口）。停止捕获会清掉该 App 仍保有 `preset:` 身份的预设规则、保留 `user:notif:` 用户规则；F12 确认/忽略/恢复/提升按钮在 IPC 飞行期间禁用，失败才恢复。真浏览器 mock-IPC 走查、Node 语法检查与 Rust 质量门均覆盖。
- **`local_only` 通知不再因开关重开而被补传上云（2026-07-19，Phase 10 验收发现）**——`process_notification_records` 判定是否调用云端分诊时只查了**当前全局** `notif_cloud_enabled`，构造 LLM 载荷时未按逐行 `local_only` 过滤。后果：关闭「通知上云」期间捕获、尚未被批处理排空的 `queued` 行，会在用户重新打开开关后连同新通知一起发往云端，违反 PRIVACY.md §2「先前标为仅本机的历史不会因后来重新开启而被补传」、ARCHITECTURE §4 隐私不变量，以及 `NotificationCaptureRecord` 类型自身的注释约定。更隐蔽的是 `set_notification_capture_state` 只改 `state/reason/event_id`、**从不回写 `local_only`**，所以外发之后该行在库里仍标着 `local_only=1`，导出、同步排除与 recall 排除等所有信任该戳的下游都会继续把它当成"从未离开本机"，审计面持续给出错误答案。现改为在云端判定**之前**按 `!record.local_only` 分流，捕获时被标仅本机的行一律落 `NeedsReview` 并写明原因；离线抽取仍在云端判定之前照跑，本地功能不受影响。暴露面仅限普通车道（紧急车道立即处理），量 = 上次成功批处理之后攒下的行、单批上限 24；宿主进程被 ROM 杀得越久攒得越多。附回归测试 `local_only_captures_are_never_backfilled_to_cloud_after_toggle`（修前实测泄漏、修后绿）。质量门三绿（208 测试 / clippy 零告警 / fmt 无漂移）。
- **同步合并不得改写本机 `notif_cloud` 偏好**——补断言覆盖：另一台设备合入同步数据后本机开关保持关闭（该开关按设计是设备本地偏好，不跨设备同步）。

### Added
- **Phase 10：F20 通知智能管线（2026-07-19）**——schema v9 新增本机 `notification_captures`、过滤提议与动作确认提议；默认空的 App 白名单在 Android listener 读取通知内容前即拦截。白名单内通知进入可逆的 F12「通知回看」，记录建事件、待处理、已过滤和 10 分钟同包名+内容 hash 判重的全部去向。
  - `pa-core::notification_intelligence` 提供纯逻辑的双车道、包范围子串/正则重要规则、微信/QQ/钉钉可编辑预设、批量 LLM 协议与防御式解析。规则先抽取；仅当通知上云开启且本地不确定时，重要车道单条/普通车道至多 24 条批量调用 LLM。云端关闭时保证零调用、零外发；失败保留为可恢复的待回看，而非静默丢弃。
  - LLM 只能提出低爆炸半径的新事件、**待用户确认**的过滤规则，或不含 id 的取消/改期意图，不能执行已有记录动作；`LLM_ACTIONS` 未扩张。已有记录动作先由 Rust 唯一匹配本机 event id，再在 F12 以确认/忽略卡片呈现；过滤规则确认前不生效，历史过滤/判重可恢复或提升为日程。
  - Android `pa-notif-access` 新增白名单策略文件、低优先级 `dataSync` 前台处理服务、电池优化/后台设置引导；只有白名单非空才启动服务，普通队列仍由宿主内部 15–30 分钟定时器处理，未引 WorkManager。CLI 新增 `pa notif-intelligence status|allow|deny|process`，壳层新增隐私设置与 F12 回看入口。
  - 同步语义保持 Phase 9：raw input 与派生数据的 `local_only` 在捕获时冻结；本批分诊元数据不参与同步，白名单与通知上云均为设备本机偏好。测试覆盖 opt-in、判重、F12 可见/恢复、离线降级和云端关闭零调用。

### Changed
- **根目录文档收口：PRODUCT/DESIGN 迁入 docs/ + README 全面对齐（2026-07-19）**——排查发现 `docs/` 全部与代码一致，过时集中在根目录。`PRODUCT.md`、`DESIGN.md` 经 `git mv` 迁入 `docs/`（根目录只留 README / AGENTS.md / CLAUDE.md 三份必须在根的），AGENTS.md 开工前第 3 条与 `dist/index.html` 语义色注释的引用路径同步更新。README 修正七处与代码脱节的事实：进度 Phase 1–7 → **Phase 1–9**（补 Soulous 双向互通与通知上云开关）、测试数 189 → **201**（实跑核实）、tauri command 40+ → **56**、视图 12 → **14**（补「隐私」「云端」两个设置页）、`docs/` 目录清单补 `PRIVACY.md` / `PRODUCT.md` / `DESIGN.md`、pa-core 模块表补 `persona.rs` 与 `export.rs`、厂商预设「9 家」→「8 家 + 自定义端点」；另补「隐私与通知上云」小节、流式输出说明与每日固定提醒 CLI 示例。PRODUCT.md 的 Positioning 与 Product Purpose 按 `PRIVACY.md` 重写（原文"隐私完全归你/一切原始数据只在本地"与通知上云默认开启矛盾），并明确细则以 `PRIVACY.md` 为单一权威、不在 PRODUCT 复述。README 面向 Build Week 评委的英文章节移入 `docs/MISC.md` 存档，README 留一行指路。取舍与核实过程见 MISC 当日两条条目。
- **OpenAI 厂商预设模型名改 `gpt-5.6`（2026-07-19）**——`LLM_PRESETS` 的 openai 项此前给 `gpt-5.2`，与 README 示例的 `gpt-5.6` 不一致；用户拍板以 `gpt-5.6` 为准。仅改 datalist 建议值（本就可自由改写），`temp: "omit"` 等其余字段不变。

### Added
- **Phase 9「通知上云」地基（2026-07-19）**——新增默认开启、可在「设置 → 隐私」和 CLI `pa notif-cloud` / `pa notif-cloud-set <on|off>` 控制的设备本地开关。`ingest_captured` 只在捕获时读取它：开启时通知 raw input、派生 event 与 notification 的 `local_only=0`，复用原有 SQLite oplog 触发器参与多设备同步，并可经既有 recall 作为聊天云端上下文；关闭时三表为 `local_only=1`、不产生同步 oplog、被 recall 排除。历史 local-only 行不回填，开关本身不跨设备同步。通知捕获仍只跑本地规则、零 LLM 调用；`LLM_ACTIONS` 仍精确限制为 `ingest` / `checkin_answer`，GenUI/Guard/Soulous 出口均未放宽。新增回归覆盖开关边界、实际跨设备合并、recall 上下文、零 LLM 捕获调用、手输路径不受影响及 legacy 迁移一次性执行。
- **聊天流式输出（文字优先，2026-07-19）**——普通闲聊回复现在逐 token 边到边显示，不再等整段生成完才出现。网关抽象新增 `Reasoner::complete_streaming`（OpenAI 兼容 `stream:true` + SSE 逐行解析，只读 `delta.content`，`reasoning_content` 与内联 `<think>` 前导块天然/主动过滤；默认实现回退到非流式 `complete`，离线与测试 reasoner 零改动），`llm::chat_reply_ui_streaming` 用 sniff-router **只把纯文本前缀的回复可见流式、以 `{`/```` ``` ````开头的 GenUI 信封整包收完再走既有严格 `parse_envelope`**（信封渲染与校验一字未改，§3.9 四条硬原则不受影响）。`Orchestrator::ingest_streaming` 新入口把闲聊可见 token 透过 `&mut FnMut(&str)` 回调外抛；pa-app 的 `ingest` 命令新增可选 `stream_id`，在 `spawn_blocking` 内经 `pa-chat-delta` 事件推回，命令仍返回完整 `IngestResp` 定稿；前端按 `streamId` 把首个 token 起就地把「正在输入」气泡转成增长气泡，resolve 后用权威终值定稿并渲染信封。上行内容不变（§3.6 最小上下文），流式只影响下行呈现。为让纯闲聊能以纯文本开头，聊天 prompt 从「永远输出信封」放松为「默认纯文本、仅有可执行动作时才输出信封」——纯闲聊渲染结果与旧「纯文本信封」完全一致。**组件级增量渲染（类 A2UI）仍明确延后**（半截 JSON 容错需重构 `parse_envelope`）。新增 pa-core 单测覆盖 SSE 行分类、`<think>` 跨块过滤、prose 流式/信封抑制、默认实现单次回调，以及 orchestrator 级 `ingest_streaming` 两路；前端经 mock-IPC harness 真浏览器走查确认渐进填充（0→6→…→36 字）、信封抑制整包渲染 choice、事件意图整包回流三条路径。质量门三绿（`cargo test` 197 / `clippy -D warnings` 零告警 / `fmt --check` 无漂移）。设计取舍见 MISC 当日条目、ARCHITECTURE §3.6 第 7 条与 §3.9。
  - **真机验收（2026-07-19）**：打真实 MiMo `mimo-v2.5` 端到端复核——纯闲聊经真实 SSE 收到 21 个可见 delta（3165→4490ms 逐个到达、纯散文 `ui:None`），含动作输入返回信封时 0 可见 delta（sniff-router 抑制、整包定稿），证实我方 SSE 解析对真实 provider 兼容且 prompt 放松让闲聊以纯文本开头可流式。真实桌面壳 `pa-app.exe`（WebView2 + 真实云端）命令行驱动走查，密集抓拍捕到真实窗口里逐字增长的中间态（4221ms「早上起床后先喝一杯温水，帮助身体唤醒」→4444ms 续到「…促进血液循环；再花」半截→填满定稿），等待期正确显示打字指示。驱动手法的坑记 PITFALLS 当日条目。
- **Phase 8.2「PA → Soulous 受控日程推送」（2026-07-18）**——出站白名单 `push_schedule_events` 默认关闭，并在「设置 → 云端」与 F12 台账持续可见；只有用户明确开启后，普通 PA 日程才出现逐条「推送」入口。每次发送必经 `soulous_push_event` Sensitive Tool 和一次性 Guard 确认，预览与实际请求复用同一最小投影：标题、类型、开始/结束时间、地点；参与人、原始输入、通知文本、人格和 PA 记忆永不出本机，第三方通知来源的 `local_only` 日程也会被核心层拒绝。
  - **接收端与防回声**：用户授权后才改动 Soull，新增认证的 `/api/external-context` 单向接收端与 `external_context` 表（H2 v23 / MySQL v24）；按用户 + `source=pa` + 类型 + stable `externalId` 幂等存储，字段白名单 fail-closed，无供 PA 读回的 API。外部事实只在接收 Soulous 用户自己开启 `aiMemoryEnabled` 时，才通过既有 `RetrievalService` 写入 RAG；两侧不交换记忆或向量库。
  - **可靠性与验收**：PA 复用 JWT refresh 处理 401 并仅把新 token 写回本地配置；网络不会进入 ingest、ticker 或同步路径。新增 Rust 单元测试覆盖投影、`local_only` 拒绝与 refresh，Soull Spring 集成测试覆盖白名单、用户隔离幂等与 AI 记忆关闭不索引；桌面 mock 走查覆盖默认关闭、显式授权、最小确认与确认参数仅含 `event_id`。
- **每日自然语言固定提醒（2026-07-18）**——带明确时间的「每天/每日/每早/每晚……」输入现在直接创建 `routine`，而非只登记一次日程；创建后立即物化今天与明天的 occurrence，复用既有提醒、Android AlarmManager、同步与 F12 台账链路。`每早` / `每晚` 与每日语义完全一致，分别沿用离线时钟解析的上午/晚上时段；标题会剥离频率、时间与「提醒我」等填充词；同名活跃 routine 不重复创建。当前刻意只支持每日频率，`每周` / `每月` / `每小时` 仍明确提示未支持，绝不伪装成一次性日程。新增回归测试覆盖创建、去重、即时触发与未支持频率边界。
- **固定提醒创建确认卡（2026-07-18）**——每日自然语言创建 routine 后，对话流会直接显示「固定提醒」卡（标题、每天时间、启用状态）及唯一的「暂停此固定提醒」动作；操作复用既有 `routine_set_active`，单次提交后前端立即禁用按钮以防重复执行。GenUI 对该动作严格校验 `{ id, active: bool }`，离线创建模板只提供可逆的暂停操作，LLM 动作子集不新增此能力。
- **固定提醒台账管理（2026-07-18）**——记忆台账中的每条固定提醒现在可编辑标题与每天时间，并直接显示「暂停」或「启用」操作；保存会立即替换尚未触发的 occurrence，已触发记录保留为历史。复用本地 `routines` / `routine_set_active` 并新增 `routine_update` command；不新增表或网络路径，既有 SQLite 同步 payload 会携带更新。按钮提交期间禁用，失败时恢复可操作状态。
- **Android release APK 重打（2026-07-18）**——含当日 Phase 8.1 与专注时间解析修复的最新代码。沿用 2026-07-14 链路：`cargo ndk`（arm64-v8a + x86_64，`--features tauri/custom-protocol`）出 release `.so` + `PA_PREBUILT_JNILIBS=1 gradlew assembleUniversalRelease`，同一 release keystore 签名（可覆盖安装升级）。产物 `gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk`（39.7 MB），apksigner 验签通过。
- **开启 Windows 安装包打包（2026-07-18）**——`tauri.conf.json` 的 `bundle.active` 置 true，目标仅 NSIS（不引入 WiX/MSI 工具链），`installMode: currentUser`（免管理员权限），安装器语言简中+英文。`tauri build` 产出 `target/release/bundle/nsis/PA_0.1.0_x64-setup.exe`（约 4 MB）。按 PITFALLS 2026-07-15 条目，安装态会注册 AUMID，toast 署名问题随之解决（待安装后真机验证）。

### Fixed
- **固定提醒重新启用后会重新排程（2026-07-18）**——暂停会撤回未触发 occurrence，但旧实现保留了 `scheduled_until` 水位；随后重新启用时，物化器会把已被撤回的日期误判为已排程，造成下一次提醒缺失。现在仅在 `inactive → active` 跃迁时清空水位并立即按注入时钟物化；重复点击启用不重排，已触发历史不受影响。新增回归测试覆盖暂停、恢复和再次到点。
- **Soulous 时间解析补小数秒格式（2026-07-18，真机验收发现）**——`parse_remote_time` 新增 `%Y-%m-%dT%H:%M:%S%.f` 分支：Java `LocalDateTime` 带不定宽小数秒（如专注 `startedAt`），原三种格式全失配导致 14 段专注静默丢 `occurs_at`、周报专注分钟恒 0。附生产真实字符串回归测试；修后真机复测专注 925 分钟正确入周报。根因与教训见 PITFALLS 当日条目。验收全过程：真实拉取（任务 8/打卡 1/专注 14）、坏 access token 触发 401→refresh→轮换落盘、不可达主机报错且缓存原样保留、`memory_facts` 保持零污染，均通过。

### Added
- **Phase 8.1「Soulous → PA 只读数据拉取」（2026-07-18）**——新增 `pa-core::soulous`：只读调用 Soulous 既有 REST API 拉取课表、考试、任务、今日打卡快照与专注时长；复用其 JWT access/refresh 双 token，401 自动刷新并把旋转后的 token 仅写回本机 gitignore 的 `pa-soulous.json`。缺配置时整项功能静默关闭。
  - **独立且可同步的事实缓存**：schema v6 新增 `soulous_facts`，所有行固定 `source=soulous`、稳定 guid、SQLite 同步触发器和行级 LWW；完整响应解析完才原子替换快照，任何拉取/解析失败都保留上次成功缓存。数据从未写入 `memory_facts`，recall 也不会读取它。
  - **消费与入口**：真实考试接入 Importance Classifier 的规则视图；考试/任务作为 F10 前瞻建议素材；课表/考试/未完成任务/打卡/专注时长进入 F14 周报的本地素材段。新增 `pa soulous pull` / `pa soulous status`，以及桌面壳「设置 → 云端」的本地配置区与手动拉取按钮；网络只在手动拉取时发生，因此不会阻塞 ingest、提醒或 ticker。
  - **已知 API 边界**：Soulous 现有 `GET /api/checkin` 只有当天状态，故只存成功拉取时的当天快照，不杜撰历史；完整历史须由 Soulous 后续提供只读聚合端点。Soull 仓库零改动。
- **Phase 8「Soulous 互通」立项（2026-07-18，仅文档声明，零代码）**——ARCHITECTURE 新增 §3.11（数据三级分级 + 四条硬约束 + 实施顺序）、§4 隐私原则第 1 条细化 + 新增第 5 条、§6 新增 Phase 8 两条待做（8.1 Soulous→PA 只读拉取先行；8.2 PA→Soulous 白名单推送后行，8.1 跑顺前不开工）。原则调整的动机与否决方案见 MISC 当日条目。实现计划交由外部智能体（GPT-5.6）执行。

### Fixed
- **computer-use 全流程走查修复批（2026-07-18，问题清单见 MISC 2026-07-17 走查条目）**——两轮真机走查发现的 bug/缺口一次修完，全 workspace 179 测试全绿、clippy 零告警：
  - **云端调用不再冻结窗口**：`ingest`/`review` 改 `async fn` + `spawn_blocking`（`AppState.orch` 换 `Arc<Mutex<_>>`），云端等待期间 UI 保持响应（根因与验证见 PITFALLS 当日条目）。
  - **snooze toast 修复**：`fmtDT()`（返回 DOM 节点）误入模板字符串显示 `[object HTMLSpanElement]`，改 `fmtDTText()`。
  - **对话高危拦截不再是死胡同**：无专属确认入口的高危回复附「前往护栏查看确认流程」按钮；D3 示例措辞「清掉/清除」补入危险词表（含路由测试），「把上周的行为日志清掉」现在正确走 `ledger_purge` 护栏流程。
  - **习惯检测抗离群点**：聚类从全量 min/max 跨度改为中位数 ±45 分钟窗口——一条深夜同名状态不再永久压制习惯建议（附反例测试）。
  - **模拟时钟注入补漏**：`suggest_set` 增加 `now` 参数（routine `created_at` 驱动 7 天暂停刹车）；壳层 `due`/`fire_due` 前置 `materialize_routines(now)`（与 CLI 对齐——此前模拟时钟下 routine 永远长不出提醒）；导出文件名改用真实墙钟。
  - **D4「一键已完成」补实现**：新 command `routine_done`（core `confirm_routine`：落行为日志 Status 条目、按日去重）+ 提醒视图 routine 行「已完成」按钮——7 天暂停刹车的确认链路从「只能靠嘴说」变成可点击闭环。
  - **今日面板首现即有内容**：挂 resize 监听（200ms 防抖），窗口跨过宽屏断点立即刷新，不再空壳等 30s 周期。
  - **文案与显示**：对话回复/台账时间戳改人类格式（`2026-07-18 20:00`，存储格式不变）；提醒计划落在过去时标注「已过点，将立即提醒」；输入含「每小时/每天…」但只建了单次时明确说明暂不支持重复提醒；台账 `[meeting]`/`(pending)` 等内部记号改中文；「取消提前0的提醒」改「取消这条到点提醒」；记忆改写弹窗确认键从危险红改琥珀主按钮；「今日简报」输入直达简报而非落进闲聊；窄屏表格给最小宽度横向滚动（人格版本历史不再一字一行）。
- **隐私与同步收口（2026-07-17）**——schema v5 将第三方通知捕获及其派生事件/提醒标记为 `local_only`，同步触发器、迁移和远端合并均强制排除，防止通知原文经同步离开设备；暂停或删除 routine 会撤销未触发的已物化 occurrence；大 backlog 同步按加密 blob 大小分批上传并保持游标安全。前端补齐简报时间显示、modal 焦点恢复/Escape/Tab 限制、生成表单 label 关联及分段导航 ARIA/键盘支持。
- **并行会话落盘的四处修复（2026-07-17，随当日功能批次一并提交留痕）**：
  - **recall 片段预算**：单条超长的高分片段此前会直接终止选取，饿死后面还能装进 500 字硬上限的短片段——改为跳过超预算条目继续选（隐私上限不变），附反例测试。
  - **Health Connect 分页**：`readRecords` 默认单页 1000 条，长窗口轮询会静默丢数据——新增 `readAll` 按 pageToken 翻页读全（心率采样量最容易触顶）。
  - **Android 通知去重**：原按「包名+标题+文本」哈希永久去重，同一 app 每天内容相同的例行通知（如打卡提醒）会被永久吞掉——改为 `StatusBarNotification.key`+内容 配对、5 分钟 TTL，只压重复回调不压后续同文通知。
  - **复盘/统计文案去 emoji**：`review`/`stats` 渲染文本里的 📋📊✅⏳ 移除，改纯文字状态——遵守 2026-07-15 设计体系「SVG 线条图标、禁 emoji」的既定约束（CLI 输出的引导性 emoji 不在此列）。

### Added
- **事件改期/取消的自然语言路径（F1 扩展，2026-07-17）**——此前事件只能删了重录，「把明天的会改到下午4点」会被当成新事件录入。现在是完整的离线规则闭环（用户拍板后续做，当日与三件套一并落地）：
  - **意图路由**：`Intent` 新增 `RescheduleEvent` / `CancelEvent`，在 IngestEvent 判定**之前**拦截（否则带事件词+时间的改期句必然误录新事件）；危险词（删掉/清空…）仍然优先走 Guard 路由。改期识别 = 「改到/改成/推迟到/提前到/挪到/换到/调到」等标记 + 右侧能解析出日期或时刻（解析不出则保持闲聊，「把会改一下」不误触）；取消识别 = 「取消X」前缀 / 「X不开了/取消了/不去了」后缀。
  - **半边保留语义**：`time_parse::parse_date_time_parts` 新公开函数把日期/时刻分开解析——「改到4点」保留事件原日期，「推迟到下周五」保留原时刻，两半都给了才全换。end 随 start 平移。
  - **目标解析**（`find_event_targets`）：只在未来事件里找；描述带日期（「明天的会」）先按天过滤，剩余词剥掉日期/填充词后与标题双向包含匹配（「会」命中「开会」）；匹配不到时退回按天候选而不是硬猜。
  - **执行与确认**：改期唯一命中→直接执行（改 start、删 pending 提醒、按规则表重排；fired/dismissed 留作历史）+ 回事件确认卡片；多命中→GenUI `choice` 逐事件带各自预组合的新时间（动作 `event_reschedule {id, start}`）。**取消永不直接删**：无论一个还是多个候选，都渲染写明「事件名 + 时间」的危险样式按钮（动作 `event_cancel {id}`），点按即确认——与 2026-07-16「删除要有确认面」的一贯立场一致。
  - **接线面**：GenUI 白名单新增 `event_reschedule` / `event_cancel` 两动作（LLM 可用子集不变——LLM 没有真实行 id）+ 两个离线构建器（`cancel_confirm` / `reschedule_pick`，标题超长截断、时间永不截断）；壳层 `event_reschedule` / `event_cancel` command + 前端 dispatch 白名单同步扩两项；CLI 直控命令 `pa reschedule <id> <自然语言时间>`（NL 路径走 `pa add`）；对话意图标签补「改期/取消日程/记忆写入」。
  - 验证：pa-core 新增 9 个测试（路由优先级、两半解析、唯一命中改期+提醒重排、无命中的温和回复、取消必须过确认点按、多命中 choice 信封），全 workspace **179 个全绿**、clippy 零告警；CLI 注入时钟走查（改时刻保日期→改日期保时刻→直控改期→取消出确认不直接删）；前端 mock-IPC harness 点通取消危险按钮（一次性禁用）与改期双选项各自回传。
- **定位缺口收口三件套：提醒 Snooze + 全量数据导出 + 记忆可编辑（2026-07-17）**——按产品定位（PRODUCT.md）逐条对照现有功能后发现三处「承诺了但没做」的缺口，全部为离线确定性小步实现，不动既有写路径与 GenUI 协议（黑客松截止前刻意控制改动面）：
  - **提醒 Snooze「稍后再响」（F2 补全）**：提醒此前只能「触发/取消」，没有管家产品的日常刚需「10 分钟后再叫我」。`Store::snooze_notification`（待触发→顺延；已触发→重新武装回 pending 并清 fired_at；**已取消的拒绝复活**——取消是明确意图，snooze 不该悄悄推翻）+ `Orchestrator::snooze`（1 分钟–24 小时区间校验）；CLI `pa snooze <id> --minutes N`；壳层 `snooze` command + 提醒视图按钮（待触发行「+10分钟」、已触发行「稍后再响」）。同步走既有 UPDATE 触发器，Android AlarmManager 镜像随下一轮 ticker 收敛，零新增机制。
  - **一键全量数据导出（§4 承诺兑现）**：架构 §4 明文「审计日志用户可导出」、定位说「数据完全归你」，但全仓库此前没有任何导出能力。新模块 `pa-core::export`——`build_export` 把用户可见的全部 12 层（raw_inputs/事件/提醒/行为日志/建议/语义记忆/routine/穿戴/人格全部版本/规则表/主动度/审计日志）聚合成一份带格式版本号的 JSON；纯只读、纯本地、不上云；唯一不导出的是同步内务数据（oplog/游标——是传输机制不是记忆）。CLI `pa export [--out 文件]`；壳层 `export_data` command 写库同目录 `pa-export-<时间戳>.json` 并返回路径，台账视图新增「导出全部数据」按钮。
  - **语义记忆可编辑（F12 补全）**：F12 规格写明「可查看、可编辑、可删除」，此前只有查看和删除。`Store::update_fact`（trim 校验、UNIQUE(content) 冲突给人话报错而非静默合并）；recall 直接读权威存储所以改写立即生效；CLI `pa fact-edit <id> <新内容>`；壳层 `fact_update` command + 台账 fact 行铅笔按钮 → 预填当前内容的改写弹窗。**只开放 fact 层编辑**：其余层是行为/事件的忠实记录，改写等于篡改历史，仍然只可删除。
  - 验证：pa-core 新增 3 个测试（snooze 三态、fact 编辑与去重、导出层完整性断言）全 workspace **168 个全绿**、clippy 零告警（顺手修掉一处历史 `redundant_closure`）、rustfmt 全仓通过；CLI 注入时钟端到端走查（顺延→不再到点→新时刻触发→再 snooze→取消后拒绝）；前端经 mock-IPC harness 浏览器点通四条链路（pending/fired 两种 snooze、导出、fact 改写弹窗预填+保存）+ node --check 语法校验。
- **README 全面更新 + LICENSE（黑客松收尾第一批，2026-07-17）**——README 补齐 Phase 5/6/7 之后的全部进度（进度段、仓库结构含 `pa-alarm` 与 pa-core 八个新模块、165 测试数、CLI 新命令示例：`daily-brief`/`recall`/`stats`/`routines`/真实 `ledger_purge` guard 演示）；新增面向 OpenAI Build Week 评委的英文章节（项目定位、Quick start、确定性离线 walkthrough、**如何用 Codex**——如实限定为 Daily Focus Brief 一块、**GPT-5.6 集成点**——OpenAI 兼容网关配置与四条云端路径 + 隐私边界）。新增 MIT LICENSE（公开仓库前置条件；版权行是占位"PA project author"，用户可换实名）。
- **Phase 6 + Phase 7 全量落地（记忆与数据地基 + 主动智能完全体，M1–M3 / D2–D6）**（2026-07-16）——2026-07-15 规划的八项待做一次做完，全 workspace 165 个测试全绿、clippy 零告警、CLI 端到端走查 + 桌面壳启动冒烟通过。schema v3→v4（`memory_facts` + `routines` 两张新表，guid + 同步触发器 + 行级 LWW 全接入）。
  - **M1 会话短期上下文**：`llm::ChatTurn`/`ChatContext`，orchestrator 进程内存持有最近 ≤4 轮对话（`VecDeque`，不持久化不同步，关进程即失），chat 系统提示新增「最近对话」区块；`MAX_HISTORY_TURNS` 硬上限在代码里。
  - **M2 语义记忆**：`pa-core::memory`（`MemoryFact`）+ `memory_facts` 表（UNIQUE(content) 容忍双设备同写，同步冲突按 suggestions 模式跳过）；Intent Router 新增 `MemoryWrite` 意图（规则匹配「记住/别忘了…」前缀，`记住明天3点开会` 仍走事件路由）；F12 台账新层 `fact` 可查可删，删除即从检索语料消失。
  - **M3 recall v1**：`pa-core::recall`——字符 bigram Dice 重合 × 时间衰减（14 天半衰）× 层权重（记忆 3 > 日志 2 > 事件 1），top-5 / 500 字硬上限；语料 = facts + 状态日志（近 60 天）+ 事件（当时 `list_recall_events` 在 SQL 层排除 `[通知·…]` 来源事件；该行为已由 Phase 9 的「通知上云」开关条件化）；接入 `chat_reply/chat_reply_ui` 系统提示「已知背景」区块；CLI `pa recall <query>` 让上行内容可肉眼审计。
  - **D2 数据回看**：`pa-core::stats`——日程 kind 分布 / 问询应答率 / 重复活动聚类（典型时间）/ 穿戴三类基线（近 28 天中位数：静息心率=日最低值中位、每晚睡眠、每日步数）+ **F11/F13 数据门槛判定（每类 ≥14 天）**；CLI `pa stats`、壳层 `stats` command。纯离线只读。
  - **D3 第一个真实 dangerous 工具 `ledger_purge`**：对话说「清空三天前的行为日志」→ 识别层（行为日志/建议/穿戴）与时间范围（上周/N天前/全部）→ 回复带真实匹配条数 + GenUI `guard_request` 危险按钮（新构建器 `genui::guard_entry`，仅是入口）→ §3.3 完整流程：**预览显示真实待删条数**（`Tool` trait 新增 `ToolCtx` 让工具可读 store）→ 人工确认 → 一次性令牌 → 真删（同步触发器捕获删除，purge 随 §3.8 传播到其他设备）→ append-only 审计。Guard 从演习变实弹；无令牌尝试被拒并审计（CLI `guard-demo ledger_purge` 走查通过）。
  - **D4 习惯闭环完全体**：`routines` 表 + `pa-core::routine`；habit 建议的 `source` 改为机器可读 `habit:<HH:MM>:<title>`，**采纳即自动建 routine**（同名活跃去重）并顺手固化一条 `memory_facts`（source=habit）；每日 occurrence 由 `materialize_routines`（今天+明天，水位防重、同名同刻防同步重复）物化为普通 event(kind=reminder)+0m notification——**触发/同步/台账/Android AlarmManager 镜像全部复用既有管线**；ticker 每分钟物化、CLI `pa fire` 前置物化；**反向刹车**：活跃 ≥7 天且 7 天内无一条匹配的状态确认 → `routine_pause` 建议（按 ISO 周去重），采纳即暂停（不删除）。CLI `pa routines`/`pa routine-set <id> on|off`；台账 `routine` 层可查可删。
  - **D5 F11 三信号 + F13 两场景**：`suggest::generate_wellness`——久坐（≥12 点且当日步数 < 基线 15%）/ 睡眠不足（昨晚 < 基线 80%）/ 静息心率连续 3 日 > 基线 110%，**阈值全部相对个人基线**（复用 stats 管道）、逐信号检查 14 天数据门槛、按日 dedup、走建议管线**不发系统通知**（2026-07-14 降噪决定）。`pa-core::scene`——纯函数：睡眠中（睡眠会话覆盖当下或 23:00–07:00）/ 日程中（事件区间内，无 end 按 1h）；**问询与自动建议在非常规场景静默，事件提醒不受影响**（F2 优先于场景礼貌）。
  - **D6 复盘叙事增强**：Digest 新增「观察」（窗口内重复活动 top3 + wellness 触发次数）与「本周我记住了什么」（窗口内新增 facts）两段；`render_core/render_extras` 拆分——**云端改写只收到数字核心段，观察与事实内容永不上行**，改写结果本地拼回 extras（`rewrite_sends_core_only` 测试锁死该边界）。
  - 接线面：`suggest_set`（CLI/壳层）现在返回可选跟进消息（建了/暂停了哪个 routine），前端 toast 展示；台账 LAYER 标签补 记忆/固定提醒 两层；`pa-app` 新 command `routines`/`routine_set_active`/`stats`。
  - 测试：pa-core 新增 20+ 单元/集成测试（recall 评分与排除、fact 去重与台账删除、purge 全流程含拒绝路径、habit→routine→物化→触发→暂停全闭环、wellness 门槛、场景门控、facts/routines 双设备同步收敛、D6 核心段隐私边界），全 workspace 165 个全绿；clippy 零告警；CLI 三段式端到端走查 + 桌面壳 12s 启动冒烟 + 前端 JS 语法校验通过。
- **「今日聚焦简报」（Daily Focus Brief，2026-07-16）**——新增一个只读的日内优先级汇总，把今天仍未开始的日程、已到点/即将到点提醒与待处理建议集中到同一份简报；不新增 Store 查询、不写入事件/提醒/建议表，也不触及 F11/F13、同步、加密或 HITL 护栏。
  - **核心聚合（`pa-core::brief`）**：`build_brief(now, events, notifications, suggestions)` 是纯函数，按当天窗口筛出日程、按 pending 状态拆分到点与接下来三条提醒、保留前 3 条 pending 建议；`Brief::render()` 提供与 F14 digest 同风格的中文文本摘要。`Orchestrator::daily_brief()` 仅复用 `upcoming_events`、`list_notifications`、`list_suggestions`，窗口规则全部留在纯模块，便于入口一致且可测试。
  - **F18 卡片**：`genui::daily_brief_prompt()` 只组合现有 `text` / `event_card` / `reminder_card` / `suggestion_card` / `button_group` 五种目录组件，建议仍绑定既有 `suggestion_set` 采纳/忽略动作；信封遵守 `MAX_COMPONENTS`、由 `checked()` 校验，真正空日返回 `None`，不创建新的 UI 组件或动作。
  - **三种入口**：CLI 新增 `pa daily-brief`；壳层新增同名 `daily_brief` command；resident ticker 每天最多构建并推送一次 `pa-daily-brief`（日期门只留在进程内，不污染持久层/同步层）。聊天页收到推送后复用既有 `renderEnvelope()` 渲染，空状态示例区也提供「刷新今日简报」手动入口（调用同一个 Tauri command）。
  - **验证**：新增 Brief 窗口单元测试、Orchestrator ingest→建议→简报集成测试、GenUI JSON 往返/白名单校验与空日测试；完整工作区 `cargo test`（131 个 `pa-core` 测试）和 `cargo clippy --all-targets -- -D warnings` 通过。
- **「设置 → 云端」API 设置界面：厂商预设下拉 + 测试连接 + 保存热切换（2026-07-15）**——此前配云端只能手改 `pa-llm.json` / 环境变量，现在壳层内可视化完成全流程。
  - **后端（`pa-app`）三个新命令**：`llm_config_get`（返回脱敏配置：base_url/model/参数/密钥尾 4 位 + 来源 env|file + 落盘路径，**永不回传完整密钥**）；`llm_config_save`（写 `PA_LLM_CONFIG` 指向的 JSON → 热切换运行中的 reasoner + 状态栏就地点亮，无需重启；API Key 留空则沿用文件里已存的，改模型不用重输密钥）；`llm_config_test`（用表单当前值真发一次 chat 请求，async + `spawn_blocking`，返回时延与模型回复，不阻塞 UI 线程）。`AppState.llm_summary` 相应改为 `Mutex<Option<String>>`。
  - **前端**：设置组新增「云端」视图。厂商预设下拉 9 项（小米 MiMo Token Plan/按量、DeepSeek、智谱 GLM、Kimi 开放平台、Qwen DashScope、OpenAI、Gemini、自定义），选中即填 base_url/模型 datalist/超时/温度模式，并给出该厂商的坑位提示（数据即 docs/LLM-PROVIDERS.md 调研结论，如 OpenAI 预设自动把 temperature 切到「不发送」、Kimi 提示订阅 key 不可用）。temperature 三态（默认 0.3 / 自定义 / 不发送）、max_tokens、超时秒可调；密钥输入框 type=password，已存密钥只显尾 4 位占位符。环境变量优先时横幅明示「这里保存只对本次运行生效」。
  - **自测**（mock-IPC harness + 真实 Chrome，全部通过）：路由/预设填充/OpenAI 温度自动切换/无 key 与坏 URL 校验 toast/测试成功与失败两路渲染/保存后页脚点亮+key 清空+占位符更新/留空 key 二次保存沿用旧密钥/`refreshAll` 不覆盖正在编辑的表单（表单只在首次加载与保存后回填）。`cargo test --workspace` 133 个全绿、clippy 零告警。
- **LLM 厂商接入调研文档 + 云端网关配置扩展（2026-07-15）**——新增 `docs/LLM-PROVIDERS.md`：按厂商分类整理 7 家（小米 MiMo Token Plan / DeepSeek / 智谱 GLM / Kimi 开放平台与订阅 / Qwen DashScope 与 Token Plan / OpenAI / Gemini）的 base_url、模型名、认证方式与参数怪癖，结论：全部有 OpenAI 兼容端点，现有「base_url + api_key + model」自定义机制即可覆盖。据调研修复三处兼容性硬伤（`pa-core::llm`）：
  - `temperature` 不再写死 0.3：配置选填，显式 `null` 表示不发送该字段（OpenAI gpt-5 系对非默认 temperature 直接 400）；
  - 新增选填 `max_tokens` 与 `timeout_secs`（原 30s 写死，思考类模型如 DeepSeek V4 / GLM-5 需要更长）；
  - 响应解析剥离部分兼容层内联在 content 里的 `<think>…</think>` 思维链前缀。
  - 对应环境变量：`PA_LLM_TEMPERATURE`（`none` 表示不发送）、`PA_LLM_MAX_TOKENS`、`PA_LLM_TIMEOUT_SECS`。旧 `pa-llm.json` 无需改动（缺省行为不变）。注意：DeepSeek 旧模型名 `deepseek-chat`/`deepseek-reasoner` 2026-07-24 停用，配置需换 `deepseek-v4-flash`/`-pro`；Kimi 订阅（Coding Plan）端点有客户端白名单，PA 直连不可用，接 Kimi 应走开放平台。
- **设置页视图说明收进点击弹出的「?」浮窗（2026-07-15）**——11 个视图（日程/提醒/建议/行为日志/穿戴数据/复盘简报/记忆台账/数字人格/主动度/重要度规则/高危护栏）标题下原本常显的一段功能说明小字，改为默认隐藏、点击标题旁 `.qmark` 圆形问号按钮后以**浮窗**形式悬浮在标题下方（绝对定位，展开/收起都不推动页面布局——用户明确要求 UI 固定不动）；再点问号、点浮窗外任意处或按 Esc 关闭，同屏只保留一个浮窗。字体比原先更轻（.78rem、`--muted`，不用 `--faint` 以守住可读文字对比度下限）。审计日志的信任类小字（`append-only，不可编辑删除`）与空状态提示按用户拍板保持常显，未纳入——踩 PRODUCT.md「记忆必须透明」原则，隐藏会削弱审计可见性。
  - 实现：`ICONS.help`（手绘圆圈+问号，同笔触规格）；每个 `<p class="desc">` 加 `id` + `hidden`，改为 `.vt` 内绝对定位浮窗（`--bg` 底 + 发丝边 + `--shadow-md`，`z-index: var(--z-sticky)`）；标题内嵌 `.qmark` 按钮（`aria-controls`/`aria-expanded`），页面加载时统一补 `help` 图标；委托 `click` + `keydown(Esc)` 两个监听器管开关与互斥，`prefers-reduced-motion` 下淡入动画退化为即时。
- **Android release 签名 + 首个 release APK 打包**（2026-07-14）——之前只打过 debug 包（无签名、可 `run-as` 调试），现在补上自用侧载所需的 release 签名链路。
  - 生成本地自签名 keystore（`crates/pa-app/pa-release.keystore`，RSA 2048，30 年有效期），密钥材料走 `keystore.properties`（`crates/pa-app/gen/android/app/`，同 `pa-llm.json`/`pa-sync.json` 模式）——**keystore 文件 + properties 都已 gitignore，本仓库外无副本**，丢失意味着以后升级 app 必须先卸载（丢本地数据）而非覆盖安装，需要用户自行妥善备份。
  - `app/build.gradle.kts` 补 `signingConfigs { release }`，仅当 `keystore.properties` 存在时生效（无该文件的全新 checkout 下 `release` 变体退化为未签名可编译、不可安装，不炸构建）。
  - 出包用 universal 变体（arm64-v8a + x86_64，覆盖真机与模拟器）：`cargo ndk --platform 26 build --release` 分别出两个 ABI 的 release `.so`（沿用 `PA_PREBUILT_JNILIBS` 逃生门，见 PITFALLS 2026-07-14 五连坑）+ `assembleUniversalRelease`。新增 rustup target `aarch64-linux-android`（此前只装了 x86_64，真机 release 从未编译过）。
  - **模拟器冒烟验证**（release 包本身，非 debug）：`apksigner verify` 确认签名证书正确、`aapt2 dump badging` 确认双 ABI 都在（`native-code: 'arm64-v8a' 'x86_64'`）；卸载旧 debug 包（签名不同无法覆盖安装）后装 release 包，冷启动无 crash、UI 正常渲染；跑通「会议示例」ingest → 事件落库 → F18 事件卡片渲染全链路；`dumpsys alarm` 确认 `pa-alarm` 插件在 ProGuard 混淆（`isMinifyEnabled=true`）下反射分发依然正常——闹钟以 `exact` 模式注册、`origWhen` 与提醒时间吻合，混淆没有把 Tauri `@Command` 反射入口或 Kotlin 插件类删掉。release 包不可 `run-as`（预期行为，验证只能走 dumpsys/logcat，不能读 app 私有目录）。
  - 产物：`crates/pa-app/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk`（约 38MB，versionName 0.1.0 / versionCode 1000，SHA-256 见构建产物本身）。
- **Android 后台提醒：AlarmManager 系统级排程（F2/F16 硬化）+ OS 通知降噪**（2026-07-14）——新 crate `pa-alarm`（Tauri Android 插件，结构同 `pa-notif-access`），模拟器端到端验证：**杀掉 app 进程后提醒仍由系统在 fire_at 准点投递**。
  - **痛点**：移动端 ticker 活在 app 进程里，app 被杀后提醒只能等下次打开补发。现在 ticker 每分钟把 pending 提醒集合镜像成 OS 级 `AlarmManager` 闹钟（签名比对，集合变了才跨桥），系统到点唤醒 `AlarmReceiver` 直接发通知——不需要 PA 进程活着。
  - **职责划分（防双响）**：Android 上 OS 提醒 toast 由 AlarmReceiver 独占，ticker 不再发系统通知；提醒**状态**（mark-fired + 行为日志）仍由 `fire_due` 独占（下次 app 运行时收敛，验证通过：重开后补记状态、无重复通知）。receiver 刻意做哑：不碰 DB、不进 Rust，通知文案在排程时烤进 intent（F16 精神——投递路径零依赖）。
  - **可靠性细节**：`USE_EXACT_ALARM`（API 33+ 自动授予，PA 是提醒类 app 属正当用途）+ `SCHEDULE_EXACT_ALARM`（31–32），运行时降级 inexact 兜底；排程集合持久化 `alarm-schedule.json`（dataDir，同 notif-inbox 模式），`BootReceiver` 开机重挂（闹钟不跨重启）；已触发条目即时从集合摘除防重复武装；一次最多镜像最近 64 条。
  - **OS 通知降噪（用户拍板 2026-07-14）**：系统通知只保留「事件提醒」一类；问询/建议/通知捕获这些信息类只走窗口内呈现（toast/横幅/对话流卡片，事件通道不变）。桌面事件提醒仍由 ticker 发系统通知（桌面没有 AlarmManager 问题）。
  - **验证**（Medium_Phone_API_36.1 模拟器，x86_64 debug）：dumpsys 确认闹钟以 exact 模式注册（`window=0`，`exactAllowReason=policy_permission`）→ `am kill` 杀进程 → 系统准点拉起进程投递「PA 提醒：『开会』（提前30m）」（通知栏截图确认）→ 已触发条目从 json 摘除 → 重开 app 状态收敛。踩坑见 PITFALLS「AlarmManager 后台提醒验证三连坑」。
  - 注：本机走 `PA_PREBUILT_JNILIBS` 逃生门构建，两份自动生成的 gradle 文件（`tauri.settings.gradle`、`app/tauri.build.gradle.kts`）手动补了 `pa-alarm` 条目——内容与下次正常 `cargo tauri android build` 的再生成结果一致。
- **F18 云端路径模拟器端到端验证 + 移动端云端配置加载**（2026-07-14）——真实 MiMo 在 Android 模拟器（Medium_Phone_API_36.1，x86_64 debug APK）上完成交互式生成全闭环。
  - **验证记录（截图逐步核对）**：① 离线模板链路：「明天下午3点在会议室和张伟开会」→ 事件卡片 +「取消提前30m的提醒」按钮 → 点击 → toast「已取消提醒 #1（事件保留）」+ 按钮一次性禁用；② **云端生成链路：「我最近总是忘记喝水，有什么办法吗」→ MiMo 真实返回信封（建议文字 +「设个喝水提醒」primary 按钮）→ 点击 → 按钮携带的 `ingest`("每天定时提醒自己喝水") 自动发出 → 规则抽取不出、云端兜底解析 → 事件【提醒】落库 → 回应再渲染事件卡片（第二代信封）+ 到点角标亮起**。LLM 生成 UI → 白名单动作 → 真实业务写路径的完整闭环成立。
  - **移动端云端配置加载**：Android 无有效 cwd，原「`./pa-llm.json`」查找永远落空、云端恒离线。`pa-app` setup 现在在移动端把 `PA_LLM_CONFIG` 指向 app-data 目录（与 SQLite 同层）；显式 env 仍最高优先，桌面行为不变。debug 包注入方法见 PITFALLS。
  - **对话「闲聊示例」chip 文案** 从「今天天气怎么样」改为「我最近总是忘记喝水，有什么办法吗」——前者只能得到纯文本回复，后者能展示 F18 交互式生成（云端在线时大概率给出提醒按钮）。
  - `gen/android` 的 `BuildTask.kt` 加 `PA_PREBUILT_JNILIBS` 逃生门（无 symlink 权限机器的构建路径，配合 cargo-ndk 手工出 .so），详见 PITFALLS「Android 构建五连坑」。
- **交互式生成 F18 v1（Generative UI，Phase 5，架构 §3.9）**（2026-07-14）——`pa-core::genui` 新模块 + 壳层前端目录内渲染器，agent 回应可携带动态组装的可交互 UI（卡片/按钮/choice/form），操作就地发生在对话流里。
  - **协议层（`pa-core::genui`）**：组件目录 v1 七种（`text` / `event_card` / `reminder_card` / `suggestion_card` / `button_group` / `form` / `choice`），`UiEnvelope { version, components }`；serde `deny_unknown_fields` 严格反序列化 + 数量/长度上限 + **动作白名单**（8 个动作全部映射既有 orchestrator 能力，逐动作参数 schema 校验）；`parse_envelope` 防御解析（剥围栏→严格反序列化→校验，任何失败返回 `None` 降级纯文本，复用 F6 抽取兜底模式）。四个离线模板构建器（事件确认卡/问询快捷答/建议采纳/人格草稿表单），id 全部来自已落库的行。
  - **Reasoner 接入（`llm::chat_reply_ui`）**：Chat 意图单次调用请求信封 JSON（回复正文放 `text` 组件），LLM 可用动作**收窄为 `ingest`/`checkin_answer` 两种**（LLM 没有真实行 id，带 id 的动作只归离线模板）；纯文本回复原样透传，坏 JSON 降级为固定话术、绝不把畸形 JSON 展示给用户。上行数据零新增（UI 描述是云端返回的内容）。
  - **渲染层（`pa-app` 前端）**：vanilla JS 目录内渲染器（JSON→DOM，textContent 赋值不 innerHTML 注入），外观由既有 CSS 固定、信封不能指定颜色/尺寸（防"把危险按钮画成主按钮"式 UI 注入）；壳层 dispatch 表二次校验白名单，未知命令直接拒绝；按钮组点击后一次性禁用（信封是当下状态的一次性渲染，不允许对已推进的状态重复执行）。**UI 描述不持久化**（对话即焚，留痕的是动作结果，取舍见 MISC 2026-07-14）。
  - 接线面：`ingest` 响应带 `ui`（事件确认卡片，离线模板；Chat 在线时为 LLM 信封）；`checkin_now` 响应与 ticker `pa-checkin` 事件带问询快捷答 choice；ticker `pa-suggestions` 事件带采纳/忽略按钮组；`persona_import_preview` 响应带草稿就地编辑 form（submit → `persona_import_save`）。`guard_request` 动作只是入口，确认弹窗 + 一次性令牌照走 F7 全流程。
  - 新增 dev 工具 `cargo run -p pa-core --example genui_dump`：打印四个模板信封 JSON，供前端开发/协议核对。
  - 测试与验证：`pa-core` 新增 9 个单元测试（目录/白名单/参数 schema/降级/LLM 子集收窄/模板构造）共 130 个全绿，clippy 零告警；前端用注入 mock IPC 的 harness（信封 JSON 由 `genui_dump` 逐字节提供）在浏览器端点通四条链路（事件卡取消提醒 / 问询 choice 快捷答 / 建议采纳 / 人格 form 就地编辑改字段后保存）+ 白名单外动作被拒的反向用例，控制台零报错；桌面壳启动冒烟通过。
- **穿戴设备接入 F5 v1：Health Connect 只读适配（Phase 4）**（2026-07-12）——`pa-core::wearable` 新模块 + 新 crate `pa-health-connect`，模拟器端到端验证通过（真实 Health Connect 授权页 + 合成心率记录写入/读回/入库/UI 展示全链路）。
  - **技术路线（架构 §3.7 决定）**：不接三星/小米私有 SDK（三星 Health Data SDK 已改合作伙伴审批制，个人开发者拿不到批准），改走 Android 官方 **Health Connect**（`androidx.health.connect:connect-client:1.1.0`）——三星健康自 2022-10 起即把数据同步进 Health Connect，只需请求标准读权限，不绑定厂商，为后续接入其他同步到 Health Connect 的平台留了口子。
  - **范围严格限定为 F5 本身**（用户 2026-07-12 拍板）：只读心率/步数/睡眠时长，落本地存储 + 纳入 F12 台账 + 随 §3.8 同步；不写回、不接 F11 情绪感知/主动关怀、不接 F13 场景切换——那是需要独立设计触发策略的后续工作。
  - `pa-core::wearable`：`HealthMetric`（HeartRate/Steps/Sleep）+ `HealthSample`（含按 kind+时间窗+来源的 `dedup_key`，重复轮询同一时间窗不会重复入库）。`store.rs` schema v2→v3：新表 `health_samples`（guid + 同步触发器，复用 §3.8 行级 LWW 机制，写法与 `suggestions` 的 dedup_key 冲突跳过一致）；纳入 F12 记忆台账（可查看来源/删除）。
  - `pa-health-connect`：Android-only Tauri 插件，结构与 `pa-notif-access` 一致（桌面 no-op 桩，`is_available()` 恒 `false`）。Kotlin 侧只声明三个 READ 权限（无任何 WRITE），权限授权走 Health Connect 自己的 `PermissionController` Intent 页面（`startActivityForResult` + `@ActivityCallback`，非标准运行时权限弹窗）；含 `PermissionsRationaleActivity`（Health Connect 授权页"隐私政策"链接指向它，侧载环境不声明会崩溃，见 PITFALLS）。
  - `pa-app`：ticker 每 5 分钟轮询一次（与后台同步同频，非每分钟——真实跨进程调用不同于本地文件读取），滚动时间窗避免重复拉取历史；顶栏/穿戴视图授权横幅（未授权时"去授权"按钮，逻辑与通知监听横幅一致）；新增「⌚ 穿戴」视图展示样本列表。app 的 Android `minSdk` 由 24 提到 26（Health Connect client 库要求）。
  - CLI `pa health`：只读列出已存样本（数据采集只在移动端发生，桌面/CLI 靠 §3.8 同步拿到）。
  - 测试：`pa-core` 新增 3 个单元/集成测试（dedup、ledger 纳入、双设备同步收敛），全套 119 个测试通过。
- **通知监听权限应用内引导（F1 收尾，Phase 3）**（2026-07-12）——新 crate `pa-notif-access`：一个本地 Tauri Android 插件，专门解决"通知使用权没有运行时申请弹窗、只能跳系统设置页"的问题（此前 CHANGELOG 里一直标着"应用内引导入口待做"）。
  - Kotlin `NotifAccessPlugin`（`@TauriPlugin`）两个命令：`isEnabled`（读 `NotificationManagerCompat.getEnabledListenerPackages` 判断本应用是否在监听白名单）、`openSettings`（跳 `Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS`）。桌面端 `desktop.rs` 直接 no-op/恒真，横幅天然只在 Android 出现。
  - `pa-app` 顶栏下新增全局横幅：未授权时提示 + 「去开启」按钮，`visibilitychange` 事件触发重新检测（用户从系统设置切回来的唯一时机），已授权则自动隐藏；「本次忽略」仅当次会话生效。
  - **模拟器端到端验证通过**（`Medium_Phone` AVD）：全新安装 → 横幅出现 → 点「去开启」→ 真实跳到系统「通知读取权限」页且 PA 正确出现在列表 → 授权 → 切回 app → 横幅自动消失；logcat 全程无异常。
  - 脚手架取舍见 PITFALLS「`cargo tauri plugin new` 在非交互终端直接报错」。
- **多设备同步上线（F17，Phase 3 第 3 项）**（2026-07-12）——`pa-core::sync` + 新 crate `pa-sync-server`，双设备（两 DB + 真实 localhost 服务器）端到端验证：事件/提醒/人格/配置双向合并、删除传播、服务器全程零明文。
  - **变更捕获在 SQLite 触发器层**（schema v1→v2）：六张同步表加 `guid` 行标识（128 位随机，跨设备稳定），所有 INSERT/UPDATE/DELETE 由触发器写入 `sync_oplog`——包括级联删除，任何 Rust 写路径都漏不掉。应用远端操作时置 `sync_applying` meta 标志抑制触发器，防回声循环。老库迁移时回填 guid 本身就生成 bootstrap 操作，新设备首次同步即获全量历史。
  - **合并 = 行级 LWW**：毫秒 UTC 时间戳（hlc）+ 设备 id 决平局，严格更新才应用，重复投递幂等跳过。FK 在捕获时翻译成 guid（整数 id 是设备本地的）；孤儿提醒（父事件已删）跳过；两设备独立生成同一建议（dedup_key 撞）跳过；**人格版本号冲突自动重编号**（版本号设备本地化，历史 append-only 不变），活动指针以 guid 形式同步、落地时解析回本地版本号。
  - **端到端加密（§3.8 预共享主密钥方案）**：XChaCha20-Poly1305，64 位 hex 主密钥手动配到各设备（`pa-sync.json`，gitignored，同 `pa-llm.json` 模式；或 `PA_SYNC_URL/TOKEN/KEY` env）。服务器只见密文——冒烟测试对中转库全部 blob 做了明文探针检查，零泄漏。`audit_log` 按 §4 设计不同步。
  - **pa-sync-server**：单二进制 + 单 SQLite，tiny_http；`POST /v1/push`（密文 blob）/ `GET /v1/pull?since=N&device=`（增量拉取，排除自己）/ `/v1/health`；Bearer token 鉴权（`PA_SYNC_SERVER_TOKEN` 必填，不设不给起）。
  - 接入面：CLI `pa sync`（一轮推拉合并）/ `pa sync-status`；壳层顶栏 🔄 手动同步按钮 + ticker 每 5 分钟静默自动同步（失败只记日志下轮重试，F16 精神），合并后刷新缓存与 UI。
  - 测试：6 个新单元/集成测试（双设备收敛、删除传播、LWW 状态之争、人格重编号+指针收敛、配置文档同步、加密往返），全套 115 个测试通过。
- **聊天记录导入 → 人格提取（F9 完全体，纯本地管道，Phase 3 第 2 项）**（2026-07-07）——`pa-core::persona_import` 新模块（8 个单元测试 + orchestrator 集成测试）。
  - **纯本地硬约束（架构 §3.4）**：解析与提取全程不出进程、不走云端——记录里有对话另一方的内容，对方未同意上云；原始记录不入库、不进同步管道，只有用户确认后的人格版本落 `persona_versions`（`source = "import"`，可回滚，F15）。
  - 解析支持两种导出格式（按行自动识别）：微信/QQ txt（`2026-07-01 12:30:45 昵称` 头部行 + 消息行，QQ 的 `(号码)`/`<邮箱>` 后缀剥除）与行内式 `昵称: 消息`（可带 `[12:30]` 前导时间戳）。系统行（撤回提示等）与媒体占位符（`[图片]`…）剔除；URL 里的 `:` 不会误判成发言人。
  - 提取只统计"我"的消息（`--me` 指定昵称，昵称不存在时报错并列出检测到的发言人）：语气词词表命中、中文 3–4 字 n-gram 高频短语（重叠窗口去重：共享 2 字片段的候选只留最优，以结构助词开头的降权）、句长/感叹/问句/表情/裸句尾比例 → 推断语气描述。产出可读报告 + `PersonaDraft` 初稿，样本 <20 条给出提示。
  - 流程强制"先预览再确认"：CLI `pa persona-import <file> --me <昵称>` 默认只打印报告，`--save` 才落库；桌面/移动壳「人格」视图新增导入卡片（前端读文件 → `persona_import_preview` → 初稿填入编辑表单可修改 → `persona_import_save`）。
  - 新公开 API：`Orchestrator::preview_persona_import`（纯函数）/ `import_persona`；tauri command `persona_import_preview` / `persona_import_save`。
- **Android 通知监听 → 自动记日程（F1/F2 v1，Phase 3）**（2026-07-07）——已在模拟器端到端验证（注入第三方通知 → 事件+提醒落库 → PA 发 OS 通知告知）。
  - Kotlin `PaNotificationListener`（NotificationListenerService，`gen/android`）：捕获通知（跳过自家包与 ongoing 常驻类，near-64 条内容去重），追加 JSON 行到 `dataDir/notif-inbox.jsonl`（应用私有目录，与 pa.sqlite 同层）。服务保持"哑"：所有筛选智能在 Rust 侧。
  - `pa-core::Orchestrator::ingest_captured`（新公开方法，2 个单元测试）：**隐私门在核心层**——`route_intent` 判非事件、或离线规则抽取不出时间的，直接丢弃且不落任何库；**绝不走云端 LLM 兜底**（第三方文本"永不出进程"，与 F9 同一决策原则）。留下的事件其台账原始条目带 `[通知·<pkg>]` 来源标记，可级联删除。
  - `pa-app` ticker 每分钟排空 inbox（读后即删），捕获成功 → OS 通知「PA 从通知里记了一条日程」+ `pa-captured` 事件 → 前端 toast + 刷新日程。排空逻辑平台无关（桌面端文件不存在则跳过，也便于本地测试）。
  - 需用户授予"通知使用权"（设置 → 通知 → 通知读取权限；模拟器可 `adb shell cmd notification allow_listener dev.pa.app/dev.pa.app.PaNotificationListener`）。应用内引导入口已补上，见上方「通知监听权限应用内引导」条目。
- **Android 壳起步：pa-app 可构建出 Android APK（Phase 3 第一步）**（2026-07-07）——同一 crate 同时充当桌面与移动壳。
  - `pa-app` lib 化：业务全部移入 `lib.rs`（`#[cfg_attr(mobile, tauri::mobile_entry_point)] pub fn run()`），`main.rs` 变桌面薄壳；`[lib] name = "pa_app_lib"`，`crate-type = ["staticlib","cdylib","rlib"]`。桌面行为不变（`cargo check` 通过）。
  - DB 路径按平台解析：`PA_DB` 始终最高优先；桌面默认仍是 cwd 下 `pa.sqlite`（与 CLI 共享）；移动端无有效 cwd，用平台 app-data 目录。
  - `cargo tauri android init` 生成 `gen/android` Gradle 工程（进版本库，未来通知监听 Kotlin 插件的家）；`cargo tauri icon` 从新画的 1024px 源图派生全套图标（含 Android mipmap，桌面 ico 一并更新）。
  - `cargo tauri android build --debug --target aarch64` 产出可安装的 debug APK；构建环境：本机 SDK + NDK 28、JDK 21、rustup 四个 Android target、tauri-cli 2.11.4、Windows 开发者模式（symlink 权限，见 PITFALLS）。
  - **已在 x86_64 模拟器（Android 37, Medium_Phone AVD）端到端验证**：应用启动、12 视图渲染、示例句 ingest → 事件抽取 → 提前 30m 提醒计划落库，SQLite 在 app-data 目录正常工作。
- **移动端自适应布局 + Android 通知权限**（2026-07-07）——同一份静态前端同时服务桌面与手机。
  - `dist/index.html` 加 `@media (max-width:720px)`：侧边导航变横向滑动标签栏、顶栏紧凑化（隐藏 DB 路径）、消息气泡/表格/弹窗按窄屏收紧；模拟器截图验证输入行不再被裁。顶栏标题去掉「桌面壳」字样（现在它不只是桌面壳）。
  - Android 13+ 通知：Manifest 声明 `POST_NOTIFICATIONS` + 启动时运行时请求（`lib.rs` setup，仅 mobile 编译；拒绝无碍——窗口内角标/列表兜底，F16 精神）。权限弹窗已在模拟器验证；OS 通知代码路径与桌面共用（桌面 toast 已验证），Android 真实提醒显示待自然触发验证。
- **Persona Manager v1（F9 手动风格设定 + F15 版本化回滚，Phase 2）**（2026-07-06）——`pa-core::persona` 新模块 + `persona_versions` 表。
  - 人格 = 可读的风格设定（称呼/语气/口头禅/自由备注），不是黑盒向量；全空的设定会被拒绝。
  - **版本化是 append-only + 活动指针**：每次保存生成 vN+1 并激活；回滚只移动指针、历史全部保留（F15 漂移控制）；`persona-clear` 行使删除权（F12 精神），删除后从 v1 重新计数。
  - 生效面：闲聊回复的系统提示注入 `style_prompt()`（云端按人格口吻回复）；复盘措辞改写（见下条）。聊天记录导入仍属 Phase 3，v1 的 `source` 恒为 `manual`。
  - CLI 新命令：`pa persona`（当前 + 版本历史）、`pa persona-set --nickname/--tone/--catchphrase/--notes/--note`（省略的字段沿用当前值）、`pa persona-rollback <v>`、`pa persona-clear`。
  - 桌面壳：新增「🎭 人格」视图（编辑表单 + 版本历史表 + 回滚/清空），4 个新 command（`persona_get/set/rollback/clear`）。
- **复盘简报云端按人格改写（F14 补全，Phase 2）**（2026-07-06）——`pa-core::llm::rewrite_digest` + `Orchestrator::review_text`，已用真实 MiMo 端到端验证。
  - 数字始终来自离线聚合（`pa-core::review`），云端只负责措辞；**本地校验改写稿必须原样保留所有非零计数**，丢数字/改数字/空回复一律打回，与云端失败一样降级为离线原文（F16 兜底）。
  - 发往云端的内容 = 离线简报文本 + 人格风格设定（均为聚合/用户自撰内容），行为日志与台账仍不出本地。
  - CLI：`pa review` 默认在云端可用时改写（输出注明），`--plain` 强制离线原文；桌面壳复盘页加「☁️ 按人格改写措辞」开关 + 结果徽标（改写成功/降级原因）。

### Changed
- **新 UI 真实壳验证 + Android 安全区兜底修复**（2026-07-15）——重构后的 UI 分别在桌面壳（`npx tauri dev`，WebView2）与 Android 模拟器（x86_64 debug 包，`PA_PREBUILT_JNILIBS` 逃生门链路）实跑截图验证：对话/日程/记忆各视图、底部标签栏、空状态、通知权限首启流程均正常。修复一处真机才暴露的问题：Android WebView `env(safe-area-inset-*)` 恒为 0 导致品牌条被状态栏压住——JS 加 `html.android` 类 + CSS `max(env(...), 24px/16px)` 兜底（详见 PITFALLS 当日条目）。
- **壳层 UI/UX 全面重构（弃旧重建，taste + impeccable 双 skill 驱动）**（2026-07-15）——`dist/index.html` 整体重写（CSS/HTML/JS 全新），Rust 侧零改动：全部 Tauri command、事件（`pa-fired`/`pa-checkin`/`pa-suggestions`/`pa-captured`/`pa-health`/`pa-tick`/`pa-synced`）、F18 渲染语义（目录 7 组件、白名单二次校验、一次性禁用、`skipLeadingText`）原样保留。
  - **视觉身份**：全新「琥珀台灯」体系（种子色 oklch hue 57°）——琥珀=「需要你注意」（主按钮、到点提醒、警示横幅、选中态），底色为纯净中性灰（浅色纯白 / 深色近黑，chroma≈0），彻底替换原靛紫 accent；品牌点「PA·」的圆点即云端在线灯（在线时琥珀呼吸微光）。深浅两套主题逐一校验正文对比 ≥4.5:1。全站 emoji 图标清除，换成 24 网格手绘 SVG 线条图标（stroke 1.75，唯一图标词汇）。战略/视觉上下文沉淀为根目录 `PRODUCT.md` + `DESIGN.md`（impeccable init/document 产物，后续设计任务的输入）。
  - **UX 结构重排**：对话成为真正的主界面——消息流滚动 + 输入框固定底部 + 发送中「正在思考」三点指示；≥1200px 桌面对话页右侧新增「今日面板」（到点提醒/接下来/待处理建议，一眼可扫）；移动端（<900px）从顶部横滚导航改为**底部标签栏**（5 组，44px+ 触控目标，safe-area 适配），到点数角标同时挂在侧栏/标签栏的「日程」上。二级导航从 pill 改为分段控件。
  - **视图级重设计**：日程改为按天分组（今天/明天/后天/日期+星期）的时间轴列表；主动度从 `<select>` 改为三段分档控件（被动/秘书/管家，带每域说明）；穿戴顶部加三项最新数值即览（等宽数字）；复盘输出改为正文排版（68ch 行长）；空状态全部带图标+引导文案；首次进入对话页为管家问候 + 六个示例卡（示例只在空状态出现）。表格保留于台账/审计/提醒等高密度数据（F12 透明性允许高密度），行悬停+发丝分隔线。
  - **工程细节**：DOM 全部经 `el()` helper 以 `createElement`/`textContent` 构建，消灭原先字符串拼 HTML + 内联 `onclick` 转义的注入面；`:focus-visible` 焦点环、`prefers-reduced-motion` 全动效退化、Esc 关模态、语义 z-index 阶梯、`color-scheme: light dark`（原生控件深色适配）、时间戳等宽+`tabular-nums`。模拟时钟移入侧栏页脚（桌面 only 的演示工具，移动端不再占顶栏）。
  - **验证**：mock-IPC harness（scratchpad 注入 `window.__TAURI__` 全 command 假实现）经真实 Chrome 逐视图截图走查：12 视图 × 浅/深主题、390px 移动端（iframe 精确视口）浅/深、对话全链路（事件登记→事件卡→GenUI 按钮点击→一次性禁用→toast）、高危模态、问询条 choice、form 组件四种字段、主动度分档切换、建议采纳/忽略、impeccable 检测器仅余中文破折号误报（英文向启发式），控制台零报错。踩坑见 PITFALLS 当日两条。
- **壳层前端导航精缩为两级 + 视觉风格翻新**（2026-07-14）——`pa-app` 前端仅动 `dist/index.html`，Rust 侧与 12 个视图的功能零改动。
  - **导航**：12 个平铺入口精缩为 5 个一级组——💬 对话、📅 日程（日程/提醒/建议）、📔 记录（行为日志/穿戴/复盘）、🧠 记忆台账、⚙️ 设置（人格/主动度/规则/护栏与审计）；组内多视图时在内容区顶部出二级 pill tab，单视图组不出。每组记住最后停留的视图，跨组切回时恢复。记忆台账保持一级入口不并入设置——F12 是隐私信任基础（架构 §4），入口可见性本身就是承诺的一部分。
  - **风格**：配色收敛为完整 CSS 变量体系（新增 `--ok-soft/--warn-soft/--gray-soft/--on-accent/--panel-2/--toast-bg` 等，原先散落的十六进制硬编码全部收编），并借此支持 **深色模式**（`prefers-color-scheme: dark` 自动切换，含 F18 生成组件、弹窗、toast）；卡片/气泡/弹窗圆角与阴影统一微调。F18 渲染器「外观由前端 CSS 固定、信封不能指定颜色尺寸」的防 UI 注入原则不受影响。
  - 移动端：一级导航仍为顶部横向可滚动行，二级 tab 行同样横向滚动；375px 视口验证无横向溢出。
  - 验证：浏览器端点通导航全部切换路径（组内切换/跨组记忆/单视图组隐藏二级 tab/问询跳转对话 `showView("chat")`）+ 移动端断点，控制台零报错。

### Fixed
- 桌面壳记忆台账里「行为」「建议」两层条目点删除报「未知记忆层」——`pa-app::parse_layer` 漏了 Phase 2 新增的 `behavior`/`suggestion` 两层，前端台账早已展示它们（2026-07-06）。
- **桌面壳升级为常驻宿主：后台 ticker + OS 系统通知 + 新视图**（2026-07-06）——`pa-app` 接 `tauri-plugin-notification`，已真机自动化验证（提醒触发的 Windows toast、问询横幅、建议自动生成、状态回答闭环全部通过）。
  - 常驻 ticker（独立线程，每 60s，**只用系统时钟**——模拟时钟仍只是 UI 演示器）：自动 `fire_due`（到点提醒）→ OS 通知 + `pa-fired` 事件；`checkin_if_due`（按 status_checkins 档位）→ OS 通知 + 对话页问询横幅；`auto_generate_suggestions`（按 life_suggestions 档位）→ OS 通知 + `pa-suggestions` 事件。
  - 新增 5 个 command：`behavior_log` / `checkin_now` / `suggestions` / `suggest_generate` / `suggest_set`；启动时自动加载 `pa-llm.json`/`PA_LLM_*` 挂载云端 Reasoner，顶栏显示「☁️ 云端在线 / 离线模式」徽标（悬停见脱敏摘要）。
  - 前端新增「📔 行为日志」「💡 建议」两视图（建议可采纳/忽略）；对话页问询横幅回一句即完成记录并收起；台账层徽标补 行为/建议 两层。
- **真实云端 Reasoner 落地：`pa-core::llm`（F6，架构 §3.6 Cloud LLM Gateway）**（2026-07-06）——接入 OpenAI 兼容端点（当前用小米 MiMo token-plan，`mimo-v2.5`），已用真实 API 验证。
  - `Reasoner` trait 升级为 `complete(system, user)` 并要求 `Send`；`LlmReasoner`（ureq，30s 超时）实现之；`Orchestrator::set_reasoner` 可插拔，不配置 = 完全离线（F16 不变）。
  - 两个接入点，均最小化上下文（只发当前这一句话 + 当前时刻，不发行为日志/台账/人格）：
    - 闲聊回复：Chat 意图由云端生成 1–3 句回复；云端失败降级为「已记录」。
    - 抽取兜底：规则抽取器解析不出时间时（如「圣诞节前一天」），LLM 输出严格 JSON 再由本地防御性解析（剥代码围栏、校验 kind/时间格式），任何不合格输出降级为普通记录，绝不让 ingest 失败。
  - 凭据加载：env `PA_LLM_BASE_URL/API_KEY/MODEL` 优先，其次 `pa-llm.json`（已入 .gitignore，key 永不入库）；`masked_summary()` 展示时只留 key 尾 4 位。
  - CLI 自动加载配置并挂载 Reasoner；新命令 `pa llm-status`。
- **Suggestion Engine v1（F10，Phase 2）**（2026-07-06）——`pa-core::suggest` 规则引擎 + `suggestions` 表，全离线确定性（LLM 措辞改写留待后续）。
  - 日程规则：考试临近→建议开始复习；截止临近→建议尽早动手；明早（10点前）有会议/课→建议今晚早休；两事件开始时间相差 ≤1h→冲突预警。
  - 习惯检测（F3 雏形）：读取近 14 天行为日志中的状态条目，同一活动在 ≥3 个不同日期、时间聚集在 90 分钟窗内→建议设为固定提醒（如「护肤 07:20」）。
  - 每条建议带 `dedup_key`（存储层 UNIQUE），tick 反复生成不会刷屏；状态 pending/accepted/dismissed 可流转；纳入 F12 台账（`MemoryLayer::Suggestion`）可查可删。
  - 自动生成受 `life_suggestions` 主动度门控：被动=仅手动、秘书=看 1 天、管家=看 3 天。
  - CLI 新命令：`pa suggest [--days N]`（生成并打印新建议）、`pa suggestions`（列表）、`pa suggest-set <id> <status>`。

### Fixed
- 抽取器会议关键词补「晨会/例会/周会」（两处词表同步改，`EVENT_WORDS` 与 `detect_kind`）：此前「明天上午8点半开晨会」被分类为 other，早会建议不触发（2026-07-06）。
- **行为日志 + 定时问询（F3/F4，Phase 2）**（2026-07-06）——`pa-core::journal` 新模块 + `behavior_log` 表。
  - 行为日志自动沉淀三类交互：用户状态回答（`status`，自动剥离「我在/正在」前缀）、agent 主动问询（`checkin_asked`）、提醒触发（`reminder_fired`）；每条带来源（`raw_input#n`/`notification#n`）。
  - 定时问询是纯策略函数：`status_checkins` 主动度档位决定频率（被动=不问、秘书=4h、管家=2h），仅清醒时段 08:00–22:00 问，距上次问询不足一个间隔不重复问；`Orchestrator::checkin_if_due(now)` 一次调用完成判定+落账+出题。
  - 行为日志纳入 F12 记忆台账（新 `MemoryLayer::Behavior`），可查看来源、可删除。
  - CLI 新命令：`pa log`（看行为日志）、`pa checkin`（到点则输出问询问题）；`forget behavior <id>` 可删日志条目。
- **图形前端落地：`pa-app` Tauri 2 桌面壳**（2026-07-06）——补齐 ARCHITECTURE.md §6 Phase 1 的最后一项「图形前端」。
  - Rust 侧 19 个 `#[tauri::command]`，全部是对 `pa_core::Orchestrator` 的薄封装（`Mutex` 共享），无业务逻辑；HITL 护栏的编译期保证原样生效——前端拿不到 `Grant`/令牌，「确认执行」按钮只是触发 Rust 侧 `confirm + run_tool`。
  - 前端为纯静态单文件 `dist/index.html`（vanilla JS/CSS，**无 npm/Node 依赖**），8 个视图：对话（意图徽章 + 事件卡片）、日程、提醒（到点列表 + 全部列表 + 触发/取消）、记忆台账 F12（分层 + 来源 + 删除确认弹窗）、复盘 F14、规则表、主动度 F8（下拉即改即存）、护栏与审计 F7（拦截演示 + 确认弹窗含后果预览 + append-only 审计表）。
  - 顶栏「模拟时钟」（datetime-local + 系统时钟开关）：注入时钟贯穿到 UI，到点/触发流程可确定性演示；顶栏红色角标每 30s 轮询到点提醒数。
  - 库默认 `pa.sqlite`（与 CLI 共享数据），`PA_DB` 环境变量可覆盖。
  - **已用屏幕自动化在真机验证**：会议/考试/高危三类语句抽取、到点触发（pending→fired）、护栏拦截→确认→执行→审计两条记录、台账删除、复盘数字核对，全部通过。
- 附带小改动：`pa-core::guard::Tool` trait 增加 `Send` 约束（Tauri 托管状态跨线程共享 Orchestrator 所需），对现有实现零影响（2026-07-06）。

- 架构讨论文档 [ARCHITECTURE.md](ARCHITECTURE.md)（2026-07-06）
- 强制文档留痕规则：踩坑集/changelog/MISC 三件套（2026-07-06）
- **Phase 1 核心闭环落地（Rust workspace）**（2026-07-06）——实现了 ARCHITECTURE.md §6 Phase 1 的「聊天输入 → 事件抽取 → 日程写入 → 分级提前通知」闭环，全部离线、确定性、可 `cargo test` 头less 验证。共 66 个测试（62 单元 + 4 集成），clippy 零告警。
  - `pa-core` 库 crate，模块：
    - `time_parse` — 中英文自然语言时间解析（今天/明天/后天/大后天、下周X/周X、N天后/N小时后/N分钟后、M月D号、ISO、上午下午晚上/点半/中文数字、`in N hours`、`next monday 3pm` 等），注入 `now` 保持纯函数可测。
    - `extract` — Intent Router（Chat/IngestEvent/StatusAnswer/DangerousCommand）+ `Extractor` trait + 离线 `RuleBasedExtractor`（事件类型/标题/地点/参与人抽取）；预留 `Reasoner`（云端网关）seam。
    - `classify` — Importance Classifier + 可编辑规则表 `RuleTable`，默认值对齐架构（exam 3d、meeting/class 30m、deadline 1d，channel push/banner）。
    - `schedule` — 通知排程（`fire_at = start - lead`）与到点查询 `due`/`next_fire`。
    - `proactivity` — 分维度主动度（schedule_reminders/life_suggestions/status_checkins/weekly_review × passive/secretary/butler），并编码 F7 硬约束 `may_autoexecute`。
    - `guard` — HITL 高危护栏：`Grant` 能力型令牌（只有 Guard 能签发）、一次性 `ExecutionToken`（绑定操作指纹、5 分钟过期、用后即焚）、append-only 审计；高危工具无令牌在代码层面无法执行。
    - `store` — 本地 SQLite（rusqlite bundled）：迁移、事件/通知/原始输入 CRUD、append-only 审计表、配置（规则表/主动度）、F12 记忆台账（分层展示 + 级联删除）。
    - `orchestrator` — 把上述装配成 `ingest()` 闭环，内置演示工具注册表（安全 `echo` + 模拟高危 `demo_delete`，不碰真实文件系统）。
  - `pa-cli` 二进制 `pa` —— 头less 演示 Phase 1 全流程：`add / agenda / due / fire / dismiss / ledger / forget / rules / proactivity[-set] / tools / guard-run / guard-demo / audit / review`，`--now` 注入时钟、`--db` 选库。
  - 通知生命周期补全：`dismiss` 可取消待触发提醒（`pending → dismissed`）而不删除事件，让 `NotificationStatus::Dismissed` 状态可达。
- **自我复盘简报 `review`（F14，Phase 2 的离线可测切片）**（2026-07-06）——`pa-core::review` 纯聚合模块：统计时间窗内「记录输入 N 条 / 新建日程 M 项（按类型）/ 计划与触发的提醒次数 / 高危尝试与拦截次数」，`Digest::render()` 出中文简报；`Orchestrator::review()/weekly_review()` + CLI `pa review [--days N]`。全部离线确定性，未来可由云端按人格语气改写措辞。

### Fixed
- `pa-app` 默认时钟（系统时间）归零秒/纳秒，与 `pa-cli` 行为一致；此前派生事件时间带纳秒尾巴，UI 显示为 `12:53:47.425371200`（2026-07-06）。

### Notes
- 新增 `docs/` 之外的根 `README.md`（上手指南）。

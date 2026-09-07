# Solum Changelog

## 2026-09-07 — 文档精简

- 通用工程与整洁约束统一引用工作区 AGENT.md；项目入口只保留特有边界，合并重复工具说明和逐会话记录。
- 仅文档变更；检查引用、编码和 diff，未打包、未执行设备或生产验证。


本页保留版本级摘要。逐提交实现、测试计数和会话过程由 Git 历史保存。

## [Unreleased]

- 2026-09-07：Android 端与多端共享层架构解耦、硬编码清理与全面国际化体系落地：
  1. **LLM 端点与模型解耦**：移除旧私网调试 IP，默认预设升级为通过 CPA Stream Guard (`http://100.70.173.50:8317/v1`) 连接 `gemini-3.8-flash-high`，解耦前端自定义端点与超时机制；更新工作区配置 `solum-llm.json` 与 `solum-core/src/llm.rs` 默认模型。
  2. **中心域名与模型单点真理**：`account_status_get` 统一导出 `default_server_url` 与 `default_model`，前端 HTML/JS 移除写死默认中心值和模型名；登录、注册、OAuth 流程全面改为动态绑定。
  3. **通知规则领域解耦**：从 `solum-core/src/notification_intelligence.rs` 中剥离微信/QQ/钉钉的硬编码 match 分支，抽离为声明式 `AppStarterRuleSpec` 策略层 `starter_patterns_for_package`，新增策略注册与回退测试。
  4. **Android 原生层真实国际化**：在 `solum-app`、`solum-alarm`、`solum-notif-access` 三大模块建立标准的 `values` 与 `values-en` 资源架构，全面提取并替换 Kotlin 代码中写死的通知渠道名、前台服务常驻通知、语音提示、OCR 错误与福利告警字符串。
  5. **前端严格 CSP 双语系统**：自研轻量、零外部依赖、符合严格 CSP 的 `dist/i18n.js`（含 `data-i18n` 声明式 DOM 翻译、参数化插值与词典投影）；在“设置 → 系统设置”新增中/英文切换控件；`check-frontend.mjs` 扩展脚本语法与 417 个独立元素 ID 检验全绿。
- 2026-09-04：废弃旧域 `oauth.tannerlab.xyz`，将 Solum Center `postgres-server.js` 中 `oauthIssuer` 默认回退值与 contract 测试断言校准为规范域名 `https://auth.tannerlab.cn`。
- 2026-09-03：Solum Center `solum-cloud` 0.8.0 服务端性能与安全发版（本地 node:test 55/55 通过，其中 6 个 Postgres 集成测试在 VPS 一次性容器真库执行）：`/v1/pull` 累计字节窗口改为先在有界候选集（≤500 行）上计尺寸、再对入选行取密文，修复新设备/长期离线设备全量拉取时解包整个租户 blob 历史的 O(全历史) 扫描；备份 commit 的对象校验移入 PostgreSQL（内置 `sha256()`），不再把最多 64×68MB 密文拉到 Node 内逐个哈希；schema 新增 `ai.usage_events` pending 部分索引与 `sync.alerts (tenant_id, seq)` 索引；`/v1/auth/oauth/exchange` 补 per-IP 限流（401 计数、502/403 不计）；修复 `/v1/alerts` 显式 `limit=0` 被静默当作 100 的问题。集成测试权限清单补齐 `auth.oauth_identities` 与 `vault.recovery_codes`（8-29 OAuth 发版后未同步的测试腐化）；PITFALLS 记录 public schema revoke 使 pgcrypto `digest()` 不可解析、须用内置 `sha256()` 的坑。
- 2026-08-29：Solum Center 生产完成 Tg-oauth 统一身份接入发版（`solum-release-0a497cd`）；Postgres 迁移 `auth.oauth_identities` 表；支持 `/v1/auth/oauth/config` 与 `/v1/auth/oauth/exchange` 自动开户与 E2EE 恢复码解耦。
- 2026-08-28：全仓文档按当前 `0.2.8/1018`、local-first 边界和真实待办重构；删除重复会话流水与过期生产断言。

## [0.2.13]

- 移动端会话选择从系统 `<select>` 改为 Solum 自绘列表框；新建对话复用唯一空白会话并清理旧版重复空会话，连续点击不再重复写入存储或重置上下文。
- 资料/组件工作台补齐顶部安全区、紧凑分段导航与窄屏内容网格；移除个人头像上遗留的铅笔/emoji 角标。
- 外置 Chrome 390×844 回归通过：连续点击六次新建后会话总数不增长，抽屉完整占满视口且无横向溢出，控制台无 warning/error。
- 发布版本递增为 `0.2.13/1023`。
- 2026-09-01：生成 `Solum-v0.2.13.apk`；包名 `dev.solum.app`，版本 `0.2.13 (1023)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`，APK Signature Scheme v2 有效；文件 48,341,621 字节，SHA-256 `5CB7DCCA4502CBF50551EF7490B2FA94E89B343BAE9A30E141FE271E966F0D60`。构建与静态发布核对通过；随后在真机 `V2405A` 从 `0.2.12` 保留数据覆盖升级、冷启动与前台 Activity 验证通过，清空日志后无崩溃信号，设备回读 APK 与交付产物 SHA-256 一致。

## [0.2.12]

- 修复从旧版覆盖升级时继承记忆页旧滚动位置和旧子页路由的问题：每个本机 profile 首次进入新版时，将记忆入口一次性归位到列表顶部，避免标题与来源筛选被顶出首屏。
- 发布版本递增为 `0.2.12/1022`。
- 2026-09-01：生成 `Solum-v0.2.12.apk`；包名 `dev.solum.app`，版本 `0.2.12 (1022)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`，APK Signature Scheme v2 有效；文件 48,345,705 字节，SHA-256 `1C81C9ECCBA0A97AEA12390811AA8D2C4E6825C7994967B7FC7069566A8129EB`。
- 外置 Chrome 以旧版 `lastView=captures`、`navLastView.memory=captures`、`scroll.ledger=520` 状态回归，一次迁移后均归位到 `ledger/0`，控制台无 warning/error。真机安装待设备重新连接后单独验证，不以 `0.2.11` 的真机结果替代。

## [0.2.11]

- 用户头像改为本地图片流程：嫩芽成为唯一默认头像；支持 PNG/JPEG/WebP 原图，12 MB 输入上限，居中裁切并压缩为不超过 220 KB 的本机 WebP；删除头像自动恢复嫩芽，移除字符、emoji 和主题色头像功能。
- 修复登录态过期后停留在伪登录界面：统一识别 IPC 鉴权错误与账号状态刷新错误，先清除本机会话并打开登录门；Android 重启后仍保留一次性重新登录提示。
- 重构记忆主入口为内容优先列表：首屏直接展示两行摘要、来源和时间；长正文、类型、编辑与删除进入独立详情页。Soulous 不再占用记忆页说明位，仅按真实来源标注。
- 导出、导入和加密备份迁入“设置 → 数据与备份”；行为足迹、健康趋势、复盘简报和通知回看也从记忆主导航收拢到设置。
- 记忆页不再重复加载 Soulous 配置和通知智能状态，通知回看独立按需刷新；外置 Chrome 390×844 / 1440×900 mock-IPC 回归通过，长通知两行截断、头像压缩/删除回退和会话过期单次退出均通过。
- 发布版本递增为 `0.2.11/1021`。

## [0.2.10]

- 2026-09-01：移除 Android WebView 在按钮点击时绘制的方形蓝色触摸高亮；移动端账户头像改为圆形淡琥珀按压反馈，同时保留键盘 `:focus-visible` 焦点轮廓。
- 发布版本递增为 `0.2.10/1020`。
- 2026-09-01：发布 `Solum-v0.2.10.apk`；包名 `dev.solum.app`，版本 `0.2.10 (1020)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`，APK Signature Scheme v2 有效；文件 44,510,769 字节，SHA-256 `5EA8E5E0C00C8CEFA2DF2F3C392BEA0C188BCB66F34F3FC44467ADC73D23789D`。
- 2026-09-01：真机 `V2405A` 覆盖安装、启动与头像按住态验证通过；按压期间未再出现方形蓝色高亮，设备回读 APK 与交付产物 SHA-256 一致。

## [0.2.9]

- 2026-08-31：发布 `Solum-v0.2.9.apk`；包名 `dev.solum.app`，版本 `0.2.9 (1019)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`，APK Signature Scheme v2 有效；文件 44,499,381 字节，SHA-256 `87250D97A49F7E012FC02DFB1225957651155D935F834298637FC7420A26F002`。
- 2026-08-31：真机 `V2405A` 安装、启动通过；设备回读 APK 与交付产物 SHA-256 一致。该结论仅代表本次连接设备验证，不替代其他机型回归。
- 重构 Android 登录与对话首页：修复窄屏账号门双列覆盖，建立展示/正文/控件字体层级，收敛中文行宽与换行，将七个示例卡精简为三条开放式快捷入口，并为 900–1099px 小平板收起会话侧栏。
- 收敛前端刷新：启动只加载壳层和当前视图，对话后不再全量刷新隐藏页；视图、全量与周期刷新增加合并/后台暂停，修复搜索重复渲染和组件详情外部点击监听累积。
- 延续 Solum 品牌印记和 Android 统一返回栈，发布版本递增为 `0.2.9/1019`。
- 2026-08-31：Android 静态 UI 按“纸墨 × 琥珀台灯”体系补齐内容组件与三档响应式布局，统一复用 Solum 品牌印记；导航拆为可读管线，并让视图、确认框、说明/资料弹窗、抽屉和移动工具面板共享 Android 返回栈语义。

## [0.2.8]

- Tg-oauth 2.0 集成与端到端恢复信封解耦。
- Agent 工具调用适配、通知默认策略与密码规则统一。
- 中心面板更新为纸墨视觉；Solum 品牌与服务入口收敛。

## [0.2.7]

- 完善 Android 本机核心闭环、精确提醒、通知/健康桥接和生产服务联调。
- 增加加密同步、账号隔离、福利告警与设备 QA 证据。

## [0.2.0–0.2.6]

- 建立 Tauri Android 应用、SQLite 本机数据、Guard、提醒、记忆、计划、搜索、账号和加密同步。
- 逐步加入通知监听、Health Connect、后台 alarm、服务端 AI 与恢复能力。

## [0.1.x]

- 建立 Rust workspace、CLI、核心 schema/ingest、行为日志、建议和初版服务端。

历史细节请查仓库 tag/提交；设备与生产状态不能由旧 Changelog 推断。

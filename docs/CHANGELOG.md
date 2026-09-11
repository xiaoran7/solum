# Solum Changelog

## 2026-09-09 — 文档治理

- 校准 README 的 `0.2.15/1025` manifest 版本；复核仓库边界、运行/验证入口与文档导航。
- 从 TODO 移除已由 0.2.15 构建、签名与静态 manifest 核验完成的 Release 重建项，保留真机联网、通知、升级和跨平台验收。
- 仅文档变更；检查链接、UTF-8 BOM、尾随空白和 diff，未打包、未执行设备或生产验证。

本页保留版本级摘要。逐提交实现、测试计数和会话过程由 Git 历史保存。

## [Unreleased]

## [0.2.16]

- 接入 CPA 网关与云端鉴权门禁深度加固：
  1. **模型与上游全面对齐 CPA**：将 Rust Core（`account.rs`）与 Node 服务端（`postgres-server.js`、`server.js`）的默认模型全面切换为 `gemini-3.8-flash-high`，服务端 upstream 默认指向 CPA 网关（`https://cpa.tannerlab.cn/v1`）。
  2. **未登录明确报错拦截**：在 `orchestrator.rs` 中强化云端调用鉴权判定，未登录状态下调用云端对话推理显式拦截报错（`"使用云端智能能力请先登录 Solum 账号"`），拒绝静默伪装成功。
  3. **锁定客户端手动配置**：界面隐藏自定义模型表单与直接配置面板，杜绝用户手动改动；同时完整保留底层自定义配置文件解析与测试 IPC 能力。
- 对齐 AndroidBridge 升级 TannerLab 通行证认证界面与交互：
  1. **主卡片视觉升维**：设置中心将「TannerLab 统一通行证认证」升为首要卡片，独立醒目呈现，传统账号密码输入框默认折叠弱化。
  2. **消除手动粘贴弹窗**：对齐 AndroidBridge（DroidRunner）原生体验，去除强制弹出的授权码输入模态框；通过 Deep Link（`solum://oauth/callback`）自动监听唤醒，并在后台自动换取令牌完成登录。超时或异常时方平滑提供 fallback。
- 发布版本递增为 `0.2.16/1026`。
- 2026-09-11：生成 `Solum-v0.2.16.apk`；包名 `dev.solum.app`，版本 `0.2.16 (1026)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`；AlarmReceiver、BootReceiver、SolumNotificationListener 均存在，APK Signature Scheme v2 有效。文件 46,575,697 字节，SHA-256 `93F691E0600E153D796DF09EA464ED5F50F365F3F560E61BA46952C99FB6B110`，签名证书 SHA-256 `F3467B14C3CA39A88D74B85F10800565D0A81BF67FCF392147EC6B4F6AEFCA87`。构建与静态发布核对全部 PASS；按铁律清理旧版本包并归档至 `Deliverables/Solum-v0.2.16.apk`。

## [0.2.15]

- 客户端安全性彻底收敛与架构归正：
  1. **敏感凭据与内部网络隔离**：彻底清除客户端代码中可能泄露的任何私有 API Key 与内网 IP，重申所有智能能力必须且仅能通过 Solum Center 鉴权代理（`https://tannerlab.cn`）安全上游转发。
  2. **发布核验套件升级**：适配 Android SDK Build-Tools 37+ 输出格式（支持 `V2 Signer:` 证书匹配），静态发布核对脚本自动化全通过。
  3. **版本号工程化递增**：递增版本号至 `0.2.15 (1025)`，打包签名纯净 Universal Release APK。
- 2026-09-08：生成 `Solum-v0.2.15.apk`；包名 `dev.solum.app`，版本 `0.2.15 (1025)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`；AlarmReceiver、BootReceiver、SolumNotificationListener 均存在，APK Signature Scheme v2 有效。文件 46,570,325 字节，SHA-256 `ABCBE91CFA23C8F3D1CBC389EBAE2BA63DA7C0B3BB60A99594611AA47267955D`，签名证书 SHA-256 `F3467B14C3CA39A88D74B85F10800565D0A81BF67FCF392147EC6B4F6AEFCA87`。构建与静态发布核对全部 PASS；按铁律清理旧版本包并归档至 `Deliverables/Solum-v0.2.15.apk`。

## [0.2.14]

- 架构解耦与硬编码清理：
  1. **中心域名与模型单点真理**：`account_status_get` IPC 动态提供 `default_server_url` 与 `default_model`，彻底消除前端 HTML/JS 写死默认值与字符串硬编码。
  2. **通知规则领域解耦**：剥离 `notification_intelligence.rs` 中微信/QQ/钉钉的硬编码分支，抽离为独立声明式策略层 `AppStarterRuleSpec`。
- 全面国际化与前端体验深度治理：
  1. **双端真实国际化**：Android 原生补充 `values` / `values-en` 双语 `strings.xml`，Kotlin 代码中所有通知渠道名、前台服务常驻标题、语音与 OCR 提示全面资源化；前端自研轻量、严苛 CSP 兼容双语系统 `dist/i18n.js`，设置中心提供即时中/英文切换。
  2. **昵称与身份去特定化**：移除个人资料与侧边栏中硬编码的“唐棠”与“本地主人”字面量，支持中英文访客/用户名动态回退与规范占位符。
  3. **深浅色主题真生效**：重构 CSS 变量体系为属性驱动（`:root[data-theme="dark|light"]`），解决仅设置 `colorScheme` 无法触发深色模式样式表的问题。
  4. **登录失效时序重构**：401 发生时由遮罩拦截并优雅注销，启动期第一帧优先全屏锁定并鲜明提示“登录状态已过期失效”，杜绝跳屏与反复弹窗。
- 发布版本递增为 `0.2.14/1024`。

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

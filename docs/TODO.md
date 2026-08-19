# TODO 待办

> 状态约定：`待验证` 尚未在目标环境跑通；`已通过` 已有可复查证据；`已复现` 已稳定观察到缺陷；`受阻` 受设备、权限或外部环境限制。Android 真机测试默认使用已安装的生产版客户端与 `https://solumsync.tannerlab.xyz`，不得卸载应用、清空数据或删除用户内容。

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
- [x] **已通过：生产服务与真机浏览器 HTTPS 对照。** 真机 Chrome 打开 `https://solumsync.tannerlab.xyz/v1/health` 返回 `status=ok`、`0.6.4 / 11c1e246` 且显示安全连接；Solum Rust 同机请求 `/v1/pull` 仍报 TLS `unexpected end of file`，因此同步失败更像客户端 TLS/VPN fake-IP 适配问题，不是服务端宕机。
- [x] **已通过：通知监听授权状态。** `enabled_notification_listeners` 与 `dumpsys notification` 均包含 `dev.solum.app.SolumNotificationListener`；当前通知白名单仍为空，应用页正确显示“还没有已捕获通知”。
- [x] **已通过：Health Connect 权限页可达，未授权真实数据。** “穿戴 → 去授权”打开系统 Health Connect 权限页，列出步数、睡眠、心率；按隐私边界未点击“允许”，所以真实健康读取仍未验证。
- [ ] **受阻：通知监听端到端捕获。** 权限已存在，但当前应用白名单为空；为了不改变用户隐私设置，未擅自加入 QQ/其他应用，也未读取个人通知正文。

### 已有验证基线

- [x] **已通过：模拟器本地核心闭环。** `0.2.7 (1017)` x86_64 Release 已验证隐私门、同意后通知权限请求、guest、英文事件摄取、30 分钟提醒、精确 AlarmManager 注册、进程重启后的事件/alarm 保留、计划/记忆/搜索/设置入口和 Guard 返回键取消。
- [x] **已通过：生产服务主机侧最小链路。** `https://solumsync.tannerlab.xyz` 的注册、登录、模型目录、用量、AI completion、pull、alerts 与测试会话 logout 均返回成功；该结果只证明服务端链路，不替代 Android 真机验证。
- [x] **已通过：真机安装状态。** vivo V2405A（Android 16 / API 36）当前安装 `dev.solum.app` `0.2.6 (1016)`，通知、网络、精确 alarm、开机恢复、前台 data sync 与 wake lock 权限已授予；应用数据保持原样。
- [ ] **受阻：干净 target 的 workspace 测试基线。** 仓库迁移后旧 Tauri 缓存仍引用 `D:\ClaudeSpace\Project\Desktop_export_2026-07-13\PA\target`，默认 `cargo test --workspace` 在执行测试前失败；需用独立 `CARGO_TARGET_DIR` 完成一次无旧路径缓存的基线。
- [x] **已通过：真机 QA 事件入库与 alarm 注册证据。** “今天晚上十点十分开会，QA真机提醒测试2” 写入事件 `2026-08-19 22:10`，生成 `21:40` 提前 30 分钟提醒；系统显示 exact `RTC_WAKEUP`、`exactAllowReason=policy_permission`。到点未产生通知，原因见上方生产 APK receiver 缺失。
- [ ] **受阻：完整后台回收提醒。** `am kill dev.solum.app` 未终止进程，因为通知监听 `dataSync` 前台服务仍在运行；不能用 force-stop 代替（会由系统取消 alarm）。需后续提供不改用户配置的进程级故障注入/构建版测试入口。

### 证据

- 审计记录：`D:\ClaudeSpace\_scratch\solum-android-prod-audit-20260819.md`
- 截图：`D:\ClaudeSpace\_scratch\solum-027-*.png`
- 当前代码基线：`088f184`（`main`，本地分支；生产服务版本需以实时只读检查为准）

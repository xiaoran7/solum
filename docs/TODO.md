# TODO 待办

> 状态约定：`待验证` 尚未在目标环境跑通；`已通过` 已有可复查证据；`已复现` 已稳定观察到缺陷；`受阻` 受设备、权限或外部环境限制。Android 真机测试默认使用已安装的生产版客户端与 `https://solumsync.tannerlab.xyz`，不得卸载应用、清空数据或删除用户内容。

## 2026-08-21 全仓优化审计行动清单

> 2026-08-21 会话对全仓做了七维度并行审计（Rust 核心 / 前端 / Node 云服务 / 同步加密 / Android / CI 发布 / 文档流程），每条发现经独立核查员对照代码与已拍板红线复核（证据不实杀掉、打分不当修正），同步维度由会话逐条抽查源码确认。以下为收敛后的行动清单，均未开工（标注 ✅ 的为当批已完成部分）；完整证据行号见当次会话记录。

### P0 立即项

- [ ] **已复现/根因明确：真机同步 TLS `unexpected EOF`（2026-08-21 现场探针定案）。** 不是本地 VPN fake-IP：域名解析阿里云北京 8.148.11.79，服务器侧按 **SNI + TLS 指纹**选择性掐断——rustls 无 GREASE 的 ClientHello 被 RST，浏览器级指纹放行，换网络/走代理均无效（80 端口有阿里云 Beaver 未备案拦截页旁证）。客户端唯一能做的已做：HttpTransport 错误映射按 DNS/连接失败/**TLS 握手失败**/超时分类为人话（ureq 2.12 中该故障固定表现为 `ConnectionFailed` + "tls connection init failed"）。**剩余动作在服务器侧、需用户拍板**：① 完成 tannerlab.xyz ICP 备案；② 同步端点迁出阿里云大陆合规范围（香港/国际节点、已备案域名或前置 CDN）；③ 先查阿里云控制台合规通知。完整证据链见 PITFALLS 当日条目。
- [x] **已做：push 侧毒丸防护（2026-08-21）。** 永久拒绝（507/403/429/400/413）只作废 push 半轮、继续 pull，原因持久化 `meta.sync_push_blocked`（下次成功推送自动清除）；单条超 7 MiB 的 op 暂存 `sync_push_holds`（只存 oplog 指针），游标照常前进；瞬时失败同样只跳过 push 半轮。设计取舍见 MISC 2026-08-21，ARCHITECTURE §3.8 已补条目。**遗留**：pull 侧 403（设备撤销同样阻断 pull）是否也要跳过半轮+持久标记，见下方新增条目。
- [x] **已做：SQLite 双连接补 `busy_timeout=5000ms`（2026-08-21）。** `Store::open` 一处覆盖 orch 与 sync_store；PRAGMA 断言测试钉住。CoreError::Storage 抹掉 rusqlite 错误码仍是小项（见小项打包）。
- [x] **已处理：公开仓泄露本机路径（本地侧）。** 根 README 两处 `D:\ClaudeSpace\...` 绝对路径已改为相对描述，tauri command 计数改为「100+」。**遗留**：origin/main 落后本地 17 提交，未推内容含生产账号名/真机型号/内网 URL；remote 政策已补进 AGENTS.md（2026-08-21，gitea 权威仓、推 origin 前过公开边界清单）；**16 个提交的 diff 级审计仍未做**，做完再定推送节奏（推送需用户当次授权）。
- [x] **已做（仓库侧）：CI 补到真实开发路径（2026-08-21）。** 新增 `.gitea/workflows/ci.yml`（rust 三门含 Tauri Linux 依赖 + web-and-cloud 挂 postgres service，集成测试防静默 skip 有显式断言）；`.github/workflows/ci.yml` 加 android-manifest 门禁（gradle 仅跑到 manifest 合并 + aapt 断言三组件）并升级 web-and-cloud；配套 `scripts/gen-android-ci-settings.mjs` 解决 `tauri.settings.gradle` 为 gitignore 绝对路径生成文件、裸 checkout 无法跑 gradle 的问题（已验证与 tauri CLI 产物一致）。**边界**：Gitea 侧需服务器安装并注册 act_runner（ubuntu-latest 标签、docker 模式）才实际生效；Android job 依赖 GitHub 托管 runner 预装 SDK，自建 runner 需先补 sdkmanager 步骤。

### 结构性（排期）

- [ ] **store.rs（5996 行）/ orchestrator.rs（5736 行）拆分**：占 solum-core 近半，分节注释完整即天然拆分线；Rust 多 impl 块机制下零 API 破坏，测试耦合已验证极低。每步独立提交三绿再进下一步；拆到哪节补哪节测试（email/soulous/notification_intelligence/store-sync 段测试密度与风险不成比例）。
- [ ] **server 双实现收敛**：postgres-server.js 与 server.js 约 500 行复制粘贴，安全相关漂移已双向发生（waitForDrain 断连检查只有 sqlite 版有、parseAllowedOrigins 行为不同、AI 代理语义分叉最大）。照 http-security.js 先例抽 `src/http-core.js` + `src/ai-proxy.js`（quota-store 接口参数化），纯 Node 零新依赖，加契约测试钉住两入口同源。
- [ ] **发布核对脚本化**：`scripts/verify-android-release.mjs`（aapt badging / ZIP 条目 / apksigner / manifest 三组件断言，末尾输出 CHANGELOG 可粘贴证据块）+ 版本三元组一致性检查（tauri.conf.json version / versionCode / Cargo.toml workspace version）+ Android 启动时 AlarmReceiver 自检绊线（PackageManager.getReceiverInfo，缺失时设置页显式告警）。0.2.6 生产 P1 正是从手工核对网缝隙漏出的。
- [ ] **sync_oplog 保守压实设计**：本地 oplog 永不修剪（health_samples 按分钟写入，一年数十万行），但 oplog 同时是 LWW 比较基线不能裸删。方案：每 (tbl,guid) 仅留 hlc 最新一条、只处理早于留存期且已被推送游标越过的行、delete op 墓碑加倍保留、压实后把最大 (hlc,origin) 写入 meta 作比较基线；先 CLI `--dry-run` 报告可回收行数。动手前先在 MISC 记录取舍。
- [ ] **中继清扫定时化 + gap 水位线**：Rust 中继留存清理只在启动时跑一次（常驻进程形同虚设），照 Node 侧 setInterval 模式改定时执行并在 /v1/stats 暴露 last_sweep_at；history_gap 判定改用 swept_through 水位线收紧（现按全局 seq 判定有多租户误报边角，误报代价是粘性警告诱导用户全量重灌）。
- [ ] **memory_ledger 分页**：全表加载无 LIMIT，作为 F12 日常界面在长寿命设备上内存与 JSON 载荷线性无界；加 (layer, limit, cursor) 按层懒加载，导出继续走 export.rs 独立路径。
- [ ] **CSP 去 unsafe-inline 收尾**（MISC 2026-07-21 明记「别忘了」）：tauri.conf.json script-src 仍带 unsafe-inline，245KB 内联 script 未拆（此后 17 个提交文件又涨 45%）。拆 dist/app.js 时 check-frontend.mjs 必须同步改为校验 app.js——否则 `<script src>` 被捕获为空内容、vm.Script("") 静默通过，静态检查空转。
- [ ] **云服务小加固打包**：/v1/auth/refresh|logout 补 IP 限流（认证面唯一没节流的未鉴权入口，垃圾 token 也打满一次 DB 事务 + advisory lock）；ai.usage_events 增加保留期清理（全库唯一无清理表）；compose 增加 pg_dump 备份编排（PG 卷是密文历史与恢复信封的唯一云端副本，目前零自动备份）+ 恢复演练一次记入 PITFALLS；recovery envelope 与 /v1/devices 补运行时集成测试（现仅 contract 正则断言，create-only 语义测不出来）。
- [x] **已完成：文档检索层**（本批）：PITFALLS/MISC 尾部时序错乱修正 + 头部主题索引与插入规则；CHANGELOG 版本速查索引；docs/README.md 导航升级；ARCHITECTURE 状态句与 §6 标题更正。详见 CHANGELOG 当日条目。

### 小项打包

- [ ] **push 毒丸后续：pull 侧 403 处理。** 设备撤销同样阻断 pull（ARCHITECTURE §3.8）；现在 pull 收到 403 仍以显式错误结束整轮（用户可见）。是否也要「跳过半轮 + 持久标记」待议——注意撤销场景下 pull 失败是**必须显眼**的，不能照抄 push 侧的自愈语义。
- [ ] 三处多步写漏包 `with_transaction`（persist_event_with_scope / fire_due / set_importance_rule；方法已支持嵌套 savepoint，机械缩进调整 + 「insert_notification 故障时事件回滚」回归测试）。
- [ ] CoreError::Storage 保留 rusqlite 错误码（BUSY/LOCKED/CONSTRAINT 现被 to_string 抹掉）；row mapper 对损坏 JSON 不再 `unwrap_or_default()` 静默吞（至少 eprintln 带 id 或可见计数）。
- [ ] push 装箱循环 O(n²) 重复加密改估算式装箱（每加一条 op 就整批重新序列化+ChaCha 加密；积压越大浪费越多，恰是离线回连场景，手机 CPU 上可感知）。
- [ ] 根目录约 35MB 一次性产物（output/ 旧 APK、演示视频、playwright 日志）移仓库外归档目录。**注意：根目录 `solum.sqlite*` 是「桌面与 CLI 共库」的刻意设计（MISC 有记录），勿清。**
- [ ] 通知渠道 ID `pa_notification_pipeline` 改名残留：与 2026-07-20 改名决议矛盾（同批改了 pa_reminders 渠道），改名（带 deleteChannel 迁移）或补进刻意保留清单，二选一显式拍板。
- [ ] TODO 受阻项代码侧推进：debug-only 进程自杀入口（前台服务随进程死、alarm 由系统持有，正好复现「后台回收后 alarm 存活」场景，替代被禁用的 force-stop）；福利告警测试无需新代码——POST `/v1/alerts` status 含 "test" 即可触发客户端「福利监控测试」通知，把配方写死即可从受阻转待验证。
- [ ] CHANGELOG 全史切版（161 条全部堆在唯一 `[Unreleased]` 下、单条最长 2242 字符）：需逐条定版本归属，属专门批次；本轮以头部版本速查索引缓解。

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
- [x] **已通过：生产服务与真机浏览器 HTTPS 对照。** 真机 Chrome 打开 `https://solumsync.tannerlab.xyz/v1/health` 返回 `status=ok`、`0.6.4 / 11c1e246` 且显示安全连接；Solum Rust 同机请求 `/v1/pull` 仍报 TLS `unexpected end of file`，因此同步失败更像客户端 TLS/VPN fake-IP 适配问题，不是服务端宕机。
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

- 审计记录：`D:\ClaudeSpace\_scratch\solum-android-prod-audit-20260819.md`
- 截图：`D:\ClaudeSpace\_scratch\solum-027-*.png`
- 当前代码基线：`088f184`（`main`，本地分支；生产服务版本需以实时只读检查为准）

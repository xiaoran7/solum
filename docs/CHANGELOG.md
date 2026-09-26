# Solum Changelog

## 2026-09-26 — solum-core 的 backup_download.rs 拆为分节模块

- `crates/solum-core/src/backup_download.rs`（1269 行）按职责切成 `backup_download/` 下 5 个模块：`http`(300，生产 ureq 客户端实现三个云端 trait) / `api`(178，模块入口：列举、删除、下载并暂存) / `stage`(129，下载后的对象还原与校验) / `types`(97，目录/删除请求/暂存结果形状与三个云端 trait) / `error`(61，不外泄的错误枚举)，`tests`(471)，`mod.rs` 保留原文档头、imports、全部常量、`DownloadResult` 别名与 4 组对外转出。纯移动，无逻辑改动。
- 8 个 `#[test]` 全部保留；全文件函数名集合与拆分前完全一致。
- 等价性硬证据：7 个分节逐一按 token 流与拆分前 `backup_download.rs` 比对，全部一致；只允许 14 处可见性放开、`use` 归位与 rustfmt 换行。`mod.rs` 不参与 token 比对，改为断言原文件 14 条 import 一条不缺且顺序不变。等价性脚本经反向验证（删一条语句、改一个测试函数名，均被拒；原树仍通过）。
- 可见性：11 个对外项拆分前就是 `pub`，re-export 不能比被转出的项更宽，故仍为 `pub`；8 个校验与 IO 助手为 `pub(super)`；`HttpBackupCloudReader::request_without_transport_retry` 原本私有但测试套件在用，改为 `pub(super)`。
- **测试套件的 `use super::*;` 不能由面包屑重复添加**：拆壳出来的 `tests/mod.rs` 原本就带着自己那一条（位置在 std/serde import 之后），再补一条就成了重复导入。`wire-sections.mjs` 现在先看正文里有没有，没有才补——保留原有那一条的位置，等价性核对才不会被"import 顺序被改"这种假差异淹没。
- 工具上又一处真实缺陷：`wire-sections.mjs` 里 `new RegExp(\`^(\\s+)...\`)` 写成了 `^(\s+)`，模板字面量里单反斜杠被字面量自身吃掉，到 RegExp 只剩 `s+`（字面的"一个或多个 s"），于是报"找不到 `request_without_transport_retry` 的声明"——而它就在文件里第 36 行。**报错信息现在会同时打印命中的正则和提及该名字的行号**，否则这种"锚点写错"与"内容不在"看起来一模一样。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；**暂存区单独导出后 `cargo test -p solum-core --lib` 441 通过**。

## 2026-09-26 — solum-core 的 portable_generation.rs 拆为分节模块

- `crates/solum-core/src/portable_generation.rs`（1494 行）按职责切成 `portable_generation/` 下 6 个模块：`handoff`(360，发布密封代次，以及跨发布存活的通知交接) / `verify`(251，回读已暂存内容，兼几个共用的 IO 助手) / `stage`(240，暂存 SQLite、WebView 文档、通知 spool 与原生策略) / `tests`(464) / `types`(59，密封句柄、bootstrap 与文档分块形状) / `reader`(77，按块读取密封代次的 bootstrap 与文档)，`mod.rs`(55) 保留原文档头、imports、全部常量与 5 组对外转出。纯移动，无逻辑改动。
- 8 个 `#[test]` 全部保留；全文件函数名集合与拆分前完全一致。
- 等价性硬证据：7 个分节逐一按 token 流（标识符/字符串/字符/裸串 + 逐字符标点）与拆分前 `portable_generation.rs` 比对，全部一致；只允许 22 处可见性放开、`use` 归位与 rustfmt 换行。`mod.rs` 不参与 token 比对（条目都在分节里），改为断言原文件 13 条 import 一条不缺且顺序不变。
- **切分边界本身这次差点出事，值得记一笔**：`stage`/`verify` 的边界最初定在 782 行，而 782 其实是 `verify_documents` 的函数体**内部**——`fn validate_bootstrap` 的签名与文档注释留在了上一节。**分片按字节拼回原文件仍然完全一致**，所以"逐字节可重组"这条证据看不出任何问题；是后面的接线步骤报"找不到该声明"才暴露，而且只是碰巧。`split-into-module.mjs` 因此补了边界守卫，且**断言的是新分节的首行是不是顶层项**（`#[]`、文档注释，或列 0 的关键字），不是旧分节的末行——被切断的条目尾部往往正好是一个列 0 的 `}` 或 `;`，看上去和干净的结尾一模一样，后面再跟一个空行时"回退跳过空行"就会停在切口上。守卫经反向验证：缺口、重叠、起始落在条目内部三种都被拒并报出切口两侧行号，合法的紧贴 `}` 边界则放行。
- 可见性 22 处：`WebViewBootstrap`/`PortableDocumentChunk`/`PortableGenerationReader`/`publish_portable_generation`/`resume_portable_handoff`/`stage_portable_generation` 拆分前就是 `pub`，转出后仍须 `pub`（re-export 不能比它转出的项更宽）；`SpoolRecord`/`ExpectedSpool`/`SealedPortableGeneration`/`BootstrapDocument` 同样拆分前是 `pub`（`backup_download.rs` 在用）；其余 12 个 IO 与校验助手为 `pub(super)`，消费方显式 `use super::<分节>::...`。9 个结构体字段（`SealedPortableGeneration` 3 个、`SpoolRecord` 4 个、`ExpectedSpool` 2 个）由 `widen-fields.mjs` 依编译器诊断放开。
- **跨分节的 `use` 由编译器驱动，不要手抄**：`use super::*;` 已经把父模块的 re-export 带进来了，手工把这些名字再 import 一遍就是 E0252（`stage`/`handoff` 一开始各撞了 8 个和 5 个）。`fix-section-imports.mjs` 改为读 `cargo check` 的 E0425/E0422 反推每个名字归属哪一节，再补 `use super::<分节>::...`，循环到干净为止；父模块已可见的名字自动跳过。
- 工具修正两处真实缺陷：`widen-fields.mjs` 的私有字段正则只认 E0616（`field \`f\` of struct \`S\` is private`），本工具链报的是 E0451 复数形式（`fields \`a\`, \`b\` and \`c\` of struct \`S\` are private`），而复数形式的字段列表带"and"，单个 `[^`]*` 跨不过去——脚本于是打印"没有 E0616 字段隐私错误"并退出，读起来像"没什么可放开的"；正则已按实测诊断修正并对两种形式各自留了样例断言。同文件的文件查找是平铺 `readdirSync`，分节落成 `*/mod.rs` 后一个都找不到，同样静默失败，现改为递归。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫通过；等价性脚本经反向验证（改语句、改测试函数名、丢语句三种变异均被拒，原树仍通过）；**暂存区单独导出后 `cargo test -p solum-core --lib` 441 通过**。
- 未解决（既有，与本次改动无关）：`profile.rs` 的 `tauri::Manager` 与 `commands/privacy_capture.rs` 的 `chrono::TimeZone` 为未使用导入，workspace Clippy 报红，均在本会话之外的改动里。

## 2026-09-26 — solum-core 的 backup_upload.rs 拆为分节模块

- `crates/solum-core/src/backup_upload.rs`（1597 行）按职责切成 `backup_upload/` 下 6 个模块：`session`(378，协调者：设备代次权限 + 已暂存会话的规划/上传/提交) / `http`(282，生产 HTTP 适配器及其私有线上结构) / `staging`(181，本地封存与暂存：密文落盘、对象 id 派生、共享校验器) / `types`(124，线上与内存形状：设备权限、对象计划、冻结会话、传输接口) / `error`(51，不外泄的错误枚举) / `tests`(546)，`mod.rs` 保留原文档头、imports 与常量。纯移动，无逻辑改动。
- 9 个 `#[test]` 全部保留，函数名逐一比对完全一致（拆分前后 diff 为空）。
- 等价性硬证据：7 个分节逐一按 token 流与拆分前 `backup_upload.rs` 比对，全部一致；只允许可见性放开、`use` 归位与 rustfmt 换行。`mod.rs` 那一段不参与 token 比对（条目都在分节里），改为断言原文件 13 条 import 一条不缺（`Deserialize`/`Serialize`/`thiserror::Error` 三项**有意下放到唯一用它们的分节**，脚本需显式声明才放行）。脚本经反向验证：塞多余语句、改测试函数名，均报错退出。
- 可见性：14 个跨分节引用的私有项放开为 `pub(super)`（`staging` 的 11 个校验/暂存助手 + `types` 的 3 个内部形状），消费方显式 `use super::<分节>::{...};`。**结构体字段要单独放开**——`BackupDeviceState`/`BackupUploadSession`/`StagedObject` 的字段被 `session.rs` 逐字段读取，只放开结构体本身会 E0616。
- 工具上的一处教训：**不要静态预测哪些字段需要放开**。私有字段名会出现在别的分节的字符串、注释、无关结构体里，文本扫描不是过度放开就是漏放。改为读 `cargo check` 的 E0616 诊断、只放开编译器点名的结构体，一轮收敛。`widen-fields.mjs` 记录了这个做法。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫通过；暂存区单独导出后 `cargo test -p solum-core --lib` 441 通过。
- 未解决（既有，与本次改动无关）：`profile.rs` 的 `tauri::Manager` 与 `commands/privacy_capture.rs` 的 `chrono::TimeZone` 为未使用导入，workspace Clippy 报红，均在本会话之外的改动里。

## 2026-09-26 — store/tests.rs 按关注点拆为 15 个分节

- `crates/solum-core/src/store/tests.rs`（1465 行）按被测关注点切为 `store/tests/` 下 15 个文件，与 `store/` 的源码分节一一对应：`widget_sync`(433) / `quarantine`(159) / `open_adopt`(119) / `crud`(114) / `legacy_capture`(107) / `migrations`(169) / `ledger_pages`(87) / `errors`(86) / `audit_config`(84) / `sync_core`(80) / `events`(13) / `ledger`(31) / `blobs`(27) / `health`(25) / `fixtures`(16，共享 `scratch` 夹具)，`mod.rs` 保留原文档头与归位的 import。文件内部保持原顺序，只做移动。
- 38 个 `#[test]` 全部保留，测试函数名逐一比对完全一致（拆分前后 diff 为空）。
- 跨文件共享物：原单文件顶部的 `use super::*;` / `Decision` / `EventKind, RiskLevel` / `NaiveDate` 提到 `tests/mod.rs`，因为兄弟文件只能经 `use super::*;` 触达；8 个共享夹具（`scratch`/`dt`/`sample_event`/`course_draft`/`push_all`/`optional_field`/`park`/`event_payload`）随其首个使用场景留在对应分节，消费方显式 `use super::<分节>::夹具;`。
- 等价性硬证据：15 个分节逐一按 token 流（标识符/字符串/字符/裸串 + 逐字符标点）与拆分前 `tests.rs` 比对，全部一致；只允许可见性放开、`use` 归位与 rustfmt 换行。`mod.rs` 那一段不参与 token 比对（条目都在分节里），改为断言 4 条 import 逐字存在且顺序不变。
- 可见性注意：这 8 个夹具**拆分前就是 `pub(crate)`**，只是原先在同一模块内用不到那么宽；拆到兄弟模块后才降为 `pub(super)`。等价性核对脚本因此不能只"删掉 `pub(...)`"——那会连带抹掉拆分前就有的 `pub(crate)`，脚本现按 `名字:拆分前可见性` 显式声明并还原。
- 脚本本身做了反向验证：往分节里塞一条多余语句、改一个测试函数名，核对脚本均报错退出；只删一行 `use` 则不报——那一行本就在允许的差异内，不该算失配。
- 验证：Rust workspace 487 通过、0 失败（与拆分前基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫通过；**暂存区单独导出后 `cargo test -p solum-core --lib` 441 通过**，确认提交的树自身可编译（上一轮 `5c840d1` 曾漏暂存 mod.rs 而发出版本不可编译的树）。

## 2026-09-26 — orchestrator/tests.rs 按关注点拆为 15 个分节

- `crates/solum-core/src/orchestrator/tests.rs`（2724 行）按被测关注点切为 `orchestrator/tests/` 下 15 个文件：`capture`(487)、`routines`(448)、`import`(286)、`notif_privacy`(286)、`guard_memory`(232)、`llm_first`(164)、`widgets`(156)、`llm`(119)、`reminders`(113)、`persona`(113)、`scene_wellness`(138)、`review_suggest`(87)、`genui`(52)、`health`(29)、`head`(14，共享 `dt`/`now` 夹具)。文件内部保持原顺序，只做移动。
- 82 个 `#[test]` 全部保留，测试函数名逐一比对完全一致（拆分前后 diff 为空）。
- 跨文件共享物：原单文件顶部的 `use super::*;` / `BehaviorKind` / `NaiveDate` 提到 `tests/mod.rs`，因为兄弟文件只能经 `use super::*;` 触达；`dt`/`now` 留在 `head.rs` 并由各分节显式 `use super::head::{dt, now};`（无引用的分节只导入实际用到的那一个）；`FakeReasoner` 随组件测试留在 `widgets.rs`、`CountingReasoner`/`SpyReasoner` 留在 `llm.rs`，消费方显式导入。
- 等价性硬证据：15 个分节逐一按 token 流与拆分前 `tests.rs` 比对，全部一致；只允许可见性放开、`use` 归位与 rustfmt 换行。
- 可见性注意：元组结构体的字段要单独放开（`pub(super) struct S(pub(super) T)`），只放开结构体本身无法让兄弟模块构造。
- 验证：Rust workspace 487 通过、0 失败；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫通过。

## 2026-09-26 — solum-core 的 llm.rs 拆为分节模块

- `crates/solum-core/src/llm.rs`（1446 行）切成 `llm/` 下 7 个模块：`tests`(558) / `prompts`(263，网关提示与聊天上下文) / `reasoner`(153) / `config`(99，连接设置) / `streaming`(93，SSE 与 think 过滤) / `parse`(80，响应解析) / `extract`(74，事件抽取)，`mod.rs`(150) 保留文档头、imports、工具协议常量与组件工具定义。纯移动，无逻辑改动。
- 等价性硬证据：8 个分节逐一按 token 流与拆分前 `llm.rs` 比对，全部一致；只允许 4 处 `pub(super)` 可见性放开与 rustfmt 换行。
- 可见性对齐原文件：`parse` 与 `streaming` 的项原本就是 `pub(crate)`，`mod.rs` 的 re-export 因此是 `pub(crate) use` 而非 `pub use`——写成 `pub` 会成为空转 glob，clippy 报 "no imported item is public enough"。`streaming.rs` 不引用父模块任何名字，其 `use super::*;` 是死导入，已删。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫通过。

## 2026-09-26 — solum-core 的 account.rs 拆为分节模块

- `crates/solum-core/src/account.rs`（2145 行）按职责切成 `account/` 下 6 个模块：`session`(350，会话落盘 + 跨进程文件锁 + 响应解析) / `flows`(676，邮箱/手机/OAuth/华为/验证码等注册登录与刷新) / `tests`(697) / `reasoner`(161，走账号代理的 Reasoner) / `authenticate`(140) / `http`(97，HTTP 抽象与 ureq 实现)，`mod.rs`(42) 保留文档头与 imports。纯移动，无逻辑改动。
- 等价性硬证据：7 个分节逐一按 token 流（标识符/字符串/字符/裸串 + 逐字符标点）与拆分前 `account.rs` 比对，全部一致；只允许 16 处 `pub(super)` 可见性放开与 rustfmt 换行。
- 可见性按最小必要放开：跨分节共用或被 `tests.rs` 触及的项（`parse_auth_response`/`with_session_lock`/`TIMEOUT_SECS`/`REVOKE_TIMEOUT_SECS`/`map_http_error`/`with_timeout`/`post_with_refresh`/`refresh_and_save_with_client`/`refresh_with_client`/`cloud_account_state_with_client`/`same_account_identity`/`session_lock_path`/`lock_session_file`/`unlock_session_file`/`LEGACY_CENTER_URLS`/`AccountSession::save`）为 `pub(super)`。`AccountSession::normalize` 的 `pub(crate)` 是拆分前既有，未动。
- 修正 `check-frontend.mjs` 对源码路径的硬依赖：原按单文件读 `solum-core/src/account.rs`，现改读整个模块目录。守卫本身不变——它断言 Rust 与 Kotlin 的 OFD/POSIX 记录锁互操作（`libc::F_OFD_SETLKW` 对 `handle.channel.lock()`），跨分节成立。
- 验证：Rust workspace 487 通过、0 失败（与拆分前基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫通过。
- 未解决（既有，与本次改动无关）：`lib.rs` 的 `tauri::Manager` 与 `privacy_capture.rs` 的 `chrono::TimeZone` 为未使用导入，workspace Clippy 报红。

## 2026-09-26 — solum-core 的 sync.rs 拆为分节模块

- `crates/solum-core/src/sync.rs`（2250 行）按文件自带的 `// ---- 分节 ----` 横幅切成 `sync/` 下 6 个模块：`config`(218) / `account_recovery_envelope`(430) / `crypto`(31) / `transport`(311) / `driver`(234) / `tests`(955)，`mod.rs`(117) 保留文档头、imports 与 `SyncOp`/`SyncBatch`/`PulledBlob`/`SyncOutcome` 四个共享类型。纯移动，无逻辑改动。
- 等价性硬证据：逐分节比对 token 流（标识符/字符串/字符/裸串 + 逐字符标点），6 个分节与拆分前 `sync.rs` 完全一致——无丢失、重复、重排或改动；只允许 11 处 `pub(super)`/`pub(crate)` 可见性放开与 rustfmt 对变长签名的换行。
- 可见性按最小必要放开：`wrap_recovery_key`/`unwrap_recovery_key`/`read_local_account_key`/`PUSHED_KEY`/`PULLED_KEY`/`cursor`/`permanent_push_status` 为 `pub(super)`（仅 `tests.rs` 用到），`SyncConfigFile`/`PBKDF2_ROUNDS`/`to_hex` 保持 `pub(super)`（两个分节共用）。`crypto` 的 `encrypt`/`decrypt` 须经 `mod.rs` 以 `pub(super) use` 转出一层，故声明为 `pub(crate)`，并在 `mod.rs` 注明原因。
- `#[cfg(test)] pub(crate) mod tests {` 外壳归 `mod.rs`，测试体去一层缩进落到 `tests.rs`；`pub(crate)` 是拆分前就有的——`orchestrator/tests.rs` 借用 `MemTransport`/`test_cfg` 做无网端到端同步测试。
- 验证：Rust workspace 487 通过、0 失败（与拆分前基线一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`check-frontend.mjs` 与 47 模块加载守卫仍通过。
- 未解决（既有，与本次改动无关）：`lib.rs` 的 `tauri::Manager` 与 `privacy_capture.rs` 的 `chrono::TimeZone` 为未使用导入，workspace Clippy 报红，二者均在本会话之外的改动里。

## 2026-09-26 — 前端样式抽出 styles.css，并补上模块加载守卫

- `index.html` 的 1560 行内联 `<style>` 抽为 `dist/styles.css` + `<link rel="stylesheet">`，样式内容逐行不变，index.html 2736 → 1224 行。
- `check-frontend.mjs` 的 12 处 CSS 断言改读 `styles.css`，并新增守卫：index.html 不得再出现内联 `<style>`，必须以 `<link>` 引入 styles.css。
- 修复一处拆分引入的加载期回归：`03-clock.js` 把 `refreshAll` 作为**值**传给 `addEventListener`，而它声明在 44 号模块。每个模块都是独立经典脚本，该名字此时尚未绑定，浏览器抛 ReferenceError。改回箭头包装，事件触发时才查找。
- 新增 `scripts/check-frontend-load.mjs`：用 DOM 桩按 `index.html` 顺序真实执行全部 47 个模块并捕获加载期异常，由 `check-frontend.mjs` 自动调用（CI 两处 workflow 无需改动）。此前所有内容断言都发现不了这类问题——`js.includes(...)` 对一个加载即崩的页面照样通过。
- 验证：`check-frontend.mjs` 通过、加载守卫通过（47/47）、Rust workspace 16 个测试二进制全绿；浏览器实测样式与抽取前逐像素一致、零控制台错误。

## 2026-09-26 — 前端 app.js 解耦为有序功能域模块

- `dist/app.js`（7606 行）按既有分节横幅切成 `dist/js/` 下 47 个模块，最大 520 行；纯移动，无逻辑改动。拼接后的模块序列与原文件逐字节相等已验证。
- `index.html` 底部的有序 `<script src="js/...">` 列表成为唯一清单，同时是运行时加载顺序与声明顺序。`check-frontend.mjs` 改为按该顺序拼接程序做断言，并双向核对孤儿模块、重复加载与文件名/加载顺序不一致。
- 每个模块额外独立通过 `vm.Script` 解析，防止某一文件的语法错误被相邻文件掩盖。
- `tests.rs` 的 6 处 `include_str!` 改为按同样顺序拼接；脚本清单契约（数量、唯一、升序、i18n.js 最先）改为断言而非硬编码单个标签。
- 验证：`check-frontend.mjs` 通过（445 个唯一 id、0 内联脚本）、Rust workspace 487 通过、fmt 通过；浏览器实测 47 个模块全部加载、23 个视图存在、访客登录门正常渲染、无控制台错误与失败请求。
- 未解决（既有，与本次改动无关）：`tauri-build` 旧产物残留 `android/target` 路径导致构建失败，清理 `target/debug/build/tauri-*` 后恢复；`lib.rs` 的 `tauri::Manager` 与 `privacy_capture.rs` 的 `chrono::TimeZone` 为未使用导入，Clippy 报红。

## 2026-09-26 — 文档核对与精简

- 核对 Cargo/Tauri 版本，删除旧版本快照，修正临时目录与容器网络的过度固定描述。

## 2026-09-26 — 单项目 Agent 入口

- 规则、长期偏好和相关排障经验迁入本项目，agent 不再依赖父目录共享文档。

## 未发布 — 2026-09-17 安全修复

- 华为登录移除模拟身份和未验证 ID Token 解码回退，固定回调与 HTTPS 身份验证端点，增加超时/响应大小限制；配置不足或验证失败不签发会话。
- 短信尚无真实发送提供商，发送、手机号注册、验证码登录统一返回 `sms_unavailable`，移除验证码日志与模拟验证码配置；保留已绑定手机号的密码登录。
- 修正邮箱/手机号密码验证 salt/hash 参数顺序，手机号不再借用普通 username 匹配身份；邮箱和 OAuth 新账号创建遵守关闭注册策略。
- 限流表满时保留未过期记录并拒绝新键；十六进制摘要比较拒绝空值、奇数位及非法字符。新增隔离认证路由与安全辅助函数回归测试。
- CLI 删除记忆/清空人格接入预览、显式 `yes` 确认和 Guard 一次性令牌；导入审计仅存固定摘要，拒绝/失败记录不复制导入正文或错误中的敏感字段。
- Android 显式禁用系统备份，并为旧版备份及 Android 12+ 云备份/设备迁移增加全域排除规则，防止普通 app-data 凭据、密钥和明文库绕过应用 E2EE 备份。
- 续审修复 refresh 宽限重试成功及数据库连接失败时的 IP 限流预留泄漏，统一 finally 释放；新增直接观察预留结算的回归。补 CLI 二进制删除确认集成测试：隔离旧入口红灯、当前入口绿灯，按记录类型和 ID 校验，不混淆台账跨表同号记录。
- 验证：服务端 57 通过、5 项 PostgreSQL 集成测试因环境未配置跳过；Rust workspace 487 通过，workspace Clippy（前轮）与 CLI all-targets Clippy（续审）、fmt 检查通过。Harmony 弱随机后备分支和凭据静态保护尚未修改；隐私同意门禁的完整上传链路尚待核实，不宣称已排除。
- 验证边界：认证测试使用本机 HTTP 与内存数据库替身、模拟华为响应，不代表真实 PostgreSQL/华为联调；Android 仅核对源 Manifest/XML，未打包或真机备份测试。本次不提交、不推送、不部署，不改生产会话或历史审计数据。

## [0.2.17] — 国内主流应用市场合规与登录体系重构

- **移除访客模式，确立强制登录使用机制**：
  1. 彻底删除 `#accountGuestBtn` 及所有未登录绕过入口（`dismissible: false`，`allowGuest: false`），启动必须完成实名化或主流 ID 认证。
  2. 保护用户端到端加密与个人日程数据主权，杜绝游离与未授权数据污染。
- **隐藏短信验证码界面，主推邮箱密码与华为账号快捷登录**：
  1. 鉴于海量短信发送成本高昂，在前端界面将手机号与短信验证码输入区安全隐藏（保留底层接口与 DOM 结构备用），将首要登录与注册通道全面平滑切换为邮箱（Email + Password）。
  2. 服务端在 `auth.users` 扩展 `email` 字段与唯一索引，新增 `/v1/auth/email/register` 与 `/v1/auth/email/login` REST API，支持邮箱格式严格校验、scrypt 慢哈希加盐与端到端同步密钥自动派生。
  3. 端侧 `solum-core` 与 `solum-app` 实现 `register_email` / `login_email` 及对应 Tauri IPC 命令 `account_email_register` / `account_email_login`，前端支持回车自动聚焦密码与提交。
- **全面移除 Telegram / TannerLab ID OAuth，接入华为账号登录 (Huawei Account Kit)**：
  1. 按照国内主流应用市场规范，移除境外与自研非标 OAuth 渠道。
  2. 对齐华为开放平台标准 OAuth 2.0 REST API 规范（`https://oauth-login.cloud.huawei.com/oauth2/v3/authorize` 与 `/token`），通过端侧 Deep Link 回调安全换取令牌与用户身份信息（UnionID / OpenID）。
  3. 服务端新增 `/v1/auth/huawei/config` 与 `/v1/auth/huawei/exchange` 接口，支持自动化用户绑定与初始化端到端加密主密钥信封。
- **底层保留手机号验证码认证能力**：
  1. 服务端集成防刷流控与验证码存储器（支持 60 秒冷却、5 分钟 TTL、单 IP 频次限制），保留 `/v1/auth/sms/send`、`/v1/auth/phone/register`、`/v1/auth/phone/login` 完整能力。
  2. 客户端保留相关 Rust IPC，便于后续配置短信网关后随时一键放开。
- **国内上架全套法律合规与隐私治理**：
  1. **双协议强制预选同意**：在登录与注册提交前，增加强制勾选《用户服务协议》与《隐私政策》复选框，未勾选显式弹窗拦截并指引阅读。
  2. **协议与政策弹窗阅读器**：内置免跳转全屏法律协议与隐私声明阅读器，满足工信部与网信办对个人信息保护的刚性审查要求。
  3. **注销账号功能闭环**：设置中心 - 账号安全新增「注销账号」能力，包含不可逆危险提示与二次确认弹窗；服务端同步实现物理级数据级联彻底抹除（抹除云端同步 oplog、加密备份、用户身份与绑定数据）。
- **版本号递增与发布核验**：Cargo workspace 升级至 `0.2.17`，Android versionCode 升级至 `1027`，服务端升级至 `0.8.1`。全工作区测试全绿。
- **发布产物归档（v0.2.17）**：
  - 2026-09-17：生成 `Solum-v0.2.17.apk`；包名 `dev.solum.app`，版本 `0.2.17 (1027)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`；AlarmReceiver、BootReceiver、SolumNotificationListener 均存在，APK Signature Scheme v2 有效。
  - 文件 47,191,129 字节，SHA-256 `1181DBDE56CC3B22BEE5EDAB6E80BEF273D521E1EC659302C122A8D463C50603`，签名证书 SHA-256 `F3467B14C3CA39A88D74B85F10800565D0A81BF67FCF392147EC6B4F6AEFCA87`。
  - 构建与静态发布核对全部 PASS；按铁律自动清理旧版本 `Solum-v0.2.16.apk`，归档至 `Deliverables/Solum-v0.2.17.apk`。

## [0.2.16] — 数据恢复码首登展示治理与 LLM-First 摄取

- 更新项目 README：对齐当前 LLM-First 摄取、本地可靠性兜底、Solum Center 鉴权代理、TannerLab 通行证 Deep Link、恢复码职责与静态前端边界；补齐前端检查、APK 静态校验和关键文档入口。
- 修复数据恢复码（E2EE 第二把钥匙）在首次登录时被静默丢弃的严重可用性缺陷：
  1. **缺陷与影响**：OAuth 首次登录会在设备本地生成 32 字节同步主密钥并自动签发恢复码、上传包裹信封；但登录成功立即重启应用（Android 重启 Activity、桌面重启进程），`display_code` 在返回 UI 前被丢弃——用户从未见过恢复码，设置页却显示「已设置恢复码」造成假安心；换设备登录时被索要一个从未展示过的 20 位码，而端到端加密设计下服务器无法协助找回。
  2. **待展示持久化与启动强提醒**：首登签发与设置页「生成/重新生成恢复码」统一把新码原子落盘到账号 profile 目录（`pending-recovery-code.txt`，与同步密钥同为本机信任域）；下次启动登录态就绪后弹出「请保存你的数据恢复码」强提醒（恢复码等宽展示 + 一键复制），点「我已保存恢复码」删除暂存不再提醒，点「稍后再说」保留暂存、每次启动继续提醒。`solum-core` 新增 `save_pending_display/load_pending_display/clear_pending_display`（落盘内容归一化为 20 字符规范形，损坏内容按缺失处理）并补 2 组单测。
  3. **新设备解密弹窗文案明确化**：输入恢复码弹窗从干巴巴的格式提示升级为完整说明——这是端到端加密数据的唯一钥匙、码的来源（首次登录时展示过一次）、找回路径（旧设备已登录时到「设置 → Solum 账号」重新生成恢复码）与丢失后果（服务器无法解密、数据不可恢复）。
  4. **验证**：全工作区 483 项单测全绿，`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --check` 通过，`check-frontend.mjs` 通过；前端按仓库规范完成 mock-IPC 与真实浏览器走查：启动弹窗展示、取消后下次启动继续提醒、点「我已保存恢复码」正确调用 ack。

## 2026-09-09 — 文档治理

- 校准 README 的 `0.2.15/1025` manifest 版本；复核仓库边界、运行/验证入口与文档导航。
- 从 TODO 移除已由 0.2.15 构建、签名与静态 manifest 核验完成的 Release 重建项，保留真机联网、通知、升级和跨平台验收。
- 仅文档变更；检查链接、UTF-8 BOM、尾随空白和 diff，未打包、未执行设备或生产验证。

本页保留版本级摘要。逐提交实现、测试计数和会话过程由 Git 历史保存。

## [0.2.16]

- 日程事件摄取架构升级：从“规则硬拦截”迈向“LLM-First 语义认知与本地规则高可用兜底”：
  1. **LLM 优先提取与语境裁决**：当用户已登录配置云端 Reasoner 时，优先由 Gemini 3.8 深入理解自然语言语境，准确辨别真实排期需求与玩笑、反讽、假设或情绪宣泄，杜绝本地机械正则把调侃强拉成系统闹钟的假阳性缺陷。
  2. **网络故障无感容灾兜底**：云端 Reasoner 发生网络超时、连接失败或 5xx 故障时，系统自动无缝激活本地确定性规则提取引擎（Reliability Floor），成功兜底创建提醒并附带友好降级标注，兼顾大模型前沿智能与单机 100% 离线高可用底线。
  3. **单测套件全面覆盖**：在 `orchestrator.rs` 增加 LLM 优先、调侃意图过滤、网络故障容灾兜底及离线单机运行等 4 组单元测试，全工作区 481 项测试全绿，0 警告 0 报错。
- 搜索界面 UI 细节与视觉调优：
  1. **去除重复层级标题**：移除搜索页顶部冗余的「全局搜索」大标题与说明副标题，首屏直接聚焦并展示「想查找什么？」核心搜索框卡片，大幅提升纵向空间利用率。
  2. **收敛快捷入口小标题**：默认隐藏下方的「最近入口 / 4个入口」标题横条，仅在搜索产生匹配结果时动态展开「搜索结果 X 条」，使页面更干净清爽。
  3. **校准搜索图标居中对齐**：使用绝对定位 `top: 50%; transform: translateY(-50%)` 精准居中包装容器，并重置图标垂直对齐基线，彻底解决搜索框放大镜图标略微偏下歪斜的问题。
- 接入 CPA 网关与云端鉴权门禁深度加固：
  1. **模型与上游全面对齐 CPA**：将 Rust Core（`account.rs`）与 Node 服务端（`postgres-server.js`、`server.js`）的默认模型全面切换为 `gemini-3.8-flash-high`，服务端 upstream 默认指向 CPA 网关（`https://cpa.tannerlab.cn/v1`）。
  2. **未登录明确报错拦截**：在 `orchestrator.rs` 中强化云端调用鉴权判定，未登录状态下调用云端对话推理显式拦截报错（`"使用云端智能能力请先登录 Solum 账号"`），拒绝静默伪装成功。
  3. **锁定客户端手动配置**：界面隐藏自定义模型表单与直接配置面板，杜绝用户手动改动；同时完整保留底层自定义配置文件解析与测试 IPC 能力。
- 对齐 AndroidBridge 升级 TannerLab 通行证认证界面与交互：
  1. **主卡片视觉升维**：设置中心将「TannerLab 统一通行证认证」升为首要卡片，独立醒目呈现，传统账号密码输入框默认折叠弱化。
  2. **消除手动粘贴弹窗**：对齐 AndroidBridge（DroidRunner）原生体验，去除强制弹出的授权码输入模态框；通过 Deep Link（`solum://oauth/callback`）自动监听唤醒，并在后台自动换取令牌完成登录。超时或异常时方平滑提供 fallback。
- 发布版本维持 `0.2.16/1026`。
- 2026-09-11：生成 `Solum-v0.2.16.apk`；包名 `dev.solum.app`，版本 `0.2.16 (1026)`，minSdk 26 / targetSdk 36，ABI `arm64-v8a`；AlarmReceiver、BootReceiver、SolumNotificationListener 均存在，APK Signature Scheme v2 有效。文件 46,572,825 字节，SHA-256 `CCF510C2C8BA8FCAE0AD8263C7D9AAC95BD0D0CD9B77204681AB7550F8427284`，签名证书 SHA-256 `F3467B14C3CA39A88D74B85F10800565D0A81BF67FCF392147EC6B4F6AEFCA87`。构建与静态发布核对全部 PASS；按铁律清理旧版本包并归档至 `Deliverables/Solum-v0.2.16.apk`。

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

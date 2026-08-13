# 息壤 Solum（Personal Agent）智能体工作规范

> 适用于在本仓库工作的**一切智能体**（Claude Code、Codex 或其他）以及人类协作者。
> CLAUDE.md 与本文件共享同一套留痕规则；两边如有出入，以本文件为准并同步修正。

## 一、开工前（必须，缺一不可）

1. **读 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**，确认要做的事与当前设计一致。
   发现代码/需求与文档不一致：**先在文档里说明，再改代码**——文档是设计的唯一权威。
2. **扫一遍 [docs/PITFALLS.md](docs/PITFALLS.md) 里与本次任务相关的条目**（按关键词搜即可），
   别把踩过的坑再踩一遍（Windows 重定向、Android 构建五连坑、WebView 安全区等都有前科）。
3. **动前端（`crates/solum-app/dist/index.html`）之前，必须先读
   [docs/PRODUCT.md](docs/PRODUCT.md) 与 [docs/DESIGN.md](docs/DESIGN.md)**——「琥珀台灯」设计体系的
   战略上下文与视觉规范都在里面，不读就改样式基本一定跑偏。
4. **确认工作区基线**：`git status` 看清已有未提交改动（别把别人的半成品混进自己的 diff）；
   记住当前测试基数（跑一次 `cargo test --workspace`），完工后对照不许变少。

## 二、工作中（红线，任何任务不得突破）

- **隐私不变量（架构 §4）**：个人数据只落本地 SQLite；第三方通知文本仅在「通知上云」
  开关开启且捕获时未标 `local_only` 时，才能作为 recall 语料/云端上下文并参与同步；关闭时
  不出进程。云端每次调用只发最小上下文（当前一句话 + 时刻 +
  短期会话窗 + 可审计的 recall 片段）；密钥材料（`solum-llm.json` / `solum-sync.json` /
  keystore）一律 gitignore，绝不入库。
- **离线优先（F16）**：云端失败必须降级到离线路径，`ingest` 不允许失败；
  提醒触发链路不得依赖网络。
- **注入时钟**：solum-core 核心逻辑不读系统时间，`now` 一律由调用方传入。
- **护栏不绕行（§3.3）**：破坏性操作必须走 Guard（预览 → 人工确认 → 一次性令牌 →
  append-only 审计）；`Grant` 只能由 `guard.rs` 签发，不得为图省事开后门。
- **前端技术栈铁律（§3.9）**：vanilla JS + 纯静态 `dist/index.html`，
  **不引入前端框架、不引入 npm 依赖**——任何"装个包就好了"的念头都违规。
- 诚实红线：对外材料（README、演示、提交表单）不夸大、不包装未做过的事。

## 三、完工后（必须，全部通过才算完）

1. **文档留痕（强制，不可省略）**——每一次改动都必须落在以下三份之一：
   - [docs/PITFALLS.md](docs/PITFALLS.md)：调试超过几分钟/走了弯路的问题。
     现象、根因、解决方式、如何避免。**没解决也要记。**
   - [docs/CHANGELOG.md](docs/CHANGELOG.md)：新增/完善/修改了功能，
     按 [Keep a Changelog](https://keepachangelog.com/) 风格记在 `Unreleased` 下。
   - [docs/MISC.md](docs/MISC.md)：否决的方案、取舍原因、非功能性观察。
   判断标准：**改了行为 → CHANGELOG；踩了坑 → PITFALLS；都不是但值得记 → MISC。**
   拿不准就多记一条到 MISC，不要漏记。本条规则对这三份文档和本文件自身的修改同样适用。
2. **质量门（Rust 侧改动）**，三条全绿才许收工：
   ```bash
   cargo test --workspace                            # 全绿，且总数不少于开工基线
   cargo clippy --workspace --all-targets -- -D warnings   # 零告警
   cargo fmt --all --check                           # 零漂移（2026-07-16 起全仓库强制）
   ```
3. **前端改动的验证**：`dist/index.html` 有改动时，至少做 JS 语法校验；
   涉及交互的用 mock-IPC harness（`window.__TAURI__` 假实现注入）+ 真实浏览器走查
   （手法见 MISC 2026-07-15「验证 harness 的形态」条目），别只靠肉眼读 diff。
   **断言必须落在渲染结果上，不能只查"意图"**——`getComputedStyle(el).display`、
   `getBoundingClientRect()` 的宽高、`offsetParent === null` 才算数；只断言
   `el.hidden === true`、类名在不在、属性设没设，是在验证自己的代码写没写，
   不是在验证用户看到了什么。这两者会分叉：`hidden` 属性就被 `.btnrow` 的
   `display:flex` 静默压过，代码/diff/属性断言全对而按钮照样满尺寸渲染
   （PITFALLS 2026-07-19 当日条目）。同理，「没报错」不等于「画出来了」
   （SVG 图标全灭那次也是无报错，见 PITFALLS）。
   **但反过来也要小心：断言方法本身会误报。** `offsetParent === null` **对
   `position: fixed` / `sticky` 元素恒为真**，拿它判 toast、弹层、吸顶栏这类
   元素会稳定得出「不可见」的错误结论（2026-07-20 实测：`#toast` 有 `.show`、
   `opacity:1`、262×63 在屏，`offsetParent` 仍是 null）。这类元素改用
   **`getComputedStyle` 的 `opacity`/`visibility`/`display` + 矩形在视口内**
   判断；`elementFromPoint` 命中测试也不适用于带 `pointer-events: none` 的元素。
   **误报和漏报同样有代价**——它会让人去"修"一个本来正确的功能。看到与预期
   相反且过于干脆的结论（"完全没渲染"、"三种条件结果一模一样"），先怀疑量具。
4. **文档跟着行为走**：改了用户可见的命令/结构/配置 → 同步 README；
   改了设计 → 同步 ARCHITECTURE 对应小节。文档说谎比没有文档更糟。
5. **仓库卫生**：不留垃圾文件（`nul`、临时 `*.sqlite*`、一次性脚本）；
   会话临时产物放系统 scratchpad，不进仓库。
6. **提交纪律（2026-07-19 用户改定）**：完工且质量门三绿后**自动 commit，不必再问**；
   一个独立功能/修复/文档批次一个 commit，信息写清楚做了什么。
   **但 `push` 仍需用户明确要求**——推送是对外动作，不在自动范围内。
   提交前先 `git status` 确认工作区里没有**别人的半成品**（本仓常有并行会话），
   只 stage 属于本次任务的文件，不要 `git add -A` 一把梭。
7. **不越权**：不改动任务范围之外的代码；完工汇报如实——测试挂了就说挂了，
   跳过了就说跳过了。

# MISC 杂项记录

> 强制记录：不适合写进 CHANGELOG（没有对应"功能变更"）、但有留存价值的内容——讨论过又否决的方案、临时决定的原因、非功能性观察等。

## 2026-08-11 私有 Git 不是凭据库；通知上云改为 opt-in

本轮复核发现一份未提交的 `.gitignore` 把数据库、API 配置、账号令牌和 Android release keystore 都改成了可跟踪，理由是仓库会备份到私有 Gitea。这个理由不成立：Git 历史难以真正删除，远端权限和镜像范围会变化，而当前仓库的 `origin` 仍是 GitHub。文档与演示材料可以进入版本管理，个人数据和密钥必须继续留在 Git 之外；需要备份的签名材料应进入独立的加密备份，而不是源码历史。

2026-07-19 为作者自用便利把通知上云定成默认开启，当时又用“App 白名单默认空”降低初始风险。现在产品已有首启隐私门、账号代理和公开发布文档，这个默认值继续存在会让“只有主动使用联网功能时才外发”的承诺变得含糊。因此推翻旧默认：新安装缺省关闭，用户必须另行开启；数据库里已保存的 `true`/`false` 仍优先，升级不会静默改写明确选择。旧条目保留为历史，不回写成当时就做了 opt-in。

## 2026-07-29 UI 状态持久化与业务缓存刻意分成两套

本轮没有引入 Service Worker、第二份 SQLite 镜像或离线业务缓存。Solum 本身就是 local-first，业务数据已经在本机权威库里；再复制一层持久缓存会让 F12 的“删后立即消失”、LWW 同步和邮件第三方内容边界同时变复杂。真正需要解决的是两件更窄的事：同一轮渲染别重复过 IPC，以及用户回到应用时别丢导航位置与未发送草稿。

因此定下两套互不混淆的机制：业务读取只做当前进程内亚秒级去重，任何写入成功整代失效；UI 状态才进 `solum.ui-state.v1`，字段白名单只含视图、滚动与聊天草稿。搜索词虽然技术上也能“提升体验”，但它可能直接暴露用户在找哪段记忆，所以没有持久化；邮件表单同理，而且其正文受 §3.14 更严格边界约束。对话草稿允许保存，是因为完整聊天历史本来就按已拍板契约留在同一 WebView 本机存储中，草稿没有扩大数据边界。

## 2026-07-27（晚）用户推翻了上面裁定里的两条：隐私门与 capture 层照做

上一条裁定里「刻意不移植」的五项，用户复核后**明确要求补做其中两项**——首启隐私门 + 应用内隐私政策页、多入口采集 capture 领域层。另外三项（鸿蒙 UI 轮、`solum-account.json` 契约差异、账号 vs 直连裁决规则）维持原状。裁定被推翻不代表当时的理由错了：那几条理由是"对自用产品收益不大"，而不是"做不了"；用户按自己的判断改了取舍权重，这是他的决定。上一条留着不改，因为它记录的是当时的判断依据。

**移植时反复用到的一条判据：结构可以照搬，事实断言必须重写。**

两个模块都带着"对外声明"性质的文本——隐私政策正文、采集入口清单——而鸿蒙那两份的内容是**为鸿蒙上架包写的**：政策里写「本版本不提供多设备同步」「不使用第三方通知监听」，清单里把「第三方通知」标成「鸿蒙未开放」、把系统分享和截图 OCR 标成「可用」。这五句话**对本仓全部为假或反向**：本仓有端到端加密同步、有 Android 通知捕获（桌面没有）、有邮箱连接器，却没有系统分享目标（`AndroidManifest.xml` 里没有 `ACTION_SEND` 过滤器）也没有接入任何 OCR。

照搬会让**界面对用户撒谎**，正好撞上 AGENTS.md 那条「对外材料里每个可核对的名词都要能落到仓库事实上」。所以：政策正文按 `docs/PRIVACY.md` 与实际代码重写，入口状态按平台真实能力算出来。**两个模块各留了一条测试专门钉死这件事**——一条禁止那两句鸿蒙文案出现在正文里，一条禁止把未实现的入口标成 `Ready`。写测试而不是写注释，是因为这类退化最可能发生在"下次有人图省事直接贴一段过来"的时候，注释拦不住，测试拦得住。

**「不支持」与「待接入」不是同义词。** 桌面端的通知捕获标成 `Unsupported`（"桌面无此机制"）而不是 `Connector`（"待接入"）——后者会让用户以为"等下个版本就有了"，而桌面根本没有对应的系统权限模型，这个承诺永远不会兑现。状态词本身就是一种承诺，别用听起来更客气的那个。

**隐私门刻意不套在 `solum-cli` 上。** 它是本机自动化入口，加交互式同意门只会卡住脚本，而它并不新增任何数据出境路径——同意门要拦的是"数据可能离开设备"这件事，不是"有人运行了二进制文件"。

**capture 层目前没有真正的生产者，这一点如实写进了 CHANGELOG。** 桌面没有系统分享/OCR，页面里的粘贴框是手动等价入口。可以争论"没有生产者的收件箱算不算死代码"，选择先落地是因为：入口清单本身对用户有价值（它回答"息壤能从哪接到信息"），而把 `CaptureInbox` 与线索抽取一起落地，等系统分享真接上时不必回头再改一次数据流形状。但**没有假装它已经能用**。

## 2026-07-27 鸿蒙版新增能力回移主仓：移植范围的裁定

用户要求「把鸿蒙版新增的、主仓没有的东西加回主仓」。先做了差距核对——**鸿蒙 0.2.0/B/C 轮的大部分"新增"其实是从主仓对齐过去的**（流式上屏 `chat_reply_ui_streaming`、会话历史 `pa.chat-sessions.v1`、厂商预设 `LLM_PRESETS`、GenUI 组件目录、英雄区问候），这些主仓本来就有，不存在回移问题。真正是鸿蒙首创、主仓缺失的四项已全部移植（见 CHANGELOG 当日条）：`server/` 云代理、账号登录模式、对话 Markdown 渲染、琥珀台灯图标。

**刻意不移植的项及理由**（别当遗漏去"补"）：

1. **首启隐私门 + 应用内隐私政策页**：那是华为上架合规的产物（AGC 审核要求首启同意）。主仓是自用桌面/侧载 Android，无商店审核；数据边界的单一权威是 `docs/PRIVACY.md`，强制同意门对自用产品只添摩擦。若将来主仓要上架，再从鸿蒙 `privacyConsent*/privacyPolicy.ets` 移植（该实现的「同意状态是设备级事实、刻意不进同步」判据仍然适用）。
2. **多入口采集（capture 领域层 + 系统分享 + 截图 OCR + 数据入口清单）**：鸿蒙是因为系统**没有**通知监听 API 才把输入模型重构成"多入口采集"；主仓有完整的 F20 通知智能管线 + F21 邮箱连接器，输入面不缺。桌面等价物（剪贴板监听/截图 OCR）是独立立项，不该以"回移"名义顺带做。
3. **鸿蒙 UI 轮（五组页签/设置目录面包屑/DataTable 三表/GenUI 数据卡/日期块/SymbolGlyph）**：主仓桌面壳 2026-07-20 已完成自己的 IA 收口（6 组导航 + 设置多级目录），且鸿蒙这套的设计令牌本来就是从主仓 `dist/index.html` 的 `:root` 搬过去的——设计源只有一份，形态按端各自演化，不回搬。
4. **`solum-account.json` 契约差异**：主仓在鸿蒙四字段形状上多存一个 `model`（鸿蒙把模型名另存）。读旧形状文件按默认模型补齐，两边互不破坏；这是刻意的超集，不是漂移。
5. **账号 vs 直连的裁决规则**：鸿蒙 0.2.0 发布版直接**删除**了直连 API Key 输入（上架包不能引导用户自填 key）；主仓保留两条通路，规则定为「登录期间账号优先，退出自动回落」——比"二选一开关"少一个状态，也和鸿蒙升级路径兼容。

**同源维护义务**：`server/` 与 `PA-harmony/server/` 是同一份代码的两份拷贝（两处 README 均已注明改动要同步）。没有做成 git submodule/软链，因为两仓是平级独立仓库且鸿蒙仓要打包进比赛源码归档，拷贝是最不惊喜的形态。

## 2026-07-27 安全审计汇总的两处未落实项（摘录留痕）

2026-07-27 全目录文档整治时核对了仓库外的 `PA_PA-harmony_安全与逻辑审核汇总.md`（2026-07-21）：
其结论绝大部分已在本仓库 CHANGELOG/MISC/PITFALLS 留痕落实，但有两项**仍未执行**，摘录在此以免只存在于仓库外文档：

1. **修复后复审未执行**：七轮复审只覆盖了汇总文件列出的问题，未扩大审计范围；
   汇总文件自我声明为未来「完成剩余模块审计后、以它为基线的修复后复审」的基线。该复审至今未做。
2. **Android 原生侧（Kotlin）零自动化验证**：闹钟落盘失败、通知收件箱竞态等路径只有代码层保证，
   没有故障注入/崩溃恢复回归；需要真机或注入框架，是下一轮该补的。

汇总文件本体保留在 `Project\Desktop_export_2026-07-13\` 根目录（作为复审基线不删）。

## 2026-07-22 演示视频 v3：重录的取舍，以及第三次守同一条诚实红线

`Solum_demo.mp4`（v1，7-15）与 `Solum_demo_script_v2.md` 的重剪方案都已过时——v1 拍摄时改名尚未发生，
此后 IA 重构、邮箱连接器、F19 持久化组件、整轮安全审核整改全部没有画面。本次重录为 v3，
产物 `Solum_demo_v3.mp4`（178s）、`Solum_demo_script_v3.md`、`Solum_demo_v3_screenshots/`（7 张 3:2），
均已 gitignore，与 v1/v2 同等待遇。

- **旁白口径：第三次拒绝包装工具链，理由与 7-15、7-17 两次相同。** 用户提出「云端用 MiMo 跑，旁白里说是 GPT-5.6」，
  因为提交表单要求旁白覆盖 "how I used Codex / how I used GPT-5.6"，而手上没有 OpenAI key。**已拒绝**——
  这正是 7-15 那条「宁可少拿分也不误导评委」的同一形状。
  - 找到的诚实解法：**把"运行时模型"和"写代码的 agent"拆开**。旁白说云端推理走
    "an OpenAI-compatible gateway"（字面事实，`llm.rs` 就是 OpenAI 协议，**不点模型名**），
    另外如实讲 GPT-5.6 是**实现这个项目的 coding agent**（7-19 之后的改名、IA 重构、邮箱连接器、
    安全审核整改、F19 组件都由它按用户设计实现——这条是用户当场澄清的，此前文档只记到
    「实现将交由外部智能体（GPT-5.6）执行」，没记结果）；Codex 仍如实限定到 Daily Focus Brief 一块。
    三条表单要求全部成立，无一句需要包装。
  - **留档意义**：前两次是"拒绝编造 Codex 使用"，这次是"拒绝错报运行时模型"。
    共同判据——**对外材料里每一个可被核对的名词都必须能落到仓库里的事实上**；
    做不到时改措辞（换成同样真实但更宽的说法），不是改事实。
- **演示数据用独立演示库，不用真实库。** 视频要公开，真实 `solum.sqlite` 里的日程/记忆/通知会连人带事出镜。
  改为 `SOLUM_DB` 指向临时演示库，按脚本喂入虚构但真实流经全链路的数据（4 条日程覆盖
  截止/事件/考试三档分级 + 2 条自动生成的建议 + 1 条走真实 spool 路径的航班变更通知）。
  仓库里的 `solum.sqlite` 与 `solum-llm.json` 全程零改动。
- **最好的一个镜头是 7-21 当天那条授权拆分。** 通知回看里那行
  「识别到可排期内容；该应用未获『自动建日程』授权，请你确认后再建」——
  确定性抽取认出了可排期内容，但因为缺少那条更窄的授权而停下来问人。
  **"能读你"和"能替你动"是两条独立授权**，这件事用画面讲比用文字讲有力得多。
  做对外材料时值得优先找这种"能演出来的克制"，而不是罗列功能。
- **仍然欠着的**：视频是离线 SAPI 合成的机器音（Zira，沿用 v1 管线），不是人声；
  界面全中文而旁白全英文，非中文观众看不懂界面文字，只能靠旁白理解。
  两条都不影响事实准确性，但下一版若还投英文场合，值得考虑加英文字幕轨。

## 2026-07-21 GitHub 开源发布：公开快照的范围与隐私边界

- **范围收口**：公开仓库只发布 Solum 的桌面端与 Android 移动端实现；鸿蒙端是独立项目，不纳入源码、提交历史或发布物。
- **公开历史采用干净快照**：既有本地开发历史含作者姓名/邮箱与旧机器路径。为避免把这些元数据和任何历史遗留内容公开，发布时保留本地 `master` 供继续开发，另建立无父提交的公开分支，只包含完成审计后的当前源码快照。
- **审计结论**：`solum-llm.json`、`solum-soulous.json`、`solum-email.json`、同步配置、SQLite 数据库、keystore 与本地演示产物均未被当前或既有提交跟踪；根 `.gitignore` 同时补充 `.env`、私钥文件和常见凭据 JSON 的兜底规则。代码中的 token/secret 字符串均为配置字段或测试夹具，没有发现真实凭据。`Cargo.toml` 的仓库元数据已从本地占位符改为公开仓库地址。质量门还发现并用 `cargo fmt` 收敛了一处既有格式漂移。

## 2026-07-21 安全审核收尾：CSP 的现实边界、破坏性命令的授权边界、以及仍然欠着的两件事

**CSP 为什么带着 `'unsafe-inline'`**。原本是 `csp: null`（完全关闭）。现在设成 `default-src 'self'`，并显式收紧 `connect-src`（只留 `'self'` 与 IPC）、`object-src 'none'`、`frame-src 'none'`、`form-action 'none'`。**但 `script-src` 仍带 `'unsafe-inline'`**，因为整个前端就是一份内联 `<script>` 的静态 `index.html`（§3.9 铁律：vanilla JS、无框架、无 npm、无构建步骤），而 CSP 的 nonce/hash 机制要求脚本可寻址。判据：**即便带着 `'unsafe-inline'`，这条 CSP 也已经拦掉了"远程加载脚本"和"把数据 POST 到外部域"这两条真正的外泄路径**——注入者拿到的是一个不能对外说话的执行环境。要彻底去掉 `'unsafe-inline'`，正确做法是把脚本拆成 `dist/app.js`（这不违反技术栈铁律，拆文件不等于引框架），但那是一次纯机械的大改动，按 AGENTS.md 必须配真浏览器逐视图走查，不适合混在安全批次里顺手做。**这条留给下一次前端批次，别忘了。**

**（2026-07-21 稍晚更新：下面这段已经过时——三个命令已按 Guard 做全，只剩 `event_cancel` 待议。保留原文是因为"当时为什么先只做一半"的判断本身值得留存。）**

**破坏性后端命令的授权边界，目前只做到"可见"，没做到"不可绕行"**。`event_cancel` / `forget` / `persona_clear` / `widget_record_delete` 这四个命令仍然不要求 Guard 令牌——它们靠前端 modal 拦着，而**前端 modal 是渲染，不是授权边界**：命令本身可以被直接调用。本轮给它们补了 append-only 审计留痕（`audit_irreversible`），理由是：**"删了但查得到"和"删了且毫无痕迹"是两个量级的问题**，先把后者消掉。
真正的修法是按 §3.3 红线把这四个命令注册成 Guard 工具走「预览 → 确认 → 一次性令牌」。**已于当日完成其中三个**（`forget` / `persona_clear` / `widget_record_delete`，见 CHANGELOG）；关键是**旧命令要整个删掉而不是留着**，留着就等于修了个寂寞。**刻意没有另造一套 nonce 机制**——AGENTS.md 明写「`Grant` 只能由 `guard.rs` 签发，不得为图省事开后门」，再发明一个平行的确认令牌恰恰就是那个后门。这条要么按 Guard 做全，要么先不做，不能做一半。

**（2026-07-21 稍晚：用户拍板选了方案 b，已实施——见 CHANGELOG。保留下面的原始分析，因为"为什么这不是我能顺手改的"这个判断本身要留存。）**

**桌面端「配置放 cwd」没有改，因为它和另一条已定设计直接冲突**。审核提的「桌面默认依赖工作目录，启动目录一变就读到别的配置」属实。但移动端早已指向 app-data，而桌面留在 cwd 是**有意的**：`solum-cli` 默认也是 `./solum.sqlite`，两者共用一份库正是当初拍板的设计（见本文件更早条目）。把桌面挪到 app-data 就等于单方面拆掉 CLI 与桌面共享数据这件事。
判据：**当一个「修复」会推翻另一条明确记录过的设计决定时，它就不再是修复，而是一次产品决策**，该由人来定而不是顺手改掉。两个方向都成立——(a) 保持共享、接受 cwd 的脆弱；(b) 都挪到 app-data，CLI 加 `--db` 指过去或同样默认 app-data。**留给用户拍板，本轮不动。**

**凭据仍是明文 JSON，没有接系统凭据库**。这一条与 7-20 邮箱连接器条目的判断一致（见下），本轮维持：改存储介质需要独立迁移 + 跨平台评审，尤其是 Android 侧——PITFALLS 2026-07-21 刚记过「给要上移动端的 Rust 项目选依赖时先查它的原生依赖」，keyring 类 crate 正是这个雷区。**但要说清楚：这不是"已修"，是"已决定推迟"**，审核报告里的 P2 依然成立。

## 2026-07-20 F21 邮箱连接器取舍：标准协议优先，邮件不成为 Agent 语料

第一版统一以 **IMAP + SMTP** 接入，而不是为 QQ、Gmail、Microsoft 各自做网页抓取或厂商私有 SDK：同一套读取、搜索、发送与 TLS 边界可覆盖主流邮箱，也给自定义服务商留下入口。代价是某个企业租户若禁用 IMAP/SMTP 或 OAuth 委托权限，Solum 必须如实报告无法连接，不能用登录网页或伪造成功来绕开组织策略。

认证分两路：QQ 的现实可用路径是网页端开启协议后的授权码；Gmail 和 Microsoft 优先 Authorization Code + PKCE，用户自行注册桌面 OAuth 应用。客户端密钥、refresh token 和应用专用密码都只进本机 `solum-email.json`，本机 loopback 回调中的 code / state / verifier 只活到授权完成或超时。当前不尝试接操作系统凭据库，原因是项目既有 LLM / Soulous 配置已采用同一「本地、gitignore、UI 不回显」约定；改变存储介质需要一次独立迁移与跨平台评审，不能在连接器首版顺手混入。

最重要的否决是「邮件一接进来就让 Agent 摘要、建日程或入记忆」。邮件含大量第三方内容，直接流入 prompt 等于把收件箱变成不受控的提示注入面，也会突破通知上云的明确范围。因此 v1 只做用户显式读取与手动撰写：邮件不落库、不进入 recall / LLM / 同步 / 导出；外发仅经 Guard 的完整预览和逐封确认。以后若做“邮件转日程/记忆”，必须是单封、最小字段投影、用户可见且再次确认的独立能力。

## 2026-07-20 改名 PA → 息壤 / Solum：取名理由与「哪些名字刻意不改」

**为什么是这两个名字**：息壤出自《山海经》，是能自己生长、永不减损的土——喂进去打卡/聊天/日程/行为日志，它自己长；"息"又双关作息、生息，正对习惯学习这条主线。英文 Solum 是拉丁语，土壤学里指土壤剖面真正有生命活动的那层，与中文语义对齐而非硬凑音译；字面藏 **soul**（延续 Soulous 血统），词根同时是 *solus*（唯一/独自，对应只服务自己一人 + 本地存储）。否决的备选：Soulon、Symbi、Wove、Ousia。

**一致性规则（后续新增文案照此办理）**：代码 / 仓库 / 域名 / bundleName 用 `solum`；`息壤` 只出现在用户真正看到的地方——品牌标、Android 桌面图标名、通知渠道名、窗口标题、发给云端的人格提示词。文档正文用 Solum，不混用。

**刻意没改的三处，都是对外契约**（判据：**这个字符串有没有第二方在读？有就不能单方面改**）：
1. `soulous::SOLUM_SOURCE` 的值仍是 `"pa"`。标识符改了，值不能改——Soulous 端按 `source=pa` 做幂等键与来源打标，本次不动 Soulous 仓，改值等于当场断掉 8.2 推送。
2. 导出格式标识写出改成 `solum-export`，读取端**同时接受** `pa-export`（`export::LEGACY_FORMAT`）。只改写出不改读入的话，用户改名前导出的每一份备份都会被判为"不是导出文档"整份拒绝——那正是导出 v2 刚修好的失败模式，回归测试 `a_backup_written_before_the_rename_still_restores` 钉住这条。
3. release keystore 的内部别名仍是 `pa-release`。别名在 keystore 二进制里，改它要 `keytool -changealias`，等于动签名身份；只改了文件名和 `keystore.properties` 的指向。

**判据推广到了所有名字**：**只有本仓两端在读的名字就改，键入已存在持久化状态的名字就不改。** 按前者改的有：Tauri 事件名（`pa-fired` → `solum-fired` 等 11 个，Rust 发射端与前端监听端同批改、已逐一对齐）、CSS 类 `.pa-dim`、Android 通知渠道 ID `pa_reminders`、通知监听服务类 `PaNotificationListener` → `SolumNotificationListener`。按后者保留的有三处，**都已就地写注释说明，别当改名遗漏"修"掉**：`CHAT_SESSIONS_KEY = "pa.chat-sessions.v1"`（localStorage 键，改了孤立用户全部会话记录）、`DOCUMENT_DB_NAME = "pa-local-documents-v1"`（IndexedDB 库名，改了已上传文档整批读不回）、sync-server 默认库名 `pa-sync.sqlite`（指向已部署中继上的既有文件，改默认值会让中继从空库起步、设备游标全丢）。

**历史日志不做替换**：CHANGELOG / PITFALLS / MISC 记的是当时的事实，含 `crates/pa-app` 这类路径和 `PA_PREBUILT_JNILIBS` 这类真实跑过的命令。批量替换会让历史条目变成"从未发生过的样子"，违反诚实红线；现状文档（ARCHITECTURE / README / PRODUCT / DESIGN / PRIVACY / LLM-PROVIDERS）则全量改。代价如实记账：读改名前的条目需要按映射换算，这个成本换的是历史可信。

**改名窗口的时机账**：本次能这么便宜（纯文本替换 + 一个迁移函数），前提是桌面无安装态、未上架、AGC 未注册。真正贵的那一半躲不掉——`applicationId` 一变，手机上的旧版就是另一个 App，只能走导出/导入（见 PITFALLS 当日条目）。**这类改名的成本随机上数据量单调上升，能早改就别拖。**

## 2026-07-20 PA 体验收口：可见名称、可编辑规则、可切换会话与固定工作台

- **通知来源**：白名单的真实键仍是 Android 包名，但这只是监听策略的内部实现。Android 插件按 PackageManager 返回可启动应用的显示名与键，壳层以名称搜索、选择和回显；桌面端没有通知监听管线，诚实地返回空列表，不编造可选应用。
- **规则编辑**：规则页保存 lead time / 渠道后，core 仅删除并重建该事件类型、尚未触发的 future pending 提醒；已经触发或忽略的通知保留为历史审计。这样「能编辑」不会变成改写过去。
- **会话历史**：完整转录落在壳层本地会话存储，选择会话时才以 `chat_context_set` 送给 core 最近 ≤ 4 轮；core 再次限长。会话不进 SQLite、同步或导出，因而历史导航不改变最小上行上下文的隐私边界。
- **工作台**：顶层导航固定为「工作台」，二级为「资料 / 组件」，不把用户新建组件变成顶层导航。资料页的 PDF 当前仅本地归档；文本文件只能由用户明确点选后带入对话输入，避免把“上传”伪装成“已理解全文”。

## 2026-07-20 Phase 11 真机口径验收：五项走查的结论与「真实数据其实不在这台机器上」

用真实构件（真 v6/v12 代码写出的库、真 `pa-sync-server` 加密链路、真导出文件、真浏览器）跑了一轮验收，两个缺陷已修（见 CHANGELOG/PITFALLS 当日条目），此处只记不进 changelog 的观察。

**前提修正——本机没有「真实 pa.sqlite」**：仓库根的 `pa.sqlite` 已是 v14、零组件、只有 23 条 `soulous_facts`；`crates/pa-app/pa.sqlite` 是 7-15 的空 v3 库；桌面没有安装态。手机 release APK 打于 **7-18 15:23，对应 schema v6**，早于 F19（7-19）——**手机上根本不存在组件数据**，`DROP COLUMN schema_json` 在真机上是空操作。所以「v13 迁移不可逆」的真实暴露面是零，**真正会在手机上第一次跑的是 v6 → v14 整条八段链**。验收对象因此从「v13 那一步」改成了整条链，这个修正比验收结论本身更重要：**验收前先确认「要验的东西到底在哪台机器上、处于哪个版本」，别按提问的措辞直接开测。**

**迁移两条都过**：v12→v14 保住字段序、两视图成员与序、`sort_by`、必填、enum options，记录逐字节一致，guid 回填 16 条 oplog；v6→v14 七张表条数与内容一致，两条 `[通知·` 链的 `local_only` 冻结正确，v10 bootstrap 把它们首次推进 oplog。

**390px 组件页（第 4 项）无缺陷**：页面在 100%–200% 的 Android `textZoom` 全档位都**不产生横向溢出**（`documentElement.scrollWidth` 恒为 390）；表格靠 `.tscroll` 在自己盒子里横滚（588px→1056px，右边界恒定钉在 374），stat 磁贴 2+1 排布、200% 时标签换到两行但不裁切。**唯一的小观察**：bool 字段用的是原生 `<input type=checkbox>`，实测高 13px，远低于 44px 触摸目标下限——不是布局缺陷，是移动端可点性 papercut，与既有「7 条 papercut」同级，未修。

**「从日程导入」的静默跳过（第 5 项）**：映射逻辑本身正确（第一个 text 收标题、第一个 datetime/date 收开始时间），真实 ingest 出的 7 条日程全部导入。但有两处形态值得记：① `import_events_into_widget` **只返回导入条数，跳过的既不计数也不给理由**——本轮那个字段序 bug 导致 7 条全跳时，界面上只有「导入 0 条」，用户无从判断是没有日程还是映射不上；② `limit` 是在**过滤之前**用 `.take(limit)` 取的，前 N 条都不合格就导入 0，哪怕后面还有能导的。两条都不是本轮修复范围，记在这里等真实使用反馈决定要不要改。

## 2026-07-20 Phase 11 第三步后记 + 「7 条 papercut」复核结论

**第三步（table/stat + 两条快照桥 + 只读 CLI）**：实现细节见 CHANGELOG 当日条目。值得单记的是**拆行的复利兑现了**——v13 把 schema 拆成一字段一行时，成本是一次结构性改动；这次加两个视图只是给 `widget_fields` 再加两个 ord 槽位，**合并语义一行没动**，并集合并、同名裁决、超限只读增长全部原样继续成立。当初若选了"blob + 字段级合并"，每加一个视图都要再动一次那套 bespoke 合并代码。

**`stat` 算子为什么不进 schema**：让 LLM 指定聚合算子，等于在声明式 schema 里开一个表达式位，而"声明式而非可执行"是 F19 第一天就定的关键路径（设计稿 ① 否决公式字段是同一条理由）。现在算子由字段类型推导（number→合计、bool→计为是、其余→已填），要扩就改 Rust。**代价要认**：用户想要"平均重量"而不是"合计重量"时，当前无法表达，只能等我们加算子——这是有意选的边界，不是没想到。

**「7 条 papercut」复核：全部已修，本轮无新代码**。逐条实测/查证：云端调用整窗假死（`ingest` 已是 `async fn`）、snooze toast 拼进 DOM 节点（已改 `fmtDTText`）、对话内高危拦截死胡同（已有「前往护栏查看确认流程」入口）、今日面板首次加载为空（宽屏冷启动实测三个分区都渲染出各自空态）、每小时提醒语义静默丢失（`recurrence_caveat` 已覆盖）、ISO 时间串/英文内部记号/提前0措辞/记忆改写确认键配色（`okStyle: "primary"`）/导出文件名用真实墙钟——均已处理。唯一未动的是「规则表 UI 只读」，MISC 原条目本就注明是**有意留待真实数据固化**，不属缺陷。**记这一条是因为"确认无事可做"本身是结论**：清单陈旧时，照单再实现一遍比不做更糟。

## 2026-07-20 Phase 11 第二步设计定稿：schema 演进 + 同步（选定"拆行"方案）

范围由用户拍板为**数据韧性优先**：schema 演进（只加可空字段）+ 多设备同步。`table` / `stat`、导入/反向提升、动态导航仍留后。定稿方式同 2026-07-19：拿「训练记录」组件推演一遍，而不是抽象讨论。

**推演暴露的核心问题**：两台设备并发加字段（手机加 `feel`、桌面加 `notes`），而 `schema_json` 是**一个列**，走现有行级 LWW 必然整份覆盖——hlc 大的赢，另一边的字段**永久消失**。更坏的是丢失是**双重的**：手机上已经填了 `feel` 的记录还在，但 schema 里没这个字段了，`validate_record` 判它们含未知字段，记录数据被 schema 孤儿化。这不是"同步没做好"，是"把可独立演进的集合塞进一个标量列"的必然结果。

**解法来自设计稿 ⑧ 自己**：「只允许加可空字段、不允许删字段/改类型」这条约束，让字段集合成为一个**只增集合（G-Set）**，而只增集合的合并就是求并集——天然收敛，不需要任何冲突解决语义。**演进与同步不是互相拖累而是互相成全**：正因为只加不删，才不必发明合并规则。

**白拿的好处：记录零迁移**。新字段必然可空，老记录只是少一个键，而 `validate_record` 对缺失的非必填字段本来就放行。这是 ⑧ 那条约束的真正回报，不是附带条件。

**方案选择：拆行（B），而非 JSON blob + 字段级合并（A）**。A 需要在 blob 里给每个字段埋 hlc 戳并为 `widget_defs` 写专用合并逻辑（约百行 bespoke 代码，且要自证收敛）。选 B 的两个理由：① **为一张表发明第二套合并语义正是 `local_only` 那类 bug 的温床**——两个看起来一样的东西行为不一样；拆行后现有的触发器 / guid / 行级 LWW / 隔离区**一行不改直接生效**，并集是自动的。② **迁移窗口现在几乎为零成本**：功能昨天落地、尚未 dogfooding、库里没有真实组件；这个窗口一旦开始真用就关上了。B 的代价如实记账：这是对已交付代码的结构性改动，`validate` / 渲染 / store API 都要跟着动。

**表结构（schema v13）**：`widget_defs` 去掉 `schema_json`、加 `guid` 与 `list_sort_by`；新增 `widget_fields`（guid、widget_id、name、label、field_type、required、options_json、`form_ord`、`list_ord`、created_at）；`widget_records` 加 `guid`。三张表进 `SYNCED_TABLES`。**视图归属折叠进字段自身**：`form_ord` / `list_ord` 为 NULL 即表示不属于该视图，非 NULL 即顺序值——这样"哪些字段在哪个视图、按什么序"不再是一个会冲突的数组，而是字段自己的属性。两个视图各自保留独立顺序（实测 LLM 确实会给 form 和 list 不同的字段序，不能合并成一个全局序）。`widget_schema_rejections` **保持设备本地**（产品证据，同 `audit_log`）。

**四条推演/读码暴露的必须处理项**：
- **FK 级联不写 oplog（读同步实现才发现）**：SQLite `recursive_triggers` 默认 OFF，本仓也没开，所以 `ON DELETE CASCADE` 删掉的子行**不会触发该表的 AFTER DELETE 触发器**。现在无害（两表都不同步），但一旦 `widget_fields` / `widget_records` 进同步，删组件会让对端**留下一地孤儿行**。故删除组件必须在 Rust 里**显式逐表删子行**再删主行，让每行各自产生 delete op——这与 §3.8 "Rust 写路径漏不掉"的既有姿态一致。
- **同名字段并发冲突是 G-Set 唯一不解决的情况**：两台设备同时给同一组件加 `notes`（一个 text 一个 number），并集会得到重名字段。规则：`(widget_id, name)` 唯一，保留 **guid 字典序小者**（确定性，不依赖 hlc），落败者**写入拒绝日志**而非静默丢弃——遵循隔离区那轮定下的「丢数据可以，静默丢不行」。
- **并集会撑破上限**：A 设备 12 字段 + B 设备 12 字段 = 24 > `MAX_FIELDS`。**接受超限为合法但降级的状态**：照常渲染全部字段，但禁止再加新字段。**不做确定性截断**——截断意味着丢用户亲手建的数据，比超限严重得多。故上限只在**本地新增时**强制，不在合并时强制；`validate()` 要拆成"新增校验"与"既有状态校验"两档。
- **`MAX_WIDGETS` 同理**：两台设备各建 8 个，合并后 16 个。同一规则——合法但禁止再建。

**风险分级不变**：加字段 = `safe`（不丢数据）；删组件仍 = `dangerous`。删字段/改类型仍然**不提供**，想改走"导出记录 → 新组件 → 按字段映射导入"，与 ⑧ 一致。

### 2026-07-20 第二步实现后记：定稿之外冒出来的三件事

定稿（上一条）里没写、实际动手才发现的：

1. **必须额外加一个 `ord` 列**。定稿设想按 `(created_at, guid)` 排字段序，但同一次 `insert_widget_definition` 里所有字段的 `created_at` **完全相同**，排序于是落到随机 guid 上——**同一批行在两台设备上会渲染出不同顺序**。是导出测试断言字段名顺序时炸出来的（期望 `item` 实得 `amount`），否则很容易带到线上才发现。规范序必须是一个显式的、随行同步的整数。
2. **新同步表有三件配套，少一件就静默失效**：① `CREATE UNIQUE INDEX ... ON x(guid)`，否则 `ON CONFLICT(guid)` 运行时报错；② 插入时显式带 `new_guid()`，否则触发器的 `WHEN NEW.guid IS NOT NULL` 会**静默跳过**该行（表现为"改了但对端没反应"，最难查）；③ 删除路径不能靠 FK 级联（见 PITFALLS 当日条目）。前两件都是本次实际踩到才补上的。
3. **一条旧测试拿 `widget_defs` 当"未来未知表"的占位符**，本次让它变成真表后测试红了。红是幸运的——它提醒了占位符选值的坑，见 PITFALLS 当日条目。

**验证方式**：Rust 侧新增 6 条测试，覆盖并发并集（两台设备各加一字段，双向同步后双方都有 4 个字段且老记录仍通过校验）、同名字段确定性裁决 + 落败者可见、超限后只读增长、加字段强制可空、v12 → v13 迁移不丢字段/视图成员/各视图顺序/sort_by 且迁移后的行带 guid、删除组件写出各子行的 delete op。前端按 AGENTS.md §3 在真 Chrome + mock-IPC 走查：「加字段」入口可见、enum 选项行的显隐用 `getComputedStyle` + `offsetParent` 验证（它带 `.frow` class，正是 2026-07-19 `hidden` 被 class 压过那个坑的形状，全局 `[hidden]{display:none!important}` 兜住了）、非法字段键报可见错误且不写库、合法字段落库后**真的出现在重渲染的表单里**（8 个控件全部可见）。质量门三绿：232 测试 / clippy 零告警 / fmt 无漂移。

## 2026-07-20 Phase 11（F19 第一条竖切）验收结论

对着 ARCHITECTURE §3.12 与 CHANGELOG 的声称逐条核对代码，不看 diff 描述。**结论：竖切按其自述的边界交付了，判完成**；发现两项收尾债，当日补完。

**核对通过的部分**（每条都落到具体代码/测试，不采信注释）：七类字段封闭且四处 `deny_unknown_fields` 齐全；`time` 是直接调 `routine::parse_time_of_day` 而非另写一份解析（复用声称属实）；拒绝日志存的是**原始** `raw_schema` 而非解析后的产物，这是它作为 v2 统计依据的前提；三表设备本地由 `store.rs` 的测试直接查 `PRAGMA table_info` 与 `sqlite_master` 硬断言，不是靠注释约定；`widget_delete` 的 Guard 链路测试验了"无 token 直调必失败 → 预览含级联条数 → 确认 → 审计两条"；云端不可用时明确返回"没有创建任何组件"而非退化建日程。级联删除依赖 FK，`PRAGMA foreign_keys = ON` 在每次开库时设置，且测试真的插了记录验删除后归零——这个经典坑没踩。

**两项收尾债**：
1. **导出漏了组件数据**——真缺陷，当日修复，见 CHANGELOG / PITFALLS 当日条目。
2. **前端 312 行未过 AGENTS.md §3 的验证门**——F19 加的是一整个 form CRUD + 排序 + 预览确认的交互面，但当时的 CHANGELOG 条目**没有声称做过任何前端验证**（对比 Phase 10 的条目明确写了"真浏览器 mock-IPC 走查"）。当日补做，方法与结果见下。

**补做的前端走查**（真 Chrome + `window.__TAURI__` 假实现，harness 在 scratchpad 未入库，形态同 2026-07-15 条目）。断言全部落在渲染结果上：固定「组件」入口实际渲染 211×41 且 `offsetParent` 非空；预览卡实渲染 464×355、七类字段全部画出、**确认前 `widget_defs` 为 0**（预览不写库这条是验过的，不是读代码推的）；确认后表单生成的原生控件类型为 `text / number / date / time / datetime-local / checkbox / select`，填入后回传给后端的值是 `2026-07-20` / `07:30` / `2026-07-20T07:35`，与 Rust 侧 `%Y-%m-%d` / `%H:%M` / `%Y-%m-%dT%H:%M` 严格解析逐一对上，`weight` 回传为 number 而非字符串；排序四种组合（weight/move × 升/降）DOM 行序全部正确，含中文拼音序；必填留空提交得到可见错误「缺少必填字段 "move"」且记录数不变；编辑预填正确、保存后列表重渲染；删除记录后行数 2→1；点「删除组件」后 `widget_defs` 数量不变（确认它走 Guard 而非直删）；390px 视口无横向溢出、控件全部可见。

**过程中的一次误报**：排序一度被判定为"状态变了但列表不重排"，实为 harness 状态跨调用污染，已撤回并记入 PITFALLS 当日条目——这条比排序功能本身更值得留存。

**二轮补充（同日）**：第一轮结论"声称全部属实"成立但**范围不够**——它只核了 ARCHITECTURE/CHANGELOG 的声称，没回核设计稿的原始约束清单。回核后发现「组件总数 ≤ 8」（设计稿 ⑥）从未实现，已补（见 CHANGELOG / PITFALLS 当日条目）。另定性：`pa-cli` 无 widget 命令属**有意**，但其可测性后果记入 §3.12 并给了只读 `pa widgets list` 作为第二步的缓解方向。

**未纳入本次验收**（属第二步范围，非缺陷）：`table` / `stat` 视图、grid/chart、动态导航、同步、events 导入/反向提升、schema 演进。**次要观察**：组件名不去重，可存在两个同名组件；预览存在内存中，app 重启即失效——但因为尚未写库，这是安全的失效方向，未改。

## 2026-07-19 同步前向兼容：为什么是隔离区，以及为什么"版本协商"这条路是死的

起因是给 Phase 11（F19）做风险推演时发现的：F19 要新增 `widget_defs`/`widget_records` 两张同步表，
而**任何新增同步表都会让版本落后的设备同步永久卡死**（机制见 PITFALLS 当日条目）。这不是 F19 的 bug，
是现在就存在的潜伏问题，只是至今没有版本不齐的设备去触发它。PA-harmony 那个 ArkTS 重写版一旦接同步，
它的表支持进度必然与主线不同步，这条对本项目不是假设。

**先否掉我自己最初的建议——服务端版本协商**。它在本架构下**结构性不可能**：relay 只过密文 blob
（§3.8，服务器解不开也不留明文），它根本不知道 blob 里装了哪些表，**无法按能力过滤或分发**。
同理"按表分通道"也死在这条上——分通道要求服务端认识表名，等于要求明文。E2E 加密和服务端智能路由
天然互斥，这是设计的代价而不是疏漏，认了。所以只剩客户端侧前向兼容一条路。

**再否掉"未知表直接跳过"（一行改动的诱人方案）**：不卡死了，但**静默丢数据**。旧设备跳过该 op
并推进游标，等它日后升级，那条 op 已经被游标越过——**永远补不回来**。组件在那台设备上永久缺失，
且没有任何迹象。这比卡死更坏：卡死至少还能被发现。

**选定隔离区（quarantine）**：读不懂的 op 原样暂存进设备本地表，游标照常前进，升级后重放。
成本是一张表 + 一个 migration 钩子，换来的是**前向兼容变成结构性能力**——此后每加一张同步表都免费，
不必再逐次考虑这个问题。F19 和被暂缓的每周重复 routine（本文件 2026-07-18 条目列的前置条件
「先设计按设备能力的同步协商或最低客户端版本策略」）都因此解锁。

**两个实现取舍**：
- **有界且可见**：上限 5000 条，溢出丢最旧的并累计到 `meta.sync_quarantine_dropped`，CLI 非零即告警。
  长期不升级的设备必然丢数据，这没法避免；能避免的是**悄悄地丢**。
- **重放后无条件出队（表已认识的前提下）**：应用成功、被 LWW 判负、或永久不可应用（如 `soulous_facts`
  的来源校验拒绝），三种结局都终局，留着只会在隔离区里反复失败。仍不认识的表才继续留存。

## 2026-07-19 Phase 11（F19）设计稿：三项未拍板的结论 + 推演暴露的四项补充

按 2026-07-18 立项讨论列的三个未拍板项逐条定稿。定稿方式是**拿一个具体组件（"课表式日程表"）
把流程从头推演一遍**，而不是抽象讨论——这轮推演直接推翻了两条原本自认为想清楚了的设计，
证明"举一个真例子跑一遍"比继续空谈通用性有效。

**① 字段类型集合 → 封闭七类**：`text` / `number` / `date` / `datetime` / `time` / `bool` / `enum(options)`。
其中 `time`（不带日期的纯时刻，存 "HH:MM"，复用 `routine.time_of_day` 的格式与解析）**是推演逼出来的**——
最初只列了六类，跑到"课表 15:00"才发现纯时刻无处安放，LLM 只能退回 `text`，导致排序变字典序
（`"9:00" > "15:00"`）、筛选聚合全废。**明确排除三类**：公式/计算字段（= 表达式求值器 = 一门语言，
直接违背"声明式而非可执行"这条已拍板的关键路径；聚合能力放视图层的固定算子，不放 schema 层）、
关联字段（外键 → 级联删除/孤儿记录/跨组件同步顺序，工程量翻倍）、数组与嵌套对象（解释器无法为其
生成合理表单控件）。类型集合按"多踩坑再加"演进，配套机制见 ④。

**② 视图清单 + 渲染模型**：完整 F19 v1 的候选为 `form`（按 schema 生成的增改表单）、`list`（可排序/筛选）、
`table`（与 list 共用数据绑定，增量成本低）、`stat`（单个聚合数字）；但 **Phase 11 第一条竖切只交付 `form` + `list`**，`table` / `stat` 明确留第二步。**不给 `grid`（二维网格）和 `chart`**，
两者各需一整套坐标/刻度语义。**要有意识地接受的后果**：视图清单直接决定"能生成什么"——v1 没有 grid
就意味着生成不出网格状组件，包括触发这轮讨论的课表本身。这是产品选择不是技术限制。
**渲染模型是本 Phase 最重要的一条决定：新建独立渲染器，不扩展 F18 信封**。`genui.rs` 是
`MAX_COMPONENTS=12`/`MAX_TEXT_LEN=4000` 的一次性快照信封（props 进、DOM 出、不回头看数据），
F19 要的是"查 `widget_records` → 渲染 → 数据变了重渲染"。硬塞进前者的代价是**每次动 F19 都可能
崩到聊天主链路**。宁可两套渲染器有重复代码，也不接受这个耦合。F18 一行不改。

**③ F7 风险分级 → 按「结构 vs 数据」一刀切**：增删改**记录** = `safe`；创建组件、加字段 = `safe`；
**删除组件（连带全部记录）= `dangerous`**（Guard 确认 + 一次性 token）；删字段、改字段类型 = `dangerous`。
一句话规则：**动结构且会丢数据的走 Guard，动数据条目的不走。** 比逐操作枚举好记，也不会随视图清单
扩大而失效。

**推演暴露的四项补充（原清单里没有）**：

- **④ 意图路由必须先解决，且允许 LLM 参与**。"帮我弄一个日程表组件"与 F1 的事件摄取在措辞上几乎同构，
  不新增意图的话最可能的结局是**建了一条标题为「日程表组件」的日程事件**。用户拍板：允许 LLM 参与路由判断。
  理由是**通知文本是第三方输入（不可信）、用户主动打的字是本人在说话（可信）**，两者信任姿态本就不同，
  让 LLM 看不越注入线。更关键的是这条与 ⑤ 咬合：**有预览确认兜底，路由错了也只是给用户看一个
  他不想要的预览**，爆炸半径为零。两条不是独立决定，是一个。
- **⑤ 必须有预览-确认环节，不能一句话直接落库**。schema 校验只管**合法性**不管**是不是用户要的**——
  被拒后 LLM 会不断降级到白名单内的东西（`grid` → `list`），最终产出"schema 合法、形态不对"的组件，
  全程无异常。这与 2026-07-18 记的 LLM_ACTIONS 那条**根同**（`validate_action` 只做 schema 校验
  不做语义校验，"文案说对、id 指错"静默通过），区别是那条能靠"不给 LLM id 生成权"解决，
  **这条没有对应解法**——不可能让规则层预先生成"用户想要的视图形态"。唯一现实的缓解是把语义校验交回给人。
- **⑥ 拒绝日志要落库**。配合"拒绝不修补"的严格校验（硬上限：字段数 ≤ 12、视图数 ≤ 4、组件总数 ≤ 8，
  超限或引用不存在字段一律整体拒绝，不做"尽力解析出能用的部分"——半个 schema 落库比失败危险）。
  每次被拒的 schema 连同理由存下来，攒一个月就是一张**按频次排序的"缺哪些类型/视图"清单**，
  ①② 的演进照它走。否则"多踩坑再加"只存在于记忆里。
- **⑦ 数据孤岛：接受，但给两条显式通路**。通用组件读不到已有 `events`，这是结构性后果不是 bug；
  让它能读等于给通用解释器开跨表访问权（权限、字段映射、同步顺序，另一个量级）。给的是
  **A. 创建时一次性导入**（建组件时问"要不要从已有日程导入 N 条"，确认后**拷贝**进 `widget_records`，
  此后各走各的，解决冷启动空表单）和 **C. 反向提升**（组件记录"提升为日程"，照抄通知回看已有的
  `promoteNotificationCapture` 交互）。**否决 B（实时只读查 events）**：要定字段映射，且撞 F12 红线——
  台账删一条，组件里那行跟不跟着消失？答"跟"要实现级联，答"不跟"破了「删除即从语料消失」。
  **A 是快照不是链接**，UI 必须写明"导入自日程（快照）"，日程改了组件不会变。
- **⑧ schema 演进只允许加字段（可空），不允许删字段/改类型**。想改走"复制数据并重建"
  （导出记录 → 新 schema → 按字段映射导入，映射不上的显式提示丢弃，不静默吞），大后期再考虑
  通用迁移框架。**要预先如实告知的粗糙点**：「enum 单选改多选」在用户眼里是微小调整，
  在实现上是改类型，会撞这条墙——推演里最普通的使用路径上就撞到了。

**启动门槛**：本设计稿定稿不等于开工。真 blocker 是同步前向兼容（见上一条），已于本日修复；
Phase 10（schema v10）落地未满一日，核心稳定这条仍建议再泡几天真机。

### 2026-07-19 F19 第一条竖切实现取舍

本次只把「一句主动输入 → schema 预览确认 → 本地组件记录 CRUD」打通：视图实际只落 `form` 与 `list`，不以「设计中提过」为理由顺手实现 `table` / `stat`；也不实现动态导航、同步、从 `events` 导入/反向提升或 schema 演进。这样可以先验证**声明式 schema + 严格整体拒绝 + 人工语义确认**这一条关键路径，而不为尚未验证的数据模型预先承诺兼容性。

`widget_defs`、`widget_records` 和 schema 拒绝日志均保持设备本地：不加 guid、不触发 oplog、不写 `SYNCED_TABLES`。这不是遗漏同步前向兼容隔离区，而是本条竖切明确不引入跨设备语义；第二步若接同步，才以 §3.8 的隔离区为地基另行设计。

## 2026-07-19 AGENTS.md 补规：前端验证的断言必须落在渲染结果上

Phase 10 九项修复的复验里，唯一漏网的一条（Android 专属按钮在桌面并未真正隐藏）具备一个共同特征：
**代码对、diff 对、属性断言也对，只有渲染结果不对**。`el.hidden = true` 确实设上了，`hasAttribute("hidden")`
返回 true，但 `.btnrow { display: flex }` 压过了 UA 的 `[hidden] { display: none }`，按钮照样 460×30 渲染。
如果走查只断言了"我的代码有没有执行"，这类 bug 会 100% 绿着通过。

因此在 AGENTS.md 「完工后」第 3 条里加硬要求：断言取 `getComputedStyle().display`、
`getBoundingClientRect()` 宽高、`offsetParent === null` 这类**用户可见的渲染事实**，而不是属性/类名这类**作者意图**。
这条与既有的「没报错不等于画出来了」（SVG 图标全灭那次）是同一个教训的两个面，一并在条目里点了名。

选择写进 AGENTS.md 而非只留 PITFALLS：PITFALLS 是"这个坑长什么样"，靠关键词检索命中；
这条是"验证方法论本身有缺陷"，必须在每次开工/完工时被无条件读到，留在 PITFALLS 会被漏检。

## 2026-07-19 Phase 10 真人走查修复取舍

**P0-2 选择 (b)：保留「恢复处理」，但严格限定为本机重跑。** F12 的恢复入口对「判重/过滤」行本来是用户取回控制权的机制，直接隐藏 `local_only` 行会让同一入口在最需要解释隐私边界时变成不可见的例外。因此保留它，但恢复时明确写「仅重跑本机规则，不会发送到云端」；若本机规则仍无法确定，第二轮写出「仅本机规则未能确定……不会发送到云端」，不再伪装成可由重试解决的普通失败。`local_only` 戳不变，流程仍先于云端分诊分流，零 LLM 调用是回归测试锁死的不变量。

**P1-6 用终态 `resolved`，不复用 `NeedsReview`。** `NeedsReview` 只能表示「尚待用户处理」；接受过滤/改期/取消提议后仍把源通知放回这个状态，会同时显示琥珀提醒和「恢复处理」这种错误入口。数据库保存的是字符串状态，新增值可被旧库直接读取；前后端映射同步为中性「已处理」。没有迁移，也不修改历史既有行。

**P1-5 不加持久化列。** 动作提议已有 `event_id`，F12 状态接口在读取时查询当前事件并附上只读标题/开始时间投影；事件已不存在就返回空投影并让卡片降级为「原日程已不存在」，不能因一张陈旧卡片让整块面板报错。

**P2-7 预设清理只依据仍可识别的身份。** 当前没有编辑预设规则的 UI 或 API；`priority_presets(pkg)` 每次产生稳定的 `preset:{pkg}:...` id，而用户新建规则固定是 `user:notif:...`。停止捕获只删前者，后者完全不碰。若将来允许编辑预设，编辑动作必须把规则转换成用户身份或记录显式 provenance；在那之前，同 id 即代表未改动的默认预设，删除没有歧义。

## 2026-07-19 Phase 10 验收：两个发现（一个已修，一个待决）

**发现一：全局开关 vs 逐行凭据的混淆（已修，见 CHANGELOG 当日 Fixed）**

根因值得单独记，因为它是个**会重犯的类别错误**：`local_only` 是"这一行在采集时刻被许诺过什么"的**不可变凭据**，`notif_cloud_enabled` 是"云端现在开没开"的**可变全局状态**。两者在开关翻转的瞬间分岔，任何"要不要外发"的判断必须查前者，全局开关只能作为附加条件。

**同仓库里做对的范例**：`store::list_recall_events`（store.rs:2839）用全局开关选 SQL 分支，但**两条分支都带 `WHERE local_only = 0`**——全局 AND 逐行，双保险。`persist_event_with_scope` 也一路透传 `capture.local_only`。整个仓库只有 `process_notification_records` 一处漏了逐行那一层，所以评审和测试都没抓到。

**为什么原测试没抓到**：`notification_intelligence_never_calls_llm_when_notification_cloud_is_off` 在翻开关**之前**先跑了一次 `process_notification_batch`，把 `local_only` 的 queued 行排空成 `NeedsReview` 了，再捕获新行。恰好绕开了"关着攒 → 直接开"这条路径。**教训：测开关类不变量时，必须显式覆盖"状态切换时的存量"，只测稳态两端会漏掉迁移态。**

**发现二：`notif_cloud` 一个开关捆了两件风险量级不同的事（未修，待用户决策）**

`local_only = 0` 同时是两件事的启用条件（store.rs:382 / 482–499，三张表 `raw_inputs`/`events`/`notifications` 的同步触发器条件）：
1. 通知文本可作为**云端 LLM 上下文**——明文发给用户自配的第三方厂商（小米/DeepSeek/OpenAI…）。
2. 通知及派生数据**参与多设备同步**——端到端加密 blob 发往用户**自建**的 sync-server，服务器解不开、不留明文（§3.8）。

这两件事的隐私风险不是一个量级，却被同一个开关捆死。**后果：用户想要"设备间同步通知"但"别喂给 LLM"时，现在无法表达**——要防 LLM 就得连自建同步一起牺牲。命名也误导："上云"在 §3.8 语境里明确不包括自建中转，叫「通知上云」却顺带关掉了同步。

用户 2026-07-19 明确表达过要跨设备同步，因此这个耦合与其实际需求冲突。**已于当日拍板并落地（schema v10，见 CHANGELOG 当日 Changed）**，但拍法比我建议的更简单：不加第二个开关，**同步无条件常开、不给关**——用户原话「多设备同步是全都要的不能关」。少一个开关就少一处要解释的语义，比对称的双开关更好。

**落地时暴露的隐藏问题（值得单独记）**：sync payload 里**从来不含** `local_only`，接收端 `apply_remote_ops` 是靠 `text.starts_with("[通知·")` **重新猜**的（store.rs:2504）。在旧语义下通知根本不同步，这行是防御性死代码，猜错也无害；一旦同步打开它立刻变成关键路径，且**必然在一个方向猜错**——捕获时明明允许上云的通知，到了对端会被前缀猜成禁止，用户在 A 机同意过的上下文在 B 机静默失效。反方向（捕获时禁止的被猜成允许）只是恰好被前缀规则挡住，属于侥幸而非设计。故改为**让戳随 payload 走**，接收端采用捕获设备的原始判定，旧 payload 缺字段时才回退到猜测。教训与本文件当日第一条同源：**任何"要不要外发"的判定都必须绑定到那条记录的采集时刻，不能靠事后重新推断，也不能靠当前全局状态。**

## 2026-07-19 提交纪律改回自动 commit（push 仍需明确要求）

用户拍板：**完工后自动 commit，不必再问**。AGENTS.md「三、6 不越权」原文「未经用户明确要求不 commit、不 push」拆成两条——新第 6 条讲提交纪律，第 7 条只留"不越权改代码 + 如实汇报"。

**边界（重要，别扩大解释）**：
- 自动的只有 `commit`，**`push` 仍需用户明确要求**。推送是对外动作，一旦推上去回滚成本完全不同，不进自动范围。
- 前置条件不变：质量门三绿（test/clippy/fmt）+ 三件套留痕做完，才谈提交。不是"改完就 commit"，是"完工才 commit"。
- **提交前必须 `git status` 看清工作区**。本仓有并行会话的历史（见 CHANGELOG 2026-07-17「并行会话落盘的四处修复」），本次文档批次执行时工作区里就躺着一个不属于本任务的 `crates/pa-core/src/orchestrator.rs` 改动。只 stage 本次任务的文件，**禁止 `git add -A`**——这是自动提交唯一真正的风险点。

**沿革**：项目早期规则就是"每完成一个功能块自动提交"，后被 AGENTS.md 收紧为"未经要求不提交"，现改回自动。注意 PA-harmony 仓一直保留自动提交，两仓规则此前刻意不同，现已重新一致（但 harmony 仓的 CLAUDE.md 措辞未同步改，那边本来就是自动，无需改）。

## 2026-07-19 根目录文档过时排查 + PRODUCT/DESIGN 迁入 docs/

**起因**：用户要求全面排查 PA 文档过时情况（不限 `docs/`，含根目录 md）。核实结论：**`docs/` 全部是 07-19 更新的、与代码一致；过时集中在根目录**——README 的下半部分（Soulous 章节、`pa notif-cloud` 命令）在 07-19 更新过，但顶部「当前进度」摘要和各处统计数字没跟着改，属于典型的"改了正文忘了改摘要"。

**核实出的硬错误（均已修）**：README 写「Phase 1–7 全部落地」（实际 Phase 8.1/8.2 + Phase 9 已完成）、「189 个测试」（实跑 **201** 全绿：193+4+4）、「40+ 个 tauri command」（实际 **56**）、「12 视图」且列表漏了「隐私」「云端」（实际 **14**，5 组导航）、`docs/` 目录清单漏 `PRIVACY.md`、pa-core 模块表漏 `persona.rs` 与 `export.rs`、「9 家厂商预设」（实际 8 家 + 自定义端点）。

**结构调整（用户拍板）**：`PRODUCT.md` / `DESIGN.md` 从根目录 `git mv` 进 `docs/`——理由是"以 docs 为准"，根目录只保留必须在根的三份（README 是仓库门面、AGENTS.md 是智能体规范入口、CLAUDE.md 被 Claude Code 从项目根读取）。同步改了引用：AGENTS.md 开工前第 3 条、`dist/index.html` 语义色注释。MISC 早前条目里"根目录两个新文件"的表述属历史记录，**不回改**。

**DESIGN.md 复核结论**：逐 token 对过 `index.html` 的 `:root`——14 个颜色 token 的明暗两套值、圆角四档、z-index 四档、ease 曲线**全部一致**，没有漂移。只是没覆盖 07-15 之后新增的流式气泡与 `.qmark` 说明按钮，属补充而非错误。

**PRODUCT.md 定位冲突（已修）**：Positioning 原文"隐私完全归你……不是把你的生活上传给云端的助手"与当日「红线重划」（通知上云默认开启）直接矛盾。按"以 docs 为准"改为"隐私边界归你掌控"，并明确指向 `PRIVACY.md` 为准、不在 PRODUCT 复述细则——避免两处各说一套再次漂移。Product Purpose 里"一切原始数据只在本地"同步改为"数据默认只在本地、外流边界由用户自己掌控"。

**`gpt-5.6` vs `gpt-5.2` 不一致**：README 示例写 `gpt-5.6`，前端 `LLM_PRESETS` 写 `gpt-5.2`。docs 里没有依据可判定，用户拍板**以 `gpt-5.6` 为准**，已改前端预设。

**未动的两项**：
- `PA_demo_script_v2.md` 留在根目录不动——排查中发现它在 `.gitignore:46` 里，和 `PA_demo.mp4` 同待遇，**本来就不是仓库文档而是本地演示素材**，移进 `docs/` 反而会让 ignored 文件混进受版本控制的目录。
- `demo_delete` 模拟高危工具仍保留。本文件 2026-07-17 条目留过"评委期结束后可评估移除"的待办，时间点已到，但本次范围只限文档，未动代码。

## 2026-07-19 README Build Week 章节归档

评委期已结束，README 顶部那段面向 OpenAI Build Week 评委的英文章节（约 55 行）从 README 移出存档于此，README 只留一行指路。保留原文的理由：它记录了当时**如实限定 Codex 使用范围**的决定（延续 2026-07-15 拒绝包装造假的立场，宁可少拿"Codex 使用深度"分也不误导评委，见本文件 2026-07-17 条目），这个诚实红线的执行证据有留存价值。

以下为原文照录（技术描述反映的是 2026-07-17 前后的状态，**不随代码更新**，勿作为当前事实引用）：

---

### For OpenAI Build Week judges (English)

**PA (Personal Agent)** is a privacy-first personal agent — Rust core, Tauri 2 desktop + Android shell, no Electron, no npm at runtime. It turns a spoken sentence ("meeting with Zhang Wei at 3pm tomorrow") into a structured event with importance-graded reminders, keeps a behavior journal via gentle check-ins, learns habits into recurring routines, recalls what it knows about you into every conversation, and renders its replies as **interactive Generative UI cards** whose button/form actions flow back into real backend state. All personal data stays in a local SQLite database; the cloud sees one sentence at a time, never your journal, ledger, or persona. Every destructive action goes through a human-in-the-loop guard with one-time capability tokens and an append-only audit log.

#### Quick start (Windows)

Requires stable Rust (developed on `x86_64-pc-windows-msvc`; SQLite is bundled and compiled from source).

```bash
cargo test --workspace            # 189 tests, fully offline (LLM is faked in tests)
cargo build                       # produces target/debug/pa(.exe) and pa-app(.exe)
cargo run -p pa-app               # desktop GUI (needs WebView2, bundled with Win11)
```

#### Sample walkthrough (deterministic, offline, no API key needed)

The CLI takes an injected clock (`--now`) so every step below reproduces exactly:

```bash
pa --now 2026-07-20T08:00:00 add "明天下午3点在会议室和张伟开会"   # NL → structured event
pa --now 2026-07-20T08:00:00 add "下周五上午九点期末考试"          # exam → higher importance
pa --now 2026-07-20T08:00:00 agenda                              # schedule view
pa --now 2026-07-20T08:00:00 daily-brief                         # Daily Focus Brief (Codex-built)
pa --now 2026-07-21T14:30:00 fire                                # reminder fires (meeting: 30 min ahead)
pa --now 2026-07-20T08:05:00 add "把明天的会改到下午4点"            # NL reschedule: new clock time, same date, reminders re-planned
pa add "记住我对花生过敏"                                          # semantic memory write
pa recall 过敏                                                    # what would be recalled for a query
pa ledger                                                        # full memory ledger — see & delete anything
pa export --out my-data.json                                     # everything you own, one JSON, never leaves the machine
pa stats                                                         # offline data review (baselines, habit clusters)
pa guard-demo ledger_purge '{"layer":"behavior","before":"2026-07-17T00:00:00"}'
                                                                 # HITL guard: real preview → confirm → one-time token → audit
```

#### How we used Codex

The **Daily Focus Brief** feature was built end-to-end with OpenAI Codex during Build Week: a read-only aggregation that pulls today's remaining schedule, due/upcoming reminders, and top pending suggestions into one prioritized card. Codex implemented:

- `crates/pa-core/src/brief.rs` — pure aggregation core (`build_brief`) + text renderer, mirroring the existing digest module's shape;
- `Orchestrator::daily_brief()` — wiring over existing store queries only (no new write paths);
- `genui::daily_brief_prompt()` — the F18 Generative UI envelope composed strictly from the existing component catalog and action whitelist;
- three entry points: CLI `pa daily-brief`, Tauri command `daily_brief`, and a once-a-day resident-ticker push rendered in the chat view;
- unit + integration tests for the window rules, the GenUI JSON round-trip, and the empty-day case.

The Codex `/feedback` session ID is provided in the Devpost submission form. Everything else in this repository predates Build Week and was built with other tooling — we're submitting the Codex-built feature honestly rather than relabeling old work.

#### How GPT-5.6 is integrated

PA's cloud reasoning goes through an OpenAI-compatible gateway (`crates/pa-core/src/llm.rs`). Point it at OpenAI and PA runs on GPT-5.6 for every cloud path:

```json
{ "base_url": "https://api.openai.com/v1", "api_key": "sk-…", "model": "gpt-5.6" }
```

(save as `pa-llm.json` in the repo root — gitignored — or use the GUI's Settings → Cloud panel, which has an OpenAI preset; it knows GPT-5 series models reject non-default `temperature` and omits the field automatically.)

GPT-5.6 is used for: chat replies with **Generative UI envelopes** (the model composes interactive cards from a whitelisted component catalog — it can request actions, never execute them), extraction fallback when the offline rule parser is unsure, and persona-toned rewriting of the weekly digest (numbers are validated locally; any tampering falls back to the offline original). By design the model receives only the current sentence, the current time, a short in-memory conversation window, and a recall snippet you can audit yourself with `pa recall` — never the raw journal, ledger, or third-party notification text. If the cloud is unreachable, every feature degrades to the offline deterministic path; reminders never depend on the network.

---

## 2026-07-19 Phase 10 实现取舍：通知智能管线

**落地范围**：F20 以 `notification_intelligence` 纯逻辑模块 + schema v9 的本机回看表实现。App 白名单默认空，是**捕获源头总阀**：Android `PaNotificationListener` 在读取 extras 前只认私有目录里的白名单策略，未授权 App 不进 inbox、不落 raw input、不进规则或 LLM。白名单是设备本机偏好，不能被同步端静默打开。

**双车道与离线地板**：重要规则复用 `RuleTable`（包名可选作用域、子串/正则；允许微信/QQ/钉钉时只填入可编辑预设），命中后即时处理；普通通知由宿主内部 15/20/30 分钟定时器最多 24 条一批处理。为保证规则先行，先用本地抽取器创建能确定的事件；只有不确定项且 `notif_cloud` 已开启才做一次 LLM 调用。关闭开关时整个管线纯本地、零 LLM/零外发；云端失败则留下 `needs_review`，绝不以「没结果」为理由丢通知。

**去重、过滤与动作边界**：同包名 + 规范化内容的 FNV-1a hash 在 10 分钟窗内确定性去重。LLM 的语义「这类通知无价值」只能提出过滤规则，先以 `pending` 提议显示，用户确认才变成本地规则；任何过滤/判重/失败都进入 F12 `notification_capture` 层，可恢复或提升为事件。对已有记录，LLM 协议只允许无 id 的 `cancel_event` / `reschedule_event` + 标题提示；Rust 只有在本机唯一匹配 event 后才保存 F12 确认卡，点按前绝不执行。危险操作仍走 Guard，本批不扩张 `LLM_ACTIONS`；来源 App 仅作路由信号，绝不作授权。

**同步边界**：Phase 10 回看表、过滤提议和 Rust 已解析但仍待确认的动作提议只保存本机分诊元数据，避免为「当前设备的通知处理历史」另造同步语义；第三方通知的 raw input、event、notification 仍沿用 Phase 9 的**捕获时** `local_only` scope。换言之，另一台设备可按 Phase 9 获得已允许同步的原始/派生数据，但不会获得本机的队列、判重、过滤或确认历史。`notif_cloud` 和 App 白名单都不进 meta LWW；重要规则复用已有 RuleTable 配置行为。

**Android 取舍**：不用 WorkManager——重要通知不能接受其周期下限，普通队列也需要和即时车道共享活体；改用 `dataSync` 前台服务（仅白名单非空时启动）+ 内部定时器，声明 `FOREGROUND_SERVICE` / `FOREGROUND_SERVICE_DATA_SYNC` 与 `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`。代价照实暴露在设置页：常驻通知栏、耗电和用户授权摩擦；国产 ROM 仍可能杀服务，故提供电池优化和应用后台设置入口，不能承诺「绝不被杀」。

## 2026-07-19 红线重划：通知文本允许出云端 + 参与同步（默认开，opt-out）

**背景**：F19「持久化自定义组件」讨论中，用户希望 widget 能自动抓取通知捕获的内容（"手动太麻烦"）。这与 §4「第三方通知文本永不出进程」及 schema v5 的 `local_only` 机制直接冲突。经讨论，用户决定**主动重划这条红线**，而非在旧约束下做 F19。

**决策（用户拍板）**：
- **通知文本允许发往云端 LLM，默认开启**（opt-out）。理由：个人自用图方便；开源发布后由使用者自行按需收紧（默认值一行可改）。设置页提供手动关闭开关。
- **通知派生数据允许参与多设备同步**：schema v5 的 `local_only` 强制排除退役/放宽，改为受上述开关控制。*（当日稍后再次修订：同步改为**无条件常开**、不再受该开关控制，见本文件同日「Phase 10 验收」条目发现二与 schema v10。）*
- **全部通知一刀切上云**，不按类别过滤（用户明确不要"仅日程类"这类缓冲）。
- **责任转移**：API 由用户自行 DIY 配置，主要数据责任推给所选 API 厂商；隐私政策写明用户后果自负。

**存证——用户被明确告知、仍决定放宽的一点**：
- "后果自负"覆盖用户自身，但**通知文本含第三方内容**（他人消息、机构通知），这些第三方从未同意上云——这正是当初画线及 F9 人格导入定"纯本地"的原因。用户接受此风险（单用户自用）。
- **护栏落在诚实披露**：隐私政策必须如实包含一句——"上传内容可能包含向你发送通知的第三方的信息"。这是"激进但诚实"与"误导用户"的分界，不可省略。见 `docs/PRIVACY.md`。

**注入线不随隐私线一起松（保留）**：
- 通知能上云 ≠ 通知内容可成为 LLM **动作**来源。F18 硬原则不变（UI 描述是数据不是代码、动作只能请求不能执行、textContent-only 渲染、`LLM_ACTIONS` 窄白名单）。通知/widget 内容进 LLM **只作上下文**，不解锁新写路径。

**爆炸半径（放宽要改的地方，供实现排期）**：
1. §4 第 1 条改写（本次已落，新增第 6 条声明开关）。
2. schema v5 `local_only`：通知捕获 raw input + 派生 event/reminder 的同步触发器/远端合并排除，改为读设置开关（§3.8）。
3. §3.6 LLM Gateway（现"每次只发当前一句话+当前时刻"）+ §3.10 记忆检索对通知来源语料的排除，随开关放宽。
4. 设置页新增开关（默认开）+ 新建隐私政策文档 `docs/PRIVACY.md`。
5. 已上传/已同步不可撤回：默认开意味着下个版本起立即开始外流，历史清理只能靠用户轮换同步密钥 + 清服务器（沿用现有条目结论）。
6. **文档一致性收口（Phase 9 完成）**：§3.8/§3.9/§3.10/§3.11-L1 已随开关代码统一改为"受开关控制"；本条保留原先的待办背景，不再表示未完成。

**顺序**：此红线放宽作为**独立、有记录的改动**先落（§4 改写 + 开关 + 同步/LLM 打通 + 隐私政策），**F19 排在其后**——F19 的"widget 抓通知 + 同步 + 云端抽取"整个踩在这块新地基上。

**Phase 9 实现收口（2026-07-19）**：`notif_cloud` 作为 SQLite meta（缺省 `true`）落地，但**不进入 meta LWW 同步白名单**——这是设备本地的外发偏好，不能由另一台设备静默改写。`ingest_captured` 仅在捕获时读取它：开启写 `local_only=0`，关闭写 `1`，raw input/event/notification 三表同 scope，原有同步触发器无需修改。历史数据决定按默认建议执行：**不回填**；从关切到开时，既有 `local_only=1` 行不翻为 0、不补 oplog/GUID，既不参与同步也不进入 recall。迁移只把旧 schema 的通知行保留为 local-only 一次，不能在每次打开数据库时误把 Phase 9 新捕获行重标回 1。开关关闭时 recall 立即继续排除所有通知来源；已经同步或发送的历史无法撤回，须按隐私政策处理。捕获路径仍是 `route_intent + RuleBasedExtractor`，本批没有接 LLM 分诊、F19、白名单或后台常驻。

## 2026-07-19 通知 LLM 分诊与动作边界：分诊 → 提议 → 确认（三档）

承上条红线重划。用户希望"让 LLM 过目通知、按来源+内容判断要不要动作"，更像助手，别什么都手动。讨论中澄清了"有动作校验还不够吗"的疑问，并把边界重新精确化。

**关键澄清——被禁的不是"LLM 判断"，是"攻击者文本静默执行状态变更"**：
- `validate_action` 只查形状（命令在白名单 + 参数 schema，如 id 是整数），**不查语义**（目标对不对、是不是用户本人要的）。一个注入进来的 `event_cancel {id:5}` 形状完美、能干净通过校验。所以校验是兜底，不是主防线。
- 真正的防线是**收窄源头**：`LLM_ACTIONS`（应为 `[ingest, checkin_answer]`）限死 LLM 能发的动作、带库 id 的动作由 Rust 规则查库生成（`cancel_confirm`/`reschedule_pick`）、危险级过 Guard token。
- 因此红线不是"LLM 不能读通知"，而是"**通知（攻击者可控文本）不能导致未经确认的、已执行的状态变更**"。LLM 读通知、判断、**提议**——安全（最坏是弹一张可无视的卡）；**静默执行**才是危险。

**三档模型（拍板）**：
| 通知触发的动作 | 处理 | 手动程度 |
|---|---|---|
| 建新数据（新加日程/提醒） | 爆炸半径低、可逆——LLM 可提议甚至自动录入（F1 规则版已在做） | 基本免手动 |
| 动已有记录（改期/取消/删，带 id） | LLM 只提议**意图**，Rust 查库解析真实 id，弹一键确认卡 | 一次点击 |
| 危险（支付/批量删） | 永远 Guard 一次性 token + 确认 | 一次确认 |
架构要点：**"判断要不要动作" = LLM 的活；"生成带真实目标的可执行动作" = Rust 的活**。LLM 全程不产出 id，故现有保证全不动，`LLM_ACTIONS` 未必要拓宽——加的是"通知→意图"分诊环节，不是放开动作生成权。

**现状事实（值得记，因为它决定这是"新能力"而非"改现有行为"）**：今天通知处理**零 LLM 调用**——`drain_capture_inbox` → `ingest_captured`（orchestrator.rs ~408）是纯本地：`route_intent` + `RuleBasedExtractor`，解析不了就丢，**没有** `ingest` 那条路（orchestrator.rs:173-177）的云端 `llm_extract` 兜底。所以"LLM 分诊通知"是**新增的后台云端调用**，今天不存在。

**三条决策**：
1. **通知 LLM 分诊是新能力，受"通知上云"总开关门控**；开关关 = 回到今天的纯本地、零 LLM、零外流。
2. **规则先行、LLM 只在拿不准时兜底调一次**（沿用现有"规则先行 LLM 兜底"模式），控成本 + 控外流，不是每条通知都无脑喂 LLM。
3. **LLM 的任何意图提议必须显式通知/展示，永不静默执行**；动已有记录/危险动作仍走 Rust 解析 id + 人确认。

**保留不放宽**：`LLM_ACTIONS` 不拓宽、id 由 Rust 生成、危险动作 Guard token 不变。**通知来源（发送方）可作 UX/排序信号，但不可当授权边界**——来源可伪造（任何 App 都能把标题写成「【系统】」），"来自 X 就自动执行"是错的；真正边界始终是"动已有记录/危险 → 人点一下"。

**顺序**：本条是设计边界，落在地基批（通知上云开关）与 F19 之间/之上；分诊环节本身属于地基之后的增量，不在最小地基批里强做。

## 2026-07-19 通知智能管线设计稿（F20：白名单 + 双车道 + LLM 分诊/写规则 + 后台常驻）

承前两条（红线重划 + 分诊/动作边界）。这是把 F1 通知摄取升级为一个完整"通知智能"子系统的设计稿。**设计稿，代码未动，排在"通知上云"地基批之后。**

**现状基线（决定这是新增子系统而非改现有行为）**：
- 通知处理今天**零 LLM 调用**（`ingest_captured` orchestrator.rs ~408 纯本地，无 `ingest` 那条的云端兜底）。
- 捕获层已较稳：`PaNotificationListener`（`NotificationListenerService`，系统绑定+自动重启，进程死了照写 `notif-inbox.jsonl`），已带 `[通知·{pkg}]` 来源标记。提醒有 `pa-alarm`（AlarmManager）兜底。
- 不稳的是**处理层**：`drain_capture_inbox` 跑在 app 进程，当前无前台服务/电池豁免/WorkManager/开机自启——app 死则 inbox 堆积不处理，直到重开。

**管线全景**：
`[App 白名单] → 捕获进 inbox（抗死亡）→ [后台常驻 B] 双车道 → LLM 提议(过滤规则+动作意图) → 规则便宜/离线执行 → 可见回看 + 人确认`

**已拍板决策**：
1. **App 白名单（默认空，opt-in）= 总阀**。用户主动挑哪些 App 的通知让 PA 接收，在 LLM/规则之前把量掐在源头。治噪声 + 隐私加分：与云端 opt-out 开关叠加后，**没加 App 之前零外流**，使"默认开启"体面。以后可挂 per-app 策略（只捕获 / 才上分诊）。
2. **后台常驻走 B（实时主动）**：前台服务或 WorkManager 周期任务 + 电池优化豁免（`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`）+ 可能 `RECEIVE_BOOT_COMPLETED`。**明确接受代价**：更耗电、常驻通知栏、权限摩擦、国产 ROM（MIUI/EMUI 等）仍可能照杀。选 B 是要 PA 后台关着也能主动提醒，而非等重开补课（A 方案否决）。
3. **双车道处理**：
   - **重要车道**：本地规则秒判 urgent（种子：`@我`/`@全体成员`；+ 用户自定义重要模式）→ **即时单条调 LLM**，不进批量队列。呼应 F2 按重要度差别对待的既有理念。
   - **普通车道**：攒进队列 → **定时批量调 LLM**，一次看一批以做**跨条去重**（单条规则做不到）。
4. **规则的角色 = 紧急度路由 + 离线地板 + LLM 结晶的过滤器**（不是 throttle）。规则不废：F16 离线兜底要求云端不可用时通知处理不停摆，提醒必须确定性不受 LLM 抽卡影响。**"LLM 写规则"采纳**：LLM 批量看后把判断结晶成便宜的本地过滤规则（贵思考一次、便宜跑一辈子），复用 §3.1 Importance Classifier 已有的"LLM 结果反哺规则表、人工确认后固化"模式。
5. **不静默丢**：被规则/LLM 过滤掉的通知进**可见"已过滤"回看列表**（万一把银行盗刷预警当噪声，能看见能捞回）；LLM 自动生成的过滤规则**生效前要确认 / 可撤销**（防注入：恶意通知诱导 LLM 写"过滤所有银行预警"来致盲用户）。
6. **动作边界不变**（承上条）：LLM 只提议不执行、带库 id 动作由 Rust 生成、危险走 Guard token、`LLM_ACTIONS` 不拓宽。通知来源可作 UX/排序信号，**不可当授权**（可伪造）。
7. **受"通知上云"总开关门控**：开关关 = 回到纯本地规则、零 LLM、零外流（离线地板仍在）。

**"何时调 LLM"地图**：App 不在白名单→不调；开关关→永不调；白名单内+命中重要规则→即时单条调；白名单内+普通→定时批量调（去重）。

**四项子决策（2026-07-19 已拍板）**：
1. **重要规则形态 = 复用现有 `rule_table` + 按 App 分预设种子 + 用户可加**。不造新存储，重要通知规则是 Importance Classifier 规则表里的新 kind。种子做成**按包名的预设库** `{pkg → [重要模式]}`（微信 `com.tencent.mm`:`@我`/`@所有人`；QQ `com.tencent.mobileqq`:`@我`/`有人@我`；钉钉 `com.alibaba.android.rimet`:DING/紧急/`@`；未知 App 回退通用集 `@我`/`@全体成员`）。规则 = `{pattern, 可选 pkg 作用域, priority}`，默认全局、可选 per-app。用户把某 App 加进白名单时若命中预设，就把那套种子作为**可改起点**填给他。**预设是 best-effort 起点、会随 App 版本改格式失效，必须用户可覆盖、不当权威**。匹配是廉价本地子串/正则，纯路由非安全边界，判错无害（假阳性多一次即时调用 / 假阴性落批量）。
2. **批机制 = 前台服务当骨干，批量跑在其内部定时器（15–30min 可调），不引 WorkManager**。重要车道要近实时、等不了 WorkManager 15min 地板，需常驻活体把 Rust core + 网络后台保活。前台服务的已知成本（选 B 的代价）：通知栏常驻一条、Android 14+ 要声明 FGS type（`specialUse`/`dataSync`）、配 `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`、国产 ROM 仍可能照杀需引导用户手动加白名单。
3. **去重/无价值 = 两层：廉价规则做确定性去重，LLM 做语义判定但结晶成规则**。精确/近似去重靠规则（同 pkg + 内容 hash + 时间窗 ~10min，确定/便宜/离线）；"这类通知对我无价值"是因人而异判断，LLM 批量时判、**提议过滤规则**、用户确认后固化 → 之后当便宜本地规则跑。不设硬数值阈值。护栏：被丢的都进可见回看（见下），LLM 自动生成的过滤规则**生效前要确认**（防注入诱导致盲）。
4. **"已过滤"回看 = 并入 F12 台账做独立一层，不做独立面板**。F12 是单一权威审计面（隐私信任锚），别碎成多面板。新增"通知"层展示每条捕获通知去向（→建事件 / 判重丢 / 判噪声丢 / 待处理），可溯源（来源 App + 哪条规则过滤）、可逆（撤回过滤 / 提升为事件）、删除语义同 F12。量大用层内筛选解决，不倒进主记忆列表。

**顺序与依赖**：地基批（通知上云开关，本文件当日第一条，✅ Phase 9 已落）先 → 本管线拆 **Phase 10a（Rust 双车道分诊逻辑，纯逻辑可测）+ Phase 10b（Android 前台服务保活 + 权限）** 降风险 → F19（Phase 11）复用本管线的捕获 + 上云地基，不各造一套。

## 2026-07-19 聊天流式（文字优先）的实现取舍

落地 2026-07-18「F18 后续方向」拍板的第一步「先把普通聊天文字改成流式」。几处关键取舍逐条备查：

- **只做纯文本流式，信封仍整包**：sniff-router 边收边判——输出以纯文本开头就逐 token 可见流式；以 `{` 或 ``` 开头（信封 JSON）就抑制可见流式，整包收完再走既有严格 `parse_envelope`。`parse_envelope` 一行未改（仍是整段配对花括号 + 严格反序列化，半截 JSON 直接失败）。组件级增量渲染（类 A2UI `updateComponents`）依旧明确延后，理由同 2026-07-18 条目。
- **prompt 从「永远信封」放松为「默认纯文本，有动作才信封」**：不改 prompt 的话流式几乎永不触发（模型输出总以 `{` 开头，全被 sniff-router 抑制）。放松后纯闲聊以纯文本开头、可流式，且渲染结果与旧「纯文本信封」完全一致（前端对纯文本信封本就渲染成纯文本）；仅当模型确实要给 `ingest`/`checkin_answer` 快捷动作时才输出信封。这没有削弱「动作只能请求不能执行」等 F18 硬原则（校验/白名单/Guard 全在信封路径上，纯文本路径本就无动作）。**已先在 ARCHITECTURE §3.6/§3.9 记录再改代码**（AGENTS.md §1.1）。
- **传输选 Tauri 事件而非 Channel**：复用仓库已在桌面/Android 双端验证过的 `app.emit`/`listen`（ticker 的 `pa-*` 事件），新增 `pa-chat-delta{stream_id,delta}`。Channel 是 Tauri 2 更"正统"的按调用流式原语，但在本仓是全新机制、Android WebView 下未验证；按「别踩新坑」原则先用成熟通道。`ingest` 命令仍返回完整 `IngestResp` 定稿（含信封），流式只是过程增量。
- **锁模型不变**：`ingest` 本就在 `spawn_blocking` 里全程持 `orch` 锁跑完整云端调用（2026-07-18 走查 1 的结论），流式总时长相同，不新增持锁开销；未拆细粒度锁。
- **think 过滤**：SSE 只读 `delta.content`，天然不碰 `reasoning_content`（provider 文档已确认 MiMo v2.5 常规不内联思考）；对内联 `<think>` 前导块仍做流式过滤，返回值与 `complete` 同为 think-stripped，保证「流出去的」= 「定稿的」。
- **pa-core 不依赖 Tauri**：流式回调是 `&mut dyn FnMut(&str)`，pa-app 侧包一层 emit；`complete_streaming` 默认实现回退到 `complete`，NullReasoner / 测试 reasoner 零改动。

## 2026-07-18 固定提醒在 F12 中可编辑，但历史 occurrence 不改写

- **边界**：`routine` 是用户维护的未来提醒配置，不同于已经发生的事件或行为日志；因此台账开放标题与每天时间的编辑，并保留暂停/启用。用户编辑后，尚未触发的 occurrence 会撤回并按新设置立刻重建；已经触发的 event/notification 保持原始标题、时间和状态，仍可审计。
- **取舍**：不把普通 event、行为日志或通知开放为同类「编辑」入口——那会篡改历史记录，仍应通过改期、snooze 或删除等各自明确的语义操作处理。也不为 routine 编辑新增 GenUI 动作：真实行 id 只在台账查询结果中可得，继续由本地前端入口携带，遵循 F18 的 id 生成边界。

## 2026-07-18 交互式生成（F18）后续方向讨论：三项拍板

针对 F18 GenUI 的后续演进方向（与 Claude 讨论，纯讨论未动代码）做了一轮梳理，结论：

- **LLM_ACTIONS 边界维持现状，且明确了"为什么"**：候选行的 id 产出权永远留在 Rust 规则层（`cancel_confirm`/`reschedule_pick` 这类已从库查出真实 id 的候选，才允许生成对应 `UiAction`），LLM 只能参与候选的重排/过滤/追问，不允许直接产出带 id 的 action。原因不是"AI 能力不够"，是 `genui::validate_action` 只做 schema 校验（id 是否为整数）不做语义校验（该 id 对应的行是否就是文案描述的那件事）——一旦 LLM 直接写 id，"文案说对、id 指错"这类错误会静默通过校验并执行，且没有任何现有机制能拦截，比自由文本错误（可见、可撤销）危险得多。真要解决"抠字眼误判"的问题，应该走"规则生成候选 + LLM 重排/解释"这条路，不是放开 id 生成权。
- **流式渲染：文字优先，组件树流式往后放**。现状是连纯文字回复都不是流式的（`llm.rs::chat_reply_ui` 用 `r.complete()` 一次性拿完整字符串，§3.6 "同步阻塞式 ureq"）。计划分两步：先把普通聊天文字改成流式（工程量可控，MiMo 是 OpenAI 兼容协议大概率原生支持 `stream: true`），GenUI 信封本身因为有硬上限（≤12 组件/4000 字符）体积很小，完整渲染的延迟在文字流式打底后大概率已经可接受；组件级增量渲染（类似 A2UI 的 `updateComponents`/`updateDataModel`）需要重新设计 `parse_envelope` 的解析方式（现在是整段配对花括号后严格反序列化，半截 JSON 直接失败），列为明确的后续工作，这次不做。
- **组件目录/自由布局：目录扩大延后到核心功能稳定+真实需求出现，自由样式/HTML 生成被否决**。否决自由布局的原因**不是**隐私泄露（"本地应用不怕泄露"这条站不住——webview 里的注入代码一样能 `fetch` 到公网，"数据不出本地"是应用层的数据流选择，不是 webview 进程的网络沙箱），也**不是**"危险按钮伪装成主按钮"（这条被推翻：`guard_request` 无论样式如何，点击后仍要走 §3.3 的确认弹窗+一次性 token，样式最多让人多点一次，拦不住真正的执行）。真正的原因是 **Tauri IPC 边界**：`genui.rs` 现在的渲染方式是"JSON → DOM，`textContent` 赋值不用 `innerHTML`"，一旦允许非纯文本内容（HTML/CSS），任何最终落进 `innerHTML` 或 `<style>` 的字符串只要含 `<script>`/`onerror=` 之类构造就会被当代码执行——而 Tauri 的 `window.__TAURI__.core.invoke` 桥不区分"合法前端代码"和"页面里意外跑起来的 JS"，注入代码可以直接绕过 `genui.rs` 全套白名单+校验，对 `safe`/`sensitive` 级命令（危险级有 token 门槛挡着）为所欲为。PA 大量摄入第三方文本（F1 通知捕获、F9 聊天记录导入、云端 LLM 转手输出）正是这类注入的典型来源，`textContent`-only 是结构性防线，不打算放开。组件目录本身的扩大（新增卡片类型，仍是受限 JSON schema）不受此影响，按 `daily_brief_prompt` 已验证的模式随时可以做，只是优先级排在其他核心功能之后。

## 2026-07-18 立项讨论：F19 持久化自定义组件（"钱包小组件"设想）

用户设想：一句自然语言（"帮我弄一个钱包小组件"）现场生成一个持久存在的功能模块——独立导航入口、独立数据存储、带增删查改。这与 F18 GenUI（一次性对话卡片，"对话即焚"不持久化）性质不同，是新方向，暂列 ARCHITECTURE.md F19（讨论稿，未拍板实现细节）。

技术可行性讨论（安全问题另议，上一节 F18 的 IPC 边界结论对这里同样成立）结论是**可行，但这是一个新子系统，不是 F18 的延伸**，需要四层新能力：

1. **通用数据层**：不为每个临时小组件单独建表/写 migration，改为通用 `widget_defs`（字段名+类型的 schema 描述）+ `widget_records`（`widget_id, data JSON, created_at`），解释器照 schema 生成表单/列表，不是每个 widget 手写 Rust struct。
2. **通用视图语法**：现有 8 组件目录是给一次性快照卡片设计的，扛不动"反复查看历史+统计"的需求，需要新增支持数据绑定的组件类型（列表/表格、聚合统计），且渲染方式要从"一次性 props 快照"变成"能随数据变化重新渲染"——这已经不是 F18 信封模型的自然延伸，是新设计。
3. **导航注册层**：`pa-app` 前端现在是硬编码的固定 tab，需要改造成"读 `widget_defs` 动态生成导航项+对应视图"的数据驱动模式。
4. **同步层**：`widget_defs`/`widget_records` 复用现有"新增同步表照模板生成触发器+guid 列"的模式（sync v1 已验证过的套路，见本文件 2026-07-12 同步取舍条目），这一层反而是最有把握的。

**关键路径拍板**：LLM 产出的必须是**声明式 schema**（字段列表+受限的视图种类），不是可执行代码——这既是唯一现实可控的实现路径（通用解释器可以对声明式内容做校验和降级，代码生成没法），也顺带避开了上一节"自由代码生成=IPC 边界被绕过"的同类风险，但这不是选它的理由，工程可控性本身就是理由。

**未拍板**：具体 schema 字段类型集合、通用视图的组件清单、这类小组件是否要过 F7 的风险分级（比如"删除小组件"算不算需要确认的操作）。这些留到真正立项实现前再定。

## 2026-07-18 重复日程的扩展边界：先收齐每日别名，不仓促上线每周规则

- **本轮实现范围**：`每天` / `每日` / `每早` / `每晚` 都是同一份“每日固定提醒”语义，只需映射进既有 `routines` 模型；`每晚 8 点` 已由离线时钟解析为 20:00，补齐识别与标题清理即可，不新增存储或同步分支。
- **每周规则暂缓**：它需要在 routine 中新增频率与星期字段、对已有 SQLite 做 schema 迁移、更新 sync payload/远端合并与全端展示。更关键的是旧客户端会忽略新字段，仍按现有每日模型物化同一条 routine，形成“新设备每周、旧设备每天”的静默错误；不能把这种破坏提醒可信度的变更当作小功能上线。
- **后续前置条件**：先设计按设备能力的同步协商或明确最低客户端版本策略，再实现每周一类的持久化模型和跨设备黄金测试（乱序同步、旧数据迁移、暂停/删除回收未触发 occurrence）。在此之前，`每周` / `每月` / `每小时` 保持显式未支持，绝不降级伪装。

## 2026-07-18 Phase 8.1 桌面壳真机联调（computer-use 驱动，验收通过，未改代码）

隔离库（`PA_DB` 指向 scratchpad + `PA_SOULOUS_CONFIG` 指向真实配置）启动真实 `pa-app.exe`，对生产 Soulous 走「设置 → 云端 → Soulous 学习数据」全链路：

- **通过项**：配置区正确回显（token 只显末四位、留空沿用提示、gitignore/不同步文案）；「立即拉取」按钮拉取期间禁用 + 进度文案；成功后绿勾汇总（任务 8/打卡 1/专注 14，与 CLI 完全一致）+ 状态卡缓存行与最近成功时间即时刷新；拉取后「记忆台账」保持为空——"只读事实源不进记忆"在前端同样成立。
- **顺带验证了失败路径**：第一次点击拉取真实遭遇网络错误（`peer closed connection without sending`，疑似本机代理软件对新进程的瞬时干扰，重试即成功），UI 正确显示「拉取失败：已保留上次缓存 + 具体原因」，符合 F16 语义。
- **观察（未定性）**：第一次拉取点击后标题栏闪过「未响应」数秒后自愈，第二次拉取全程无此现象。`soulous_pull` 是 async + spawn_blocking，怀疑是冷启动首次网络调用与 WebView 初始化的瞬时争抢，不复现则不修，先记录在案。

## 2026-07-18 Soulous 互通：隐私原则从"一刀切"调整为三级分级（Phase 8 立项决策）

用户提出双向诉求：Soulous 拿 PA 数据辅助学习规划、PA 拿 Soulous 课表/任务更懂用户。讨论结论与取舍：

- **完全合并两个项目被否决**：两者设计哲学根本冲突（PA local-first 单人 Rust/Tauri vs Soulous 服务器中心多用户 Java/React），硬合必推翻一方的隐私模型，等于重写。采用"保持独立、数据互通"。
- **原则调整的正当性**：§4 原文"原始数据不出本地"防的是第三方云；Soulous 服务器是用户自有 VPS，性质不同。但 Soulous 是多用户系统（好友/自习室/访客路径都是暴露面），不能全开闸——所以是"分级"而不是"废除"：L1（聊天记录原文/人格/审计/穿戴原始数据/通知捕获）红线原样保留，只对 L2 派生粗粒度事实开白名单口子。
- **否决项：两侧 AI 记忆同步**。只交换事实不同步记忆——Soulous 曾因"两套向量库双 embedding"踩过数据漂移坑，跨系统再加一条同步只会重演，且回声（PA 推的事实经 Soulous 回流被 PA 当新事实学回）必须靠来源打标在架构层堵死。
- **8.2 推送初期刻意走 Tool + sensitive 确认**而非后台静默同步：让每次数据外流可见，是对"数据完全归你"定位的一致性要求；跑顺后是否降级另行拍板。
- 本次仅文档立项（§3.11/§4/§6），零代码。实现将交由外部智能体（GPT-5.6）执行，作为其能力测试——届时按 AGENTS.md 全套规范验收。

## 2026-07-18 Phase 8.1 实现取舍：外部学习数据是可同步事实缓存，不是 PA 记忆

- **独立表而非 `events` / `memory_facts`**：课表、考试、任务、打卡和专注数据保留为 `soulous_facts(source=soulous)`；这样同步时有稳定 guid 与行级 LWW，但不会进入 F12 记忆台账、recall 或云端 chat 上下文。它们只作为 Importance Classifier、F10 和 F14 的只读素材，杜绝“外部记录被 PA 当成自己记住的事”的回声。
- **手动拉取而非 ticker 自动联网**：拉取只经 CLI 或桌面按钮显式触发。这样用户能控制何时访问自有服务，且网络超时、token 刷新或 DTO 异常不可能占用 ingest/提醒的关键链路；失败时按完整快照原子替换规则继续使用旧缓存。
- **双 token 的刷新必须立即持久化**：Soulous refresh token 会轮换，因此即使随后某个业务 endpoint 拉取失败，也要先把已刷新的 access/refresh token 写回本机配置；否则下次启动会丢失唯一可用的 refresh token，离线缓存虽还在却无法恢复联网。
- **打卡历史明确降级**：现有 `/api/checkin` 仅为今日快照，8.1 不通过“连续天数”反推历史；需要历史周报素材时，等 Soulous 增加只读聚合接口后再扩展，保持 PA 只读且不臆造事实。

## 2026-07-17 桌面壳 computer-use 全流程走查（QA 观察，未改代码）

> **2026-07-18 更新**：下列发现除注明外已全部修复，见 CHANGELOG 当日条目；发现 11（杂窗）查明为 debug 构建的控制台窗口（`main.rs` 只在 release 挂 `windows_subsystem = "windows"`），release 无此窗口，**非 bug 不修**——关它等于杀进程属控制台正常行为。

用隔离测试库（`PA_DB` 指向 scratchpad）驱动真实 `pa-app.exe` 走了一遍核心业务：事件摄取→NL 改期→高危拦截→状态/记忆写入→云端闲聊+GenUI 按钮闭环→提醒触发/snooze→建议/复盘/台账/护栏确认/级联删除/导出/模拟时钟。主链路全部跑通；发现问题按严重度排列：

1. **云端调用期间整窗假死（bug，最高优先）**：闲聊/简报改写等走云端时窗口被 Windows 标记「未响应」、标题栏变红，持续到响应返回（MiMo 思考型 10–20s）。根因：`ingest` 等 Tauri command 是同步 `fn`（[lib.rs:341](../crates/pa-app/src/lib.rs)），云端 HTTP 阻塞在主线程消息泵上。用户很可能在等待期把 app 杀掉。改法方向：把含云端路径的 command 改 `async fn`（tauri 2 会挪到线程池）。
2. **snooze toast 字符串拼入 DOM 节点（bug）**：点「稍后再响」toast 显示「提醒 #2 将于 [object HTMLSpanElement] 再响」——[index.html:1653](../crates/pa-app/dist/index.html) 把返回 DOM 节点的 `fmtDT()` 插进模板字符串，应为 `fmtDTText()`（1075 行注释本身就警告过这一点）。
3. **对话内高危拦截是死胡同（UX 缺口）**：聊天里说「帮我删除所有照片」只回一句「需要你显式确认后才能进行」，不给任何确认/查看入口，护栏页也不会出现待确认项——用户不知道下一步去哪。应至少给「前往护栏」入口或就地 `guard_request` 按钮。
4. **右侧今日面板首次加载为空**（疑似 refresh 时序）：最大化出现宽屏面板时三个分区连空态文案都没有，直到下一次 ticker/交互才渲染出内容与空态。
5. **「每小时提醒」语义静默丢失**：GenUI 按钮发出「设置一个每小时喝水提醒的日程」，云端解析落成单次提醒（时间=当下），回复不提示「不支持重复提醒」——用户以为设好了每小时。至少应在确认文案里说明只建了一次性提醒。
6. 小项：ISO 时间串（`2026-07-18T15:00:00`）直接进对话文案偏机器味；台账/提醒行内 `[meeting]`/`(pending)`/`(fired)` 英文内部记号外漏；「取消提前0的提醒」措辞；记忆改写弹窗确认键用红色危险样式（编辑非破坏操作）；导出文件名时间戳用了模拟时钟时间；规则表 UI 只读（架构写「用户可编辑」，属有意留待真实数据固化，记录在案）；输入「今日简报」落进闲聊意图而非简报功能（简报入口只在欢迎屏 chip，会话开始后不可达）。

验证手法备忘：computer-use 直驱真实 exe 可行（需先启动进程再 `request_access` 传 `pa-app.exe` 基名）；WebView 的日期控件弹窗是独立进程窗口，会被截图遮罩，模拟时钟输入框用 triple_click+type 会被 datetime-local 控件部分吞掉，改值不可靠（正确姿势：点年段 → 输 4 位 → 方向键切段逐段输入）。

**第二轮（边缘业务）补充发现，同日：**

7. **采纳习惯建议→materialize routine 用的是真实时钟而非注入时钟（bug）**：模拟时钟 07-20 采纳「护肤 08:00」建议，生成的 occurrence 落在真实时间的明天（07-18 08:00）——`suggestion_set` 命令链路没把模拟 now 传进 `materialize_routines`（[orchestrator.rs:738](../crates/pa-core/src/orchestrator.rs) 本身逻辑正确，是调用方没传）。违反 AGENTS.md「注入时钟」红线的路径遗漏；真实使用不受影响，但所有依赖模拟时钟的验证/演示会踩。
8. **D3 示例措辞「清掉」不入高危路由**：「把上周的行为日志清掉」（ARCHITECTURE §6 D3 的原话）落进闲聊，云端还答「我没有删除数据的权限，建议联系管理员」（事实性错误+偏离定位）；「删除上周的行为日志」则正确走 `ledger_purge` 完整护栏流程（条数预览→发起确认→弹窗→审计）。意图规则该补「清掉/清空/清理」等同义词。
9. **habit 检测被单个离群点杀死**：聚类判据是全量 min/max 跨度 ≤90min，一条 23:09 的旧「护肤」状态让三条 08:00 的记录永远凑不成习惯，且无任何提示。真实使用里偶发一次深夜同名状态就会永久压制该习惯建议——建议改成截尾/众数窗口或至少按最近 N 天开窗。
10. **D4「routine 触发后一键已完成」未实现**：壳层与 CLI 都没有该入口（壳层唯一 routine 表面是台账层标签；CLI 只有 routine list/set on/off）。7 天未确认的反向暂停建议（`generate_routine_pauses`）依赖的「确认」实际只能靠用户主动说一句状态来达成——刹车链路成立但体验没闭环。
11. **杂窗 bug**：任务栏始终有第二个标题为 exe 完整路径的空白窗口；文件选择器流程后点它的 X 会把整个应用带走。疑似 Tauri/WebView2 的辅助 HWND 未隐藏。
12. **窄屏（400px）人格版本历史表格失控**：设定列一字一行竖排、回滚按钮被裁出视口——表格无窄屏适配。移动端底部标签栏/对话/设置其余页面正常。
13. 第二轮验证通过的项：NL 取消事件确认点按（按钮带事件名+时间，点按即执行且一次性）、`ledger_purge` 全链路+审计、问询横幅+四快捷项+应答落日志、习惯建议→routine 创建→到点触发（触发本身用模拟时钟，正确）、人格表单回填/保存 v2/回滚 v1（历史保留）/删除全部人格（确认弹窗写明后果）、云端测试连接（async 命令不冻结 UI——反证发现 1 的根因就是同步命令）、空输入发送被忽略、聊天记录导入 UI 存在但文件选择步骤因测试环境权限受限未走通（用户拒绝了 WebView2 进程授权）。
14. 附带观察：考试事件「提前 3d」提醒计划可以落在过去（7/20 考试今天录入 → 提醒计划 7/17 09:00 已过），UI 不提示"这个提前量已经来不及了"。

## 2026-07-17 通知隐私边界优先于跨设备一致性

- ~~第三方通知原文的「不出进程」是架构硬不变量，端到端加密同步也不能构成例外。因此 schema v5 选择为通知捕获链及其派生数据增加 `local_only` 范围并从 oplog 排除，而不是仅依赖 recall 的 SQL 过滤。代价是这类自动捕获日程不会跨设备复制；这是有意的隐私取舍。~~ **已于 2026-07-19 推翻两次**：先由「红线重划」放开通知上云（受开关控制），再由同日的「同步与上云解耦」（schema v10）改为**同步无条件常开**——本条当时的核心判断"端到端加密同步也不能构成例外"正是被推翻的部分，理由是自建、服务器解不开明文的中转与"交给第三方厂商"不属同一风险面。`local_only` 保留列名但语义收窄为「不得作为云端 LLM 上下文」。以本文件 2026-07-19 条目与 ARCHITECTURE §3.8 为准。
- 历史上已经推到同步服务器的密文无法由客户端追溯删除。迁移仅能阻止后续上传并清理本地待上传操作；若用户要彻底消除历史副本，需要清理服务端数据并轮换同步密钥。这一限制在 ARCHITECTURE §3.8 明示，避免把「停止新增泄露」包装成「抹掉既有副本」。

## 2026-07-17 按产品定位做的功能增删评估（结论：补三个缺口，不删任何功能）

应用户要求，把现有功能逐条对照 PRODUCT.md 定位（「隐私完全归你、越用越懂你的私人管家」）与 ARCHITECTURE F1–F18 做了一轮增删评估：

- **增（已实现，见 CHANGELOG 当日条目）**：三处「文档承诺了但代码没做」的缺口——提醒 Snooze（F2：可靠提醒是信任基础，但到点只能点掉，没有"稍后再叫我"）、全量数据导出（§4 明文"审计日志用户可导出"，全仓库却没有任何导出路径）、语义记忆可编辑（F12 规格三连"可查看/可编辑/可删除"缺了中间一个）。三者共性：离线确定性、不动写路径、不碰 GenUI 协议与同步机制——黑客松 07-21 截止前刻意只做小步收口。
- **此前取舍的推翻**：2026-07-16 记过「fact 的编辑 = 删 + 重新记住」，本次推翻——删+重记会丢 `created_at`/`source` 溯源信息，且对"改一个错别字"级别的操作交互太重；UNIQUE(content) 冲突改为人话报错。其余层（事件/日志/审计）仍不可编辑：那些是行为的忠实记录，改写等于篡改历史。
- **删（结论：现阶段没有该删的）**：逐项过了 12 个视图与全部 CLI 命令，每一项都能对回 F1–F18 的明确需求，没有发现"带聊天框的工具箱"式的堆料。唯一候选是 `demo_delete` 模拟高危工具——它已被真实的 `ledger_purge` 取代了"演示 Guard"的职责，但黑客松演示（护栏页完整流程走查）仍在用它，**评委期结束后可评估移除**；届时护栏演示统一走 `ledger_purge`。
- **评估中发现但不属于"缺口"的更大项**：事件改期/取消的自然语言路径（「把明天的会改到4点」——目前事件只能删了重录，这是与"管家"体验差距最大的一块）——**当日用户拍板「可以动」，已实现**，见 CHANGELOG 同日条目；实现取舍：改期唯一命中直接执行（改期可再改，可逆），取消一律过确认点按（删除不可逆，按钮写明事件名+时间，点按即确认——不给"一句话误删"留口子）。仍然记入待办的：免打扰时段（quiet hours——F13 场景静默已覆盖睡眠/日程中两景，通用时段配置价值存疑，等真实使用反馈）；导出的反向操作（导入/恢复——导出格式已带版本号为此留了门）。

## 2026-07-17 AGENTS.md 扩写为完整智能体工作规范

原 AGENTS.md 只是 CLAUDE.md 留痕规则的复制（当时为让 Codex 也守规矩）。应用户要求扩写为三段式规范：**开工前必须**（读 ARCHITECTURE / 扫 PITFALLS / 前端先读 PRODUCT+DESIGN / 确认工作区与测试基线）、**工作中红线**（隐私不变量、离线优先、注入时钟、护栏不绕行、vanilla JS 铁律、对外材料诚实）、**完工后必须**（留痕三选一、test/clippy/fmt 三条质量门、前端 harness 走查、文档跟行为、仓库卫生、不越权提交）。**全部条目取自项目既有约束**（架构 §3.3/§3.9/§4/F16、2026-07-16 rustfmt 决定、2026-07-15 设计体系决定），没有发明新规矩——这份文件是"把散落的规矩收拢给智能体看"，不是新立法。CLAUDE.md 加了指向 AGENTS.md 的引言，声明出入时以 AGENTS.md 为准，避免两份文件漂移。

## 2026-07-17 黑客松收尾第一批（免费 credit 表已由用户提交）

- README/LICENSE 见 CHANGELOG 当日条目。英文评委章节刻意**如实限定 Codex 的使用范围**（只有 Daily Focus Brief 是 Codex 构建，其余明说 predates Build Week）——延续 2026-07-15 拒绝包装造假的决定，宁可少拿"Codex 使用深度"分也不误导评委。
- 视频重剪方案落在根目录 `PA_demo_script_v2.md`（已 gitignore，同 PA_demo.mp4 待遇）：保留 v1 六段画面、全部旁白重写为覆盖 Codex + GPT-5.6 的英文版，追加一段 ~25s 的 Daily Focus Brief 实录（总长 ~175s ≤ 3min 上限）。TTS 管线复用 v1（SAPI Zira + ffmpeg adelay/amix，1–6 段时间轴 offset 不变）。**旁白稿待用户审后再录**——上一版用户中途从中文改英文，提交关键物先过目再合成。
- 仍待用户亲手做：仓库设 public（或私享 testing@devpost.com + build-week-event@openai.com）、LICENSE 版权行换实名（可选）、视频上传 YouTube、Devpost 表单提交（session ID `019f64c6-11dc-7d41-ac1c-fdd5907aa6a1`）。

## 2026-07-16 验收前收尾：全仓库 rustfmt 统一 + 仓库卫生清理

验收级全面检查（clippy -D warnings 零告警、165 测试全绿）发现两项非功能问题，随本次提交一并处理：

- **全仓库跑了一次 `cargo fmt --all`**：此前项目从未强制 rustfmt，32 个文件存在约 200 处格式漂移（历史积累，非单次改动造成）。统一后 `cargo fmt --all --check` 通过；以后提交前应保持该检查干净，避免 diff 混入格式噪音。
- **清理三个工作区遗留文件**：`nul`（bash 里误用 Windows 式 `2>nul` 重定向产生的垃圾文件，内容为 GBK 错误输出）、`demo.sqlite-shm`/`demo.sqlite-wal`（演示库孤儿 WAL 残留，主库早已不在）。`.gitignore` 原来只覆盖 `pa.sqlite*`，补了 `*.sqlite` / `*.sqlite-wal` / `*.sqlite-shm` 三条通配，堵住"演示/临时库不小心入库"的口子（个人数据绝不入库，§4）。
- 检查还留了几条不阻塞的观察（purge 时间范围解析不出时静默退化为全量、routine 物化事件会让场景检测判定"日程中"一小时、pa-app 测试用进程级环境变量），暂不改，待真实使用中再定。

## 2026-07-16 Phase 6/7 实现取舍（相对 §3.10/§6 设计稿的偏差与决定）

功能变更见 CHANGELOG 当日条目，这里记实现时拍板的取舍：

- **routine 的每日触发不是独立管线，而是"物化为普通 event + notification"**：设计稿只说"复用既有提醒/AlarmManager 链路"，实现选择每天把 occurrence 落成 `events(kind=reminder)` + 0m 提醒——触发、同步、台账、Android 后台闹钟全部零改动复用；代价是每天每 routine 多一行事件（可见于日程视图，其实是优点）。跨设备同步的重复物化用「同 title 同 start 的事件已存在则跳过」抹平。
- **采纳 habit 建议后没有 GenUI form 微调时间，直接按检测出的典型时间自动建**（设计稿写了 form 微调）：新增一个白名单动作 + form 构建器 + 前端 dispatch 的成本对 v1 不划算；时间不满意可删掉 routine 重新养成，或后续再补 form。偏差待补记入 §6。
- **`memory_facts.last_used_at` 字段建了但 v1 不更新**：recall 每次命中就 UPDATE 会让同步触发器每次闲聊都产生 oplog 噪声（一次检索 = 最多 5 条跨设备操作）。字段留作未来相关性调优，v1 纯展示为空。
- **fact 的"编辑" = 删 + 重新记住**：台账 UI 只有删除动作，没为 fact 单独做编辑表单；语义上等价（fact 是一句话），F12 的知情/删除权不受影响。
- **stats / recall 的主要交互面是 CLI**（`pa stats` / `pa recall`），壳层只暴露了 `stats` command 未做专门视图：D2 是一次性的"数据够了没"检查点、recall 调试是开发者动作，都不是日常使用面；等真实使用中确有需要再补 UI。
- **wellness 的久坐信号用"当日累计步数 < 基线 15% 且已过中午"而非滑动窗口**：步数样本的窗口粒度依赖 Health Connect 聚合方式，日累计是最稳的最小可行判据；滑动窗口留给有真实数据后再调。
- **`ledger_purge` 只允许 behavior/suggestion/wearable 三层**：raw_input/event/notification 有级联语义，"将删除 N 条"的预览无法诚实概括连带后果，v1 明确拒绝（错误信息引导去台账逐条删）。
- **`Tool` trait 加了 `ToolCtx`（借用 store）而不是给工具塞 store 引用**：真实工具需要读写本地数据，orchestrator 在调用期把 store 借给 guard 传下去；preview 也拿同一个 ctx 才能显示真实条数，但契约上仍禁止 preview 写。
- **场景（F13）只静默"问询 + 自动建议"，不动提醒**：闹钟你自己定的就必须响（F2 > 场景礼貌），这条写进了 scene.rs 的模块文档。

## 2026-07-15 缺口评估 → Phase 6/7 规划入库（ARCHITECTURE §3.10 + §6 + §7 更新的"为什么"）

**背景**：用户问"根据设计理念和初衷还要继续完善什么"。对照 §0 定位（"真正了解你的 agent，而不是无状态问答机器人"）逐项盘点后结论：F1–F18 功能清单完成度高，但**最大缺口在"了解你"本身**——云端 chat 至今只发当前一句话，台账里的记忆从未参与推理。用户拍板：全部细化设计并整合进待做。产出 = §3.10 新节（记忆检索与语义记忆）+ §6 Phase 6/7 路线图 + §7 待决清单更新。设计判断与取舍记录如下：

- **优先级排序的依据**：记忆检索（Phase 6 M1–M3）排第一，因为它直接决定产品是"PA"还是"带日程表的聊天框"，且 §3.6 原文早就承诺了"按需检索相关片段"——这是补欠账不是加需求。D2 数据回看排第二，因为 §7 两条"等真实数据"的待决项（规则表初值、F11/F13 阈值）需要一个明确的"数据够了没"检查点，否则会无限期搁置。D3（真实 dangerous 工具）排第三：Guard 只接过模拟工具 `demo_delete`，一次性 token/审计一直在演习。
- **recall v1 为什么不用向量库**：记忆条目量级只有数百条，全量拉取 + 内存打分（bigram 重合 × 时间衰减 × 层权重）足够；FTS5 unicode61 对中文分词不友好，数据量也配不上索引成本。v2 若上 embedding **必须本地模型**——句子发云端做 embedding 等于内容出本地，违反 §4 第 1 条；模型体积/移动端可行性是真实风险，所以 v1 先跑出命中率数据再决定，不预设"一定要上向量"。
- **第一个真实 dangerous 工具为什么选 `ledger_purge`（批量删台账）而不是"发消息"/"删文件"**：它是 agent 主动执行的真实破坏性写路径、零外部依赖（不需要接任何第三方账号）、审计留痕有实际意义（删了什么、删前预览过条数）；且 GenUI 的 `guard_request` 动作白名单早已预留入口，正好接上真实载荷。注意与 F12 的区别：用户在台账 UI 里手动逐条删除是行使删除权、不算 dangerous；由对话指令触发的**批量**删除才走 Guard。
- **memory_facts 不做问询回答自动提炼（v1）**：未经用户确认的自动写记忆与 F15 人格污染是同类风险——写入必须经确认（用户直说的 MemoryWrite 规则匹配可直接落，LLM 兜底判定的须确认），同 Importance 反哺"人工确认后才固化"的既有原则。
- **会话短期上下文单列为 M1**：它不需要任何存储/检索设施（壳层内存最近 4 轮，关窗即失），成本最低、体验收益最大，不该被 recall 的工期绑架。
- **D5 阈值全部相对个人基线**（近 28 天中位数 ±百分比）而非绝对值：绝对阈值（如"心率 >100"）对不同人就是拍脑袋，基线相对值才配得上"了解你"；基线计算复用 D2 的统计管道，一份代码两处用。
- **D4 的反向闭环（连续 7 天未确认 → 建议暂停 routine）是刻意设计**：主动性必须自带刹车，否则习惯提醒会退化成骚扰——与 OS 通知降噪（2026-07-14）同一价值观。
- **Daily Focus Brief 不在此列**：后端 `brief.rs` + `Orchestrator::daily_brief` 已落盘（Codex 任务书见本文件上方条目），不重复规划。
- **仓库卫生顺带记录**：工作区有 `demo.sqlite-shm/-wal`（演示库 WAL 残留）和一个 `nul` 文件（Windows 重定向踩坑产物）未清理，`dist/index.html`/CHANGELOG 有未提交改动——不属架构待办，随下次提交清理。

## 2026-07-15 UI 全面重构的设计决策记录（CHANGELOG 当日条目的"为什么"部分）

用户明确指令：基于新装的两个 skill（`design-taste-frontend-v1` + `impeccable`）弃旧重建 UI/UX、更换风格。功能行为变更见 CHANGELOG，这里记设计判断与取舍：

- **视觉身份为什么是「琥珀台灯」**：impeccable 的品牌种子脚本给出 hue 57°（琥珀/蜂蜜）；场景句「深夜书房里一盏安静待命的台灯」——贴合"私人管家"的定位，同时天然避开两个 skill 都点名的重灾区（taste 明令禁止的"AI 紫"——旧 UI 的 `#4f46e5` 正中枪口；impeccable 禁止的米黄纸张底色）。色彩策略取 Restrained（中性底 + 单强调色 ≤10%），语义一色一义：**琥珀 = 需要你注意**（主按钮/到点提醒/警示/选中态），在线状态用绿、危险用红、静默状态用中性。warn 与 accent 同族是有意的：对 PA 来说"警示"和"主动作"都是"灯亮了"。
- **PRODUCT.md / DESIGN.md 入库**：impeccable 流程产物，根目录两个新文件。战略上下文（用户/定位/反例/设计原则）从架构文档提炼而来——本项目文档足够完整，未走 init 的用户采访（autonomous /goal session，阻塞式提问与指令冲突）。后续任何设计任务应先读这两份。
- **技术栈坚决不动**：taste skill 默认 React/Tailwind/Framer Motion，但架构 §3.9 写死"vanilla JS、不引入前端框架/npm"——架构文档优先，taste 的设计原则（反 emoji、反 AI 紫、交互状态全覆盖、tabular-nums、:active 物理反馈）用 vanilla CSS/JS 落地，动效强度按其 4-7 档（纯 CSS transition/animation）。
- **字体用系统栈**：中文 UI 可嵌字体动辄数 MB 且要离线，impeccable 的 product register 明文允许系统栈（"System fonts are legitimate here"）。个性由琥珀色、图标笔触、间距节奏承担，不由字体承担。
- **模拟时钟移到桌面侧栏页脚、移动端不再暴露**：它是演示/调试器不是产品功能，占旧顶栏最显眼的位置是错位；移动端是真实使用面，不该被调试件占空间。需要在移动端模拟时钟的场景（真机演示）可以先在桌面调好再同步。
- **示例 chips 只在对话空状态出现**：它们是 onboarding 教具，一旦用户开始真实对话就不该常驻（旧 UI 常驻在输入框下）。
- **记忆台账保持一级入口**（沿袭 2026-07-14 决定）：F12 是隐私信任承诺，入口可见性即承诺。
- **表格没有被一刀切消灭**：日程/建议/日志这类"扫一眼"的视图改为列表行，但台账/审计/提醒全表/版本历史保留表格——高密度数据是 F12 透明性的正当需求（impeccable product register："Density is a permission"）。
- **验证 harness 的形态**：`window.__TAURI__` 全 command 假实现（mock.js）注入 dist/index.html 生成 preview.html，node 静态服务器 + 真 Chrome 截图走查；390px 移动端用 iframe 内嵌精确模拟（Chrome 窗口缩不到手机宽）。这套件在 scratchpad，属会话临时产物未入库——F18 时代的"mock-IPC harness"思路相同，如以后高频使用可考虑正式入 repo（`crates/pa-app/dev/` 之类），本次先不做。
- **旧 UI 未保留**：用户指令"摒弃现有的 UI 全面重新构建"，git 历史即备份（重构前最后版本在 c91c427 后的工作区提交前状态，diff 可回溯）。
## 2026-07-15 OpenAI Build Week 参赛类别选定 + Codex 实作任务立项（Daily Focus Brief）

**背景**：用户打算报名 OpenAI Devpost「Build Week」黑客松（openai.devpost.com，截止 2026-07-21 17:00 PT）。官方规则明确要求项目"用 Codex + GPT-5.6 构建"，提交表单要求填一个真实的 `/feedback` Codex Session ID（标注"核心功能的大部分在该 session 完成"），评分第一项就是"Codex 使用深度"。PA 迄今为止全部由 Claude Code 构建、云端推理接的是小米 MiMo——如实告知用户后讨论了两条路：① 编造/包装成用了 Codex（技术上做不到——没有真实 session ID 无法蒙混，且属于对评委造假，**已拒绝配合**）；② 真刀真枪用 Codex 做一块真实的新功能，如实提交。用户选了②。

**选定赛道**：Work and productivity（官方原文："Tools that make teams faster or more effective, from workflow automation and customer support to analytics, sales, and back-office operations."）。

**立项任务：Daily Focus Brief（每日聚焦简报）**——范围克制，明确是"新增一块"，不是"重写"或"套壳凑数"：
- **目标**：PA 现有信息分散在日程/提醒/建议/行为日志四个视图里，用户每天要手动切视图核对。新增一张按优先级聚合的"今日聚焦"卡片——今天的日程 + 到点/即将到点的提醒 + 排名前 1~3 条未处理建议，一次看完。这是典型的 workflow automation / 减少人工核对，贴合赛道描述，也是真实需求（不是为了凑赛道现造的功能）。
- **范围**：只读聚合，不碰任何既有写路径；新增 `Orchestrator::daily_brief(now)`（`pa-core`）+ CLI 命令（`pa brief`，参照 `pa review` 的现有模式）+ 对话页复用 F18 GenUI 协议渲染成卡片（复用既有组件目录，不新增组件类型——协议升级不在这次任务范围内）。
- **明确不做**：不碰 F11/F13（场景感知，架构 §7 已注明"待后续评估触发策略，不拍脑袋定阈值"，时间也不够）；不新增 GenUI 组件类型；不改动同步/加密/护栏等现有子系统。
- **执行方式**：这块要交给真实的 OpenAI Codex（ChatGPT 桌面版或 Codex CLI）实现——Claude（我）没有 Codex 工具可调用，只能把任务拆到这个精确度，实际编码由用户自己跑 Codex 完成，跑完拿到的 `/feedback` session ID 就是提交表单要填的那个。完成后要按本文件顶部的强制留痕规则在 CHANGELOG 补一条（新增功能）。

**Devpost 表单「类别 + 1-2 句项目简介」的回答**（已给用户，备查）：
> Work and productivity — PA is a privacy-first personal agent (Rust/Tauri desktop + Android) that turns a spoken sentence into a structured event with graded reminders, and renders its replies as dynamic, interactive Generative UI cards whose actions flow back into real backend state. For Build Week we're using Codex + GPT-5.6 to add a Daily Focus Brief — an on-demand, GenUI-rendered digest that pulls today's schedule, due reminders, and top open suggestions into one prioritized card, cutting the daily triage PA's users currently do by hand.

**状态（2026-07-15）**：用户决定这块**先只留文档，暂不实施**——本节即完整任务书，之后会另开一个 session（用真实 Codex）单独完成，不占用当前 Claude Code session 的进度。下面的提示词是基于代码库实际模式（`review.rs`/`Orchestrator::review()`/`genui::suggestions_prompt`/`checkin_now` 等真实签名，由 Explore 子代理核对过）写好的，可以直接复制给 Codex 用，不需要再重新研究代码库。

<details>
<summary>完整 Codex 任务提示词（点击展开）</summary>

```
You are working in the PA (Personal Agent) Rust workspace. Before writing any code, read `docs/ARCHITECTURE.md` (especially §3.9, the Generative UI / F18 protocol) and `CLAUDE.md` (mandatory documentation rules — every change must land in docs/CHANGELOG.md, docs/PITFALLS.md, or docs/MISC.md).

TASK: Implement "Daily Focus Brief" — a read-only feature that aggregates today's schedule, due/upcoming reminders, and top pending suggestions into one prioritized summary. It must be exposed three ways: CLI command, Tauri command, and a pushed F18 Generative UI card in the chat view.

This repo already has an almost identical feature you should mirror closely: the "review/digest" feature (`pa-core/src/review.rs` + `Orchestrator::review()`/`weekly_review()` + CLI `Review` subcommand). Follow that exact shape.

--- STEP 1: pa-core/src/brief.rs (new file) ---
Add a pure aggregation module, same shape as review.rs's Digest/build_digest:

    pub struct Brief {
        pub date: NaiveDate,
        pub events_today: Vec<Event>,
        pub due_reminders: Vec<Notification>,
        pub upcoming_reminders: Vec<Notification>,   // next few, not yet due
        pub top_suggestions: Vec<Suggestion>,          // status == Pending, take(3)
    }

    pub fn build_brief(
        now: NaiveDateTime,
        events: &[Event],
        notifications: &[Notification],
        suggestions: &[Suggestion],
    ) -> Brief { ... }   // pure, no I/O — filters/windows what's passed in, exactly like build_digest does

    impl Brief {
        pub fn render(&self) -> String { ... }   // Chinese-language human-readable summary, same tone as Digest::render()
    }

Unit-test build_brief with hand-built Event/Notification/Suggestion fixtures (no store needed) — mirror the test style already used in review.rs / suggest.rs.

--- STEP 2: Orchestrator::daily_brief in orchestrator.rs ---
Add, right next to review()/weekly_review():

    pub fn daily_brief(&self, now: NaiveDateTime) -> Result<crate::brief::Brief> {
        let events = self.store.upcoming_events(now)?;       // then filter to .date() == now.date()
        let notifications = self.store.list_notifications()?;
        let suggestions = self.store.suggestions_pending_only... // filter self.suggestions()? to Pending, no new store query needed
        Ok(crate::brief::build_brief(now, &events, &notifications, &suggestions))
    }

Do not add new Store methods — `upcoming_events`, `list_notifications`/`due_notifications`, and `list_suggestions` already cover everything; do the "today only" / "pending only" filtering in brief.rs, same division of responsibility as review.rs (Store returns full lists, the pure builder windows them).

Add an Orchestrator::in_memory()-based integration test exercising daily_brief after a couple of o.ingest(...) calls, same style as the existing suggestions_generate_dedup_and_gate test in orchestrator.rs.

--- STEP 3: genui::daily_brief_prompt in pa-core/src/genui.rs ---
Compose the card from the EXISTING 7-component catalog only — do NOT add a new UiComponent variant to the enum. Follow the exact pattern of `suggestions_prompt` (loop building components, respect MAX_COMPONENTS/MAX_BUTTONS, end with `checked(UiEnvelope::new(components))` which debug_assert!s validate() passes):

    pub fn daily_brief_prompt(brief: &Brief) -> Option<UiEnvelope> {
        // Text header, then EventCard per today's event, ReminderCard per due reminder,
        // SuggestionCard + ButtonGroup(采纳/忽略) per top suggestion — reuse the existing
        // action names already in ALLOWED_ACTIONS (reminder_dismiss, suggestion_set, ingest).
        // Return None if there's genuinely nothing to show (empty day).
    }

Add a JSON round-trip test: serde_json::to_string(&env) contains the expected component types, and genui::validate(&env, ALLOWED_ACTIONS).is_ok().

--- STEP 4: CLI — pa-cli/src/main.rs ---
Add subcommand (doc comment becomes --help text, follow Review's style exactly):

    /// Show today's brief: agenda, due/upcoming reminders, and top pending suggestions.
    DailyBrief,

    ...
    Cmd::DailyBrief => {
        let brief = o.daily_brief(now).map_err(to_err)?;
        println!("{}", brief.render());
    }

--- STEP 5: Tauri command — pa-app/src/lib.rs ---
Follow the checkin_now pattern exactly (CmdResult<T>, lock! macro, parse_now, core_err):

    #[tauri::command]
    fn daily_brief(state: State<AppState>, now: Option<String>) -> CmdResult<pa_core::brief::Brief> {
        let now = parse_now(now)?;
        lock!(state).daily_brief(now).map_err(core_err)
    }

Register it in the `invoke_handler![...]` list at the bottom of run() alongside the other ~30 existing commands (do not remove/reorder any existing entries).

--- STEP 6: push event (optional but preferred) — pa-app/src/lib.rs ticker() ---
The ticker() function already runs every 60s and does this exact pattern for checkin/suggestions (search for `app.emit("pa-checkin"` and `app.emit("pa-suggestions"`). Add an equivalent `app.emit("pa-daily-brief", ...)` BUT gated to fire once per calendar day, not every tick — there's no existing "once per day" gate in this codebase, so add a minimal one (e.g. track the last-emitted date in AppState behind the existing Mutex, or store it as a behavior-journal marker). Build the envelope via genui::daily_brief_prompt.

--- STEP 7: frontend — pa-app/dist/index.html ---
Add a `listen("pa-daily-brief", (e) => { ... })` handler next to the existing `listen("pa-suggestions", ...)` / `listen("pa-checkin", ...)` blocks (search for those). Render the envelope into a chat bubble using the existing `renderEnvelope()` function — do not write a new renderer. Also add a manual "刷新今日简报" entry point (a button, e.g. near the existing chat example chips) that calls `invoke("daily_brief", {...})` on demand, so it's not push-only.

--- STEP 8: docs (mandatory per CLAUDE.md) ---
Add one entry under `## [Unreleased]` → `### Added` in docs/CHANGELOG.md describing this feature, matching the exact formatting/voice of the existing entries in that file (Chinese, bold feature name, bullet breakdown of what was added and why). If you hit any real debugging detour along the way, log it in docs/PITFALLS.md per that file's format.

--- HARD CONSTRAINTS ---
- Read-only aggregation only. Do not add or modify any write path to events/notifications/suggestions tables.
- Do NOT touch F11 (emotional/anomaly awareness) or F13 (scene-mode switching) — explicitly out of scope per architecture doc §7.
- Do NOT add a new UiComponent variant to the genui catalog — compose the brief from the existing 7.
- Do NOT touch the sync layer, encryption, or the HITL Guard/audit subsystem.
- Match existing code style and idioms exactly — same CmdResult/lock!/parse_now/core_err plumbing, same `checked()`-wrapped envelope validation, same test module conventions (#[cfg(test)] mod tests at bottom of file, `dt(...)` helper for NaiveDateTime fixtures).
- `cargo test` (whole workspace) and `cargo clippy` must be green / zero warnings after the change.
- Do not change the signature or behavior of any existing command, CLI subcommand, or GenUI action.
- The CHANGELOG.md entry is part of this change, not a follow-up — include it in the same commit/session.

Work incrementally: brief.rs + its unit tests first, then orchestrator wiring + integration test, then genui builder + its test, then CLI, then Tauri command, then frontend, then docs. Run `cargo test` and `cargo clippy --all-targets` after each step.
```

</details>

## 2026-07-15 生成 hackathon 演示视频（PA_demo.mp4）

**背景**：用户要参加 hackathon，Devpost 提交需要一个 YouTube 视频演示链接，要求录制真实屏幕（非动画/口播稿），范围定为「核心闭环（推荐起点）+ F18 交互式生成（亮点）」。

**做法**：用已有的 `target/debug/pa-app.exe`（发现 `dist/index.html` 有未提交的最新改动——5 组一级导航 + 深色模式适配，先 `cargo build -p pa-app` 重新编译才让画面对得上最新设计）+ 全新 `PA_DB=demo.sqlite` 空库跑桌面壳；因为 Claude 桌面自动化工具（computer-use MCP）认不出这个自定义 exe（不在其"已安装应用"名单里，`request_access` 反复报 not-installed），改用 PowerShell + Win32 API（`SendInput` 模拟真实 keydown/mouse_event，而非 `SendKeys`/WM_CHAR）驱动窗口点击与中文文本输入；`ffmpeg`（现装，`winget install Gyan.FFmpeg`）用 `gdigrab` 按窗口区域坐标录屏，`GenerateConsoleCtrlEvent` 发送优雅停止信号后精确裁剪出 140 秒成片。云端 F18 演示因沙箱网络问题放弃，见 PITFALLS 同日条目；改用离线模板（会议/考试示例卡片 + 取消提醒按钮点击回流）完整展示了协议闭环。

**产物**：仓库根目录 `PA_demo.mp4`（已加入 `.gitignore`，不进版本库——纯演示素材，非源码）。用户需要自行上传到 YouTube 拿到公开链接，供 Devpost 提交用。

**补充（同日）配音**：用户反馈纯静音演示太枯燥，加了旁白。本机 Windows SAPI 自带语音包，`System.Speech.Synthesis` 离线合成 6 段解说词（对应六个画面阶段：开场/核心抽取/F18 概念/第二个例子/按钮回流/记忆台账），`ffmpeg adelay` 按画面实际出现时间对齐每段起始点后 `amix` 混合，再 `-c:a aac` 复用进视频。全离线合成，不依赖任何云端 TTS。视频总长顺带从 140s 延到 150s，给记忆台账收尾段留够旁白时间。先做的中文版（`Microsoft Huihui Desktop`），用户改口要纯英文，又用 `Microsoft Zira Desktop`（en-US）重新合成一版英文解说词换上——时间轴 offset 复用同一套（各画面阶段的起止秒数不变，只是语音源换了语言），最终提交版是纯英文配音。

## 2026-07-14 交互式生成（Generative UI）理念立项讨论

**背景**：用户提出，要把 PA 打造成真正意义上的个人 agent，「交互式生成」必不可少——即 agent 的回应不再局限于纯文本 + 预置视图，而是能按对话语境**动态生成可交互的 UI**（卡片、表单、选择器、确认面板），用户在生成的 UI 上的操作再回流给 agent 形成闭环。此前项目里只有精神上相近的两处：F9 导入的"先预览再确认"、F7 HITL 确认弹窗——都是**预置的**交互面，不是生成的。

**参考项目：[AGenUI](https://github.com/AGenUI/AGenUI)**（高德 + 千问 C 端团队开源，2026-05）——首个覆盖 iOS/Android/HarmonyOS 三端的原生 A2UI（Agent-to-UI）渲染框架，实现 Google A2UI v0.9 协议。对 PA 有借鉴价值的核心设计：
- **协议与渲染分离**：LLM 只输出结构化的 UI 描述（A2UI JSON：组件树 + 数据模型 + 动作绑定），渲染层（共享 C++ 核心 + 三端原生绘制）负责把描述变成真实控件。UI 描述是数据，不是代码——**LLM 永远不生成可执行代码**，安全边界清晰。
- **流式增量渲染**：`updateComponents` / `updateDataModel` 增量消息边生成边渲染（虚拟组件树 diff），用户不用等整个回复生成完。
- **动作回传闭环**：按钮/表单交互触发 Action 事件回传给 agent，agent 据此继续下一轮生成；客户端工具通过 Function Call 框架暴露给 LLM。
- **组件白名单**：内置 22 个组件 + 有限 CSS 属性集，LLM 只能在目录（`agenui_catalog.json`）内组装，不能逸出。

*（2026-07-14 更新：已升格为 ARCHITECTURE.md §3.9（F18，Phase 5），含组件目录 v1 与动作白名单 v1；下方"尚未拍板"四项已随之拍板——目录 7 组件见 §3.9、不持久化、v1 先整包不流式、自定精简格式字段命名参考 A2UI。）*

**映射到 PA 的设计要点（讨论稿，实施前需升格为 ARCHITECTURE.md §3.x 正式小节）**：
1. **协议层放 pa-core**：新模块（暂名 `genui`）定义 PA 自己的组件目录（v1 从小做起：文本、事件卡片、按钮组、表单、确认面板、列表），`Reasoner` 输出的 UI 描述 JSON 由本地防御性解析 + schema 校验（复用 F6 抽取兜底"剥围栏、不合格降级"的成熟模式）——**校验不过就降级为纯文本回复，绝不让对话失败**（F16 精神）。
2. **渲染层放壳层前端**：`dist/index.html` 是无框架 vanilla JS，天然适合做一个小型"目录内组件渲染器"（JSON → DOM），不必引入 AGenUI 本体（它是移动原生 SDK，技术栈不匹配；**借协议理念，不借实现**）。
3. **动作回传 = 现有 command 白名单**：生成的按钮/表单只能绑定到已有 `#[tauri::command]`（ingest / 提醒操作 / 建议采纳…），LLM 在描述里引用动作名 + 参数，Rust 侧按白名单校验后才执行——与 F7 的原则同构：**生成的 UI 可以"请求"动作，无权直接执行**；高危动作照走 Guard 确认流程，生成的确认按钮只是入口不是令牌。
4. **离线也要有交互式生成的退化形态**：无云端时由规则引擎按意图选预置模板（事件卡片、建议卡片本来就是模板），LLM 在线时才做自由组装——渲染器只认协议不认来源，两条路共用。
5. **隐私边界不变**：发给云端的仍只有当前输入 + 必要上下文；UI 描述是云端**返回**的东西，不涉及新增上行数据。第三方通知文本"永不出进程"是本条讨论当时的前提，已由 2026-07-19 Phase 9 的「通知上云」开关例外取代；注入线、动作白名单与 Guard 不受影响。

**为什么值得做（而不是过度工程）**：PA 的交互本质上是"对话中夹杂结构化操作"（确认日程、挑选提醒档位、编辑人格初稿），目前靠预置视图 + 用户切页完成,操作与对话割裂;交互式生成让操作就地发生在对话流里,是"个人 agent"区别于"带聊天框的工具箱"的关键体验。AGenUI/A2UI 的出现说明这条路线已有工业级验证与开放协议可对齐,不是自造轮子。

**尚未拍板、实施前要过的决定**：组件目录 v1 的具体清单;UI 描述要不要持久化(倾向不持久化,对话即焚,台账只记动作结果);流式渲染 v1 要不要做(倾向先整包后流式,MiMo 输出不长);是否直接对齐 A2UI/AG-UI 协议格式还是自定精简格式(倾向自定精简格式,字段命名参考 A2UI 以便将来对齐)。

## 2026-07-12 Phase 4 F5 的实现取舍（wearable / Health Connect）
- **否决方案：三星 Health Data SDK / 小米私有 SDK 直连**。查证后确认三星的合作伙伴接入走审批制（旧版 Android SDK 已于 2025-07-31 废弃，迁移到新 Data SDK 要求重新走一遍合作伙伴审批），个人开发者拿不到批准；改走 Android 平台层的 Health Connect，三星健康 app 本身就把数据同步进去，效果等价且无需审批。这也是为什么架构决定里特别强调"不关心数据最初来自哪个厂商 app"——只要用户手机上装的运动健康 app 肯同步进 Health Connect，适配层代码不用改一行。
- **F5 范围与 F11/F13 切开，不是偷懒是分层**：用户明确要求"只做 F5：把心率/睡眠/步数接进来，落地存储"。数据接入（读、去重、存、可查可删）和"基于数据做主动决策"（异常阈值、打扰频率、避免变成骚扰）是两层完全不同的复杂度和风险——后者做错了直接损害用户信任（对应 F7/F8 的克制主动度精神）。先把地基打扎实，再谈上层策略，避免为了赶功能而拍脑袋定一套阈值。
- **验证方法：临时写权限 + 合成数据，而非等真实 Samsung Health 数据**。开发环境的模拟器上不可能有真实穿戴设备同步的数据，若不验证只读代码路径就直接判定"应该没问题"是自欺欺人。选择了给插件临时加一个 `insertTestSample` command（连带临时 `WRITE_HEART_RATE` 权限），写入一条合成心率记录后走完整的 `readRecent → convert → dedup insert → 台账展示` 链路，验证通过后再把临时代码整个撤掉。这是对的方法论：Health Connect 作为数据存储层，不关心记录的来源 app，用自己写入的数据验证自己的读取代码在协议层面完全等价于验证真实三方数据。
- **`minSdk` 从 24 提到 26**：Health Connect client 库本身要求 API 26+（Health Connect app/模块要 28+，但那是运行时 `getSdkStatus()` 判断，不是编译期硬约束）。Gradle 的 manifest merger 要求消费方 app 的 `minSdk` ≥ 每个库模块的 `minSdk`，所以整个 `pa-app` 的 Android 最低支持版本被动提高。对自用应用是可接受的代价（API 26 = Android 8.0，2017 年发布，覆盖面早已足够）。
- **轮询频率对齐同步频率（5 分钟一次），不是每分钟**：ticker 本来每分钟跑一轮到点提醒检查，但 Health Connect 读取是真实跨进程 IPC 调用（不是本地文件读取），没必要每分钟都打一次；复用了 F17 后台同步已有的"每 5 个 tick 一次"节流模式。

## 2026-07-07 通知监听的实现取舍（F1 v1）
- **文件队列（JSONL inbox）而非正经 tauri Android 插件（JNI 直调）**：插件方案要为一个"传几行文本"的需求引入完整的插件工程（Kotlin 插件类 + Rust binding + 权限声明）；v1 用应用私有目录里的 append-only 文件 + ticker 轮询，跨语言接口就是"一行一个 JSON"，两侧都好测。将来要做"应用内一键跳转通知使用权设置页"时反正要建插件，届时再迁移。
- **读后即删的竞态窗口（读与删之间新追加的行会丢）**：接受。捕获是辅助性输入而非审计记录，丢一条的代价是"少记一个日程"，而消除竞态需要文件锁或轮换协议，v1 不值得。已在代码注释与本条留痕。
- **隐私门放在 pa-core（`ingest_captured`）而不是壳层**：第三方通知文本绝不进云端（连规则解析失败时的 LLM 兜底也不走），这个不变量跟 HITL Guard 一样应该由核心层保证，壳层只是搬运工。丢弃的文本不落任何表——不囤积。
- **PA 自己的提醒通知会被监听服务捕获成回环**：Kotlin 侧按包名过滤自家通知，这是必须的第一道闸（否则"PA 提醒你开会"又被抽取成新事件）。

## 2026-07-12 多设备同步的实现取舍（sync v1）
- **行级 LWW 而非 CRDT 字段级合并**：架构稿原建议 CRDT，实际评估后 v1 用"行级 last-write-wins（毫秒 UTC 时钟 + 设备 id 决平局）"。理由：本项目的行几乎都是"单动作产物"（一次 ingest、一次 dismiss），同一行被两台设备并发改不同字段的场景近乎不存在；CRDT 的复杂度花在这里买不到东西。数据模型里天然并发的部分（人格版本历史）本来就是 append-only，用"版本号本地重编号 + guid 去重"就消化了冲突。将来如果出现真实的字段级并发编辑需求（如共享编辑一条日程的不同字段），再升级。
- **变更捕获放 SQLite 触发器而不是 Rust 写路径**：写路径有十几处（含级联删除、`INSERT OR IGNORE`），逐处手动记 oplog 必漏；触发器在库层兜底，级联删除天然被捕获。代价是两条不变量必须守住：① 应用远端操作前必须置 `sync_applying` meta 标志（防回声），封装在 `apply_remote_ops` 单一入口里；② 新增同步表时要同步加触发器 + guid 列（migrate_sync 里模板化生成，照抄即可）。
- **整数主键不出设备**：同步身份是随机 guid 列，FK（notifications.event_id、events.raw_input_id）在触发器捕获时用子查询翻译成对方的 guid，应用侧再解析回本地 id。孤儿（父行已删）直接跳过不报错——删除是合法竞态。
- **hlc 用墙钟毫秒而非真 HLC/向量钟**：自用两三台设备、同步间隔分钟级，时钟偏差造成的错序窗口远小于实际并发窗口；平局用设备 id 决定，保证两边收敛到同一结果（收敛性 > 谁赢）。
- **老库迁移即 bootstrap**：guid 回填 UPDATE 放在触发器创建之后执行，回填本身生成全量 upsert 操作——新设备第一次 sync 就拿到完整历史，不需要单独的"首次全量导出"路径。
- **oplog 只增不删**：v1 不做修剪。体量估算：个人日程/日志量级下一年也就几万行，SQLite 无压力；将来要剪也容易（保留每 guid 最新一条即可）。

## 2026-07-07 F9 导入提取的实现取舍（persona_import）
- **不引入分词库，用"语气词词表 + 中文 3–4 字 n-gram"凑高频短语**：加 jieba 类依赖只为一个初稿功能不值；n-gram 的代价是重叠窗口噪音（同一句「稳了稳了哈哈哈」会产出 稳了稳了/了稳了哈/冲就完事 等一堆计数相同的碎片），迭代了三轮才收敛：① 中文 gram 只取 3–4 字（2 字 gram 基本全是边界碎片，真实 2 字口头禅由词表覆盖）；② 贪心选取时，与已选短语共享任意 2 字片段的候选直接压掉；③ 平局排序加"以结构助词/笑字开头者降权"（只罚开头不罚结尾——中文口头禅常以「了」结尾如「完事了」，但以「了」开头必是窗口滑过词边界）+ 字典序兜底保证确定性（HashMap 迭代序不稳定，同分候选会随机换人）。
- **连笑归一化**：哈哈哈哈…（3+ 连字）先折叠成 2 连再统计，否则一条狂笑消息会把「哈哈」计数刷爆；计数口径统一为"出现在多少条消息里"而非总次数，防单条刷屏。
- **昵称（怎么称呼用户）不从记录里推断**：那是"别人怎么叫你"，跟"你希望 PA 怎么叫你"是两码事，留给用户手动填。
- 顺带：改完提取逻辑要记得重新 `cargo build -p pa-cli` 再跑冒烟——`cargo test` 不重编 CLI 二进制，拿旧 exe 验证新逻辑白看半天。

## 2026-07-07 Phase 3 开工前两项设计决策（用户拍板）
- **F9 聊天记录导入管道：强制纯本地，不给云端开任何口子。** 决定性理由不只是"本地优先"原则，还有：聊天记录包含**对话另一方**的内容，对方从未同意上云——这跟用户自己的日程/日志性质不同。提取用本地规则/统计（高频词、语气助词、句长分布、常用表达），效果打折可接受，产出只是人格"初稿"，Persona v1 的手动编辑就是兜底。附带硬规则：原始聊天记录不进同步管道，只同步用户确认过的人格版本。已写入 ARCHITECTURE.md §3.4。
- **Sync Server 密钥管理：自托管场景，从简。** 用户明确"服务器也是我自己的，不用这么谨慎"——威胁模型里没有"不可信服务商"，因此**否决**了原建议的配对码 + 公钥交换 + 密钥轮换方案（为自用两三台设备做这套，复杂度收益比太低）。v1 定为：预共享对称主密钥手动配置到各设备（沿用 `pa-llm.json` 的 gitignored 配置模式）+ TLS + 简单 token 鉴权；数据仍加密落盘（保留"服务器被攻破拿不到明文"的底线，因为成本近乎为零）。设备吊销 = 换密钥全量重传。将来多用户/上架再升级。已写入 ARCHITECTURE.md §3.8。
- 同时敲定 Phase 3 推进顺序（移动端壳 → F9 导入（可并行）→ 同步 → 穿戴殿后），并把 §7 待决清单中已决两项划掉、Importance Classifier 初值改为"用 Phase 2 积累的真实数据固化"、无障碍合规标记为"自用侧载阶段搁置"。

## 2026-07-06 项目启动决策记录
- 项目位置定为独立仓库，与 lx-music 无关，避免混在一起。
- 客户端技术栈选定 Tauri 2.x（而非 Electron/Flutter），理由是 desktop+mobile 可共用前端，系统级权限（通知/无障碍）通过原生插件控制更精细。见 ARCHITECTURE.md 第 5 节。
- 使用范围定为"自己 + 多设备同步"，暂不考虑多用户/多租户，架构因此可以简化（无需鉴权体系）。
- 多设备冲突解决计划复用此前桌面统计模块验证过的 CRDT 模式，不重新设计一套。

## 2026-07-06 原始需求 ↔ 架构映射审计（应用户要求复核）
用户贴回最初的 9 条原始需求，要求复核架构是否合理完善。结论：**全部有映射、无遗漏**——1→F1/F2（已实现，移动端无障碍在 Phase 3）、2→Phase 1 路线（先桌面 demo）、3→F3/F4（Phase 2）、4→F5（Phase 3）、5→F6（Reasoner seam 已留）、6→F7（已实现且比要求强：编译期约束）、7→F8（增强为分维度）、8→F9+F15（人格版本化是架构补充）、9→F10（Phase 2）。架构额外补的 F11–F17 属合理延伸（F12 台账与 F16 离线兜底是信任地基）。真正薄弱点是三处现实约束，均已在 §7 待决清单：移动端无障碍权限的商店审核、国内穿戴设备 API 开放度（个人开发者可能只能蓝牙直连）、同步密钥管理未设计。

## 2026-07-06 云端推理 provider 改为小米 MiMo（用户决定）
- 架构稿 §3.6/§5 原定默认 Claude API；用户提供了 MiMo token-plan 的 key 与 base_url，Phase 2 的真实 Reasoner 据此实现。接口是标准 OpenAI 兼容 `/chat/completions`，`pa-core::llm` 不绑定任何厂商，将来换 provider 只改配置。
- 实测 `/v1/models` 可用聊天模型：`mimo-v2.5`（默认）、`mimo-v2.5-pro`；其余为 ASR/TTS。
- **key 管理**：`pa-llm.json`（仓库根目录，已 .gitignore）或 `PA_LLM_*` 环境变量；代码与文档中永不出现明文 key；`llm-status`/UI 只展示尾 4 位。
- **同步阻塞式 HTTP（ureq）**而非 async：pa-core 全库同步，为一个增强路径引入 tokio 不划算；30s 超时兜底。若将来 UI 反馈卡顿，再考虑在壳层把 ingest 挪到后台线程。
- 隐私边界执行情况：每次云端调用只携带「当前一句话 + 当前时刻」，行为日志/台账/人格一概不出本机，符合 §4 第 1 条。

## 2026-07-06 常驻 ticker 的时钟选择
- ticker 只认**系统时钟**，UI 的「模拟时钟」不驱动它——否则拨模拟时钟会把真实提醒全部触发掉，演示器污染真实数据流。模拟时钟仍作用于所有手动操作（录入/试探问询/手动生成建议），两套并存互不干扰。
- OS 通知失败静默忽略（`let _ =`）：窗口内角标/列表始终兜底，通知渠道挂了不能影响提醒落库（F16 精神）。

## 2026-07-06 Persona v1 与复盘改写的取舍
- **人格存 SQLite `persona_versions` 表，而非架构稿 §3.4 原文的独立 `persona_profile_vN.json` 文件**：单一权威存储（备份/同步/F12 删除语义都只走一条路），表内容就是可读 JSON，"用户能看懂画像"这一点不受影响。ARCHITECTURE.md §3.4 已加实现注记。
- **回滚 = 移动活动指针，不是把旧版本复制成新版本**：历史保持线性且不膨胀，任何版本永远可再回去；代价是"当前用的是 v1"这种状态没有独立版本号记录——对单用户本地工具足够。
- **人格不入 F12 记忆台账**：它有自己的专属视图（查看/编辑/回滚/清空），删除权由 `persona-clear` 提供；塞进台账反而要为"删除某个中间版本"发明语义。若将来做记忆全景审计再考虑并入。
- **复盘改写的防幻觉手段选了"非零计数必须原样出现"的本地校验，而不是让云端输出结构化 JSON 再本地渲染**：后者等于让云端重新决定说什么（失去了"按人格自由措辞"的意义），前者保住事实底线、把措辞完全交给模型。子串匹配故意从宽（如要求"1"时"12"也算命中），目标是抓丢失/编造，不是管文风。
- 实测观察：MiMo 对"计划 1 次提醒、触发 0 次"会自作主张补一句原因猜测（"可能跟日程调整有关"），提示词追加"不要替数字猜测原因"后消失。零计数不强制出现在改写稿里（"高危 0 次"说成"没有高危操作"是合理措辞）。

## 2026-07-06 pa-app（Tauri 壳）实现的取舍
- **前端是纯静态单文件 `dist/index.html`（vanilla JS），不引入 npm/Node/打包器。** Phase 1 的 UI 复杂度撑不起一套前端工程；`withGlobalTauri: true` 直接用 `window.__TAURI__.core.invoke`。将来要上框架时 `frontendDist` 指向新的构建产物即可，命令层不用动。
- **`Tool` trait 加了 `Send` 约束**：Tauri 托管状态要求 `Send + Sync`，`Mutex<Orchestrator>` 需要 `Orchestrator: Send`，而工具注册表里是 `Box<dyn Tool>`。工具本就应当可跨线程，属于把隐含假设显式化，对现有实现零影响。
- **`guard_confirm` 在 Rust 侧一步完成 `confirm`（签发一次性令牌）+ `run_tool`（花掉令牌）**：令牌与 `Grant` 从不出现在前端/IPC 层，前端只是「人类点了确认」这一事实的传声筒，护栏的类型层兜底不因加了 UI 而变薄。
- **弹窗一律用自绘 modal，不用 `window.confirm`**：WebView 环境下原生对话框行为不可靠，且自绘可以展示后果预览等富内容。
- **DB 默认与 CLI 共享 `pa.sqlite`（按当前工作目录），`PA_DB` 环境变量可覆盖**；打包分发（bundle）暂关（`bundle.active: false`），Phase 1 只需要 `cargo run` 起窗口。
- **系统级 OS 通知（托盘/toast）未接**——需要 tauri-plugin-notification 与权限配置，当前用页面内到点角标 + 提醒中心演示通知闭环，OS 渠道列入后续。
按 CLAUDE.md「不一致要先在文档里说明」的要求，记录本轮实现相对架构讨论稿的具体选择——都属于 Phase 1 简化，不改变整体设计意图：

- **先做 Rust 核心（`pa-core`）+ CLI（`pa-cli`），暂不搭 Tauri UI。** 架构的「大脑」是 Agent Orchestrator（Rust 核心），它 UI 无关且能在 headless 下 `cargo test` 完整验证核心闭环，是本轮性价比最高、最可测的落点。Tauri 壳、前端、移动端原生插件留待后续；`pa-core` 的 API 已按「被 UI 调用」设计，不会白做。
- **时间用 `NaiveDateTime`（本地墙钟）+ 注入 `now`。** Phase 1 不引入时区，换来纯函数可测（解析、排程、护栏都吃外部时钟）。时区/DST 感知留到需要跨时区同步时再补，届时集中在 `time_parse`/`model` 两处。
- **规则表每种事件类型用 `Vec<LeadTime>`（而非单值）。** 架构示例是单值（exam 3d 等），这里做成列表以支持分层提醒（如考试 3d + 1h），**默认值严格等于架构单值**，是超集不冲突。
- **配置（规则表、主动度）存在 `meta` 表的 JSON 里**，没有为它们各建表。Phase 1 配置量小、整体读写，JSON 更省事；将来要做字段级编辑/同步再拆表。
- **高危工具用「模拟」实现（`demo_delete` 不碰真实文件系统）。** Phase 1 没有接真实的破坏性工具，但护栏链路（请求→确认→一次性令牌→执行/拒绝→审计）已端到端跑通并测试。真实工具接入时只需实现 `Tool` trait + 声明 `risk_level`，护栏对它们天然生效。
- **护栏用「能力型令牌」在类型层面兜底**：`Grant` 只能由 `guard.rs` 内部构造，`Tool::execute` 必须收 `&Grant`，因此模块外无法绕过 Guard 直接执行高危工具——把架构 §3.3「执行层本身没有直接调用权限」落成了编译期约束，而非仅运行期检查。
- **新增依赖 `regex`**（架构未列）：中文时间/实体解析离不开它，且纯 Rust、构建无副作用。其余依赖（chrono/rusqlite-bundled/serde/thiserror/getrandom/clap/anyhow）均在架构选型范围内。
- **`Reasoner`（云端 LLM 网关）只留了 trait + `NullReasoner` 占位**，没接真实 Claude API：headless 无 key/无网时无法测，且离线兜底（F16）本就要求不依赖云端。真实网关（脱敏/最小化上下文，架构 §3.6）在有 key 的环境再实现，接口已就位。

## 2026-07-15 装了两个第三方前端设计 Claude Skill（impeccable + taste-skill）

**背景**：用户要求给 Claude 装两个 GitHub 上的前端"品味"技能，用来在写 UI 代码时约束设计质量、防止生成风格平庸的界面。两者都通过 `npx <pkg> install` 交互式安装脚本完成（下载并执行第三方脚本，用户自己在终端跑的，不是 Claude 代为自动执行——涉及"从不可信来源下载执行代码"，按规矩这类操作只能由用户本人触发）。

**装了什么、落在哪**：
- **impeccable@3.2.1**（[pbakaus/impeccable](https://github.com/pbakaus/impeccable)）——项目级安装到 `.claude/skills/impeccable/`（`SKILL.md` + `reference/` + `scripts/`），并在 `.claude/settings.local.json` 注册了一个 `PostToolUse` hook：每次 `Edit|Write|MultiEdit` 之后跑 `scripts/hook.mjs`，检测 UI 文件改动、把发现的问题回灌成 system reminder。本地配置落在 `.impeccable/config.local.json`。**这个 hook 会在此后每次改前端代码时自动触发**，如果发现它误报太多或拖慢节奏，可以用 `/impeccable hooks off` 关掉。
- **taste-skill v1**（design-taste-frontend-v1，[Leonxlnx/taste-skill](https://github.com/leonxlnx/taste-skill)）——通过另一个安装器（`npx skills add`）装的，选择目标是 "Claude Code"，但实际落地在 `.agents/skills/design-taste-frontend-v1/SKILL.md`（"Universal" 目录，多个 CLI agent 共享的约定），**没有**像 impeccable 一样出现在 `.claude/skills/` 下。仓库根目录新增了 `skills-lock.json`（该安装器的版本锁定文件）。**已查证 Claude Code 目前不读 `.agents/skills`**：官方文档只提到项目技能来自 `.claude/skills/`（从工作目录向上扫到仓库根），支持 `.agents/skills` 还只是一个 open 的 feature request（[anthropics/claude-code#66352](https://github.com/anthropics/claude-code/issues/66352)）。因此把 `SKILL.md` 又复制了一份到 `.claude/skills/design-taste-frontend-v1/`，两边内容目前一致；`.agents/skills/` 下的原始安装保留不动（给其他也支持这个约定的工具用），以后 taste-skill 升级要记得两边都更新，或者干脆等 Claude Code 原生支持 `.agents/skills` 后删掉 `.claude/skills/` 里这份手动拷贝。

**`.gitignore` 更新（用户 2026-07-15 拍板）**：`.claude/`、`.agents/`、`.impeccable/`、`skills-lock.json` 整体作为"本机 AI agent 工具配置"忽略，不进仓库——用户明确选择"本机工具"而非"团队共享技能"这条路，所以连 `skills/impeccable/`、`.agents/skills/` 这些技能内容本身也一并忽略，不只是 `*local*` 配置文件。以后想让这两个 skill 对其他协作者也生效，需要单独把对应子路径从忽略规则里摘出来再提交。

## 2026-07-18 Phase 8.2：受控日程推送的边界选择

**用户授权与范围**：用户明确授权修改 Soull 接收端；首批仅允许 PA 日程事件，并且只能在 Soulous 接收用户自行开启 AI 记忆后才进入 RAG。由此把实现收窄为一个可关闭的类别和一条单向接收链路，而非泛化的“同步”。

- **不做后台自动推送。** 即使用户已打开类别白名单，仍须在每条日程上显式点「推送」并通过 Sensitive Tool 的一次性 Guard 确认；这是当前“用户允许才可以”的代码化表达。自动批量同步、重要度/任务/场景/穿戴等新类别都需新的用户拍板。
- **投影比架构 L2 总表更窄。** v1 只发送标题、类型、开始/结束时间和地点；不因为字段“也许属于 L2”就带上重要度、参与人或任何来源线索。预览直接由将要序列化的投影生成，防止界面承诺和实际 payload 脱节。
- **`local_only` 在核心层二次兜底。** 前端隐藏入口不作为安全边界；任意 IPC/CLI 调用若试图推送第三方通知来源的事件，`pa-core` 都会拒绝。推送 HTTP 不调用 ingest、提醒 ticker 或 sync，避免派生副作用。
- **Soull 只入、不回流。** 接收端按 JWT 用户隔离、稳定外部 ID 幂等覆盖、严格拒绝未列 payload 字段，且刻意不提供 PA 读取这些外部上下文的 API；这样 RAG 中带 `source=pa` 的事实不会经现有 Soulous → PA 拉取路径回声放大。AI 记忆关闭时仍可保留用户自己的外部事实记录，但绝不新建 embedding。

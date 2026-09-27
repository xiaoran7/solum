# Solum Changelog

## 2026-09-27 — solum-core 的 extract.rs 拆为分节模块

- `crates/solum-core/src/extract.rs`（717 行）切成 `extract/` 下 10 个分节加 `mod`。纯移动，无逻辑改动。6 个条目被放开到 `pub(super)`，全部经验证拆分前确为私有。
  分法按「这一步在决定什么」切：`kind`(33，**分类本身，即那个类型**) / `words`(89，路由要匹配的词表) / `widget`(29，离线小组件路由) / `eventwords`(21，日期解析前面那道门) / `reschedule`(59) / `cancel`(27) / `intent`(41，**分类顺序本身**) / `reasoner`(75，云端接缝与空实现) / `extractor`(197) / `tests`(178)。`kind` 单独成节是因为 `Intent` 和返回它的 `route_intent` 是同一个想法，而顺序的每条约束都要对着变体才读得通——`intent` 里那两条注释（「改期要在可调度之前测」「记住明天3点开会是事件、记住我不吃辣是事实」）只有看着变体才说得清。
- **`macro_rules!` 不是路径，这是拆分的第三类跨分节引用**（前两类是条目和字段）。父模块里的 `lazy_re!` 用 `use super::*;` 到不了任何分节——它没有命名空间可以被具名，`use super::parent::lazy_re` 也不存在——而 `mod` 块是锚在**第一条 `use`** 上生成的，于是这个宏落在所有 `mod` **下面**。2018 版起父模块的宏对子模块**按词法可见**，所以修法**不是加 `#[macro_use]`**（那只是 2015 版的遗留路径），是**把宏挪到 `mod` 块上面**。
  症状是这一批里最坏的：属性加上了、每一步都报成功、`cargo check` 仍然说找不到宏。**属性在、宏不可达**，两个事实同时成立，所以任何查「有没有加属性」的检查都会通过。新增的提升步骤只做位置这件事，并且控制组断言的是**树能不能编译**，不是属性在不在。
  附带一条：定位它时用了两个自己骗自己的探针。一个的正则路径类是 `[a-zA-Z_:0-9]+[\/][a-z_]+[\/]`，而 crate 目录叫 `solum-core`——**连字符两类都不匹配**，于是在一条就躺在日志里的诊断上得到 0 命中；旁边的 `UNRESOLVED`/`PRIV_METHOD` 用的是 `.*?` 懒惰前缀，所以它们好好的，三条同形正则里只有一条不对。另一个是 `node -e '...'`：shell 会吃掉 `[\\/]` 里的反斜杠，探针自己打印的 `src` 于是和文件里写的不是同一行。**探针必须从文件读工具真正用的那份正则**，否则测的是探针自己的抄写。
- **带着走的 `///` 会和下一节原有的注释合成一段。** `carry-doc-comments.mjs` 去空行的前瞻原来只认「下一行是条目开头」，而带着的整块注释落在**下一节本来就有的注释**前面时，中间那个空行落在**一段注释的中间**——`empty_line_after_doc_comments` 于是报在一个**单看完全正常**的 `///` 行上。前瞻现在也接受「下一行还是 `///`」。
- **拆分前 `pub` 而拆分后没人够得到的类型，rustc 会开始报 dead_code**，而修法不是 `#[allow]`：`NullReasoner` 拆分前是 `pub`（所以没有告警），进到私有模块后就没人构造了，诚实的修法是把它也转出——转出表本来就该按「拆分后确有外部文件到达」列。另有 3 个 `use super::*;` 转成 0 警告后删掉。
- 8 条控制组（`negative-control-extract-macros.mjs`），其中**两条在控制组自己身上栽了**，都是把「机制是什么」认错了：
  1. 断言「用到它的分节带 `#[macro_use]`」——而这棵树里**根本没有这个属性**，而它编译通过。第一版控制组照着「加了属性所以通了」写，工具却发现不需要属性。**控制组要断言实测出来的机制，不是设想的那个。**
  2. 断言 clippy 输出里有 `empty_line_after_doc_comments`——`--message-format short` 打的是**消息文本**，lint 名根本不出现。可观测的判据应该是「消息 + 它指名的文件」两样都有：只查 lint 名是在测 cargo 的输出格式，不是在测工具。另外这条还要 `utimesSync` 一下——cargo 按 mtime 判指纹，**把相同的字节写回去不算改动**，于是第一次跑 clippy 看的是一个没重编的 crate。
- 验证：11/11 分节与拆分前逐 token 相同；`cargo test --workspace` 487 通过、0 失败；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；8 条控制组全部按预期报错；暂存树贴进干净 HEAD 的 worktree 后同样 487 通过、0 失败、fmt 干净、clippy 零告警。
## 2026-09-27 — solum-core 的 backup.rs 拆为分节模块

- `crates/solum-core/src/backup.rs`（930 行）切成 `backup/` 下 6 个分节加 `mod`。纯移动，无逻辑改动。`tests` 节的 4 个 `include_str!` 重新落到 `../../../tests/fixtures/backup-v1/`。
  分法：`format`(118，容器**是什么**——每个魔数、帧长、尺寸上限都写成具名常量，编解码器和它的调用方于是读同一组数字) / `seal`(139，明文进容器出) / `open`(118，容器进、验过的文档出，**刻意不接 `Store`**，所以这里失败不可能部分改动本地数据) / `validate`(86，开封前跑的那些检查；单独成节是因为它是第二个读这个格式的人必须重新实现的那部分规则) / `helpers`(93，两个方向共用的原语) / `tests`(402)。
- 20 个条目加 `DecodeLimits` 的 2 个字段被放开到 `pub(super)`，全部经验证拆分前确为私有。**字段可见性是独立的一层**：一个类型可能因为某个兄弟分节要提到它而放宽，同时它的字段因为另一个分节要读而单独放宽，两件事不相关，所以 `WIDENED` 里要写成 `DecodeLimits:fields+`——`fields` 表示「只动字段」，`+` 表示这个类型自己原本的 `pub(super)` 是拆分加的。
- 这次流水线本身有一处缺陷，形态是**「按错误码分派，却漏了一类」**：`wire-until-clean.sh` 的导入步骤在失败时数 `E0603`/`E0624` 两种错误，数到 0 就当成「这一步做完了」并继续。但 `E0616`（字段私有）**不是那两种之一**，而它是 `widen-fields.mjs` 的活——于是树带着 7 个 E0616 走进下一步，被报告成「本轮无可放宽的私有项」，循环退出时那 7 个字段仍然是私有的。修法是**每个错误码都要说清它归谁**：E0603/E0624 归放宽轮次，E0616 归字段放宽步骤，其余才是这一步自己的问题、才该放弃。判据是「循环结束时是否干净」，不是「这一步能做的做完了吗」。
- 13 条控制组（`negative-control-verify-backup.mjs`），其中**三条一开始在控制组自己身上栽了**，而且失败的样子和「验证器有 bug」一模一样：
  1. 「函数体里的算术改动」这条的锚点写的是 `MAX_HEADER_BYTES`——它只出现在 `format` 里，`validate` 里根本没有；而真正的目标 `limits.plaintext as u64` 已经被 rustfmt 折到了下一行。sed 一处都没替换，文件**逐字节没变**，验证器读一棵没被扰动的树，报「通过」。
  2. 「include 路径」这条用的是 `payload.json`，而它**正是**拆分重新落到 `../../../` 的那一个——`PATH_GAIN=2` 的回退会把它改回去，两边仍然相等。回退**按设计**会撤销它自己做的改动，所以要测「路径指向的文件不存在」必须去动 `container.hex` 里的**文件名**，那是回退不管的部分。
  3. 「分节内条目重排」这条用的是 `open` 节，而它**只有一个条目**，「移到末尾」等于没动。同样是空操作，同样报「通过」。
  修法是每条用例**先断言自己的前提**：锚点字符串在不在、替换后文件有没有真的变、这一节是不是有两条以上的条目。前提不成立就**报出来**，而不是让断言安静地通过。**一次没有真的扰动的控制组，对被测物一无所知**——它测的是它自己写对了锚点。
- 验证：7/7 分节与拆分前逐 token 相同；`cargo test --workspace` 487 通过、0 失败（与基线逐项一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；13 条控制组全部按预期报错；暂存树打成补丁贴进干净 HEAD 的 worktree 后同样 487 通过、0 失败、fmt 干净、clippy 零告警。
## 2026-09-27 — solum-core 的 orchestrator/tools.rs 拆为分节模块

- `crates/solum-core/src/orchestrator/tools.rs`（948 行）切成 `tools/` 下 8 个分节加 `mod`。纯移动，无逻辑改动，本次无 `include_str!` 需要重写。13 个条目加 1 个方法被放开到 `pub(super)`，全部经验证拆分前确为私有。
- **分节按这些工具做什么切，不按「每行装几个工具」切。** 先试了按体积分成 7 个一组，结果 `DataImportTool` 的 `struct` 和 `impl Tool for DataImportTool` 被切到两个分节，编译器给 E0277「trait bound not satisfied」——**不是**「找不到这个类型」，错误行还指向 `Box::new(DataImportTool)`，读起来像注册表写错了。改按职责切之后，那两个 impl 仍然分处 `records` 和 `ledger`。Rust 允许跨分节实现一个类型，所以这不构成硬性限制，但**一旦允许，分节边界就变成任意的**——那说明该重新想怎么切，而不是让工具迁就。最后把 540/541 之间的边界挪开：`records` 到 540 结束，`ledger` 从 541 的 `impl Tool for DataImportTool` 开始，类型留给声明它的分节，实现跟着走。
  最后的分法：`exec`(75，受护执行入口：确认 → 一次性令牌 → 执行 → 落审计，执行完还要重载缓存，因为工具是**从背后**经 `ToolCtx.store` 写库的) / `parse`(243，纯解析：意图、层级与事件种类标签、清除请求及其天数范围、复现措辞与「这些措辞没有正确的调度模型」的caveat、改期合成、标题匹配前剥掉的填充词) / `registry`(30，内置工具清单，单独成节是因为它是唯一必须一次看全所有工具的地方) / `echo`(42，只报告不改动的两个：echo 与全流程模拟删除) / `records`(184，改写整组行：数据导入、备份删除、账本清除) / `ledger`(135，删用户能指名的单个东西：记忆层、事件、人格、挂件记录) / `channel`(137，对外发出去的三件：挂件删除、Soulous 推送、邮件发送) / `previews`(157，预览行的呈现，含 `truncate_for_preview`——放在 `previews` 而不是 `channel`，是让新增一个对外工具时有一个地方可以查「预览该怎么截断」)。
- 这次拆分里**流水线本身有六处缺陷，每一处都表现为「报告成功而树是坏的」**，比代码改动本身更值得记：
  1. `fix-section-imports.mjs` 用「文本里有没有 `error[`」判断「无事可做」就返回成功，而 `cargo --message-format short` 对**解析错误**打的是没有方括号码的裸 `error:`，于是三个 E0761 全部漏过，打印「no unresolved names」并 exit 0。修法是把「有没有可动的东西」和「能不能编译」分成两个问题，后者只看退出码。
  2. 循环脚本调它时不看退出状态，于是这一步失败了也照常往下走。改成看退出码，而且要分清它**停在哪类错误上**：E0603/E0624 是「放宽」这一步的活（导入是它引出来的），遇到这类错误应当继续；只有「没有任何分节声明这个名字」和「根本不是未解析名」才该放弃。
  3. 收敛检测比的是**放宽表**。某个名字归不到任何分节时表确实不再变，于是循环在一棵带 2 个 E0624 的树上宣布「已达不动点」。改成同时比**诊断本身**——连续两轮同一批错误就是卡住了，不是收敛了。这个「固定点」判据之前记过一次（字段那一半），这次是它的另一半。
  4. E0624（私有关联函数）和 E0603 是一件事的两种样子，但归属查找把前者当成了后者：方法写在 `impl` 块里、缩进着，`^` 锚点的顶层声明式匹配不到，于是回退到**够到它的那个分节**——恰好是唯一不可能有用的答案。修法两条：E0624 走 `methods` 表（按缩进锚定），归属查找把 `impl Trait for X` 行也当作 X 的声明。
  5. 切点落在 `///` 注释和它注释的条目之间，注释就留在文件末尾什么也没注释（E0761）。这是**解析错误**，单独 `cargo check` 一个分节看不到，要整个模块拼起来才出现——比其余错误都晚。新增 `carry-doc-comments.mjs` 把它带进下一个分节，顺序必须取自 spec 的 `ranges`：用 `readdirSync` 的话注释会落到**另一个**分节上，而那同样编译通过、同样没人发现。附带两条：切分器会给每节补换行，所以判断「本节以注释结尾」必须按**最后一个非空行**；带过去之后注释与条目之间的空行会让 `cargo fmt --check` 失败，而错误指向那个条目。
  6. `wire-until-clean.sh` 把 spec 的 `ranges` 原样传下去，丢了 `:slug` 后缀，而切分器要求 `range:slug`——每轮都「split failed」并退出，看起来像切分器坏了。派生的量和它们**对应的名字**必须成对从 spec 出，长度不一致时拒绝而不是错位：错位一对会得到一棵分节名正确、里面代码错误的树，通过所有只看名字的检查。
- 8 条控制组（`negative-control-doc-comments.mjs`）。其中两条是这次**控制组自己**栽的：扰动只改了临时副本而工具按路径读仓库里的文件，于是两条用例都报「通过」；「新切分树少了文档行」这个前提写成了「仓库里的树少了文档行」，而仓库里的树**已经带过了**，计数相等，前提悄悄变假。**对一件事的控制要能区分「工具做对了」和「前提不成立」**，所以每条用例都先断言前提，前提不成立时**报出来**。
- `kind_label` 从转出表里删掉：它拆分前是 `pub(crate)`，被转出后 clippy 报未使用导入——`review.rs` 有自己的私有同名函数，`tools::kind_label` 从来没人够到过。转出表应按「拆分后确有外部文件到达」列，不是按「mod.rs 看起来完整」。
- 验证：9/9 分节与拆分前逐 token 相同；`cargo test --workspace` 487 通过、0 失败（与基线逐项一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；暂存树打成补丁贴进干净 HEAD 的 worktree 后同样 487 通过、0 失败、fmt 干净。
## 2026-09-27 — solum-core 的 profile_generation.rs 拆为分节模块

- `crates/solum-core/src/profile_generation.rs`（897 行）切成 `profile_generation/` 下 6 个分节加 `tests`：`types`(124，目录常量与指针/清单/工件的数据形状，**加**「解析当前活动目录」——它必须和它读的那个形状住在同一节，因为两者一起定义了这个格式) / `stage`(72，开工：建暂存目录、密封成不可变世代) / `publish`(113，提交：把暂存目录**原子地**换成活动世代) / `verify`(203，准入：SQLite 镜像完整性、指针格式、清单形状、大小与文件数上限、符号链接与 WAL/SHM 旁文件一律拒绝) / `io`(199，共用原语：随机 id、十六进制、JSON 读取、路径越界与符号链接检查、世代锁) / `tests`(212)。纯移动，无逻辑改动，本次无 `include_str!` 需要重写。7 个名字经 `crate::profile_generation::X` 被 4 个文件到达。
- 这次拆分里真正咬人的不是拆分本身，是**验证器的一个错误推断**。26 个条目被放开到 `pub(super)`，其中 5 个结构的字段也被放开（字段的放开由编译期 E0616 驱动，和条目放开是两个独立的不动点）。回退时字段该恢复成什么，早先的代码是**从类型的可见性推出来的**——理由是「放开字段的那一步写的和类型一样的 `pub(..)`」。这描述的是**那一步写了什么**，不是**文件原本是什么**：
  1. `StagedProfileGeneration` 拆分前是 `pub struct` 但**字段私有**，于是推出来的字段文本是 `pub `，而原文一个字都没有。比较报出「一个多出来的 `pub`」，读起来像拆分动了这个结构体。
  2. 这个方向**危险在它编出来的东西是合理的**：`pub struct` 上带 `pub` 字段是再普通不过的 Rust，报告里没有任何一处说「这是猜的」。
  3. 修法不是换个更细的规则，是**去原文里读字段自己的 `pub`**。类型和字段的拆分前文本互不相关，三种组合都真实存在（`pub` 类型 + 私有字段、私有类型 + 私有字段、`pub(crate)` 类型 + `pub(crate)` 字段）。
  4. 附带修掉两处**静默不生效**的正则：具名字段列表的匹配把头组卡在 `{` 上，于是 `body` 从换行开始，`\bpub\(...\)\s+` 没有词边界可打——整段回退变成空操作，而报告把它读成「丢了一个关键字」。同类形状早已在 `struct ParsedChunk<'a> {` 上栽过一次。
- `verify-split.mjs` 的差异报告只打印 **want 一侧**的上下文，而 want 一侧是拆分前的切片、不是被比较的那段文本，于是「多了一个 `pub`」和「少了一个词」看起来一模一样。新增 `_scratch/probe-diff.mjs` 把两侧并排打出来。它是 verify 的 `tokens` 的**逐字拷贝**而不是仿写：仿写版本把 `#[derive(..)]` 当 5 个 token、裸字符串当 3 个，在报出任何差异之前就已经和验证器差了 20 个 token，于是它报告的「首个差异」是验证器早就同意过的那个位置——一个探针照着和验证器不同的规则分词，报出来的差异就不是差异。
- 10 条控制组（`negative-control-fields.mjs`），其中三条是这次新增的：改动结构体字段名必须被抓到；字段被放到比拆分更宽的 `pub` 也必须被抓到（回退只认 `pub(super)`/`pub(crate)`，裸 `pub` 会**幸存**下来——只有前面那条的控制的话，回退把满文件的 `pub` 都删掉也能通过）。还有一条是**控制组的控制组**：本文件的任何历史里都没有「`pub struct` + `pub` 字段」这个形状，所以只有专门造的 fixture 能把「读字段」和「读类型」两条规则分开——而它们恰好在掩盖真实改动的方向上不一致。
- 控制组自己踩了两个坑：① 扰动只改了临时副本，验证器却拿 `--dir` 当参数直接按路径读，**根本没看见扰动**，于是两条新用例都报「通过」；② 早期版本把 `prevPath` 写成了 `_scratch/` 里的快照，于是每条 `include_str!` 都解析到错误的目录——快照和仓库里的文件深度不同，这类工具错起来是**系统性的**，不是某一条路径的意外。
- 验证：workspace 487 通过、0 失败（与基线逐项一致）；8 个 profile_generation 测试全过；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警。暂存树打成补丁贴进干净 HEAD 的 worktree 后 `cargo test --workspace` 同样 487 通过、0 失败。
- 工作区里 solum-app 有 2 个 clippy 错误（`recovery_code` never read）属于在途的 remove-recovery-code 分支，未改动；另 2 个（`tauri::Manager`、`chrono::TimeZone` 未使用导入）在 HEAD 的干净 worktree 里同样存在，是先前拆分留下的基线问题，与本次无关——clippy 的基线是**量出来的**，不是假定的。
## 2026-09-27 — solum-core 的 attachment.rs 拆为分节模块

- `crates/solum-core/src/attachment.rs`（993 行）切成 `attachment/` 下 4 个分节加 `tests`：`format`(132，容器格式本身——每个魔数、长度与体积上限都是具名常量；只写在执行它的那段代码里的上限，是没人能对照检查的上限，所以对外的 ceiling 是 `pub`，协调方执行的和编解码器执行的是同一批数字) / `seal`(152，明文进容器出，`validate_attachment_set` 是写任何东西**之前**对整组的预检：条数、总字节、整组必须同属一个 user / device generation / backup) / `open`(145，反方向，且刻意更严——任何明文字节在交给调用方之前都已过认证并核对清单摘要) / `helpers`(156，两个方向共用的那些：密钥派生、AAD、长度前缀分帧、决定容器该有几块的边界算术；与 `seal`/`open` 分开，谁都不持有对方的规则) / `tests`(390)。纯移动，无逻辑改动。9 个名字经 `crate::attachment::X` 被 10 个文件到达。
- **这次拆分里有一处不是移动，是必须做的真实编辑**：分节下移一层，`include_str!` 的相对路径就断了。新增 `fix-include-paths.mjs` 专门做这件事，并配 10 条控制组——因为流水线里每加一个「真的改东西」的步骤，控制组就是它唯一的诚实性保证。
- 三处缺陷全是「静默地给出错误答案」，其中一处编译器和测试都不一定当场抓到：
  1. **「下移了几层」不是正确算法**。原路径 `../tests/fixtures/x.hex` 写在 `src/attachment.rs` 里，指向 `crates/solum-core/tests/`——**在 `src/` 之外**；分节只比 `src/` 深 2 层，于是得出 2，写出一个不存在的路径。改成**先解析再重新表达**：在拆分前文件所在目录里解析成绝对目标，再从新位置重新表达同一个目标。深度与 `../` 个数根本不是一回事。
  2. **重写必须替换原有的 `../` 串，不能前缀上去**，否则每跑一次多一层，第一遍修好第二遍就坏。工具因此必须**幂等**：从 `--pristine` 的原始副本取字面量，而不是从磁盘上当前的文本取。
  3. **按 `readdirSync` 顺序消费一个全局列表会错位**，于是某个路径被拿**另一个**原始字面量去重算——结果能解析、能编译、指向另一个文件。需要 `--order` 显式给出分节顺序。
  4. 第四处更隐蔽：**找不到被 rustfmt 折行的字面量**。参数前可能隔着换行和缩进，只跳空格与制表符就会漏掉每一个已格式化的路径，于是工具打印「无需重写」——一句看起来像成功的报告。
- 验证器的回退要减「**增加量**」而非「结果总数」：原路径本来就带 `../`（`../tests/x.hex` → `../../../tests/x.hex`，增量是 2），减全量会让两侧差恰好一个原始计数，看起来像「丢了一个路径」。这里刻意只做**盲减**而不重新解析——验证器手里只有拆分前的文本和分节自己的文本，没有各自的写入目录，算不出目标；而一个靠猜的重新解析会**放行**一个指向错误文件的路径，那正是这个检查存在的意义。
- 28 个条目可见性放开（`format` 的 13 个常量/类型、`helpers` 的 13 个函数、测试用到的 `seal_attachment_inner` 与 `open_attachment_with_limits`），全部经验证拆分前确为私有。另有 12 个结构体**字段**由编译器驱动的 `widen-fields.mjs` 放开——字段的可见性不写在 spec 里，因为静态预测不可靠：一个私有字段的名字会出现在别的分节的字符串、文档注释和无关结构体里，文本扫描不是过度放开就是漏掉，而编译器是权威。
- 验证：workspace 487 通过、0 失败（与基线逐项一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；12 个 attachment 相关测试全过，含 `fixed_cross_platform_fixture_is_byte_stable_and_readable`——正是它读的那个 fixture 路径需要修的那一个。暂存树单独验过：`git diff --cached` 打成补丁贴进干净 HEAD 的 worktree 后 `cargo check` 通过。
## 2026-09-27 — solum-core 的 notification_intelligence.rs 拆为分节模块

- `crates/solum-core/src/notification_intelligence.rs`（944 行）切成 `notification_intelligence/` 下 4 个分节加 `tests`：`config`(238，什么算值得留的通知、以及留住它们的那几条规则，`normalized()` 是那扇 fail-closed 的门) / `capture`(104，一条被捕获的通知及其存储形态；`MAX_FIELD_CHARS` 是隐私边界，在**写进去之前**就截断) / `proposals`(260，先判定再征求确认的那一半：过滤与动作提议、装机起始规则、`decide_intake`——这里不做任何动作，最多是写一行要用户确认的记录) / `triage`(207，可选的 LLM 批次通道) / `tests`(128)，`mod.rs`(13) 保留原文档头与 imports。纯移动，无逻辑改动。22 个名字经 `crate::notification_intelligence::X` 被 17 个文件到达，mod.rs 显式转出全部对外项。
- 第一版切口把 `LlmTriageDecision` 的文档注释留在了上一节，编译报 `expected item after doc comment`——正是先前记为「已知盲区」的那一种。切口从 615 上移到 612，注释与它的枚举一起走。
- 22 个对外名字不再靠记忆，改由 `external-names.mjs` 从树上扫出来（含跨行 `use mod::{...}` 块与 `mod::Name::Variant` 路径式引用），并按文件分组打印。第一次跑就抓到我漏掉的 `IntakeDecision`——它在 `proposals`（`decide_intake` 的返回类型）而不是我以为的 `triage`。
- 边界守卫的词法器**改为跨行有状态**：此前逐行扫描，而 Rust 的字符串字面量可以合法跨行。`batch_triage_prompt` 的提示词是一个用行尾反斜杠续行的普通 `"..."` 字面量，逐行词法器在第 1 行开、第 2 行关，于是把第 2 行里字面量自己的 `{{` 当成代码、又数了 2 个幽灵深度，把一个干净收在 `}` 上的分节判成「留下未闭合条目」。现在普通/原始字符串与块注释都延续到下一行。23 条样例单测覆盖（含 4 条**必须仍然报「未闭合」的正例**，否则守卫会变成永远放行）。
- 同一处守卫还有个新缺陷：字符串开头的判据漏了 `r`。`r##"..."##` 的 `r##"` 于是被当作三个普通字符加一个裸 `"`，开了一个**普通**字符串，它内部的 `"##` 看起来正好像一个从未进入过的原始字符串的终结符。判据改成 `"` / `r` / `b` 三种开头都要试。
- 改完把**已提交的五次拆分全部重跑一遍**，全部仍通过——守卫的每次放松都可能让之前的坏切口蒙混过关。
- 控制组暴露了一件方法论上的事：`notification_intelligence.rs` 里**没有**能让反向检查（上一分节是否留下未闭合条目）单独发力的行——每个列 0 的条目起点都在深度 0，而深度非 0 的行（`parse_batch_triage` 里那对 `#[derive]` / `struct Response`）是缩进的，正向检查先咬住。反向检查要验就得**另造夹具**。只对着真实文件能写出来的控制组，是那个悄悄不再测它本来要测的检查的控制组。控制组本身也栽在这上面两次：先是造了破坏连续性的 mutant（被恒开的平铺检查拦下，什么也证明不了），又误以为「正向检查应当放行」而把反向检查的正常拦截报成失败。
- 验证：workspace 487 通过、0 失败（与基线逐项一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；4 个模块测试全过。暂存树单独验过：`git diff --cached` 打成补丁贴进干净 HEAD 的 worktree 后 `cargo check` 通过，确认拆分自包含于提交。
## 2026-09-27 — solum-core 的 genui.rs 与 export.rs 拆为分节模块

- `crates/solum-core/src/genui.rs`（993 行）按已有的分节横幅切成 `genui/` 下 4 个分节加 `tests`：`templates`(371，事件/例程/打卡/简报/护栏/人格的离线规则构造器，F16 的无需模型路径) / `validation`(184，逐组件逐动作对照目录上限) / `types`(167，线协议：版本、动作白名单、组件目录、`UiEnvelope`) / `parse`(26，F16 降级缝，解析失败返回 `None` 而非报错) / `tests`(242)，`mod.rs`(53) 保留原文档头、imports 与 4 组对外转出。纯移动，无逻辑改动。
- `crates/solum-core/src/export.rs`（749 行）切成 `export/` 下 3 个分节加 `tests`：`coverage`(209，格式常量与覆盖清单，逐条对照 `sqlite_schema` 而不是信任它) / `import`(90，写入侧，刻意分两步：`plan_import` 只说会动什么、`import_document` 才动手) / `build`(35，读取侧) / `tests`(398)，`mod.rs`(37) 保留原文档头、imports 与 3 组对外转出。纯移动，无逻辑改动。
- 两次都靠**已有的分节横幅**切，不需要自造边界。`genui` 的三块横幅（validation / defensive parsing / offline template builders）与 `export` 的「常量+覆盖清单 → 构建 → 导入 → 测试」天然对齐；反过来说，没有横幅的文件（如 `store/sync.rs`，整个文件是一个 `impl Store`，其中单个 `apply_one` 就有 447 行、是一个巨大的 `match op.tbl`）**不适合纯移动式拆分**——按方法切会把 `match` 分支切两半，那已经不是移动而是改写。
- **边界守卫改成同时看两侧**，判据是括号深度的变化而不是末行长相。这一版守卫接受了 `genui` 的三条横幅，却在 `export` 上仍漏了一种：上一分节以**悬空文档注释**结尾（注释属于下一条目）时放行，于是 `build` 段第一版切在 `build_export` 的文档注释之后一行，编译报 `expected item after doc comment`。已记为已知盲区，未修。
- **守卫的放松要求重跑已提交的拆分**。新增 `recheck-all-splits.mjs`：从各分节树反推区间（锚点落在条目的整个前导——文档注释加跨行属性，测试段锚在 `#[cfg(test)]` 本身）后全部重跑。`email`/`export_v3`/`soulous`/`account`/`genui` 全部通过；`account` 起初报 4 处边界，是反推锚点落在 `fn` 行而非条目前导所致，**树本身正常**（25 个 `#[tauri::command]` 一个不少）。
- 反推区间时暴露的正则缺陷：`pub(?:\([^)]*\)\s+)?` 整个组可选，于是裸 `pub` 匹配时**不带走尾随空格**，接着 `fn` 无从接上，`pub fn send(...)` 返回 `null`——所有 `pub fn` 分节都定位不到自己。两支都要各自带空格。
- 词法器的 `'a` 判据原先把字节字面量当生命周期，`(b'a'..=b'f')` 的括号永不闭合、深度多出 2，把一个干净收在 `}` 上的分节判成「留下未闭合条目」。判据改为同时看前一个字符并要求生命周期名字在引号前已结束，用六条样例验过：`&'a str` / `'{'` / `b'a'` / `(b'a'..=b'f')` / 字符串里的 `{` / 注释里的 `{`。
- **「所有条目都还在」的检查此前用列 0 匹配声明**。测试套件在 `#[cfg(test)] mod tests {}` 里整体缩进一层，于是 13 个测试助手一个都没被计入，随后「重复声明」检查把**任何带测试的拆分**判成失败——它此前只在无测试的文件上跑干净过。改为允许前导空白。
- **加宽表条目 `["Name"]` 的默认可见性是 `undefined`**，直接插值进声明变成 `undefinedconst MAX_BUTTONS`，该条目随即从计数里消失，报成「改写丢了条目」。默认改为 `pub(super)`。
- `coverage` 一节在 spec 里声明 `"glob": false`：它只在两条文档链接里提到 `import_document`，不需要兄弟分节的任何名字，而无条件加的 `use super::*;` 就是一条拆分前并不存在的未使用导入告警。加宽脚本此前对每个分节都加 glob，这类「加了没人用」只能等 clippy 报出来再回头改 spec。
- 验证：两次拆分后 workspace 均 487 通过、0 失败（与基线逐项一致）；`cargo fmt --all --check` 通过；`solum-core` clippy 零告警；`genui` 13 个测试全过。两次都用**暂存树**单独验过：`git diff --cached` 打成补丁贴进干净 HEAD 的 worktree 后 `cargo check` 通过，确认拆分自包含于提交、不依赖未跟踪文件。
## 2026-09-27 — solum-cloud 的 postgres-server.js 拆为 lib/ 分节 + 装配入口

- `server/src/postgres-server.js`（3160 行、140 KB）拆为 `server/lib/` 下 19 个分节，入口退化为 100 行的装配器。分节按职责：`http`(443，请求与响应原语、`HttpError`、字段校验、备份计划解析、访问令牌签发校验) / `routes-session`(364，会话与设备) / `routes-backups`(350，备份生成、取回、版本) / `routes-auth`(298，注册登录、短信、华为、OAuth) / `ai-proxy`(291，上游代理、流式转发、超时与断连) / `config`(247，`buildConfig` 与全部上限常量) / `routes-account`(185，账号资料与偏好) / `routes-identity`(176，令牌、密钥、租户档案) / `ai-usage`(166，配额投影与预占) / `tenant`(131，`withTenant`、锁、配额、blob 上限) / `routes-sync`(106) / `server`(82，`createPostgresServer` 的骨架) / `routes-keys`(83) / `routes-alerts`(61) / `routes-captcha`(55) / `preamble`(53，`use strict`、imports、26 个常量) / `startup`(25，`main()` 与 `require.main`) / `routes-root`(22) / `routes-ai`(22)。纯移动，无逻辑改动。
- **入口用 `vm.runInThisContext` 把分节拼回同一个模块作用域，而不是把它们改成 CommonJS 模块**。它们切自同一个作用域，本来就互相按裸名引用（`handle()` 直接调 `bearer()`、`verifyAccessToken()` 和每个路由助手）；改成真模块就得手写 import 表，那是会与拆分漂移的新代码。代价是拿不到逐分节的栈帧，换来的是「分节按原顺序拼回原文件逐字节一致」这条等价性证据仍然成立——它正是 13 处**读原始源码文本**的断言所依赖的（`test/postgres-contract.test.js` 用 `indexOf` 切出路由区间再断言区间内容）。`__dirname`／`module`／`require` 显式传进包装函数：前者让 `dashboard.html`、`../package.json` 和每个 `./` 相对 require 仍按 `src/` 解析，后者让 `startup` 里的 `require.main === module` 依然比较本模块——**这正是 `npm start` 与 `node src/postgres-server.js` 会拉起服务的前提**。
- **单遍求值是硬要求，不是风格选择**。`auth-security.test.js` mock 掉 `http-security` 的导出后**删缓存重新 require** 本模块，`postgres-integration.test.js` 在 require 之前替换 `pg.Pool`——两者都依赖那些 require 发生在本模块**首次**求值期间。惰性按需加载会静默破坏这两个测试。
- 测试侧新增 `test/postgres-source.js`：按入口里声明的 `LIB_PARTS` 顺序拼接 `lib/*.js`。**顺序从入口读出而不是在这里另列一份**，这样分节的增删与重排表现为测试失败，而不是一份悄悄按错顺序读到的源码。13 处 `readFileSync(...postgres-server.js)` 全部改为 `postgresSource()`，断言逻辑与断言文本一字未动。
- 等价性硬证据：`lib/` 19 个分节按声明顺序拼接，与拆分前的 `postgres-server.js` **逐字节完全一致**（140496 字节、3160 个 CRLF 行尾全部保留），测试侧的 `postgresSource()` 同样复现该文本。`server/` 全树是 CRLF（`core.autocrlf=true`），拆分器因此把行尾符一并带进每个分节——只做「归一化后一致」的比较会掩盖这一点，而逐字节这条契约是按**磁盘上的字节**衡量的。
- 工具与负向验证：
  - 拆分器新增**边界守卫**——断言每个新分节的第一条非空行是顶层项（`function`/`class`/`const`…、行注释、`use strict` 指令、`require`，或**缩进 4 空格的路由头**）。路由分发嵌在请求处理器里，缩进正好 4 格。逐字节一致**证明不了**切点正确：一个函数被切成两半，拼回去仍然逐字节相同。
  - 控制组证明守卫真的会咬人：把边界挪到函数体内部（`return value.trim() || null;` 与 `const name = error && …`）必须被**边界守卫**拒绝而不是被拼接检查拒绝——两者都跑通会让人误以为测的是边界守卫，所以断言报错文本里确实含边界守卫那句。
  - 装配器生成后**先解析再写盘**，并校验两张表的**形状**：早期版本把导出表写成裸标识符 `buildConfig,`，那能正常解析，却在 `require` 时抛 `buildConfig is not defined`——只做语法检查正好漏掉这一类。
  - 反向验证过测试夹具真的读的是分节：把 `lib/routes-backups.js` 里的 `createHash('sha256').update(ciphertext).digest()` 改成 base64，`backup routes keep ciphertext opaque…` 立刻失败；改回即恢复。删掉任一分节则在 `require` 时 ENOENT 立即失败，不会得到一个残缺模块。
- 部署：`server/Dockerfile` 增 `COPY lib ./lib`（`lib/` 位于 `server/` 根、与 `package.json` 同级；`COPY src ./src` 不会带上它，漏了这行镜像里会缺文件）。
- 验证：`npm test` 64 项、59 通过、0 失败、5 跳过——与拆分前基线逐项一致（5 个跳过项是需 Postgres 的集成测试，本机无库）。入口导出 18 个名字，与拆分前 `module.exports` 逐名相同（入口运行时双向核对：少一个或多一个都直接抛错）。
## 2026-09-26 — solum-app 的 commands/account.rs 拆为分节模块

- `crates/solum-app/src/commands/account.rs`（1288 行，26 条 tauri 命令）按命令族切成 `account/` 下 6 个分节：`sms_email`(381，挡在请求与账户之间的几道门：短信验证码、图形验证码、邮箱验证码注册与登录、手机号注册与登录) / `logout`(263，退出、服务端删号、恢复码：状态、签发、跨重启的一次性交接、用恢复码登录) / `login`(260，对着自建 solum-cloud 登录与注册，以及 OAuth 授权码路径与外部浏览器跳转) / `session`(207，会话本身：设置页要的状态、尚未接管的会话的撤销调度、换一个 profile 的重启) / `huawei`(128，华为账号路径) / `settings`(83，需要活会话的两项设置：改密码、选模型)，`mod.rs`(33) 保留原 imports、章节横幅与 6 组 `pub(crate)` 转出。纯移动，无逻辑改动。
- 这是 Phase 4 里**第一个 `solum-app` 的文件**，也逼出了工具链上一类全新的空缺：这里的每个条目都是 `pub(crate) async fn`，而整套脚本此前只认同步的顶层项。
- 等价性硬证据：7 个分节逐一按 token 流与拆分前 `account.rs` 比对，全部一致；只允许 `use` 归位、字段可见性放开与 rustfmt 换行。`mod.rs` 改为断言原文件那 1 条 import 一条不缺。脚本经反向验证（删一条语句被拒；本文件无 `#[cfg(test)]` 套件，"改测试函数名"这一项直接跳过而不是报设置失败——控制组要证明的是"能咬人"，一项咬住就够）。
- 工具四处真实缺陷，全部是这次新文件类型暴露的：
  - **加宽表的正则不认 `async`/`unsafe`**。它们位于可见性与关键字之间，漏掉就对每个 `async fn` 都匹配不到，报"找不到该声明"——而函数就在文件里。加宽时构造新签名一律改用具名捕获组：位置捕获组的下标会随着模式的改动整体位移，一次改动就把修饰符写到了关键字该在的位置。
  - **"条目都在"这一检查把集合当成了多重集**。`restart_account_app` 在原文件里声明**两次**——`#[cfg(target_os = "android")]` 一份、其余平台一份——这是两个平台各自的定义，不是重复。旧的"同名即报错"在这里报成接线 bug。改为**按原文件的声明次数核对**，多出才是重复，少了才是丢失。
  - **模块头可以没有 `//!` 文档头**。本文件以 `use super::*;` 开头、随后是一行 `//` 横幅；原来"找不到文档头"直接抛错，把拆分卡死。文档头现在是可选的，没有就不恢复。
  - **装配脚本里新写的"跳过文件头部"循环没有上界**。走到文件末尾后 `split("\n", 1)[0]` 恒为 `""`，而 `""` 匹配空行判断，`after` 无限增长——脚本挂住不报错，比报错更难查。已按长度收敛。
- **re-export 不能省**：`commands/mod.rs` 里是 `pub(crate) use account::*;`，而 glob **不会递归进子模块**，所以 26 条命令全都必须从 `account/mod.rs` 转出，否则 `lib.rs` 的 `generate_handler!` 一条都找不到。这 9 个参数结构体（`AccountOAuthLoginArgs` 等）则**不能**转出：它们只是 tauri 宏的反序列化目标，crate 内无人按名引用，转出后 `pub(crate) use` 自身变成未使用，clippy 报 3 条新告警。基线只有 2 条告警，拆分后必须仍是 2 条——为此专门把工作区导出到干净 HEAD 复测过基线，而不是凭印象。
- 可见性只放开一处：`AccountHuaweiLoginArgs`（声明在 `sms_email`，被 `huawei` 用）的 5 个私有字段。**结构体本身是 `pub(crate)`、字段却是私有的**——等价性脚本的撤销表因此需要 `名字:fields` 这一形式：只撤字段的加宽，不动结构体自己的可见性；沿用 `:crate` 会把 `pub(crate)` 一并还原到字段上，多出三个 token。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；clippy 回到基线的 2 条告警（`profile.rs` 的 `tauri::Manager`、`commands/privacy_capture.rs` 的 `chrono::TimeZone`，均为本会话之外的既有改动）；`check-frontend.mjs` 与 47 模块加载守卫通过。

## 2026-09-26 — solum-core 的 soulous.rs 拆为分节模块

- `crates/solum-core/src/soulous.rs`（1150 行）按职责切成 `soulous/` 下 6 个分节加 `tests`：`http`(288，HTTP 接缝：trait、ureq 实现、配置好的拉取/推送入口) / `parse`(191，把 Soulous 的 JSON 变成 fact：字段读取、时间与截止日期解析、每个端点一个解析器) / `domain`(203，Solous 眼中的 Soulous 领域：五个来源、fact、考试权重、派生状态与建议) / `tests`(235) / `push`(105，出站事件推送：离开设备的最小结构化投影与发送前的预览) / `review`(64，交给云端改写的本地 F14 复习材料) / `config`(86，存下来的 Soulous 连接：来源常量与 `SoulousConfig`)，`mod.rs` 保留原文档头、imports 与 5 组对外转出。纯移动，无逻辑改动。
- 这个文件的**物理顺序本来就是按职责排好的**，六个块严丝合缝，首尾各差一行文档注释就断开——是四个已完成的大文件里切分最省力的一个。`PullOutcome` 原本紧挨在 `review_material` 后面、属于 HTTP 那一组，把边界上移一行把它划进 `http`。
- 6 个 `#[test]` 全部保留；全文件函数名集合与拆分前完全一致。
- 等价性硬证据：8 个分节逐一按 token 流与拆分前 `soulous.rs` 比对，全部一致；只允许可见性放开、`use` 归位与 rustfmt 换行。原文件 9 条 import 一条不缺且顺序不变。脚本经反向验证（删一条语句、改一个测试函数名，均被拒；原树仍通过）。
- 可见性 41 项：14 个对外项拆分前就是 `pub`，re-export 不能比它们更宽，故仍为 `pub`；其余助手为 `pub(super)`。`SoulousKind::payload` 是 `impl` 内的**方法**，被 `review` 调用——E0624 只说是私有方法，调用方要的是**类型**在作用域里，这个组合编译器不会告诉你，所以方法进了 spec 的加宽表。`ExternalContextRequest` 5 个字段与 `ScheduleEventPayload` 5 个字段由 `widen-fields.mjs` 依编译器诊断放开。
- 工具补一处：`external-names.mjs` 原来只匹配 `mod::Name`，漏掉全限定写法 `solum_core::soulous::Name` 与 `crate::soulous::Name`——本文件的外部引用**全是**全限定形式，脚本因此返回空列表，看起来像"这个模块没人用"。现在两种写法都匹配。写这类脚本时值得记住：先拿一个已知有外部引用的模块验一遍脚本本身，别把"空结果"当成事实。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`soulous` 相关 7 个测试通过；`soulous` clippy 零告警。

## 2026-09-26 — solum-core 的 export_v3.rs 拆为分节模块

- `crates/solum-core/src/export_v3.rs`（1288 行）按职责切成 `export_v3/` 下 4 个分节加 `tests`：`build`(329，组装便携文档：库快照、导出本体、整档校验，以及私有的通知流水/审计/待处理构造器) / `validate`(371，逐分节校验器：对话、文档、待处理通知、设置、恢复意图、流水与审计行) / `types`(254，便携文档的全部形状) / `helpers`(118，构造器与校验器共用的原始工具：guid 还原、稳定 id、规范 JSON 与标量检查)，`tests`(215)，`mod.rs` 保留原文档头、imports、全部 16 个常量与 3 组对外转出。纯移动，无逻辑改动。
- 4 个 `#[test]` 全部保留；全文件函数名集合与拆分前完全一致。
- 等价性硬证据：6 个分节逐一按 token 流与拆分前 `export_v3.rs` 比对，全部一致；只允许可见性放开、`use` 归位与 rustfmt 换行。原文件 10 条 import 一条不缺且顺序不变。脚本经反向验证（删一条语句、改一个测试函数名，均被拒；原树仍通过）。
- **原文件的物理顺序决定了这里的切法**：`validate_portable_document`（一个整档校验器）夹在 `build_portable_export` 与三个私有构造器之间，所以"全部构造器归 build、全部校验器归 validate"在单边界切分下做不到。取舍是把它与三个构造器放在一起，`build` 一节因此同时含"组装"与"整档校验"；`validate` 一节则是纯粹的逐分节校验器。
- **私有常量要留在父模块**。16 个 `MAX_*` 常量原本与公开的 `PORTABLE_*` 同在文件头；切分时它们跟着 `types` 走了，于是 `validate` 引用它们时报 E0603（私有常量）。常量属于父模块的公共词汇表——兄弟分节正是通过 `use super::*;` 拿到它们的——所以 `mod` 一节的行区间扩到包含整段常量，其余分节从常量之后开始。
- **re-export 不能比被转出的项更宽**。`validate_chat`/`validate_documents`/`validate_llm`/`validate_restore_intents` 拆分前是 `pub(crate)`，按 `pub use` 转出即 E0364。spec 的 re-export 条目现在可自带可见性（`"pub(crate) validate::{...}"`），且会校验条目形状——写错的条目在装配阶段就报错，而不是留到编译期。
- 装配脚本补一处：re-export 块原本锚在"第一个 const/type/derive 之前"。当 `mod` 一节自己没有任何条目时（本次所有常量都在别处）就没有锚点，报"找不到锚点"；现在退化为锚在最后一条 import 之后——父模块的绑定都已就位的那个位置。
- 可见性 30 项：`snapshot_portable_store`/`build_portable_export`/`validate_portable_document` 拆分前就是 `pub`，re-export 不能比它们更宽，故仍为 `pub`；4 个原本 `pub(crate)` 的校验器保持 `pub(crate)`；其余 23 个助手为 `pub(super)`。本文件无结构体字段需要放开。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`export_v3` 相关 4 个测试通过；`export_v3` clippy 零告警。

## 2026-09-26 — solum-core 的 email.rs 拆为分节模块

- `crates/solum-core/src/email.rs`（1144 行）按职责切成 `email/` 下 6 个模块：`config`(291，provider/TLS/auth 枚举与落盘的账户、配置形状) / `mail`(288，IMAP 会话：连接、列文件夹、列举与搜索、取信并解析成 UI 形状) / `oauth`(165，授权码换 token、XOAUTH2 认证器、取 access token) / `types`(130，UI 投影形状：账户摘要、文件夹、消息摘要与发送草稿) / `smtp`(85，发送与预览草稿) / `helpers`(78，主机与地址校验、脱敏、头解码、HTML 剥离)，`tests`(87)，`mod.rs` 保留原文档头、imports、全部常量与 5 组对外转出。纯移动，无逻辑改动。
- 4 个 `#[test]` 全部保留；全文件函数名集合与拆分前完全一致。
- 等价性硬证据：8 个分节逐一按 token 流与拆分前 `email.rs` 比对，全部一致；只允许可见性放开、`use` 归位与 rustfmt 换行。`mod.rs` 改为断言原文件 11 条 import 一条不缺且顺序不变。等价性脚本经反向验证（删一条语句、改一个测试函数名，均被拒；原树仍通过）。另加两项针对本次新增撤销规则的定向反向验证：改 `type` 别名的右值、给 `pub(super) fn` 加一个参数，都被拒。
- **分节名不能与依赖 crate 同名**。最初把 IMAP 那一节也叫 `imap`，`mod imap;` 于是遮蔽了外部 `imap` crate，模块作用域里所有 `imap::Session`/`imap::Connection`/`imap::types::Fetches` 都指向分节而非 crate，`use super::*;` 引进来的 crate 路径集体失效，报错读起来像"某分节没声明 Session"。改名为 `mail`。
- **工具真实缺陷一：`wire-sections.mjs` 的装配步骤不可重入**。第二次运行会重复写入文档头和 `mod` 声明块。重复的 `mod X;` 报 E0428 看着无害，但它让模块拿到两个 DefId，于是所有经由该模块到达的类型不再与自身匹配——编译器在完全正确的代码上报 E0308 “expected `email::config::EmailProvider`, found a different `email::config::EmailProvider`”。现在每样写入都按自身标记先擦除再写（文档头、`mod` 块、re-export 块各带标记），`mod` 声明按名字逐个擦除，空行折叠移到该步最后（头部是前缀重写、`mod` 块在其后插入，折叠跑得太早看不到它们留下的空行，每跑一次多一行）。`mod X;` 的校验从"匹配到即可"改为"**恰好出现一次**"，文档头同样按原文出现次数校验。连跑两次的输出已验证逐字节一致。
- **工具真实缺陷二：等价性脚本的撤销规则三处漏**。(1) 撤销正则的关键字组缺 `type`，`pub(super) type ImapSession = ...` 别名完全不撤销，差异落在 token 0，读起来像丢了一整行。(2) 撤销要按**原样拼写**放回 `pub `，而不是语义等价的 `pub(crate) `——分词把限定形式的括号和标识符算作额外 token，恢复错了会报出一个"少一个 token"的假差异；`WIDENED` 条目因此要能区分裸 `pub` 与限定形式。这两点合起来让 `mail`/`smtp` 两节各报一处不存在的差异。新增 `pre-split-visibility.mjs` 从拆分前的原文件直接读出每个被加宽项的可见性，不再靠猜。(3) 拆分新增的 `use super::<section>::{...};` 只在 `tests` 段被忽略，换个文件立刻多出两处假差异；现在对每一节都忽略，且必须放在 `tests` 分支**之后**——那个分支会重新赋值 `got`/`want0`，放在前面看着对、实际什么都没做（这一点先写错过一次）。
- 可见性 41 项：8 个对外项拆分前就是 `pub`，re-export 不能比被转出的项更宽，故仍为 `pub`；其余助手为 `pub(super)`。`EmailProvider::oauth_endpoints` 是 `impl` 内的**方法**，被 `oauth` 调用——E0624 只说是私有方法，调用方要的是**类型**在作用域里，这个组合编译器不会告诉你，所以方法也进了 spec 的加宽表。`OAuthEndpoints` 的 `scopes`/`token`/`authorize` 与 `XOAuth2` 的 `response` 4 个字段由 `widen-fields.mjs` 依编译器诊断放开。
- 验证：Rust workspace 487 通过、0 失败（与基线一致）；`cargo fmt --all --check` 通过；`email` 相关 7 个测试通过（4 个属本模块）。
- 未解决（既有，与本次改动无关）：`profile.rs` 的 `tauri::Manager` 与 `commands/privacy_capture.rs` 的 `chrono::TimeZone` 为未使用导入，workspace Clippy 报红，均在本会话之外的改动里。

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

use std::path::PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "solum", version, about = "Personal Agent — Phase 1 CLI")]
pub(crate) struct Cli {
    /// Path to the local SQLite store. Defaults to the per-user app-data
    /// directory — the same one the desktop shell uses, so the two keep
    /// sharing one store without it depending on the current directory.
    #[arg(long, global = true)]
    pub(crate) db: Option<String>,
    /// Inject "now" (e.g. 2026-07-06T10:00:00 or "2026-07-06 10:00"). Defaults
    /// to the system clock.
    #[arg(long, global = true)]
    pub(crate) now: Option<String>,
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Ingest an utterance (event / chat / dangerous command).
    Add {
        /// The utterance, e.g. 明天下午3点在会议室开会
        text: Vec<String>,
    },
    /// List upcoming events.
    Agenda,
    /// Show notifications that are due at `--now`.
    Due,
    /// Deliver (mark fired) everything due at `--now`.
    Fire,
    /// Cancel a pending reminder by id (without deleting its event).
    Dismiss { id: i64 },
    /// Reschedule an event by id: `reschedule 3 明天下午4点`（自然语言时间，
    /// 相对 --now 解析；提醒按规则表重排）。自然语言路径走
    /// `solum add "把明天的会改到下午4点"`。
    Reschedule { id: i64, time: Vec<String> },
    /// Snooze a reminder: ring again N minutes from `--now`（待触发的顺延，
    /// 已触发的重新拉响；已取消的不会被复活）。
    Snooze {
        id: i64,
        /// 多少分钟后再响（1 到 1440）。
        #[arg(long, default_value_t = 10)]
        minutes: i64,
    },
    /// Show the memory ledger (F12): everything the agent remembers.
    Ledger,
    /// Forget a memory: `forget <event|raw_input|notification> <id>`.
    Forget { layer: String, id: i64 },
    /// Edit a memory fact's wording in place（F12 可编辑；recall 立即生效）。
    FactEdit { id: i64, content: Vec<String> },
    /// Export everything you own as one JSON document（§4：数据完全归你——
    /// 备份/迁移/离机审查。纯只读、纯本地，含审计日志与人格全部版本）。
    Export {
        /// 输出文件路径；不传则打印到 stdout。
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Restore/merge a previously exported document（与 Export 互为反向；
    /// 走 Guard 确认，不删除任何本机数据，本机更新过的记录保持本机版本）。
    Import {
        /// 导出文件路径（solum-export v2 及以上，改名前的 pa-export 同样接受）。
        file: PathBuf,
        /// 只看会导入什么，不实际写入。
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect persistent custom widgets（F19，只读）。写操作只在图形壳层：
    /// schema 驱动的表单在命令行没有合理形态，硬做就是把渲染逻辑复制一份。
    /// 这条命令的意义是排障与导出核对，让组件数据在 headless 下可观测。
    Widgets {
        /// 只看某一个组件的记录；不传则列出全部组件。
        #[arg(long)]
        id: Option<i64>,
    },
    /// Show the importance rule table.
    Rules,
    /// Show per-dimension proactivity levels.
    Proactivity,
    /// Set a proactivity level: `proactivity-set <dimension> <level>`.
    ProactivitySet { dimension: String, level: String },
    /// Show whether subsequently captured notification text may use sync and
    /// cloud-chat recall context (default: on).
    NotifCloud,
    /// Allow future captures as cloud-AI context: `notif-cloud-set <on|off>`.
    /// Device-to-device sync is unconditional and unaffected by this switch.
    NotifCloudSet { enabled: String },
    /// Notification intelligence (F20): app whitelist, routing rules, and
    /// the local-visible capture queue. The Android shell owns native capture.
    NotifIntelligence {
        #[command(subcommand)]
        command: NotifIntelligenceCmd,
    },
    /// List available tools and their risk levels.
    Tools,
    /// Attempt to run a tool with no confirmation (demonstrates the guard).
    GuardRun { tool: String, args: Vec<String> },
    /// Run the full request → confirm → execute flow for a tool.
    GuardDemo { tool: String, args: Vec<String> },
    /// Show the append-only audit log.
    Audit,
    /// Self-review for the trailing N days ending at `--now` (F14). With a
    /// cloud reasoner + persona it rewrites the digest in your voice; --plain
    /// forces the offline render.
    Review {
        #[arg(long, default_value_t = 7)]
        days: i64,
        /// Skip the cloud persona rewrite even when configured.
        #[arg(long)]
        plain: bool,
    },
    /// Show today's brief: agenda, due/upcoming reminders, and top pending suggestions.
    DailyBrief,
    /// Show the behavior journal (F4): status reports, check-ins, fired reminders.
    Log,
    /// Ask a proactive status check-in if one is due at `--now` (F3).
    Checkin,
    /// Generate suggestions looking N days ahead and print what's new (F10).
    Suggest {
        #[arg(long, default_value_t = 3)]
        days: i64,
    },
    /// List all suggestions with status.
    Suggestions,
    /// Set a suggestion's status: `suggest-set <id> <accepted|dismissed|pending>`.
    SuggestSet { id: i64, status: String },
    /// Show cloud reasoner status (provider config is env SOLUM_LLM_* or solum-llm.json).
    LlmStatus,
    /// Soulous read-only study data source (Phase 8.1). Configuration is
    /// local-only in solum-soulous.json; missing configuration stays quiet.
    Soulous {
        #[command(subcommand)]
        command: SoulousCmd,
    },
    /// One sync round with the relay (F17 §3.8): push local changes, pull +
    /// merge other devices'. Config: SOLUM_SYNC_URL/TOKEN/KEY or solum-sync.json.
    Sync,
    /// Show sync config and this device's sync identity.
    SyncStatus,
    /// Derive the relay token + e2e key from a username+password pair
    /// (2026-07-22) without writing anything — for one-time relay setup
    /// (paste the printed token into SOLUM_SYNC_SERVER_TOKEN) or to hand-build
    /// a `{url,token,key}` solum-sync.json. Every device just needs the same
    /// username+password from here on; this command exists so you never have
    /// to retype the derived hex by hand more than once.
    SyncDerive { username: String, password: String },
    /// Show the active persona and full version history (F9 v1).
    Persona,
    /// Save manual style settings as a new persona version. Omitted fields
    /// keep their current value; pass an empty string to clear one.
    PersonaSet {
        /// 怎么称呼你（如 老板）。
        #[arg(long)]
        nickname: Option<String>,
        /// 语气描述（如 "干练、简洁"）。
        #[arg(long)]
        tone: Option<String>,
        /// 口头禅，可多次传入（会整组替换）。
        #[arg(long = "catchphrase")]
        catchphrases: Vec<String>,
        /// 其他风格/价值观备注。
        #[arg(long)]
        notes: Option<String>,
        /// 这个版本的说明（进版本历史）。
        #[arg(long)]
        note: Option<String>,
    },
    /// Import a chat-log export and extract a persona draft — 纯本地，原始
    /// 记录不入库不上云（F9 §3.4）。默认只预览报告；加 --save 保存为新版本。
    PersonaImport {
        /// 聊天记录导出文件（txt：微信/QQ 导出或每行「昵称: 消息」）。
        file: PathBuf,
        /// 你在这份记录里的昵称（只统计你自己的消息）。
        #[arg(long)]
        me: String,
        /// 确认保存为新人格版本（source=import）；不加只预览。
        #[arg(long)]
        save: bool,
        /// 这个版本的说明（进版本历史）。
        #[arg(long)]
        note: Option<String>,
    },
    /// Roll the active persona back to an earlier version (history is kept).
    PersonaRollback { version: i64 },
    /// Delete every persona version (cannot be undone).
    PersonaClear,
    /// List stored wearable health samples (F5, Phase 4). Ingestion itself
    /// only happens on the mobile shell (Health Connect); this is a
    /// read-only view — useful on desktop once samples arrive via sync.
    Health,
    /// 本地记忆检索调试（§3.10 M3）：看一句话会检索出哪些片段——也就是
    /// 云端调用前"将要上行的背景"，可肉眼审计（F12 精神）。
    Recall { query: Vec<String> },
    /// 数据回看（D2）：日程分布 / 问询应答 / 重复活动 / 穿戴基线与
    /// F11/F13 数据门槛。纯本地只读。
    Stats,
    /// List standing routines (F3 完全体)。
    Routines,
    /// Enable/disable a routine: `routine-set <id> <on|off>`。
    RoutineSet { id: i64, state: String },
}

#[derive(Subcommand)]
pub(crate) enum SoulousCmd {
    /// Pull course schedule, exams, tasks, today's check-in snapshot, and
    /// focus sessions. A failed request keeps the last successful cache.
    Pull,
    /// Show local configuration/cache status without making a network call.
    Status,
}

#[derive(Subcommand)]
pub(crate) enum NotifIntelligenceCmd {
    /// Show the local whitelist, priority rules, queued captures, and pending filter proposals.
    Status,
    /// Allow one Android package and seed its editable important-notification presets.
    Allow { package_name: String },
    /// Stop accepting new notifications from one Android package.
    Deny { package_name: String },
    /// Run one ordinary-lane batch now (uses the cloud only when notification-cloud is enabled).
    Process,
}


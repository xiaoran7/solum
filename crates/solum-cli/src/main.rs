//! pa — a headless CLI that drives the solum-core closed loop.
//!
//! This is the Phase 1 "demo" surface from ARCHITECTURE.md §6: it lets you feed
//! natural language in, see events extracted and reminders scheduled, inspect
//! and prune the memory ledger, and exercise the HITL guard — all without a GUI.
//!
//! `--now <ISO>` injects the clock so runs are reproducible; `--db <path>`
//! chooses the local SQLite store (default: ./solum.sqlite).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime};
use clap::{Parser, Subcommand};

use solum_core::model::{fmt_ts, MemoryLayer};
use solum_core::proactivity::{ProactivityDimension, ProactivityLevel};
use solum_core::Orchestrator;

#[derive(Parser)]
#[command(name = "solum", version, about = "Personal Agent — Phase 1 CLI")]
struct Cli {
    /// Path to the local SQLite store. Defaults to the per-user app-data
    /// directory — the same one the desktop shell uses, so the two keep
    /// sharing one store without it depending on the current directory.
    #[arg(long, global = true)]
    db: Option<String>,
    /// Inject "now" (e.g. 2026-07-06T10:00:00 or "2026-07-06 10:00"). Defaults
    /// to the system clock.
    #[arg(long, global = true)]
    now: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
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
    /// `pa add "把明天的会改到下午4点"`。
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
enum SoulousCmd {
    /// Pull course schedule, exams, tasks, today's check-in snapshot, and
    /// focus sessions. A failed request keeps the last successful cache.
    Pull,
    /// Show local configuration/cache status without making a network call.
    Status,
}

#[derive(Subcommand)]
enum NotifIntelligenceCmd {
    /// Show the local whitelist, priority rules, queued captures, and pending filter proposals.
    Status,
    /// Allow one Android package and seed its editable important-notification presets.
    Allow { package_name: String },
    /// Stop accepting new notifications from one Android package.
    Deny { package_name: String },
    /// Run one ordinary-lane batch now (uses the cloud only when notification-cloud is enabled).
    Process,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let now = resolve_now(cli.now.as_deref())?;
    // Two adoptions, in order, mirroring the desktop shell:
    //   1. the pre-rename `pa.sqlite` (2026-07-20), still in the old location;
    //   2. the pre-app-data `./solum.sqlite` (2026-07-21), moved by the resolver.
    // Doing (1) first means a user who skipped a release ends up with one store
    // rather than two half-populated ones.
    if solum_core::store::adopt_legacy_db(Path::new("pa.sqlite"), Path::new("solum.sqlite"))
        .map_err(|e| anyhow!("adopt pre-rename store: {e}"))?
    {
        eprintln!("已接管改名前的数据库 pa.sqlite → solum.sqlite");
    }
    let db = match &cli.db {
        Some(p) => p.clone(),
        None => solum_core::paths::resolve_with_adoption("solum.sqlite")
            .to_string_lossy()
            .into_owned(),
    };
    let mut o = Orchestrator::open(&db).map_err(|e| anyhow!("open store: {e}"))?;
    // Cloud reasoner is optional by design (F16): no config → stay offline.
    let llm_cfg = solum_core::llm::LlmConfig::load();
    if let Some(cfg) = &llm_cfg {
        o.set_reasoner(Box::new(solum_core::llm::LlmReasoner::new(cfg.clone())));
    }

    match cli.cmd {
        Cmd::Add { text } => {
            let text = text.join(" ");
            if text.trim().is_empty() {
                return Err(anyhow!("nothing to add"));
            }
            let out = o.ingest(&text, now).map_err(|e| anyhow!(e.to_string()))?;
            println!("意图: {:?}", out.intent);
            println!("{}", out.message);
        }
        Cmd::Agenda => {
            let events = o.agenda(now).map_err(to_err)?;
            if events.is_empty() {
                println!("（没有即将到来的日程）");
            }
            for ev in events {
                println!(
                    "#{:<3} [{}] {} — {}{}",
                    ev.id.unwrap_or(0),
                    ev.kind.as_str(),
                    ev.title,
                    fmt_ts(&ev.start),
                    ev.location.map(|l| format!(" @{l}")).unwrap_or_default()
                );
            }
        }
        Cmd::Due => {
            let due = o.due(now).map_err(to_err)?;
            if due.is_empty() {
                println!("（当前没有到点的提醒） now={}", fmt_ts(&now));
            }
            for n in due {
                println!(
                    "🔔 event#{} 提前{} @ {}",
                    n.event_id,
                    n.lead_label,
                    fmt_ts(&n.fire_at)
                );
            }
        }
        Cmd::Fire => {
            // D4: materialize upcoming routine occurrences before delivering.
            o.materialize_routines(now).map_err(to_err)?;
            let fired = o.fire_due(now).map_err(to_err)?;
            if fired.is_empty() {
                println!("（没有需要触发的提醒） now={}", fmt_ts(&now));
            }
            for n in fired {
                println!("✅ 已触发 event#{} 提前{}", n.event_id, n.lead_label);
            }
        }
        Cmd::Dismiss { id } => {
            o.dismiss(id).map_err(to_err)?;
            println!("已取消提醒 notification#{id}（事件保留）");
        }
        Cmd::Reschedule { id, time } => {
            let time = time.join(" ");
            let parsed = solum_core::time_parse::parse_datetime(&time, now)
                .ok_or_else(|| anyhow!("无法从「{time}」解析出新时间"))?;
            let (ev, stored) = o.reschedule_event(id, parsed.start, now).map_err(to_err)?;
            println!("📅 已把「{}」改到 {}", ev.title, fmt_ts(&ev.start));
            for n in &stored {
                println!("   ⏰ 提醒 {}（提前{}）", fmt_ts(&n.fire_at), n.lead_label);
            }
        }
        Cmd::Snooze { id, minutes } => {
            let until = o.snooze(id, minutes, now).map_err(to_err)?;
            println!("⏰ notification#{id} 将于 {} 再响（+{minutes} 分钟）", fmt_ts(&until));
        }
        Cmd::Ledger => {
            let ledger = o.ledger().map_err(to_err)?;
            if ledger.is_empty() {
                println!("（记忆台账为空）");
            }
            for m in ledger {
                println!(
                    "{:<12} #{:<3} {}  [{}]{}",
                    m.layer.as_str(),
                    m.id,
                    m.summary,
                    fmt_ts(&m.created_at),
                    m.source.map(|s| format!(" ⟵{s}")).unwrap_or_default()
                );
            }
        }
        Cmd::Forget { layer, id } => {
            let layer = parse_layer(&layer)?;
            o.forget(layer, id).map_err(to_err)?;
            println!("已从记忆中删除 {}#{}（不可通过对话恢复）", layer.as_str(), id);
        }
        Cmd::FactEdit { id, content } => {
            let content = content.join(" ");
            o.update_fact(id, &content).map_err(to_err)?;
            println!("✏️ 已改写 fact#{id}：{}", content.trim());
        }
        Cmd::Export { out } => {
            let json = o.export_json(now).map_err(to_err)?;
            match out {
                Some(path) => {
                    // Refuse to clobber: `--out` pointing at an existing file is
                    // far more likely to be a mistake than an intent to destroy
                    // the backup already there.
                    if path.exists() {
                        anyhow::bail!(
                            "{} 已存在，拒绝覆盖备份。请换个文件名或先移走旧文件。",
                            path.display()
                        );
                    }
                    solum_core::fsatomic::write_atomic(&path, &json)
                        .with_context(|| format!("could not write {}", path.display()))?;
                    println!("📦 已导出到 {}（{} 字节，纯本地未上云）", path.display(), json.len());
                }
                None => println!("{json}"),
            }
        }
        Cmd::Import { file, dry_run } => {
            let raw = std::fs::read_to_string(&file)
                .with_context(|| format!("could not read {}", file.display()))?;
            let doc: serde_json::Value =
                serde_json::from_str(&raw).context("导入文件不是合法 JSON")?;
            let plan = solum_core::export::plan_import(&doc).map_err(to_err)?;
            println!(
                "📥 来自设备 {}，导出于 {}，共 {} 条",
                plan.origin,
                plan.exported_at.format("%Y-%m-%d %H:%M"),
                plan.total()
            );
            for (tbl, n) in &plan.counts {
                println!("   {tbl}: {n}");
            }
            if dry_run {
                println!("（--dry-run：未写入任何数据）");
            } else {
                // Same Guard path the shell uses: preview → one-time token →
                // audited execution. The CLI does not get a shortcut.
                let args = serde_json::json!({ "document": doc }).to_string();
                let pending = o.request_confirmation("data_import", &args, now).map_err(to_err)?;
                println!("
⚠️  {}", pending.request.effect_preview);
                let token = o.confirm(&pending.id, now).map_err(to_err)?;
                let message = o
                    .run_tool("data_import", &args, Some(token), now)
                    .map_err(to_err)?;
                println!("✅ {message}");
            }
        }
        Cmd::Widgets { id } => match id {
            None => {
                let defs = o.widget_definitions().map_err(to_err)?;
                if defs.is_empty() {
                    println!("（还没有组件；在图形壳层的对话里说「帮我做一个……组件」来创建）");
                }
                for d in defs {
                    let views = d
                        .schema
                        .views
                        .iter()
                        .map(|v| v.view_type.as_str())
                        .collect::<Vec<_>>()
                        .join("/");
                    let count = o.widget_records(d.id).map_err(to_err)?.len();
                    println!(
                        "#{:<3} {}  字段 {} 个 · 视图 {} · 记录 {} 条",
                        d.id,
                        d.name,
                        d.schema.fields.len(),
                        views,
                        count
                    );
                }
            }
            Some(id) => {
                let d = o
                    .widget_definitions()
                    .map_err(to_err)?
                    .into_iter()
                    .find(|d| d.id == id)
                    .with_context(|| format!("没有 id 为 {id} 的组件"))?;
                println!("📦 {}（#{}）", d.name, d.id);
                for f in &d.schema.fields {
                    println!(
                        "   {:<14} {:<10} {}",
                        f.name,
                        f.field_type.as_str(),
                        if f.required { "必填" } else { "" }
                    );
                }
                let records = o.widget_records(id).map_err(to_err)?;
                println!("   —— {} 条记录 ——", records.len());
                for r in &records {
                    println!("   #{:<4} {}", r.id, r.data);
                }
                let stats = d.schema.stats(&records);
                for stat in stats {
                    println!("   📊 {} {}：{}", stat.label, stat.op_label, stat.value);
                }
            }
        },
        Cmd::Rules => {
            let table = o.rule_table();
            for kind in solum_core::model::EventKind::all() {
                let r = table.rule(kind);
                let leads: Vec<String> = r.lead_times.iter().map(|l| l.label.clone()).collect();
                let chans: Vec<&str> = r.channels.iter().map(|c| c.as_str()).collect();
                println!(
                    "{:<9} 提前: {:<10} 渠道: {}",
                    kind.as_str(),
                    leads.join(","),
                    chans.join("+")
                );
            }
        }
        Cmd::Proactivity => {
            let p = o.proactivity();
            for dim in ProactivityDimension::all() {
                println!("{:<20} {}", dim.as_str(), p.level(dim).as_str());
            }
        }
        Cmd::ProactivitySet { dimension, level } => {
            let dim: ProactivityDimension = dimension.parse().map_err(to_err)?;
            let lvl: ProactivityLevel = level.parse().map_err(to_err)?;
            o.set_proactivity(dim, lvl).map_err(to_err)?;
            println!("已设置 {} = {}", dim.as_str(), lvl.as_str());
        }
        Cmd::NotifCloud => {
            let enabled = o.notif_cloud_enabled().map_err(to_err)?;
            println!(
                "通知上云（发往云端 AI）：{}（只影响之后新捕获的通知；设备间同步不受此开关影响）",
                if enabled { "开启" } else { "关闭" }
            );
        }
        Cmd::NotifCloudSet { enabled } => {
            let enabled = match enabled.as_str() {
                "on" | "true" => true,
                "off" | "false" => false,
                _ => return Err(anyhow!("通知上云只接受 on 或 off")),
            };
            o.set_notif_cloud_enabled(enabled).map_err(to_err)?;
            println!(
                "通知上云已{}；只影响之后新捕获的通知发不发往云端 AI，设备间同步照常。",
                if enabled { "开启" } else { "关闭" }
            );
        }
        Cmd::NotifIntelligence { command } => match command {
            NotifIntelligenceCmd::Status => {
                let config = o.notification_intelligence_config().map_err(to_err)?;
                println!(
                    "通知白名单：{}",
                    if config.allowed_packages.is_empty() {
                        "（空，未捕获任何应用）".into()
                    } else {
                        config.allowed_packages.join("、")
                    }
                );
                println!("普通通知批量间隔：{} 分钟", config.batch_interval_minutes);
                for rule in o.rule_table().notification_priority_rules() {
                    println!(
                        "即时规则 {}：{} [{} · {}]",
                        rule.id,
                        rule.pattern,
                        rule.package_name.as_deref().unwrap_or("全局"),
                        match rule.matcher {
                            solum_core::classify::NotificationMatchKind::Substring => "包含",
                            solum_core::classify::NotificationMatchKind::Regex => "正则",
                        }
                    );
                }
                for capture in o.notification_captures().map_err(to_err)? {
                    println!(
                        "通知#{:<3} [{}·{}] {} · {}{}",
                        capture.id.unwrap_or(0),
                        capture.lane.as_str(),
                        capture.state.as_str(),
                        capture.package_name,
                        capture.title,
                        capture
                            .reason
                            .map(|reason| format!(" · {reason}"))
                            .unwrap_or_default(),
                    );
                }
                for proposal in o.notification_filter_proposals().map_err(to_err)? {
                    println!(
                        "过滤提议#{:<3} [{}] {} · {}",
                        proposal.id.unwrap_or(0),
                        proposal.state.as_str(),
                        proposal.pattern,
                        proposal.reason,
                    );
                }
                for proposal in o.notification_action_proposals().map_err(to_err)? {
                    println!(
                        "动作提议#{:<3} [{}] {}「{}」{}",
                        proposal.id.unwrap_or(0),
                        proposal.state.as_str(),
                        match proposal.kind {
                            solum_core::notification_intelligence::NotificationActionKind::CancelEvent => "取消",
                            solum_core::notification_intelligence::NotificationActionKind::RescheduleEvent => "改期",
                        },
                        proposal.event_title,
                        proposal
                            .new_start
                            .map(|start| format!(" → {}", solum_core::model::fmt_ts_human(&start)))
                            .unwrap_or_default(),
                    );
                }
            }
            NotifIntelligenceCmd::Allow { package_name } => {
                o.set_notification_app_enabled(&package_name, true).map_err(to_err)?;
                println!("已允许捕获 {package_name}；已写入可编辑的重要通知预设（如有）。");
            }
            NotifIntelligenceCmd::Deny { package_name } => {
                o.set_notification_app_enabled(&package_name, false).map_err(to_err)?;
                println!("已停止捕获 {package_name}；历史回看与规则保留。");
            }
            NotifIntelligenceCmd::Process => {
                let count = o.process_notification_batch(now).map_err(to_err)?;
                println!("已处理 {count} 条普通队列通知；请用 notif-intelligence status 查看去向。");
            }
        },
        Cmd::Tools => {
            for name in o.tool_names() {
                let risk = o.tool_risk(&name).map(|r| r.as_str()).unwrap_or("?");
                println!("{:<14} risk={}", name, risk);
            }
        }
        Cmd::GuardRun { tool, args } => {
            let args = args.join(" ");
            match o.run_tool(&tool, &args, None, now) {
                Ok(out) => println!("{out}"),
                Err(e) => println!("⛔ 被守卫拦截: {e}"),
            }
        }
        Cmd::GuardDemo { tool, args } => {
            let args = args.join(" ");
            println!("— 步骤 0：无确认直接执行（应被拦截）");
            match o.run_tool(&tool, &args, None, now) {
                Ok(out) => println!("  意外执行: {out}"),
                Err(e) => println!("  ⛔ {e}"),
            }
            println!("— 步骤 1：请求确认，预览后果");
            let pending = o.request_confirmation(&tool, &args, now).map_err(to_err)?;
            println!("  待确认 id={}", pending.id);
            println!("  后果预览: {}", pending.request.effect_preview);
            println!("— 步骤 2：人工确认，签发一次性令牌");
            let token = o.confirm(&pending.id, now).map_err(to_err)?;
            println!("  已签发令牌 id={}", token.id);
            println!("— 步骤 3：凭令牌执行");
            let out = o.run_tool(&tool, &args, Some(token), now).map_err(to_err)?;
            println!("  {out}");
        }
        Cmd::Review { days, plain } => {
            let from = now - chrono::Duration::days(days);
            if plain {
                let digest = o.review(from, now).map_err(to_err)?;
                println!("{}", digest.render());
            } else {
                let (text, styled) = o.review_text(from, now).map_err(to_err)?;
                println!("{text}");
                if styled {
                    println!("（☁️ 已由云端按人格改写，数字经本地校验；--plain 可看离线原文）");
                }
            }
        }
        Cmd::DailyBrief => {
            let brief = o.daily_brief(now).map_err(to_err)?;
            println!("{}", brief.render());
        }
        Cmd::Log => {
            let log = o.behavior_log().map_err(to_err)?;
            if log.is_empty() {
                println!("（行为日志为空）");
            }
            for b in log {
                println!(
                    "#{:<3} {} [{}] {}{}",
                    b.id.unwrap_or(0),
                    fmt_ts(&b.ts),
                    b.kind.as_str(),
                    b.content,
                    b.source.map(|s| format!(" ⟵{s}")).unwrap_or_default()
                );
            }
        }
        Cmd::Checkin => match o.checkin_if_due(now).map_err(to_err)? {
            Some(q) => println!("🕐 {q}"),
            None => println!("（当前不需要问询：档位为被动 / 非清醒时段 / 距上次问询太近）"),
        },
        Cmd::Suggest { days } => {
            let fresh = o.generate_suggestions(now, days).map_err(to_err)?;
            if fresh.is_empty() {
                println!("（未来 {days} 天内没有新的建议）");
            }
            for s in fresh {
                println!("💡 #{:<3} [{}] {}", s.id.unwrap_or(0), s.kind.as_str(), s.text);
            }
        }
        Cmd::Suggestions => {
            let all = o.suggestions().map_err(to_err)?;
            if all.is_empty() {
                println!("（还没有任何建议）");
            }
            for s in all {
                println!(
                    "#{:<3} {} [{}] ({}) {}",
                    s.id.unwrap_or(0),
                    fmt_ts(&s.created_at),
                    s.kind.as_str(),
                    s.status.as_str(),
                    s.text
                );
            }
        }
        Cmd::SuggestSet { id, status } => {
            let status: solum_core::suggest::SuggestionStatus = status.parse().map_err(to_err)?;
            let follow_up = o.set_suggestion_status(id, status, now).map_err(to_err)?;
            println!("已设置 suggestion#{id} = {}", status.as_str());
            if let Some(msg) = follow_up {
                println!("{msg}");
            }
        }
        Cmd::LlmStatus => match &llm_cfg {
            Some(cfg) => println!("☁️ 云端推理已配置：{}", cfg.masked_summary()),
            None => println!(
                "（离线模式：未配置云端推理。设置 SOLUM_LLM_BASE_URL/SOLUM_LLM_API_KEY 或创建 solum-llm.json）"
            ),
        },
        Cmd::Soulous { command } => match command {
            SoulousCmd::Pull => match o.pull_soulous(now).map_err(to_err)? {
                Some(r) => println!(
                    "📚 Soulous 拉取完成：课表 {} 条、考试 {} 场、任务 {} 项、打卡快照 {} 条、专注 {} 段{}",
                    r.courses,
                    r.exams,
                    r.tasks,
                    r.checkins,
                    r.focus_sessions,
                    if r.refreshed_tokens { "（JWT 已自动刷新）" } else { "" }
                ),
                None => println!(
                    "（Soulous 未配置，保持静默离线。创建 solum-soulous.json 后可拉取；不会影响录入或提醒。）"
                ),
            },
            SoulousCmd::Status => {
                match solum_core::soulous::SoulousConfig::load() {
                    Some(cfg) => println!("📚 Soulous 已配置：{}", cfg.masked_summary()),
                    None => println!("（Soulous 未配置：solum-soulous.json 缺失或不完整，功能已静默关闭）"),
                }
                let status = o.soulous_status(now).map_err(to_err)?;
                if status.total == 0 {
                    println!("本地没有已成功拉取的 Soulous 缓存。");
                } else {
                    println!(
                        "本地缓存：课表 {} 条、考试 {} 场、任务 {} 项、打卡快照 {} 条、专注 {} 段；最近成功拉取 {}",
                        status.courses,
                        status.exams,
                        status.tasks,
                        status.checkins,
                        status.focus_sessions,
                        status
                            .last_success_at
                            .map(|t| fmt_ts(&t))
                            .unwrap_or_else(|| "未知".into())
                    );
                    for exam in status.upcoming_exams {
                        let leads = exam
                            .rule
                            .lead_times
                            .iter()
                            .map(|lead| lead.label.as_str())
                            .collect::<Vec<_>>()
                            .join(",");
                        println!(
                            "  考试分类：{} @ {}（规则提前 {}）",
                            exam.title,
                            fmt_ts(&exam.occurs_at),
                            leads
                        );
                    }
                }
            }
        },
        Cmd::Sync => {
            let Some(cfg) = solum_core::sync::SyncConfig::load() else {
                println!("（同步未配置。设置 SOLUM_SYNC_URL/SOLUM_SYNC_TOKEN/SOLUM_SYNC_KEY 或创建 solum-sync.json：{{\"url\":…,\"token\":…,\"key\":\"64位hex主密钥\"}}）");
                return Ok(());
            };
            let transport = solum_core::sync::HttpTransport::new(&cfg).map_err(to_err)?;
            let r = o.sync_now(&transport, &cfg).map_err(to_err)?;
            println!(
                "🔄 同步完成：推送 {} 条变更；拉取 {} 个批次，合并 {} 条（跳过 {} 条旧/重复）",
                r.pushed, r.pulled_blobs, r.applied, r.skipped
            );
            // Silence here would defeat the point of holding them at all.
            if r.quarantined > 0 {
                println!(
                    "   ⏸ 另有 {} 条本版本读不懂，已暂存等升级（对端版本比本机新，数据没丢）",
                    r.quarantined
                );
            }
            if r.history_gap {
                println!(
                    "   ⛔ 本机同步游标已落在中继的留存窗口之外：这轮**不是完整同步**，
                           期间的部分远端变更已被中继清理、无法再取回。
                           请从另一台设备导出备份并在本机导入，重新对齐后再继续增量同步。"
                );
            }
            if r.bad_blobs > 0 {
                println!(
                    "   ⚠ 有 {} 个批次无法解密，已暂存并跳过（密文保留）。\
                     最常见原因是某台设备的 SOLUM_SYNC_KEY 与本机不一致。",
                    r.bad_blobs
                );
            }
            // 无法解密的批次同样要报，包括被上限淘汰掉的——那是被丢弃的恢复材料。
            let (bad_held, bad_dropped) = o.bad_blob_stats().map_err(to_err)?;
            if bad_dropped > 0 {
                println!(
                    "   ⚠ 已有 {bad_dropped} 个无法解密的批次因超出暂存上限被丢弃（当前暂存 {bad_held} 个）。
                           请先核对各设备的 SOLUM_SYNC_KEY 是否一致，否则会持续丢弃。"
                );
            } else if bad_held > 0 {
                println!("   ⏸ 当前暂存 {bad_held} 个无法解密的批次（密文保留）。请核对各设备的 SOLUM_SYNC_KEY。");
            }
            let (held, dropped) = o.sync_quarantine_stats().map_err(to_err)?;
            if dropped > 0 {
                println!("   ⚠ 暂存区已满，累计丢弃 {dropped} 条（当前暂存 {held} 条）。请升级本机版本。");
            }
        }
        Cmd::SyncDerive { username, password } => {
            let (token, key) =
                solum_core::sync::derive_credentials(&username, &password).map_err(to_err)?;
            println!("relay token（写进 solum-sync-server 的 SOLUM_SYNC_SERVER_TOKEN）：{token}");
            println!("e2e key（solum-sync.json 的 key 字段，64 位十六进制）：{key}");
            println!("（密码进了这条命令的 shell 历史，用完记得清一下）");
        }
        Cmd::SyncStatus => {
            match solum_core::sync::SyncConfig::load() {
                Some(cfg) => println!("🔄 同步已配置：{}", cfg.masked_summary()),
                None => println!("（同步未配置：SOLUM_SYNC_URL/SOLUM_SYNC_TOKEN/SOLUM_SYNC_KEY 或 solum-sync.json）"),
            }
            println!("本机设备标识：{}", o.sync_device_id().map_err(to_err)?);
            // 持久化的缺口标记必须在状态里看得见——只在同步那一轮打印一次，
            // 用户很可能根本没在看终端。
            if let Some(gap) = o.sync_history_gap().map_err(to_err)? {
                println!(
                    "⛔ 同步不完整：本机进度曾落在中继留存窗口之外（{gap}）。
                        期间的部分远端变更已被清理、无法取回；请从另一台设备导出备份并在本机导入以重新对齐。"
                );
            }
            let (bad_held, bad_dropped) = o.bad_blob_stats().map_err(to_err)?;
            if bad_held > 0 || bad_dropped > 0 {
                println!("无法解密的批次：暂存 {bad_held} 个，已因超限丢弃 {bad_dropped} 个（核对各设备 SOLUM_SYNC_KEY）");
            }
        }
        Cmd::Persona => {
            match o.persona() {
                Some(p) => println!(
                    "🎭 当前人格 v{}（{}，{}）\n   {}",
                    p.version,
                    p.source,
                    fmt_ts(&p.created_at),
                    p.summary()
                ),
                None => println!("（未设置人格——`pa persona-set --tone 干练` 试试）"),
            }
            let versions = o.persona_versions().map_err(to_err)?;
            if !versions.is_empty() {
                println!("版本历史：");
                let active = o.persona().map(|p| p.version);
                for v in versions {
                    println!(
                        "  v{:<3} {} {}{}{}",
                        v.version,
                        fmt_ts(&v.created_at),
                        v.summary(),
                        v.note.as_ref().map(|n| format!("（{n}）")).unwrap_or_default(),
                        if Some(v.version) == active { "  ← 当前" } else { "" }
                    );
                }
            }
        }
        Cmd::PersonaSet { nickname, tone, catchphrases, notes, note } => {
            // Start from the active draft so partial edits keep the rest.
            let mut draft = o.persona().map(|p| p.draft.clone()).unwrap_or_default();
            if let Some(n) = nickname {
                draft.nickname = Some(n);
            }
            if let Some(t) = tone {
                draft.tone = t;
            }
            if !catchphrases.is_empty() {
                draft.catchphrases = catchphrases;
            }
            if let Some(s) = notes {
                draft.style_notes = Some(s);
            }
            let p = o.set_persona(draft, note, now).map_err(to_err)?;
            println!("🎭 已保存人格 v{}：{}", p.version, p.summary());
        }
        Cmd::PersonaImport { file, me, save, note } => {
            let raw = std::fs::read_to_string(&file)
                .with_context(|| format!("读取 {} 失败", file.display()))?;
            let report = o.preview_persona_import(&raw, &me).map_err(to_err)?;
            println!("📥 聊天记录提取报告（纯本地，原始记录不入库）\n{}", report.render());
            if save {
                let note = note.or(Some(format!("从 {} 导入", file.display())));
                let p = o.import_persona(report.suggested, note, now).map_err(to_err)?;
                println!("\n🎭 已保存人格 v{}（source=import）：{}", p.version, p.summary());
                println!("   不满意可 `pa persona-set` 修改，或 `pa persona-rollback <旧版本>` 回滚");
            } else {
                println!("\n（预览模式，未保存。确认无误加 --save，或先用 persona-set 手动调整后再保存）");
            }
        }
        Cmd::PersonaRollback { version } => {
            let p = o.rollback_persona(version).map_err(to_err)?;
            println!("🎭 已回滚到人格 v{}：{}", p.version, p.summary());
        }
        Cmd::PersonaClear => {
            o.clear_persona().map_err(to_err)?;
            println!("已删除全部人格版本（不可恢复）");
        }
        Cmd::Health => {
            let samples = o.health_samples().map_err(to_err)?;
            if samples.is_empty() {
                println!("（还没有穿戴数据——需要在手机壳上授权 Health Connect，或等待同步）");
            }
            for h in samples {
                println!(
                    "#{:<3} [{}] {} @ {} ⟵{}",
                    h.id.unwrap_or(0),
                    h.kind.label(),
                    h.value,
                    fmt_ts(&h.start),
                    h.source
                );
            }
        }
        Cmd::Recall { query } => {
            let query = query.join(" ");
            if query.trim().is_empty() {
                return Err(anyhow!("nothing to recall"));
            }
            let hits = o.recall(&query, now).map_err(to_err)?;
            if hits.is_empty() {
                println!("（没有检索到相关记忆——云端调用将不携带任何背景片段）");
            } else {
                println!("以下片段会作为「已知背景」随云端调用上行（top {}，总长受硬上限约束）：", hits.len());
                for s in &hits {
                    println!("  {:.3}  [{}] {}", s.score, s.source(), s.content);
                }
            }
        }
        Cmd::Stats => {
            let report = o.stats(now).map_err(to_err)?;
            println!("{}", report.render());
        }
        Cmd::Routines => {
            let routines = o.routines().map_err(to_err)?;
            if routines.is_empty() {
                println!("（还没有固定提醒——采纳一条习惯建议即可创建）");
            }
            for r in routines {
                println!(
                    "#{:<3} [{}] 每天 {} 「{}」{}",
                    r.id.unwrap_or(0),
                    if r.active { "启用" } else { "暂停" },
                    r.time_of_day,
                    r.title,
                    r.source.map(|s| format!(" ⟵{s}")).unwrap_or_default()
                );
            }
        }
        Cmd::RoutineSet { id, state } => {
            let active = match state.as_str() {
                "on" | "enable" | "启用" => true,
                "off" | "disable" | "暂停" => false,
                other => return Err(anyhow!("state 必须是 on|off，得到 {other}")),
            };
            o.set_routine_active(id, active, now).map_err(to_err)?;
            println!("routine#{id} 已{}", if active { "启用" } else { "暂停" });
        }
        Cmd::Audit => {
            let rows = o.audit_log().map_err(to_err)?;
            if rows.is_empty() {
                println!("（审计日志为空）");
            }
            for r in rows {
                println!(
                    "#{:<3} {} [{}] {} → {} {}",
                    r.id,
                    r.ts,
                    r.risk,
                    r.summary,
                    r.decision,
                    if r.detail.is_empty() {
                        String::new()
                    } else {
                        format!("({})", r.detail)
                    }
                );
            }
        }
    }
    Ok(())
}

fn to_err(e: solum_core::CoreError) -> anyhow::Error {
    anyhow!(e.to_string())
}

fn parse_layer(s: &str) -> Result<MemoryLayer> {
    Ok(match s.to_lowercase().as_str() {
        "raw_input" | "raw" | "input" => MemoryLayer::RawInput,
        "event" | "ev" => MemoryLayer::Event,
        "notification" | "notif" | "n" => MemoryLayer::Notification,
        "notification_capture" | "capture" => MemoryLayer::NotificationCapture,
        "behavior" | "log" | "b" => MemoryLayer::Behavior,
        "suggestion" | "suggest" | "s" => MemoryLayer::Suggestion,
        "wearable" | "health" | "w" => MemoryLayer::Wearable,
        "fact" | "f" | "memory" => MemoryLayer::Fact,
        "routine" | "r" => MemoryLayer::Routine,
        other => return Err(anyhow!("unknown memory layer: {other}")),
    })
}

fn resolve_now(opt: Option<&str>) -> Result<NaiveDateTime> {
    match opt {
        None => Ok(Local::now().naive_local().with_second_zero()),
        Some(s) => parse_flexible(s).with_context(|| format!("could not parse --now {s:?}")),
    }
}

fn parse_flexible(s: &str) -> Result<NaiveDateTime> {
    let s = s.trim();
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(dt);
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap());
    }
    Err(anyhow!("unrecognized datetime format"))
}

/// Small helper: zero out sub-minute noise so display is clean.
trait SecondZero {
    fn with_second_zero(self) -> Self;
}
impl SecondZero for NaiveDateTime {
    fn with_second_zero(self) -> Self {
        use chrono::Timelike;
        self.with_second(0)
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(self)
    }
}

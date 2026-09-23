use super::*;

impl Orchestrator {
    // ---- guarded tool execution ------------------------------------------

    pub fn tool_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tools.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn tool_risk(&self, name: &str) -> Option<RiskLevel> {
        self.tools.get(name).map(|t| t.risk_level())
    }

    /// Step 1 of the HITL flow: describe an action and get a pending
    /// confirmation. Nothing executes.
    pub fn request_confirmation(
        &mut self,
        tool: &str,
        args: &str,
        now: NaiveDateTime,
    ) -> Result<PendingConfirmation> {
        let t = self
            .tools
            .get(tool)
            .ok_or_else(|| CoreError::NotFound(format!("tool {tool}")))?;
        let ctx = ToolCtx {
            store: &self.store,
            now,
        };
        Ok(self.guard.request_confirmation(t.as_ref(), args, now, &ctx))
    }

    /// Step 2: human approves → mint a one-time token.
    pub fn confirm(&mut self, pending_id: &str, now: NaiveDateTime) -> Result<ExecutionToken> {
        self.guard.confirm(pending_id, now)
    }

    /// Step 3: run the tool. Persists audit records (executed or refused) to the
    /// append-only log regardless of outcome.
    pub fn run_tool(
        &mut self,
        tool: &str,
        args: &str,
        token: Option<ExecutionToken>,
        now: NaiveDateTime,
    ) -> Result<String> {
        let t = self
            .tools
            .get(tool)
            .ok_or_else(|| CoreError::NotFound(format!("tool {tool}")))?;
        let ctx = ToolCtx {
            store: &self.store,
            now,
        };
        let res = self.guard.run(t.as_ref(), args, token, now, &ctx);
        for entry in self.guard.drain_audit() {
            self.store.append_audit(&entry)?;
        }
        // A tool writes through `ToolCtx.store`, i.e. behind this orchestrator's
        // back, so anything it cached may now be stale. `persona_clear` is the
        // concrete case: without this the deleted persona stays live in memory
        // and keeps shaping replies until the next restart.
        if res.is_ok() {
            self.reload_caches()?;
        }
        res
    }
}

pub(crate) fn intent_str(intent: Intent) -> &'static str {
    match intent {
        Intent::Chat => "chat",
        Intent::CreateWidget => "create_widget",
        Intent::IngestEvent => "ingest_event",
        Intent::StatusAnswer => "status_answer",
        Intent::MemoryWrite => "memory_write",
        Intent::RescheduleEvent => "reschedule_event",
        Intent::CancelEvent => "cancel_event",
        Intent::DangerousCommand => "dangerous_command",
    }
}

/// D3: recognize a purge-shaped dangerous command and derive its arguments.
/// Requires a purge verb (already guaranteed by the danger routing), a layer
/// keyword, and optionally a time range ("上周/N天前"; absent = 全部).
pub(crate) fn parse_purge_request(
    text: &str,
    now: NaiveDateTime,
) -> Option<(MemoryLayer, NaiveDateTime)> {
    let layer = if text.contains("行为日志") || text.contains("日志") {
        MemoryLayer::Behavior
    } else if text.contains("建议") {
        MemoryLayer::Suggestion
    } else if text.contains("穿戴") || text.contains("健康数据") {
        MemoryLayer::Wearable
    } else {
        return None;
    };
    // "上周的/一周前的" → older than 7 days; "N天前" → older than N days;
    // no range word (or 全部/所有) → everything (before `now`).
    let before = if text.contains("上周") || text.contains("一周") {
        now - Duration::days(7)
    } else if let Some(days) = extract_days_back(text) {
        now - Duration::days(days)
    } else {
        now
    };
    Some((layer, before))
}

/// "3天前 / 三天以前" → 3. Digits and simple Chinese numerals only.
fn extract_days_back(text: &str) -> Option<i64> {
    let idx = text.find("天前").or_else(|| text.find("天以前"))?;
    let head: String = text[..idx]
        .chars()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    // Trailing run of digit-ish chars right before "天前".
    let digits: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || "零一二两三四五六七八九十".contains(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        return None;
    }
    if let Ok(n) = digits.parse::<i64>() {
        return (n > 0).then_some(n);
    }
    let mapped: i64 = match digits.as_str() {
        "一" => 1,
        "两" | "二" => 2,
        "三" => 3,
        "四" => 4,
        "五" => 5,
        "六" => 6,
        "七" => 7,
        "八" => 8,
        "九" => 9,
        "十" => 10,
        _ => return None,
    };
    Some(mapped)
}

pub(crate) fn layer_label(layer: MemoryLayer) -> &'static str {
    match layer {
        MemoryLayer::RawInput => "原始输入",
        MemoryLayer::Event => "日程",
        MemoryLayer::Notification => "提醒",
        MemoryLayer::Behavior => "行为日志",
        MemoryLayer::Suggestion => "建议",
        MemoryLayer::Wearable => "穿戴数据",
        MemoryLayer::Fact => "记忆",
        MemoryLayer::Routine => "固定提醒",
        MemoryLayer::NotificationCapture => "通知捕获",
    }
}

pub(crate) fn summarize_event(ev: &Event, notifs: &[Notification], now: NaiveDateTime) -> String {
    let loc = ev
        .location
        .as_ref()
        .map(|l| format!("，地点：{l}"))
        .unwrap_or_default();
    let people = if ev.people.is_empty() {
        String::new()
    } else {
        format!("，参与：{}", ev.people.join("、"))
    };
    let mut msg = format!(
        "已记录【{}】{}，时间：{}{}{}",
        kind_label(ev.kind),
        ev.title,
        crate::model::fmt_ts_human(&ev.start),
        loc,
        people
    );
    if notifs.is_empty() {
        msg.push_str("；未设置提前提醒。");
    } else {
        msg.push_str(&format!("；{}", summarize_reminders(notifs, now)));
    }
    msg
}

/// Just the reminder-plan tail of [`summarize_event`], for reschedule replies.
/// A reminder whose fire time is already behind `now`（比如今天才录入三天后的
/// 考试）is called out — otherwise the user thinks a heads-up is still coming.
pub(crate) fn summarize_reminders(notifs: &[Notification], now: NaiveDateTime) -> String {
    if notifs.is_empty() {
        return "未设置提前提醒。".into();
    }
    let times: Vec<String> = notifs
        .iter()
        .map(|n| {
            let when = crate::model::fmt_ts_human(&n.fire_at);
            if n.fire_at <= now {
                format!("{when}（提前{}·已过点，将立即提醒）", n.lead_label)
            } else {
                format!("{when}（提前{}）", n.lead_label)
            }
        })
        .collect();
    format!("提醒计划：{}", times.join("，"))
}

/// Daily phrases are mapped to `routines`; the remaining recurrence phrases
/// still have no correct scheduler model, so say so instead of silently
/// degrading to a one-shot entry.
pub(crate) fn is_daily_recurrence(text: &str) -> bool {
    ["每天", "每日", "每早", "每晚"]
        .iter()
        .any(|word| text.contains(word))
}

pub(crate) fn recurrence_caveat(text: &str) -> Option<&'static str> {
    const WORDS: &[&str] = &["每小时", "每周", "每月"];
    WORDS.iter().any(|w| text.contains(w)).then_some(
        "注意：暂不支持重复提醒，这里只登记了最近的一次（习惯类可通过「固定提醒」实现）。",
    )
}

/// Combine a reschedule request with the target event: whichever half the
/// phrase didn't specify keeps the event's original value ("改到4点" keeps
/// the date, "改到周五" keeps the clock time).
pub(crate) fn compose_new_start(
    ev: &Event,
    req: &crate::extract::RescheduleRequest,
) -> NaiveDateTime {
    let date = req.new_date.unwrap_or_else(|| ev.start.date());
    let time = req
        .new_time
        .and_then(|(h, m)| chrono::NaiveTime::from_hms_opt(h, m, 0))
        .unwrap_or_else(|| ev.start.time());
    NaiveDateTime::new(date, time)
}

/// Filler words stripped from a target description before title matching —
/// they scope the search (dates handle that separately) but never name it.
// Longer phrases first: "下星期五" must strip before "星期五" leaves "下"
// behind, "大后天" before "后天".
pub(crate) const TARGET_FILLERS: &[&str] = &[
    "下星期一",
    "下星期二",
    "下星期三",
    "下星期四",
    "下星期五",
    "下星期六",
    "下星期日",
    "下星期天",
    "星期一",
    "星期二",
    "星期三",
    "星期四",
    "星期五",
    "星期六",
    "星期日",
    "星期天",
    "下周一",
    "下周二",
    "下周三",
    "下周四",
    "下周五",
    "下周六",
    "下周日",
    "下周天",
    "周一",
    "周二",
    "周三",
    "周四",
    "周五",
    "周六",
    "周日",
    "周天",
    "下周",
    "这周",
    "本周",
    "大后天",
    "后天",
    "今天",
    "明天",
    "上午",
    "下午",
    "晚上",
    "早上",
    "中午",
    "那个",
    "那场",
    "这个",
    "这场",
    "的",
];

pub(crate) fn kind_label(kind: crate::model::EventKind) -> &'static str {
    kind.label()
}

// ---- built-in tools ---------------------------------------------------------

pub(crate) fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(EchoTool),
        Box::new(SimulatedDeleteTool),
        Box::new(LedgerPurgeTool),
        Box::new(WidgetDeleteTool),
        Box::new(DataImportTool),
        Box::new(BackupDeleteTool),
        Box::new(SoulousPushEventTool),
        Box::new(EmailSendTool),
        Box::new(MemoryForgetTool),
        Box::new(PersonaClearTool),
        Box::new(WidgetRecordDeleteTool),
        Box::new(EventCancelTool),
    ]
}

/// A trivially safe tool.
struct EchoTool;
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Safe
    }
    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        format!("回显：{args}")
    }
    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        Ok(args.to_string())
    }
}

/// A **simulated** dangerous tool for demonstrating the guard end-to-end. It
/// deliberately does not touch anything real; kept alongside `ledger_purge`
/// as the harmless demo surface.
struct SimulatedDeleteTool;
impl Tool for SimulatedDeleteTool {
    fn name(&self) -> &str {
        "demo_delete"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        format!("将永久删除（模拟，不会真正删除）：{args}")
    }
    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        Ok(format!("（模拟）已删除：{args}"))
    }
}

/// The first **real** dangerous tool (D3): bulk-delete ledger rows of one
/// layer older than a cutoff. Args are JSON:
/// `{"layer":"behavior|suggestion|wearable","before":"YYYY-MM-DDTHH:MM:SS"}`.
/// The preview reads the store so the human confirms a real row count; the
/// execution is unreachable without the guard's one-time token, and both
/// outcomes land in the append-only audit log.
struct LedgerPurgeTool;

impl LedgerPurgeTool {
    fn parse_args(args: &str) -> Result<(MemoryLayer, NaiveDateTime)> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("ledger_purge 参数不是 JSON: {e}")))?;
        let layer = match v["layer"].as_str() {
            Some("behavior") => MemoryLayer::Behavior,
            Some("suggestion") => MemoryLayer::Suggestion,
            Some("wearable") => MemoryLayer::Wearable,
            other => {
                return Err(CoreError::Invalid(format!(
                    "ledger_purge.layer 必须是 behavior|suggestion|wearable，得到 {other:?}"
                )))
            }
        };
        let before = v["before"]
            .as_str()
            .ok_or_else(|| CoreError::Invalid("ledger_purge 缺少 before".into()))
            .and_then(crate::model::parse_ts)?;
        Ok((layer, before))
    }
}

impl Tool for LedgerPurgeTool {
    fn name(&self) -> &str {
        "ledger_purge"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::parse_args(args) {
            Ok((layer, before)) => {
                let count = ctx.store.count_memory_before(layer, before).unwrap_or(0);
                format!(
                    "将从记忆台账永久删除 {count} 条「{}」记录（早于 {}）。删除后不可通过对话恢复。",
                    layer_label(layer),
                    fmt_ts(&before)
                )
            }
            Err(e) => format!("参数无效，无法执行：{e}"),
        }
    }
    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let (layer, before) = Self::parse_args(args)?;
        let n = ctx.store.purge_memory_before(layer, before)?;
        Ok(format!(
            "已永久删除 {n} 条「{}」记录（早于 {}）。",
            layer_label(layer),
            fmt_ts(&before)
        ))
    }
}

/// Restoring a backup writes across every layer at once. It is not
/// destructive by design (nothing is deleted, and LWW keeps newer local rows),
/// but "merge a whole other database into mine" is exactly the kind of
/// broad-blast-radius write the Guard exists for — the user should see how
/// many rows, from which device, and from when, before it happens.
struct DataImportTool;

impl DataImportTool {
    /// Ceiling on the raw argument string, checked **before** parsing.
    ///
    /// The shell checks `file.size` before reading, but that is a courtesy in
    /// the UI, not a limit: the IPC command takes a string and can be invoked
    /// directly, and `serde_json::from_str` allocates the whole document tree
    /// before any of the row/field limits downstream get a chance to look at
    /// it. A limit that only exists in the file picker is not a limit.
    const MAX_ARGS_BYTES: usize = 64 * 1024 * 1024;

    fn document(args: &str) -> Result<serde_json::Value> {
        if args.len() > Self::MAX_ARGS_BYTES {
            return Err(CoreError::Invalid(format!(
                "导入内容超过 {} MiB 上限，已拒绝解析",
                Self::MAX_ARGS_BYTES / 1024 / 1024
            )));
        }
        let value: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("data_import 参数不是 JSON：{e}")))?;
        value
            .get("document")
            .cloned()
            .ok_or_else(|| CoreError::Invalid("data_import 缺少 document".into()))
    }
}

/// Permanently remove one complete opaque backup version. The exact catalog
/// metadata the user saw is embedded in the Guard fingerprint; execution
/// re-lists the catalog and the server atomically checks the plan ETag again.
struct BackupDeleteTool;

impl BackupDeleteTool {
    fn request(args: &str) -> Result<crate::backup_download::BackupDeleteRequest> {
        let request: crate::backup_download::BackupDeleteRequest = serde_json::from_str(args)
            .map_err(|error| {
                CoreError::Invalid(format!("backup_delete 参数不是严格 JSON：{error}"))
            })?;
        crate::backup_download::validate_backup_delete_request(&request)
            .map_err(|error| CoreError::Invalid(error.to_string()))?;
        Ok(request)
    }

    fn size_label(bytes: u64) -> String {
        const MIB: f64 = 1024.0 * 1024.0;
        if bytes >= 1024 * 1024 {
            format!("{:.1} MiB", bytes as f64 / MIB)
        } else {
            format!("{:.1} KiB", bytes as f64 / 1024.0)
        }
    }

    fn committed_label(value: &str) -> String {
        DateTime::parse_from_rfc3339(value)
            .map(|date| {
                date.with_timezone(&Utc)
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string()
            })
            .unwrap_or_else(|_| value.to_string())
    }
}

impl Tool for BackupDeleteTool {
    fn name(&self) -> &str {
        "backup_delete"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn request_summary(&self, _args: &str) -> String {
        "删除一个云端加密备份版本".into()
    }

    fn audit_summary(&self, _args: &str) -> String {
        "删除一个云端加密备份版本（标识已脱敏）".into()
    }

    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        match Self::request(args) {
            Ok(request) => format!(
                "将永久删除 {} 提交的云端加密备份（{}，{} 个密文对象）。删除后不能再用这个封缄点恢复；本机数据和其他备份版本不受影响。",
                Self::committed_label(&request.committed_at),
                Self::size_label(request.total_bytes),
                request.object_count
            ),
            Err(error) => format!("备份版本信息无效，无法删除：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        if crate::privacy::PrivacyConsent::needs_consent() {
            return Err(CoreError::Invalid(
                "请先阅读并同意当前隐私政策，再管理加密云备份".into(),
            ));
        }
        let request = Self::request(args)?;
        let mut account = crate::account::AccountSession::load_full()
            .ok_or_else(|| CoreError::Invalid("登录后才能删除加密云备份".into()))?;
        if account.stable_user_id().is_none() {
            return Err(CoreError::Invalid("当前账号缺少不可变用户标识".into()));
        }
        let transport = crate::backup_download::HttpBackupCloudReader::new();
        crate::backup_download::delete_backup_version(&transport, &mut account, &request)
            .map_err(|error| CoreError::Invalid(error.to_string()))?;
        Ok("已永久删除所选云端加密备份版本。".into())
    }
}

impl Tool for DataImportTool {
    fn name(&self) -> &str {
        "data_import"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn request_summary(&self, _args: &str) -> String {
        // Constant-sized even for malformed/oversized input. The Guard still
        // fingerprints the complete arguments independently of this label.
        "导入本机备份（正文不记入审计）".into()
    }

    fn audit_summary(&self, args: &str) -> String {
        self.request_summary(args)
    }

    fn audit_error_summary(&self, _error: &CoreError) -> String {
        // Import errors may quote document-controlled table/field names.
        "导入失败（详细原因不记入审计）".into()
    }

    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        match Self::document(args).and_then(|doc| crate::export::plan_import(&doc)) {
            Ok(plan) => {
                let detail = plan
                    .counts
                    .iter()
                    .map(|(tbl, n)| format!("{tbl} {n} 条"))
                    .collect::<Vec<_>>()
                    .join("、");
                format!(
                    "将合并来自设备 {} 于 {} 导出的 {} 条数据（{}）。\
                     不会删除任何本机数据；本机更新过的记录保持本机版本。",
                    plan.origin,
                    plan.exported_at.format("%Y-%m-%d %H:%M"),
                    plan.total(),
                    detail
                )
            }
            Err(error) => format!("无法读取该导出文件：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let doc = Self::document(args)?;
        let (plan, counts) = crate::export::import_document(ctx.store, &doc, ctx.now)?;
        Ok(format!(
            "已从 {} 的备份合并 {} 条数据：写入 {} 条，跳过 {} 条（本机已是更新版本或重复导入）。",
            plan.origin,
            plan.total(),
            counts.applied,
            counts.skipped
        ))
    }
}

/// F12's per-row deletion, behind the Guard.
///
/// This used to be a bare IPC command (`forget`) gated only by a confirmation
/// dialog in the WebView. A dialog is a rendering, not an authorization
/// boundary: the command underneath it can be invoked directly, so anything
/// that got script execution inside the WebView could empty the ledger row by
/// row. §3.3 says destructive operations go through preview → confirmation →
/// one-time token, and there is no reason this one was exempt.
struct MemoryForgetTool;

impl MemoryForgetTool {
    fn parse_args(args: &str) -> Result<(MemoryLayer, i64)> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("forget 参数不是 JSON: {e}")))?;
        let layer: MemoryLayer = v["layer"]
            .as_str()
            .ok_or_else(|| CoreError::Invalid("forget 缺少 layer".into()))?
            .parse()?;
        let id = v["id"]
            .as_i64()
            .ok_or_else(|| CoreError::Invalid("forget 缺少 id".into()))?;
        Ok((layer, id))
    }
}

impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    /// The row's own text is the user's private content; keep it out of the
    /// audit trail, which would otherwise become a second copy of everything
    /// they asked to delete.
    fn audit_summary(&self, args: &str) -> String {
        match Self::parse_args(args) {
            Ok((layer, id)) => format!("memory_forget({}#{id})", layer.as_str()),
            Err(_) => "memory_forget(参数无效)".to_string(),
        }
    }
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::parse_args(args) {
            Ok((layer, id)) => {
                let label = layer_label(layer);
                let summary = ctx
                    .store
                    .memory_summary(layer, id)
                    .ok()
                    .flatten()
                    .map(|s| format!("「{}」", truncate_for_preview(&s)))
                    .unwrap_or_else(|| format!("#{id}"));
                let cascade = ctx
                    .store
                    .describe_memory_deletion(layer, id)
                    .unwrap_or_default();
                format!("将永久删除{label}{summary}{cascade}。删除后不可通过对话恢复。")
            }
            Err(e) => format!("参数无效，无法执行：{e}"),
        }
    }
    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let (layer, id) = Self::parse_args(args)?;
        ctx.store.delete_memory(layer, id)?;
        Ok(format!("已永久删除{}#{id}。", layer_label(layer)))
    }
}

/// Cancelling an event cascades to its reminders, so a visible button is not
/// enough authorization. The preview binds the token to the event's current
/// title, time and reminder count; edits between preview and execution make the
/// token stale.
struct EventCancelTool;

impl EventCancelTool {
    fn event_id(args: &str) -> Result<i64> {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| CoreError::Invalid(format!("event_cancel 参数不是 JSON：{e}")))?
            .get("event_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| CoreError::Invalid("event_cancel 缺少正整数 event_id".into()))
    }
}

impl Tool for EventCancelTool {
    fn name(&self) -> &str {
        "event_cancel"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn audit_summary(&self, args: &str) -> String {
        match Self::event_id(args) {
            Ok(id) => format!("event_cancel(event#{id})"),
            Err(_) => "event_cancel(参数无效)".to_string(),
        }
    }

    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::event_id(args).and_then(|id| {
            let event = ctx.store.get_event(id)?;
            let reminders = ctx
                .store
                .list_notifications()?
                .into_iter()
                .filter(|notification| notification.event_id == id)
                .count();
            Ok((event, reminders))
        }) {
            Ok((event, reminders)) => format!(
                "将永久取消日程「{}」（{}），并删除其关联的 {} 条提醒。此操作不可恢复。",
                event.title,
                crate::model::fmt_ts_human(&event.start),
                reminders
            ),
            Err(error) => format!("参数或日程无效，无法执行：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let id = Self::event_id(args)?;
        let event = ctx.store.get_event(id)?;
        ctx.store.delete_event(id)?;
        Ok(format!("已取消「{}」并删除其提醒。", event.title))
    }
}

/// Deleting every persona version — the user's whole style history at once.
struct PersonaClearTool;

impl Tool for PersonaClearTool {
    fn name(&self) -> &str {
        "persona_clear"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, _args: &str, ctx: &ToolCtx) -> String {
        let n = ctx
            .store
            .list_persona_versions()
            .map(|v| v.len())
            .unwrap_or(0);
        format!("将永久删除全部 {n} 个人格版本与当前活动指针。版本历史不可恢复。")
    }
    fn execute(&self, _args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        ctx.store.clear_persona()?;
        Ok("已删除全部人格版本。".to_string())
    }
}

/// One widget record. Individually small, but it is still an irreversible
/// deletion of user data and the same rule applies.
struct WidgetRecordDeleteTool;

impl WidgetRecordDeleteTool {
    fn parse_args(args: &str) -> Result<(i64, i64)> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("widget_record_delete 参数不是 JSON: {e}")))?;
        let widget_id = v["widget_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Invalid("widget_record_delete 缺少 widget_id".into()))?;
        let record_id = v["record_id"]
            .as_i64()
            .ok_or_else(|| CoreError::Invalid("widget_record_delete 缺少 record_id".into()))?;
        Ok((widget_id, record_id))
    }
}

impl Tool for WidgetRecordDeleteTool {
    fn name(&self) -> &str {
        "widget_record_delete"
    }
    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }
    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::parse_args(args) {
            Ok((widget_id, record_id)) => {
                let name = ctx
                    .store
                    .get_widget_definition(widget_id)
                    .map(|d| d.name)
                    .unwrap_or_else(|_| format!("#{widget_id}"));
                format!("将永久删除组件「{name}」的记录 #{record_id}。删除后不可恢复。")
            }
            Err(e) => format!("参数无效，无法执行：{e}"),
        }
    }
    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let (widget_id, record_id) = Self::parse_args(args)?;
        ctx.store.delete_widget_record(widget_id, record_id)?;
        Ok("记录已删除。".to_string())
    }
}

/// Keep a preview line readable when the underlying content is long.
fn truncate_for_preview(s: &str) -> String {
    const MAX: usize = 40;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}

/// F19's one destructive structural operation. Record CRUD is safe, but a
/// definition deletion cascades to its complete local dataset and therefore
/// can only run through the Guard's preview → confirmation → one-time Grant.
struct WidgetDeleteTool;

impl WidgetDeleteTool {
    fn widget_id(args: &str) -> Result<i64> {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| CoreError::Invalid(format!("widget_delete 参数不是 JSON：{e}")))?
            .get("widget_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| CoreError::Invalid("widget_delete 缺少正整数 widget_id".into()))
    }
}

impl Tool for WidgetDeleteTool {
    fn name(&self) -> &str {
        "widget_delete"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Dangerous
    }

    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::widget_id(args).and_then(|id| {
            let definition = ctx.store.get_widget_definition(id)?;
            let records = ctx.store.widget_record_count(id)?;
            Ok((definition.name, records))
        }) {
            Ok((name, records)) => {
                format!("将永久删除组件「{name}」及其全部 {records} 条记录。此操作不可恢复。")
            }
            Err(error) => format!("参数或组件无效，无法执行：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let id = Self::widget_id(args)?;
        let (name, records) = ctx.store.delete_widget_definition(id)?;
        Ok(format!("已永久删除组件「{name}」及其 {records} 条记录。"))
    }
}

/// L2 Phase 8.2: the only outbound Solum → Soulous action initially exposed.
/// It is `Sensitive`, so every execution goes through Guard's visible preview,
/// one-time token, and append-only audit record. The tool has no scheduler or
/// sync caller by design; it cannot silently become background export.
struct SoulousPushEventTool;

impl SoulousPushEventTool {
    fn event_id(args: &str) -> Result<i64> {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| CoreError::Invalid(format!("soulous_push_event 参数不是 JSON: {e}")))?
            .get("event_id")
            .and_then(serde_json::Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| CoreError::Invalid("soulous_push_event 缺少正整数 event_id".into()))
    }
}

impl Tool for SoulousPushEventTool {
    fn name(&self) -> &str {
        "soulous_push_event"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Sensitive
    }

    fn preview(&self, args: &str, ctx: &ToolCtx) -> String {
        match Self::event_id(args).and_then(|id| crate::soulous::preview_event_push(ctx.store, id))
        {
            Ok(preview) => preview,
            Err(error) => format!("参数或日程无效，无法推送：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, ctx: &ToolCtx) -> Result<String> {
        let event_id = Self::event_id(args)?;
        let outcome = crate::soulous::push_event_configured(ctx.store, event_id)?;
        Ok(format!(
            "已向 Soulous 推送日程「{}」{}。",
            outcome.title,
            if outcome.refreshed_tokens {
                "（登录 token 已自动刷新）"
            } else {
                ""
            }
        ))
    }
}

/// F21's only outbound mail action. The exact serialized draft is the Guard
/// token fingerprint, so confirmation cannot be replayed against a changed
/// recipient or body. The audit summary deliberately excludes all message
/// content; the full data remains visible only in the transient preview.
struct EmailSendTool;

impl Tool for EmailSendTool {
    fn name(&self) -> &str {
        "email_send"
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Sensitive
    }

    fn request_summary(&self, args: &str) -> String {
        match crate::email::EmailSendDraft::parse_json(args) {
            Ok(draft) => format!(
                "邮件发送：账户 {}，收件人 {} 位",
                draft.account_id,
                draft.to.len() + draft.cc.len() + draft.bcc.len()
            ),
            Err(error) => format!("邮件发送（草稿无效：{error}）"),
        }
    }

    fn audit_summary(&self, args: &str) -> String {
        crate::email::EmailSendDraft::parse_json(args)
            .map(|draft| draft.audit_summary())
            .unwrap_or_else(|_| "邮件发送：草稿无效，内容已脱敏".into())
    }

    fn preview(&self, args: &str, _ctx: &ToolCtx) -> String {
        match crate::email::EmailSendDraft::parse_json(args)
            .and_then(|draft| crate::email::preview_configured(&draft))
        {
            Ok(preview) => preview,
            Err(error) => format!("邮件草稿或账户无效，无法发送：{error}"),
        }
    }

    fn execute(&self, args: &str, _grant: &Grant, _ctx: &ToolCtx) -> Result<String> {
        let draft = crate::email::EmailSendDraft::parse_json(args)?;
        crate::email::send_configured(&draft)
    }
}

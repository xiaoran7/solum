//! Soulous read-only fact source (Phase 8.1, ARCHITECTURE.md §3.11).
//!
//! This module is deliberately a *data source*, not a memory writer: fetched
//! rows stay in `soulous_facts`, never enter `memory_facts` or recall, and
//! network failures leave the prior complete snapshot untouched. All business
//! timestamps are supplied by the shell as `now`; solum-core never reads a clock.

use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::classify::{ImportanceRule, RuleTable};
use crate::error::{CoreError, Result};
use crate::model::parse_ts;
use crate::store::Store;
use crate::suggest::{Suggestion, SuggestionKind, SuggestionStatus};

pub const SOURCE: &str = "soulous";
/// Source marker for the narrowly-scoped Solum → Soulous outbound contract.
/// It is intentionally distinct from [`SOURCE`], which marks read-only rows
/// pulled *from* Soulous into Solum.
pub const SOLUM_SOURCE: &str = "pa";
pub const SCHEDULE_EVENT_TYPE: &str = "schedule_event";

fn default_timeout_secs() -> u64 {
    15
}

/// Local-only connection credentials for the user's own Soulous deployment.
/// The file (`solum-soulous.json`) is gitignored; this type never carries a
/// username/password because Solum reuses Soulous's existing dual-token session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoulousConfig {
    #[serde(alias = "base_url")]
    pub server_url: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// L2 outbound whitelist. It is deliberately opt-in and currently has
    /// only one category: a user-confirmed Solum schedule event.
    #[serde(default)]
    pub push_schedule_events: bool,
}

impl SoulousConfig {
    /// Load a complete config. Missing or malformed local configuration is an
    /// intentional silent-off state, consistent with `solum-llm.json`.
    pub fn load() -> Option<Self> {
        let raw = std::fs::read_to_string(Self::path()).ok()?;
        Self::from_json(&raw).ok()
    }

    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("SOLUM_SOULOUS_CONFIG") {
            return p.into();
        }
        crate::paths::resolve_with_adoption("solum-soulous.json")
    }

    pub fn from_json(raw: &str) -> Result<Self> {
        let mut config: Self = serde_json::from_str(raw)?;
        config.normalize()?;
        Ok(config)
    }

    pub fn normalize(&mut self) -> Result<()> {
        self.server_url = crate::net::validate_endpoint(&self.server_url, "Soulous server_url")?;
        self.access_token = self.access_token.trim().to_string();
        self.refresh_token = self.refresh_token.trim().to_string();
        self.timeout_secs = self.timeout_secs.clamp(3, 120);
        if self.access_token.is_empty() || self.refresh_token.is_empty() {
            return Err(CoreError::Invalid(
                "solum-soulous.json 缺 access_token 或 refresh_token".into(),
            ));
        }
        Ok(())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        // Atomic: a half-written config silently degrades to "not configured".
        crate::fsatomic::write_atomic(path, &text)
            .map_err(|e| CoreError::Soulous(format!("写入 {} 失败: {e}", path.display())))
    }

    pub fn masked_summary(&self) -> String {
        format!(
            "{} · access:{} · refresh:{}",
            self.server_url,
            token_tail(&self.access_token),
            token_tail(&self.refresh_token)
        )
    }
}

/// The minimum, structured projection Solum may send to the user's own Soulous
/// server. Never add `people`, `raw_input`, notification text, or any other
/// provenance-bearing field here: those remain L1 local-only data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ExternalContextRequest {
    source: &'static str,
    #[serde(rename = "type")]
    context_type: &'static str,
    #[serde(rename = "externalId")]
    external_id: String,
    #[serde(rename = "occurredAt")]
    occurred_at: String,
    payload: ScheduleEventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ScheduleEventPayload {
    title: String,
    kind: String,
    start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushOutcome {
    pub external_id: String,
    pub title: String,
    pub refreshed_tokens: bool,
}

fn outbound_event_request(store: &Store, event_id: i64) -> Result<ExternalContextRequest> {
    let event = store.get_event(event_id)?;
    let (guid, local_only) = store.event_guid_and_local_only(event_id)?;
    if local_only {
        return Err(CoreError::Soulous(
            "此日程来自第三方通知，按隐私规则不得推送到 Soulous".into(),
        ));
    }
    Ok(ExternalContextRequest {
        source: SOLUM_SOURCE,
        context_type: SCHEDULE_EVENT_TYPE,
        external_id: guid,
        occurred_at: crate::model::fmt_ts(&event.created_at),
        payload: ScheduleEventPayload {
            title: event.title.trim().to_string(),
            kind: event.kind.as_str().to_string(),
            start: crate::model::fmt_ts(&event.start),
            end: event.end.map(|at| crate::model::fmt_ts(&at)),
            location: event.location.and_then(|value| {
                let value = value.trim().to_string();
                (!value.is_empty()).then_some(value)
            }),
        },
    })
}

/// Human-readable effect preview for the Sensitive Tool confirmation. This is
/// deliberately generated from the same restricted projection that will be
/// serialized, so the UI cannot promise a narrower data surface than the
/// actual request.
pub fn preview_event_push(store: &Store, event_id: i64) -> Result<String> {
    let request = outbound_event_request(store, event_id)?;
    let location = request
        .payload
        .location
        .as_deref()
        .map(|value| format!("；地点：{value}"))
        .unwrap_or_default();
    Ok(format!(
        "将向你自己的 Soulous 推送日程「{}」（{}，{}{}）。仅发送标题、类型、时间和地点；不会发送参与人、原始输入、通知文本或 Solum 记忆。",
        request.payload.title,
        request.payload.kind,
        request.payload.start,
        location
    ))
}

fn token_tail(token: &str) -> String {
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!("…{tail}")
    }
}

/// The five sources Soulous currently exposes through its authenticated REST
/// controllers. Kept separate from Solum's memory layers on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoulousKind {
    Course,
    Exam,
    Task,
    Checkin,
    Focus,
}

impl SoulousKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SoulousKind::Course => "course",
            SoulousKind::Exam => "exam",
            SoulousKind::Task => "task",
            SoulousKind::Checkin => "checkin",
            SoulousKind::Focus => "focus",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SoulousKind::Course => "课表",
            SoulousKind::Exam => "考试",
            SoulousKind::Task => "学习任务",
            SoulousKind::Checkin => "打卡",
            SoulousKind::Focus => "专注",
        }
    }
}

impl std::str::FromStr for SoulousKind {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "course" => Ok(SoulousKind::Course),
            "exam" => Ok(SoulousKind::Exam),
            "task" => Ok(SoulousKind::Task),
            "checkin" => Ok(SoulousKind::Checkin),
            "focus" => Ok(SoulousKind::Focus),
            _ => Err(CoreError::Invalid(format!("unknown Soulous kind: {s}"))),
        }
    }
}

/// A normalized, read-only Soulous record. `payload_json` preserves the
/// controller DTO losslessly for local inspection while the selected columns
/// support deterministic local rules and summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoulousFact {
    pub id: Option<i64>,
    pub external_id: String,
    pub kind: SoulousKind,
    pub title: String,
    pub occurs_at: Option<NaiveDateTime>,
    pub ends_at: Option<NaiveDateTime>,
    pub payload_json: String,
    pub source: String,
    pub imported_at: NaiveDateTime,
}

impl SoulousFact {
    /// Stable source identity. Unlike user-authored Solum rows this is derived
    /// from Soulous's own primary key, so two Solum devices converge on one row.
    pub fn guid(&self) -> String {
        format!("soulous:{}:{}", self.kind.as_str(), self.external_id)
    }

    fn payload(&self) -> Value {
        serde_json::from_str(&self.payload_json).unwrap_or(Value::Null)
    }
}

/// A classifier output over an actual imported exam. It does not create a Solum
/// event or notification; it simply gives F10/the UI the same canonical rule
/// decision that Solum-authored exam events receive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoulousExamImportance {
    pub external_id: String,
    pub title: String,
    pub occurs_at: NaiveDateTime,
    pub rule: ImportanceRule,
}

pub fn classify_exams(
    facts: &[SoulousFact],
    rules: &RuleTable,
    now: NaiveDateTime,
) -> Vec<SoulousExamImportance> {
    let mut exams: Vec<_> = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Exam)
        .filter_map(|f| {
            let occurs_at = f.occurs_at?;
            (occurs_at >= now).then(|| SoulousExamImportance {
                external_id: f.external_id.clone(),
                title: f.title.clone(),
                occurs_at,
                rule: rules.rule(crate::model::EventKind::Exam),
            })
        })
        .collect();
    exams.sort_by_key(|e| e.occurs_at);
    exams
}

/// Read-only status for CLI/UI. `last_success_at` comes solely from the local
/// cached snapshot; no network call is made to render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoulousStatus {
    pub total: usize,
    pub courses: usize,
    pub exams: usize,
    pub tasks: usize,
    pub checkins: usize,
    pub focus_sessions: usize,
    pub last_success_at: Option<NaiveDateTime>,
    pub upcoming_exams: Vec<SoulousExamImportance>,
}

pub fn build_status(facts: &[SoulousFact], rules: &RuleTable, now: NaiveDateTime) -> SoulousStatus {
    let count = |kind| facts.iter().filter(|f| f.kind == kind).count();
    SoulousStatus {
        total: facts.len(),
        courses: count(SoulousKind::Course),
        exams: count(SoulousKind::Exam),
        tasks: count(SoulousKind::Task),
        checkins: count(SoulousKind::Checkin),
        focus_sessions: count(SoulousKind::Focus),
        last_success_at: facts.iter().map(|f| f.imported_at).max(),
        upcoming_exams: classify_exams(facts, rules, now),
    }
}

/// F10 material sourced from Soulous. It intentionally produces only local
/// suggestions, not `events`/notifications: a read-only source may inform Solum
/// but must not take over the reminder trigger chain.
pub fn generate_suggestions(
    facts: &[SoulousFact],
    now: NaiveDateTime,
    horizon_days: i64,
) -> Vec<Suggestion> {
    let until = now + chrono::Duration::days(horizon_days);
    let mut out = Vec::new();
    for fact in facts {
        let Some(when) = fact.occurs_at else { continue };
        if when <= now || when > until {
            continue;
        }
        let source = format!("soulous:{}:{}", fact.kind.as_str(), fact.external_id);
        match fact.kind {
            SoulousKind::Exam => out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::ExamPrep,
                text: format!(
                    "Soulous 课表显示「{}」将在 {} 考试，建议从今天开始安排复习块。",
                    fact.title,
                    fact.occurs_at.expect("checked above").format("%m-%d %H:%M")
                ),
                dedup_key: format!("exam_prep:{source}"),
                source: Some(source),
                status: SuggestionStatus::Pending,
            }),
            SoulousKind::Task if task_is_open(fact) => out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::DeadlineCrunch,
                text: format!(
                    "Soulous 任务「{}」截止到 {}，建议先拆出一个可完成的小步骤。",
                    fact.title,
                    fact.occurs_at.expect("checked above").format("%m-%d")
                ),
                dedup_key: format!("deadline_crunch:{source}"),
                source: Some(source),
                status: SuggestionStatus::Pending,
            }),
            _ => {}
        }
    }
    out
}

fn task_is_open(fact: &SoulousFact) -> bool {
    !matches!(
        fact.payload()
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "DONE" | "COMPLETED" | "CLOSED" | "CANCELLED"
    )
}

/// Local-only F14 material. No details from these records are included in the
/// cloud rewrite's numeric core (review.rs appends this section afterwards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewMaterial {
    pub courses: usize,
    pub exams: usize,
    pub open_tasks: usize,
    pub checkin_days: usize,
    pub focus_minutes: i64,
}

pub fn review_material(
    facts: &[SoulousFact],
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> ReviewMaterial {
    let courses = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Course)
        .count();
    let exams = facts.iter().filter(|f| f.kind == SoulousKind::Exam).count();
    let open_tasks = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Task && task_is_open(f))
        .count();
    let checkin_days = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Checkin)
        .filter(|f| f.occurs_at.is_some_and(|t| t >= from && t <= to))
        .filter(|f| {
            f.payload()
                .get("checkedInToday")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    let focus_minutes = facts
        .iter()
        .filter(|f| f.kind == SoulousKind::Focus)
        .filter(|f| f.occurs_at.is_some_and(|t| t >= from && t <= to))
        .map(|f| {
            f.payload()
                .get("elapsedSeconds")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                / 60
        })
        .sum();
    ReviewMaterial {
        courses,
        exams,
        open_tasks,
        checkin_days,
        focus_minutes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullOutcome {
    pub courses: usize,
    pub exams: usize,
    pub tasks: usize,
    pub checkins: usize,
    pub focus_sessions: usize,
    pub refreshed_tokens: bool,
}

/// The small HTTP seam keeps parsing and offline-safety tests deterministic.
/// The production implementation below uses `ureq`; no mock server is needed.
pub trait SoulousHttp {
    fn get_json(
        &self,
        url: &str,
        access_token: &str,
    ) -> std::result::Result<Value, SoulousHttpError>;
    fn post_json(&self, url: &str, body: Value) -> std::result::Result<Value, SoulousHttpError>;
    fn post_json_authorized(
        &self,
        url: &str,
        access_token: &str,
        body: Value,
    ) -> std::result::Result<Value, SoulousHttpError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoulousHttpError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for SoulousHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoulousHttpError::Unauthorized => f.write_str("认证已过期"),
            SoulousHttpError::Other(message) => f.write_str(message),
        }
    }
}

/// Real REST adapter for Soulous's existing Spring Boot controller contract.
pub struct HttpSoulousClient {
    agent: ureq::Agent,
}

impl HttpSoulousClient {
    pub fn new(config: &SoulousConfig) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(config.timeout_secs))
                .build(),
        }
    }
}

impl SoulousHttp for HttpSoulousClient {
    fn get_json(
        &self,
        url: &str,
        access_token: &str,
    ) -> std::result::Result<Value, SoulousHttpError> {
        let response = self
            .agent
            .get(url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .call()
            .map_err(map_http_error)?;
        response
            .into_json()
            .map_err(|e| SoulousHttpError::Other(format!("响应不是 JSON: {e}")))
    }

    fn post_json(&self, url: &str, body: Value) -> std::result::Result<Value, SoulousHttpError> {
        let response = self
            .agent
            .post(url)
            .send_json(body)
            .map_err(map_http_error)?;
        response
            .into_json()
            .map_err(|e| SoulousHttpError::Other(format!("响应不是 JSON: {e}")))
    }

    fn post_json_authorized(
        &self,
        url: &str,
        access_token: &str,
        body: Value,
    ) -> std::result::Result<Value, SoulousHttpError> {
        let response = self
            .agent
            .post(url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .send_json(body)
            .map_err(map_http_error)?;
        response
            .into_json()
            .map_err(|e| SoulousHttpError::Other(format!("响应不是 JSON: {e}")))
    }
}

fn map_http_error(error: ureq::Error) -> SoulousHttpError {
    match error {
        ureq::Error::Status(401, _) => SoulousHttpError::Unauthorized,
        other => SoulousHttpError::Other(other.to_string()),
    }
}

/// Pull using the local config file. A missing config is a quiet `Ok(None)`;
/// any failed request returns an error *without touching cached rows*. If a
/// refresh token was rotated just before a later request failed, the new pair
/// is persisted anyway so the valid replacement is not lost.
pub fn pull_configured(store: &Store, now: NaiveDateTime) -> Result<Option<PullOutcome>> {
    let path = SoulousConfig::path();
    let Some(mut config) = SoulousConfig::load() else {
        return Ok(None);
    };
    let before = config.clone();
    let client = HttpSoulousClient::new(&config);
    let result = pull_with_client(store, &mut config, &client, now);
    if config != before {
        config.save_to(&path)?;
    }
    result.map(Some)
}

/// Execute the L2 outbound schedule-event contract through a configured
/// Soulous session. This function is intentionally never called from ingest,
/// the ticker, sync, or reminder delivery: its only caller is the Sensitive
/// Tool after a human has approved a one-time Guard token.
pub fn push_event_configured(store: &Store, event_id: i64) -> Result<PushOutcome> {
    let path = SoulousConfig::path();
    let Some(mut config) = SoulousConfig::load() else {
        return Err(CoreError::Soulous(
            "Soulous 未配置；请先在设置中保存你的自有服务器登录 token".into(),
        ));
    };
    if !config.push_schedule_events {
        return Err(CoreError::Soulous(
            "Soulous 日程事件推送白名单未开启；请先在设置中授权".into(),
        ));
    }
    let before = config.clone();
    let client = HttpSoulousClient::new(&config);
    let result = push_event_with_client(store, &mut config, &client, event_id);
    if config != before {
        config.save_to(&path)?;
    }
    result
}

/// Test seam for the explicit, confirmed outbound request. A 401 refreshes
/// the dual-token session exactly once and retries the same idempotent
/// `(source, externalId)` fact. The Soulous receiver owns dedup/upsert.
pub fn push_event_with_client<T: SoulousHttp>(
    store: &Store,
    config: &mut SoulousConfig,
    client: &T,
    event_id: i64,
) -> Result<PushOutcome> {
    if !config.push_schedule_events {
        return Err(CoreError::Soulous(
            "Soulous 日程事件推送白名单未开启".into(),
        ));
    }
    let request = outbound_event_request(store, event_id)?;
    let access_before = config.access_token.clone();
    post_with_refresh(
        client,
        config,
        "/api/external-context",
        serde_json::to_value(&request)?,
    )?;
    Ok(PushOutcome {
        external_id: request.external_id,
        title: request.payload.title,
        refreshed_tokens: config.access_token != access_before,
    })
}

/// Fetch every endpoint first, then atomically replace the local snapshot.
/// This ordering is the F16 guard: a timeout/malformed response cannot leave
/// a partial new cache behind or affect ingest/reminder execution.
pub fn pull_with_client<T: SoulousHttp>(
    store: &Store,
    config: &mut SoulousConfig,
    client: &T,
    now: NaiveDateTime,
) -> Result<PullOutcome> {
    let access_before = config.access_token.clone();
    let courses = fetch(client, config, "/api/timetable")?;
    let exams = fetch(client, config, "/api/timetable/exams")?;
    let tasks = fetch(client, config, "/api/tasks")?;
    let checkin = fetch(client, config, "/api/checkin")?;
    let focus = fetch(client, config, "/api/focus/sessions")?;

    let mut facts = Vec::new();
    facts.extend(parse_courses(&courses, now)?);
    facts.extend(parse_exams(&exams, now)?);
    facts.extend(parse_tasks(&tasks, now)?);
    facts.push(parse_checkin(&checkin, now)?);
    facts.extend(parse_focus(&focus, now)?);
    store.replace_soulous_snapshot(&facts)?;

    Ok(PullOutcome {
        courses: facts
            .iter()
            .filter(|f| f.kind == SoulousKind::Course)
            .count(),
        exams: facts.iter().filter(|f| f.kind == SoulousKind::Exam).count(),
        tasks: facts.iter().filter(|f| f.kind == SoulousKind::Task).count(),
        checkins: facts
            .iter()
            .filter(|f| f.kind == SoulousKind::Checkin)
            .count(),
        focus_sessions: facts
            .iter()
            .filter(|f| f.kind == SoulousKind::Focus)
            .count(),
        refreshed_tokens: config.access_token != access_before,
    })
}

fn fetch<T: SoulousHttp>(client: &T, config: &mut SoulousConfig, path: &str) -> Result<Value> {
    let url = format!("{}{}", config.server_url, path);
    match client.get_json(&url, &config.access_token) {
        Ok(value) => Ok(value),
        Err(SoulousHttpError::Unauthorized) => {
            let refreshed = client
                .post_json(
                    &format!("{}/api/auth/mobile/refresh", config.server_url),
                    json!({ "refreshToken": config.refresh_token }),
                )
                .map_err(|e| CoreError::Soulous(format!("刷新 token 失败: {e}")))?;
            let access_token = string_field(&refreshed, "accessToken")?;
            let refresh_token = string_field(&refreshed, "refreshToken")?;
            config.access_token = access_token;
            config.refresh_token = refresh_token;
            client
                .get_json(&url, &config.access_token)
                .map_err(|e| CoreError::Soulous(format!("刷新后重试 {path} 失败: {e}")))
        }
        Err(error) => Err(CoreError::Soulous(format!("读取 {path} 失败: {error}"))),
    }
}

fn post_with_refresh<T: SoulousHttp>(
    client: &T,
    config: &mut SoulousConfig,
    path: &str,
    body: Value,
) -> Result<Value> {
    let url = format!("{}{}", config.server_url, path);
    match client.post_json_authorized(&url, &config.access_token, body.clone()) {
        Ok(value) => Ok(value),
        Err(SoulousHttpError::Unauthorized) => {
            let refreshed = client
                .post_json(
                    &format!("{}/api/auth/mobile/refresh", config.server_url),
                    json!({ "refreshToken": config.refresh_token }),
                )
                .map_err(|e| CoreError::Soulous(format!("刷新 token 失败: {e}")))?;
            config.access_token = string_field(&refreshed, "accessToken")?;
            config.refresh_token = string_field(&refreshed, "refreshToken")?;
            client
                .post_json_authorized(&url, &config.access_token, body)
                .map_err(|e| CoreError::Soulous(format!("刷新后重试 {path} 失败: {e}")))
        }
        Err(error) => Err(CoreError::Soulous(format!("写入 {path} 失败: {error}"))),
    }
}

fn expect_array<'a>(value: &'a Value, endpoint: &str) -> Result<&'a [Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| CoreError::Soulous(format!("{endpoint} 响应不是数组")))
}

fn string_field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::Soulous(format!("Soulous 响应缺少 {key}")))
}

fn id_field(value: &Value, kind: SoulousKind) -> Result<String> {
    match value.get("id") {
        Some(Value::String(id)) if !id.trim().is_empty() => Ok(id.trim().to_string()),
        Some(Value::Number(id)) => Ok(id.to_string()),
        _ => Err(CoreError::Soulous(format!(
            "{} 条目缺少 id，拒绝覆盖本地缓存",
            kind.label()
        ))),
    }
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_remote_time(raw: &str) -> Option<NaiveDateTime> {
    parse_ts(raw)
        .ok()
        // Java LocalDateTime serializes with fractional seconds of varying
        // width (e.g. focus `startedAt: 2026-06-01T20:59:53.63512`) — the
        // production shape that motivated this branch.
        .or_else(|| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f").ok())
        .or_else(|| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M").ok())
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|dt| dt.naive_local())
        })
}

fn parse_deadline(raw: &str) -> Option<NaiveDateTime> {
    parse_remote_time(raw).or_else(|| {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(23, 59, 0))
    })
}

fn fact(
    kind: SoulousKind,
    external_id: String,
    title: String,
    occurs_at: Option<NaiveDateTime>,
    ends_at: Option<NaiveDateTime>,
    payload: &Value,
    now: NaiveDateTime,
) -> Result<SoulousFact> {
    Ok(SoulousFact {
        id: None,
        external_id,
        kind,
        title,
        occurs_at,
        ends_at,
        payload_json: serde_json::to_string(payload)?,
        source: SOURCE.into(),
        imported_at: now,
    })
}

fn parse_courses(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
    expect_array(value, "/api/timetable")?
        .iter()
        .map(|course| {
            let id = id_field(course, SoulousKind::Course)?;
            let name = string_field(course, "courseName")?;
            let day = course.get("dayOfWeek").and_then(Value::as_i64).unwrap_or(0);
            let time = [
                optional_string(course, "startTime"),
                optional_string(course, "endTime"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("–");
            let title = if time.is_empty() {
                format!("{name}（周{day}）")
            } else {
                format!("{name}（周{day} {time}）")
            };
            fact(SoulousKind::Course, id, title, None, None, course, now)
        })
        .collect()
}

fn parse_exams(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
    expect_array(value, "/api/timetable/exams")?
        .iter()
        .map(|exam| {
            let id = id_field(exam, SoulousKind::Exam)?;
            let title = string_field(exam, "courseName")?;
            let at = optional_string(exam, "examTime")
                .as_deref()
                .and_then(parse_remote_time)
                .ok_or_else(|| CoreError::Soulous(format!("考试「{title}」缺少可解析 examTime")))?;
            fact(SoulousKind::Exam, id, title, Some(at), None, exam, now)
        })
        .collect()
}

fn parse_tasks(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
    expect_array(value, "/api/tasks")?
        .iter()
        .map(|task| {
            let id = id_field(task, SoulousKind::Task)?;
            let title = string_field(task, "title")?;
            let deadline = optional_string(task, "deadline")
                .as_deref()
                .and_then(parse_deadline);
            fact(SoulousKind::Task, id, title, deadline, None, task, now)
        })
        .collect()
}

fn parse_checkin(value: &Value, now: NaiveDateTime) -> Result<SoulousFact> {
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::Soulous("/api/checkin 响应不是对象".into()))?;
    let checked = object
        .get("checkedInToday")
        .and_then(Value::as_bool)
        .ok_or_else(|| CoreError::Soulous("/api/checkin 缺少 checkedInToday".into()))?;
    let streak = object.get("streak").and_then(Value::as_i64).unwrap_or(0);
    let mut payload: Map<String, Value> = object.clone();
    payload.insert("snapshotDate".into(), Value::String(now.date().to_string()));
    let title = if checked {
        format!("今日已打卡（连续 {streak} 天）")
    } else {
        format!("今日未打卡（当前连续 {streak} 天）")
    };
    fact(
        SoulousKind::Checkin,
        now.date().to_string(),
        title,
        now.date().and_hms_opt(0, 0, 0),
        None,
        &Value::Object(payload),
        now,
    )
}

fn parse_focus(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
    expect_array(value, "/api/focus/sessions")?
        .iter()
        .map(|session| {
            let id = id_field(session, SoulousKind::Focus)?;
            let title = optional_string(session, "title").unwrap_or_else(|| "未命名专注".into());
            let start = optional_string(session, "startedAt")
                .as_deref()
                .and_then(parse_remote_time)
                .or_else(|| {
                    optional_string(session, "createdAt")
                        .as_deref()
                        .and_then(parse_remote_time)
                });
            let end = optional_string(session, "endedAt")
                .as_deref()
                .and_then(parse_remote_time);
            fact(SoulousKind::Focus, id, title, start, end, session, now)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use chrono::NaiveDate;

    use super::*;

    fn dt(day: u32, hour: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap()
    }

    fn config() -> SoulousConfig {
        SoulousConfig {
            server_url: "https://soulous.test".into(),
            access_token: "access-old".into(),
            refresh_token: "refresh-old".into(),
            timeout_secs: 15,
            push_schedule_events: false,
        }
    }

    struct FakeHttp {
        refresh_first: bool,
        fail_path: Option<&'static str>,
        calls: RefCell<Vec<(String, String)>>,
        authorized_bodies: RefCell<Vec<Value>>,
    }

    impl FakeHttp {
        fn normal() -> Self {
            Self {
                refresh_first: false,
                fail_path: None,
                calls: RefCell::new(Vec::new()),
                authorized_bodies: RefCell::new(Vec::new()),
            }
        }
    }

    impl SoulousHttp for FakeHttp {
        fn get_json(
            &self,
            url: &str,
            access: &str,
        ) -> std::result::Result<Value, SoulousHttpError> {
            self.calls.borrow_mut().push((url.into(), access.into()));
            if self.refresh_first && access == "access-old" {
                return Err(SoulousHttpError::Unauthorized);
            }
            if self.fail_path.is_some_and(|path| url.ends_with(path)) {
                return Err(SoulousHttpError::Other("offline".into()));
            }
            let body = if url.ends_with("/api/timetable/exams") {
                json!([{ "id": 11, "courseName": "算法", "examTime": "2026-07-20T09:00:00", "room": "A101" }])
            } else if url.ends_with("/api/timetable") {
                json!([{ "id": 10, "courseName": "算法", "dayOfWeek": 1, "startTime": "08:00", "endTime": "09:40" }])
            } else if url.ends_with("/api/tasks") {
                json!([{ "id": 12, "title": "完成算法作业", "deadline": "2026-07-19", "status": "TODO" }])
            } else if url.ends_with("/api/checkin") {
                json!({ "checkedInToday": true, "streak": 6, "balance": 80 })
            } else if url.ends_with("/api/focus/sessions") {
                json!([{ "id": 13, "title": "算法复习", "elapsedSeconds": 5400, "status": "DONE", "startedAt": "2026-07-17T19:00:00", "endedAt": "2026-07-17T20:30:00" }])
            } else {
                return Err(SoulousHttpError::Other(format!("unexpected URL {url}")));
            };
            Ok(body)
        }

        fn post_json(
            &self,
            url: &str,
            body: Value,
        ) -> std::result::Result<Value, SoulousHttpError> {
            assert!(url.ends_with("/api/auth/mobile/refresh"));
            assert_eq!(body["refreshToken"], "refresh-old");
            Ok(json!({ "accessToken": "access-new", "refreshToken": "refresh-new" }))
        }

        fn post_json_authorized(
            &self,
            url: &str,
            access: &str,
            body: Value,
        ) -> std::result::Result<Value, SoulousHttpError> {
            self.calls.borrow_mut().push((url.into(), access.into()));
            self.authorized_bodies.borrow_mut().push(body);
            if self.refresh_first && access == "access-old" {
                return Err(SoulousHttpError::Unauthorized);
            }
            if !url.ends_with("/api/external-context") {
                return Err(SoulousHttpError::Other(format!("unexpected URL {url}")));
            }
            Ok(json!({ "ok": true }))
        }
    }

    #[test]
    fn pull_refreshes_dual_tokens_and_keeps_data_out_of_memory_and_recall() {
        let store = Store::open_in_memory().unwrap();
        let mut cfg = config();
        let http = FakeHttp {
            refresh_first: true,
            fail_path: None,
            calls: RefCell::new(Vec::new()),
            authorized_bodies: RefCell::new(Vec::new()),
        };
        let now = dt(18, 10);
        let outcome = pull_with_client(&store, &mut cfg, &http, now).unwrap();
        assert!(outcome.refreshed_tokens);
        assert_eq!(cfg.access_token, "access-new");
        assert_eq!(cfg.refresh_token, "refresh-new");
        assert_eq!(
            outcome.courses
                + outcome.exams
                + outcome.tasks
                + outcome.checkins
                + outcome.focus_sessions,
            5
        );

        let facts = store.list_soulous_facts().unwrap();
        assert_eq!(facts.len(), 5);
        assert!(facts.iter().all(|f| f.source == SOURCE));
        assert!(store.list_facts().unwrap().is_empty());
        assert!(store.list_recall_events(true).unwrap().is_empty());

        let status = build_status(&facts, &RuleTable::default(), now);
        assert_eq!(status.upcoming_exams.len(), 1);
        assert_eq!(status.upcoming_exams[0].rule.lead_times[0].label, "3d");
        let suggestions = generate_suggestions(&facts, now, 3);
        assert_eq!(suggestions.len(), 2);
        assert!(suggestions
            .iter()
            .all(|s| s.source.as_deref().unwrap().starts_with("soulous:")));

        let material = review_material(&facts, dt(17, 0), dt(18, 23));
        assert_eq!(material.open_tasks, 1);
        assert_eq!(material.checkin_days, 1);
        assert_eq!(material.focus_minutes, 90);
    }

    #[test]
    fn failed_pull_preserves_the_last_complete_snapshot() {
        let store = Store::open_in_memory().unwrap();
        let now = dt(18, 10);
        let mut cfg = config();
        pull_with_client(&store, &mut cfg, &FakeHttp::normal(), now).unwrap();
        let before = store.list_soulous_facts().unwrap();

        let mut failed_cfg = config();
        let offline = FakeHttp {
            refresh_first: false,
            fail_path: Some("/api/focus/sessions"),
            calls: RefCell::new(Vec::new()),
            authorized_bodies: RefCell::new(Vec::new()),
        };
        assert!(pull_with_client(&store, &mut failed_cfg, &offline, dt(19, 10)).is_err());
        assert_eq!(store.list_soulous_facts().unwrap(), before);
    }

    #[test]
    fn remote_time_accepts_java_fractional_seconds() {
        // 生产 FocusSession 的真实形态：LocalDateTime 带不定宽小数秒。
        // 首次真机拉取（2026-07-18）因缺这个分支导致 14 段专注全部丢时间。
        let parsed = parse_remote_time("2026-06-01T20:59:53.63512").unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 6, 1)
            .unwrap()
            .and_hms_micro_opt(20, 59, 53, 635_120)
            .unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(parse_remote_time("2026-07-20T09:00:00").unwrap(), dt(20, 9));
    }

    #[test]
    fn malformed_config_stays_silent_off() {
        assert!(SoulousConfig::from_json("{\"server_url\":\"not-a-url\"}").is_err());
        assert_eq!(token_tail("secret-9876"), "…9876");
    }

    #[test]
    fn confirmed_schedule_push_sends_only_the_whitelisted_projection_and_refreshes() {
        let store = Store::open_in_memory().unwrap();
        let mut event = crate::model::Event::new(
            "毕业设计答辩",
            crate::model::EventKind::Meeting,
            dt(20, 9),
            "明天和导师在 A101 答辩",
            dt(18, 10),
        );
        event.end = Some(dt(20, 10));
        event.location = Some("A101".into());
        event.people = vec!["导师".into()];
        let event_id = store.insert_event(&event, None).unwrap();

        let mut cfg = config();
        cfg.push_schedule_events = true;
        let http = FakeHttp {
            refresh_first: true,
            fail_path: None,
            calls: RefCell::new(Vec::new()),
            authorized_bodies: RefCell::new(Vec::new()),
        };
        let outcome = push_event_with_client(&store, &mut cfg, &http, event_id).unwrap();

        assert_eq!(outcome.title, "毕业设计答辩");
        assert!(outcome.refreshed_tokens);
        assert_eq!(cfg.access_token, "access-new");
        let bodies = http.authorized_bodies.borrow();
        assert_eq!(bodies.len(), 2, "401 retry keeps the same idempotent body");
        let body = &bodies[0];
        assert_eq!(body["source"], SOLUM_SOURCE);
        assert_eq!(body["type"], SCHEDULE_EVENT_TYPE);
        assert_eq!(body["payload"]["title"], "毕业设计答辩");
        assert_eq!(body["payload"]["kind"], "meeting");
        assert!(body["payload"].get("people").is_none());
        assert!(body["payload"].get("raw_input").is_none());
        assert!(body["payload"].get("notification_text").is_none());
    }

    #[test]
    fn notification_derived_event_is_refused_even_when_the_category_is_enabled() {
        let store = Store::open_in_memory().unwrap();
        let event = crate::model::Event::new(
            "通知里的会议",
            crate::model::EventKind::Meeting,
            dt(20, 9),
            "第三方通知原文",
            dt(18, 10),
        );
        let event_id = store
            .insert_event_with_scope(&event, None, true, None)
            .unwrap();
        let mut cfg = config();
        cfg.push_schedule_events = true;
        let err =
            push_event_with_client(&store, &mut cfg, &FakeHttp::normal(), event_id).unwrap_err();
        assert!(err.to_string().contains("第三方通知"));
    }
}

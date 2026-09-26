//! `http` -- the HTTP seam: the trait that keeps parsing and offline-safety tests deterministic, the ureq implementation behind it, and the configured pull/push entry points.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::parse::{
    parse_checkin, parse_courses, parse_exams, parse_focus, parse_tasks, string_field,
};
use super::push::outbound_event_request;
use super::*;

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

pub(super) fn map_http_error(error: ureq::Error) -> SoulousHttpError {
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

pub(super) fn fetch<T: SoulousHttp>(
    client: &T,
    config: &mut SoulousConfig,
    path: &str,
) -> Result<Value> {
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

pub(super) fn post_with_refresh<T: SoulousHttp>(
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

//! `parse` -- turning Soulous's JSON into facts: the field readers, the time and deadline parsing, and one parser per endpoint.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) fn expect_array<'a>(value: &'a Value, endpoint: &str) -> Result<&'a [Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| CoreError::Soulous(format!("{endpoint} 响应不是数组")))
}

pub(super) fn string_field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::Soulous(format!("Soulous 响应缺少 {key}")))
}

pub(super) fn id_field(value: &Value, kind: SoulousKind) -> Result<String> {
    match value.get("id") {
        Some(Value::String(id)) if !id.trim().is_empty() => Ok(id.trim().to_string()),
        Some(Value::Number(id)) => Ok(id.to_string()),
        _ => Err(CoreError::Soulous(format!(
            "{} 条目缺少 id，拒绝覆盖本地缓存",
            kind.label()
        ))),
    }
}

pub(super) fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(super) fn parse_remote_time(raw: &str) -> Option<NaiveDateTime> {
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

pub(super) fn parse_deadline(raw: &str) -> Option<NaiveDateTime> {
    parse_remote_time(raw).or_else(|| {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(23, 59, 0))
    })
}

pub(super) fn fact(
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

pub(super) fn parse_courses(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
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

pub(super) fn parse_exams(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
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

pub(super) fn parse_tasks(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
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

pub(super) fn parse_checkin(value: &Value, now: NaiveDateTime) -> Result<SoulousFact> {
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

pub(super) fn parse_focus(value: &Value, now: NaiveDateTime) -> Result<Vec<SoulousFact>> {
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

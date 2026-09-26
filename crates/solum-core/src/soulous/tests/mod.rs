//! `tests` -- the suite for all of the above.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use std::cell::RefCell;

use chrono::NaiveDate;

use super::http::{pull_with_client, push_event_with_client};
use super::parse::parse_remote_time;
use super::push::token_tail;
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
    fn get_json(&self, url: &str, access: &str) -> std::result::Result<Value, SoulousHttpError> {
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

    fn post_json(&self, url: &str, body: Value) -> std::result::Result<Value, SoulousHttpError> {
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
        outcome.courses + outcome.exams + outcome.tasks + outcome.checkins + outcome.focus_sessions,
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
    let err = push_event_with_client(&store, &mut cfg, &FakeHttp::normal(), event_id).unwrap_err();
    assert!(err.to_string().contains("第三方通知"));
}

//! `tests` -- the routing, reschedule, cancel and extraction tests — the cases where a rule order change would silently move a line between intents.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

use chrono::NaiveDate;

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, 6)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

#[test]
fn routes_dangerous() {
    assert_eq!(
        route_intent("帮我删除今天的所有文件", now()),
        Intent::DangerousCommand
    );
    assert_eq!(
        route_intent("transfer 500 to Alice", now()),
        Intent::DangerousCommand
    );
    // ARCHITECTURE §6 D3 的对话入口示例措辞必须能入高危路由。
    assert_eq!(
        route_intent("把上周的行为日志清掉", now()),
        Intent::DangerousCommand
    );
}

#[test]
fn routes_event() {
    assert_eq!(route_intent("明天下午3点开会", now()), Intent::IngestEvent);
    assert_eq!(route_intent("下周五要考试", now()), Intent::IngestEvent);
}

#[test]
fn routes_explicit_widget_before_event_ingest() {
    assert_eq!(
        route_intent("帮我弄一个日程表组件", now()),
        Intent::CreateWidget
    );
}

#[test]
fn routes_chat() {
    assert_eq!(route_intent("今天天气怎么样", now()), Intent::Chat);
}

#[test]
fn routes_reschedule_before_event_ingest() {
    // Carries both an event word and a time — must NOT become a new event.
    assert_eq!(
        route_intent("把明天的会改到下午4点", now()),
        Intent::RescheduleEvent
    );
    assert_eq!(
        route_intent("期末考试推迟到下周五", now()),
        Intent::RescheduleEvent
    );
    // A marker without a resolvable new time stays chat.
    assert_eq!(route_intent("把会改一下吧", now()), Intent::Chat);
}

#[test]
fn routes_cancel() {
    assert_eq!(route_intent("取消明天的会", now()), Intent::CancelEvent);
    assert_eq!(route_intent("明天的会不开了", now()), Intent::CancelEvent);
    // Danger words still win: "删掉" routes to the guard, not to cancel.
    assert_eq!(
        route_intent("把明天的会删掉", now()),
        Intent::DangerousCommand
    );
}

#[test]
fn parse_reschedule_splits_target_and_halves() {
    // Time only → date half empty (keeps the event's own date).
    let r = parse_reschedule("把明天的会改到下午4点", now()).unwrap();
    assert_eq!(r.target, "明天的会");
    assert_eq!(r.new_date, None);
    assert_eq!(r.new_time, Some((16, 0)));
    // Date only → time half empty (keeps the event's own clock time).
    let r = parse_reschedule("期末考试推迟到下周五", now()).unwrap();
    assert_eq!(r.target, "期末考试");
    assert_eq!(r.new_date, NaiveDate::from_ymd_opt(2026, 7, 17));
    assert_eq!(r.new_time, None);
    // Both halves present.
    let r = parse_reschedule("将周会挪到明天上午十点", now()).unwrap();
    assert_eq!(r.target, "周会");
    assert_eq!(r.new_date, NaiveDate::from_ymd_opt(2026, 7, 7));
    assert_eq!(r.new_time, Some((10, 0)));
}

#[test]
fn parse_cancel_extracts_target() {
    assert_eq!(parse_cancel("取消明天的会").as_deref(), Some("明天的会"));
    assert_eq!(parse_cancel("明天的会不开了").as_deref(), Some("明天的会"));
    assert_eq!(parse_cancel("今天天气怎么样"), None);
}

#[test]
fn routes_english_deadline_without_clock_time() {
    // Date-only (no explicit time) but an English event keyword → event.
    assert_eq!(
        route_intent("in 3 days submit the report", now()),
        Intent::IngestEvent
    );
    let ex = RuleBasedExtractor::new();
    let ev = ex
        .extract("in 3 days submit the report", now())
        .unwrap()
        .unwrap();
    assert_eq!(ev.kind, EventKind::Deadline);
    assert_eq!(
        ev.start.date(),
        chrono::NaiveDate::from_ymd_opt(2026, 7, 9).unwrap()
    );
}

#[test]
fn routes_status() {
    assert_eq!(route_intent("我在护肤", now()), Intent::StatusAnswer);
}

#[test]
fn extracts_meeting() {
    let ex = RuleBasedExtractor::new();
    let ev = ex
        .extract("明天下午3点在会议室和张伟开会", now())
        .unwrap()
        .unwrap();
    assert_eq!(ev.kind, EventKind::Meeting);
    assert_eq!(
        ev.start,
        NaiveDate::from_ymd_opt(2026, 7, 7)
            .unwrap()
            .and_hms_opt(15, 0, 0)
            .unwrap()
    );
    assert_eq!(ev.location.as_deref(), Some("会议室"));
    assert!(ev.people.contains(&"张伟".to_string()));
    assert!(!ev.title.contains("下午"));
    assert!(!ev.title.contains("3点"));
    // Location/people are stripped from the title (they have their own fields).
    assert_eq!(ev.title, "开会");
}

#[test]
fn extracts_exam_kind() {
    let ex = RuleBasedExtractor::new();
    let ev = ex
        .extract("下周五上午九点期末考试", now())
        .unwrap()
        .unwrap();
    assert_eq!(ev.kind, EventKind::Exam);
}

#[test]
fn non_event_returns_none() {
    let ex = RuleBasedExtractor::new();
    assert!(ex.extract("随便聊聊", now()).unwrap().is_none());
}

#[test]
fn reminder_title_strips_filler() {
    let ex = RuleBasedExtractor::new();
    let ev = ex
        .extract("提醒我明天上午十点交作业", now())
        .unwrap()
        .unwrap();
    // "交作业" is a deadline-ish reminder; title should not carry the time.
    assert!(!ev.title.contains("十点"));
    assert!(ev.title.contains("作业"));
}

//! Notification scheduling: turn an event + its importance rule into concrete
//! fire times, and answer "what is due now?".
//!
//! This is pure arithmetic on wall-clock times — no async, no timers. The
//! caller (orchestrator / device runtime) is responsible for actually waking up
//! at the right moment; keeping the math pure is what lets reminders be
//! reliable and testable (see the "通知可靠性" NFR).

use chrono::{Duration, NaiveDateTime};

use crate::classify::ImportanceRule;
use crate::model::{Event, Notification, NotificationStatus};

/// Plan the notifications for `event` under `rule`. One notification per lead
/// time; `fire_at = event.start - lead`.
pub fn plan_notifications(
    event: &Event,
    rule: &ImportanceRule,
    created_at: NaiveDateTime,
    event_id: i64,
) -> Vec<Notification> {
    rule.lead_times
        .iter()
        .map(|lead| Notification {
            id: None,
            event_id,
            fire_at: event.start - Duration::minutes(lead.minutes),
            lead_label: lead.label.clone(),
            channels: rule.channels.clone(),
            status: NotificationStatus::Pending,
            created_at,
            fired_at: None,
        })
        .collect()
}

/// Pending notifications whose fire time has arrived (`fire_at <= now`),
/// earliest first.
pub fn due(notifs: &[Notification], now: NaiveDateTime) -> Vec<&Notification> {
    let mut out: Vec<&Notification> = notifs
        .iter()
        .filter(|n| n.status == NotificationStatus::Pending && n.fire_at <= now)
        .collect();
    out.sort_by_key(|n| n.fire_at);
    out
}

/// The soonest upcoming (still-pending, not-yet-due) fire time, if any.
pub fn next_fire(notifs: &[Notification], now: NaiveDateTime) -> Option<NaiveDateTime> {
    notifs
        .iter()
        .filter(|n| n.status == NotificationStatus::Pending && n.fire_at > now)
        .map(|n| n.fire_at)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::RuleTable;
    use crate::model::EventKind;
    use chrono::NaiveDate;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn exam_event() -> Event {
        Event::new(
            "期末考试",
            EventKind::Exam,
            dt(2026, 7, 20, 9, 0),
            "7月20号上午九点期末考试",
            dt(2026, 7, 6, 10, 0),
        )
    }

    #[test]
    fn exam_fires_three_days_ahead() {
        let table = RuleTable::default_table();
        let ev = exam_event();
        let notifs = plan_notifications(&ev, &table.rule(ev.kind), dt(2026, 7, 6, 10, 0), 1);
        assert_eq!(notifs.len(), 1);
        // 3 days before 7-20 09:00 = 7-17 09:00.
        assert_eq!(notifs[0].fire_at, dt(2026, 7, 17, 9, 0));
        assert_eq!(notifs[0].lead_label, "3d");
    }

    #[test]
    fn due_filters_and_sorts() {
        let table = RuleTable::default_table();
        let ev = exam_event();
        let notifs = plan_notifications(&ev, &table.rule(ev.kind), dt(2026, 7, 6, 10, 0), 1);
        // Before fire time: nothing due.
        assert!(due(&notifs, dt(2026, 7, 17, 8, 59)).is_empty());
        // At/after fire time: due.
        assert_eq!(due(&notifs, dt(2026, 7, 17, 9, 0)).len(), 1);
    }

    #[test]
    fn next_fire_reports_upcoming() {
        let table = RuleTable::default_table();
        let ev = exam_event();
        let notifs = plan_notifications(&ev, &table.rule(ev.kind), dt(2026, 7, 6, 10, 0), 1);
        assert_eq!(
            next_fire(&notifs, dt(2026, 7, 6, 10, 0)),
            Some(dt(2026, 7, 17, 9, 0))
        );
        assert_eq!(next_fire(&notifs, dt(2026, 7, 18, 0, 0)), None);
    }

    #[test]
    fn layered_leads_produce_multiple() {
        let mut table = RuleTable::default_table();
        let mut rule = table.rule(EventKind::Exam);
        rule.lead_times
            .push(crate::classify::LeadTime::parse("1h").unwrap());
        table.set(rule);
        let ev = exam_event();
        let notifs = plan_notifications(&ev, &table.rule(ev.kind), dt(2026, 7, 6, 10, 0), 1);
        assert_eq!(notifs.len(), 2);
        // One at 3d ahead, one at 1h ahead (7-20 08:00).
        assert!(notifs.iter().any(|n| n.fire_at == dt(2026, 7, 17, 9, 0)));
        assert!(notifs.iter().any(|n| n.fire_at == dt(2026, 7, 20, 8, 0)));
    }
}

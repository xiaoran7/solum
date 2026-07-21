//! Daily Focus Brief: a read-only, deterministic aggregation of today's
//! schedule, reminders, and pending suggestions.
//!
//! Like [`crate::review`], this module only windows records already fetched
//! from storage. Keeping that policy here makes every entry point agree and
//! leaves the result straightforward to test without I/O.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::model::{Event, Notification, NotificationStatus};
use crate::suggest::{Suggestion, SuggestionStatus};

const UPCOMING_REMINDER_LIMIT: usize = 3;
const TOP_SUGGESTION_LIMIT: usize = 3;

/// A prioritized, read-only summary of what needs attention today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    pub date: NaiveDate,
    pub events_today: Vec<Event>,
    pub due_reminders: Vec<Notification>,
    /// The next few pending reminders that have not reached their fire time.
    pub upcoming_reminders: Vec<Notification>,
    /// The first few pending suggestions in the store's priority order.
    pub top_suggestions: Vec<Suggestion>,
}

/// Build a brief from already-fetched records. Events are limited to `now`'s
/// calendar date; only pending notifications are actionable. The caller owns
/// fetching and ordering, just as it does for [`crate::review::build_digest`].
pub fn build_brief(
    now: NaiveDateTime,
    events: &[Event],
    notifications: &[Notification],
    suggestions: &[Suggestion],
) -> Brief {
    let events_today = events
        .iter()
        .filter(|event| event.start.date() == now.date())
        .cloned()
        .collect();
    let due_reminders = notifications
        .iter()
        .filter(|notification| notification.status == NotificationStatus::Pending)
        .filter(|notification| notification.fire_at <= now)
        .cloned()
        .collect();
    let upcoming_reminders = notifications
        .iter()
        .filter(|notification| notification.status == NotificationStatus::Pending)
        .filter(|notification| notification.fire_at > now)
        .take(UPCOMING_REMINDER_LIMIT)
        .cloned()
        .collect();
    let top_suggestions = suggestions
        .iter()
        .filter(|suggestion| suggestion.status == SuggestionStatus::Pending)
        .take(TOP_SUGGESTION_LIMIT)
        .cloned()
        .collect();

    Brief {
        date: now.date(),
        events_today,
        due_reminders,
        upcoming_reminders,
        top_suggestions,
    }
}

impl Brief {
    /// A Chinese-language, human-readable summary for the CLI and text-only
    /// surfaces. The card UI reuses the structured fields instead.
    pub fn render(&self) -> String {
        let mut lines = vec![format!("🧭 今日聚焦（{}）", self.date)];
        push_section(
            &mut lines,
            "今日日程",
            self.events_today.iter().map(|event| {
                format!(
                    "{} {}（{}）",
                    event.start.format("%H:%M"),
                    event.title,
                    event.kind.as_str()
                )
            }),
        );
        push_section(
            &mut lines,
            "到点提醒",
            self.due_reminders.iter().map(reminder_line),
        );
        push_section(
            &mut lines,
            "即将提醒",
            self.upcoming_reminders.iter().map(reminder_line),
        );
        push_section(
            &mut lines,
            "待处理建议",
            self.top_suggestions
                .iter()
                .map(|suggestion| suggestion.text.clone()),
        );
        lines.join("\n")
    }
}

fn reminder_line(notification: &Notification) -> String {
    format!(
        "{} event#{}（提前{}）",
        notification.fire_at.format("%H:%M"),
        notification.event_id,
        notification.lead_label
    )
}

fn push_section<I>(lines: &mut Vec<String>, title: &str, entries: I)
where
    I: IntoIterator<Item = String>,
{
    let entries: Vec<_> = entries.into_iter().collect();
    if entries.is_empty() {
        lines.push(format!("- {title}：无"));
        return;
    }
    lines.push(format!("- {title}（{}）：", entries.len()));
    lines.extend(entries.into_iter().map(|entry| format!("  - {entry}")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, EventKind};
    use crate::suggest::SuggestionKind;
    use chrono::NaiveDate;

    fn dt(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    fn event(title: &str, start: NaiveDateTime) -> Event {
        Event::new(title, EventKind::Meeting, start, "raw", dt(14, 9, 0))
    }

    fn notification(
        id: i64,
        event_id: i64,
        fire_at: NaiveDateTime,
        status: NotificationStatus,
    ) -> Notification {
        Notification {
            id: Some(id),
            event_id,
            fire_at,
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status,
            created_at: dt(14, 9, 0),
            fired_at: None,
        }
    }

    fn suggestion(id: i64, status: SuggestionStatus) -> Suggestion {
        Suggestion {
            id: Some(id),
            created_at: dt(14, 9, 0),
            kind: SuggestionKind::ExamPrep,
            text: format!("建议{id}"),
            dedup_key: format!("suggestion:{id}"),
            source: None,
            status,
        }
    }

    #[test]
    fn builds_today_due_upcoming_and_pending_windows() {
        let now = dt(15, 10, 0);
        let brief = build_brief(
            now,
            &[
                event("晨会", dt(15, 9, 0)),
                event("明日会", dt(16, 9, 0)),
                event("昨日会", dt(14, 9, 0)),
            ],
            &[
                notification(1, 11, dt(15, 9, 30), NotificationStatus::Pending),
                notification(2, 12, dt(15, 11, 0), NotificationStatus::Pending),
                notification(3, 13, dt(15, 12, 0), NotificationStatus::Fired),
                notification(4, 14, dt(15, 13, 0), NotificationStatus::Pending),
                notification(5, 15, dt(15, 14, 0), NotificationStatus::Pending),
                notification(6, 16, dt(15, 15, 0), NotificationStatus::Pending),
            ],
            &[
                suggestion(1, SuggestionStatus::Pending),
                suggestion(2, SuggestionStatus::Accepted),
                suggestion(3, SuggestionStatus::Pending),
                suggestion(4, SuggestionStatus::Pending),
                suggestion(5, SuggestionStatus::Pending),
            ],
        );

        assert_eq!(brief.date, now.date());
        assert_eq!(brief.events_today.len(), 1);
        assert_eq!(brief.events_today[0].title, "晨会");
        assert_eq!(
            brief.due_reminders.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![Some(1)]
        );
        assert_eq!(
            brief
                .upcoming_reminders
                .iter()
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            vec![Some(2), Some(4), Some(5)]
        );
        assert_eq!(
            brief
                .top_suggestions
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(3), Some(4)]
        );
    }

    #[test]
    fn render_is_readable_for_a_quiet_day() {
        let brief = build_brief(dt(15, 10, 0), &[], &[], &[]);
        let text = brief.render();
        assert!(text.contains("今日聚焦（2026-07-15）"));
        assert!(text.contains("今日日程：无"));
        assert!(text.contains("待处理建议：无"));
    }
}

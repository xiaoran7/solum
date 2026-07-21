//! Self-review digest (F14 周期性自我复盘简报, drawing on the F4 behavior log).
//!
//! Pure aggregation over what the store already holds — no cloud, no LLM — so
//! the "我为你做了什么/观察到什么" summary is deterministic and testable. The
//! cloud reasoner can later *rewrite* this digest in the user's persona voice,
//! but the numbers come from here.

use chrono::NaiveDateTime;

use crate::journal::{BehaviorEntry, BehaviorKind};
use crate::memory::MemoryFact;
use crate::model::{fmt_ts, Event, EventKind, Notification};
use crate::soulous::SoulousFact;
use crate::store::AuditRow;
use crate::suggest::{Suggestion, SuggestionKind};

/// A summary of the agent's activity over a time window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    pub from: NaiveDateTime,
    pub to: NaiveDateTime,
    pub raw_inputs: usize,
    pub events_total: usize,
    /// Non-zero event counts, in canonical kind order.
    pub events_by_kind: Vec<(EventKind, usize)>,
    pub notifications_planned: usize,
    pub notifications_fired: usize,
    pub dangerous_attempts: usize,
    pub dangerous_refused: usize,
    /// 观察（D6）：recurring status activities in the window, most frequent
    /// first — offline analysis, rendered locally, never rewritten by the cloud.
    pub top_activities: Vec<(String, usize)>,
    /// Wellness (F11) signals raised in the window.
    pub wellness_count: usize,
    /// 「本周我记住了什么」：facts written in the window. Never sent upstream.
    pub new_facts: Vec<String>,
    /// Soulous is a separate, read-only fact source. This is rendered locally
    /// in the F14 extras and never becomes semantic memory or LLM context.
    pub soulous: crate::soulous::ReviewMaterial,
}

fn in_window(t: NaiveDateTime, from: NaiveDateTime, to: NaiveDateTime) -> bool {
    t >= from && t <= to
}

/// Build a digest from already-fetched records. `raw_inputs` is the count of
/// raw utterances in the window (the store counts them cheaply).
///
/// Nine read-only record slices is the honest shape of "aggregate everything
/// the store holds" — bundling them into a struct would only rename the
/// problem, so the lint is waived deliberately.
#[allow(clippy::too_many_arguments)]
pub fn build_digest(
    from: NaiveDateTime,
    to: NaiveDateTime,
    raw_inputs: usize,
    events: &[Event],
    notifications: &[Notification],
    audit: &[AuditRow],
    behaviors: &[BehaviorEntry],
    suggestions: &[Suggestion],
    facts: &[MemoryFact],
    soulous_facts: &[SoulousFact],
) -> Digest {
    let windowed_events: Vec<&Event> = events
        .iter()
        .filter(|e| in_window(e.created_at, from, to))
        .collect();

    let mut events_by_kind = Vec::new();
    for kind in EventKind::all() {
        let n = windowed_events.iter().filter(|e| e.kind == kind).count();
        if n > 0 {
            events_by_kind.push((kind, n));
        }
    }

    let notifications_planned = notifications
        .iter()
        .filter(|n| in_window(n.created_at, from, to))
        .count();
    let notifications_fired = notifications
        .iter()
        .filter(|n| n.fired_at.is_some_and(|f| in_window(f, from, to)))
        .count();

    let dangerous: Vec<&AuditRow> = audit
        .iter()
        .filter(|a| a.risk == "dangerous")
        .filter(|a| {
            crate::model::parse_ts(&a.ts)
                .map(|t| in_window(t, from, to))
                .unwrap_or(false)
        })
        .collect();
    let dangerous_attempts = dangerous.len();
    let dangerous_refused = dangerous.iter().filter(|a| a.decision == "refused").count();

    // 观察: recurring status activities in the window.
    use std::collections::BTreeMap;
    let mut activity_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for b in behaviors
        .iter()
        .filter(|b| b.kind == BehaviorKind::Status && in_window(b.ts, from, to))
    {
        *activity_counts.entry(b.content.as_str()).or_default() += 1;
    }
    let mut top_activities: Vec<(String, usize)> = activity_counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(c, n)| (c.to_string(), n))
        .collect();
    top_activities.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top_activities.truncate(3);

    let wellness_count = suggestions
        .iter()
        .filter(|s| s.kind == SuggestionKind::Wellness && in_window(s.created_at, from, to))
        .count();

    let new_facts: Vec<String> = facts
        .iter()
        .filter(|f| in_window(f.created_at, from, to))
        .map(|f| f.content.clone())
        .collect();
    let soulous = crate::soulous::review_material(soulous_facts, from, to);

    Digest {
        from,
        to,
        raw_inputs,
        events_total: windowed_events.len(),
        events_by_kind,
        notifications_planned,
        notifications_fired,
        dangerous_attempts,
        dangerous_refused,
        top_activities,
        wellness_count,
        new_facts,
        soulous,
    }
}

impl Digest {
    /// The numeric core — the only part the cloud may rewrite (persona voice,
    /// fact-checked by `llm::digest_counts_preserved`).
    pub fn render_core(&self) -> String {
        let by_kind = if self.events_by_kind.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = self
                .events_by_kind
                .iter()
                .map(|(k, n)| format!("{}{}", kind_label(*k), n))
                .collect();
            format!("（{}）", parts.join("、"))
        };
        format!(
            "复盘简报（{} ~ {}）\n\
             - 记录输入 {} 条，新建日程 {} 项{}\n\
             - 计划提醒 {} 次，已触发 {} 次\n\
             - 高危尝试 {} 次，其中被护栏拦截 {} 次",
            fmt_ts(&self.from),
            fmt_ts(&self.to),
            self.raw_inputs,
            self.events_total,
            by_kind,
            self.notifications_planned,
            self.notifications_fired,
            self.dangerous_attempts,
            self.dangerous_refused,
        )
    }

    /// 观察 + 记忆两段（D6）。Rendered locally and appended verbatim to both
    /// the offline and cloud-rewritten outputs — fact contents never travel
    /// upstream. Empty when the window produced nothing to observe.
    pub fn render_extras(&self) -> String {
        let mut s = String::new();
        if !self.top_activities.is_empty() || self.wellness_count > 0 {
            s.push_str("\n【观察】");
            for (content, n) in &self.top_activities {
                s.push_str(&format!("\n- 「{content}」出现 {n} 次"));
            }
            if self.wellness_count > 0 {
                s.push_str(&format!("\n- 触发身体状态提示 {} 次", self.wellness_count));
            }
        }
        if !self.new_facts.is_empty() {
            s.push_str("\n【本周我记住了什么】");
            for f in &self.new_facts {
                s.push_str(&format!("\n- {f}"));
            }
        }
        if self.soulous.courses > 0
            || self.soulous.exams > 0
            || self.soulous.open_tasks > 0
            || self.soulous.checkin_days > 0
            || self.soulous.focus_minutes > 0
        {
            s.push_str("\n【Soulous 学习数据】");
            s.push_str(&format!(
                "\n- 课表 {} 条、考试 {} 场、待办任务 {} 项",
                self.soulous.courses, self.soulous.exams, self.soulous.open_tasks
            ));
            if self.soulous.checkin_days > 0 {
                s.push_str(&format!("\n- 已打卡 {} 天", self.soulous.checkin_days));
            }
            if self.soulous.focus_minutes > 0 {
                s.push_str(&format!("\n- 专注 {} 分钟", self.soulous.focus_minutes));
            }
        }
        s
    }

    /// The full readable Chinese summary: core + extras.
    pub fn render(&self) -> String {
        format!("{}{}", self.render_core(), self.render_extras())
    }
}

fn kind_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Exam => "考试",
        EventKind::Meeting => "会议",
        EventKind::Class => "课程",
        EventKind::Deadline => "截止",
        EventKind::Reminder => "提醒",
        EventKind::Other => "事件",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, NotificationStatus};
    use chrono::NaiveDate;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn ev(kind: EventKind, created: NaiveDateTime) -> Event {
        Event::new("t", kind, dt(2026, 7, 20, 9, 0), "raw", created)
    }

    fn notif(created: NaiveDateTime, fired: Option<NaiveDateTime>) -> Notification {
        Notification {
            id: Some(1),
            event_id: 1,
            fire_at: dt(2026, 7, 7, 9, 0),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: if fired.is_some() {
                NotificationStatus::Fired
            } else {
                NotificationStatus::Pending
            },
            created_at: created,
            fired_at: fired,
        }
    }

    fn audit(ts: NaiveDateTime, decision: &str) -> AuditRow {
        AuditRow {
            id: 1,
            ts: fmt_ts(&ts),
            tool: "demo_delete".into(),
            risk: "dangerous".into(),
            summary: "demo_delete(/x)".into(),
            decision: decision.into(),
            token_id: None,
            detail: String::new(),
        }
    }

    #[test]
    fn aggregates_window() {
        let from = dt(2026, 7, 6, 0, 0);
        let to = dt(2026, 7, 12, 23, 59);
        let events = vec![
            ev(EventKind::Meeting, dt(2026, 7, 6, 10, 0)),
            ev(EventKind::Meeting, dt(2026, 7, 7, 10, 0)),
            ev(EventKind::Exam, dt(2026, 7, 8, 10, 0)),
            ev(EventKind::Class, dt(2026, 6, 30, 10, 0)), // outside window
        ];
        let notifs = vec![
            notif(dt(2026, 7, 6, 10, 0), Some(dt(2026, 7, 7, 9, 0))),
            notif(dt(2026, 7, 6, 10, 0), None),
        ];
        let audits = vec![
            audit(dt(2026, 7, 6, 10, 0), "refused"),
            audit(dt(2026, 7, 6, 10, 1), "executed"),
        ];
        let d = build_digest(from, to, 5, &events, &notifs, &audits, &[], &[], &[], &[]);
        assert_eq!(d.events_total, 3); // class is outside the window
        assert_eq!(
            d.events_by_kind,
            vec![(EventKind::Exam, 1), (EventKind::Meeting, 2)]
        );
        assert_eq!(d.notifications_planned, 2);
        assert_eq!(d.notifications_fired, 1);
        assert_eq!(d.dangerous_attempts, 2);
        assert_eq!(d.dangerous_refused, 1);

        let text = d.render();
        assert!(text.contains("新建日程 3 项"));
        assert!(text.contains("拦截 1 次"));
    }

    #[test]
    fn empty_window_is_clean() {
        let from = dt(2026, 7, 6, 0, 0);
        let to = dt(2026, 7, 12, 0, 0);
        let d = build_digest(from, to, 0, &[], &[], &[], &[], &[], &[], &[]);
        assert_eq!(d.events_total, 0);
        assert!(d.events_by_kind.is_empty());
        assert!(d.render().contains("新建日程 0 项"));
        // No observations → extras stay empty, render == core.
        assert_eq!(d.render(), d.render_core());
    }

    #[test]
    fn extras_carry_observations_and_facts() {
        use crate::journal::{BehaviorEntry, BehaviorKind};
        use crate::memory::MemoryFact;
        let from = dt(2026, 7, 6, 0, 0);
        let to = dt(2026, 7, 12, 23, 59);
        let status = |d: u32| BehaviorEntry {
            id: None,
            ts: dt(2026, 7, d, 7, 20),
            kind: BehaviorKind::Status,
            content: "护肤".into(),
            source: None,
        };
        let facts = vec![
            MemoryFact {
                id: Some(1),
                content: "我不吃辣".into(),
                source: "chat".into(),
                created_at: dt(2026, 7, 8, 10, 0),
                last_used_at: None,
            },
            // Outside the window → excluded.
            MemoryFact {
                id: Some(2),
                content: "旧事实".into(),
                source: "chat".into(),
                created_at: dt(2026, 6, 1, 10, 0),
                last_used_at: None,
            },
        ];
        let d = build_digest(
            from,
            to,
            0,
            &[],
            &[],
            &[],
            &[status(7), status(8), status(9)],
            &[],
            &facts,
            &[],
        );
        assert_eq!(d.top_activities, vec![("护肤".to_string(), 3)]);
        assert_eq!(d.new_facts, vec!["我不吃辣".to_string()]);
        let extras = d.render_extras();
        assert!(extras.contains("【观察】"));
        assert!(extras.contains("护肤"));
        assert!(extras.contains("本周我记住了什么"));
        assert!(extras.contains("我不吃辣"));
        assert!(!extras.contains("旧事实"));
        // The cloud-facing core must NOT contain the fact contents (§3.10).
        assert!(!d.render_core().contains("我不吃辣"));
    }
}

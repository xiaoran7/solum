//! Suggestion Engine v1 (F10) — rule-triggered, fully offline.
//!
//! Architecture §2 splits the engine into "规则触发时机 + LLM 生成建议内容";
//! v1 ships the rule half with deterministic template text, so suggestions
//! keep working with the cloud down (F16). The LLM can rewrite wording later.
//!
//! Rules over the schedule (within a proactivity-derived horizon):
//! exam → start prepping; deadline → start early; early meeting/class
//! tomorrow → rest early; two events too close → conflict warning.
//! Rules over the behavior journal: a status reported at a similar time on
//! several distinct days → offer to make it a standing reminder (the F3
//! habit-learning seed).
//!
//! Every suggestion carries a `dedup_key`, unique in storage, so re-running
//! generation on every tick never spams duplicates.

use chrono::{Duration, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::journal::BehaviorEntry;
use crate::model::{Event, EventKind};
use crate::proactivity::ProactivityLevel;

/// Which rule produced a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    ExamPrep,
    DeadlineCrunch,
    EarlyEvent,
    ScheduleConflict,
    HabitReminder,
    /// F11 v1: wellbeing signals relative to the personal baseline (D5).
    Wellness,
    /// The anti-nag brake: a routine unconfirmed for 7 days → offer to pause.
    RoutinePause,
}

impl SuggestionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuggestionKind::ExamPrep => "exam_prep",
            SuggestionKind::DeadlineCrunch => "deadline_crunch",
            SuggestionKind::EarlyEvent => "early_event",
            SuggestionKind::ScheduleConflict => "schedule_conflict",
            SuggestionKind::HabitReminder => "habit_reminder",
            SuggestionKind::Wellness => "wellness",
            SuggestionKind::RoutinePause => "routine_pause",
        }
    }
}

impl std::str::FromStr for SuggestionKind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "exam_prep" => SuggestionKind::ExamPrep,
            "deadline_crunch" => SuggestionKind::DeadlineCrunch,
            "early_event" => SuggestionKind::EarlyEvent,
            "schedule_conflict" => SuggestionKind::ScheduleConflict,
            "habit_reminder" => SuggestionKind::HabitReminder,
            "wellness" => SuggestionKind::Wellness,
            "routine_pause" => SuggestionKind::RoutinePause,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown suggestion kind: {other}"
                )))
            }
        })
    }
}

/// User-facing lifecycle of a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Dismissed,
}

impl SuggestionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuggestionStatus::Pending => "pending",
            SuggestionStatus::Accepted => "accepted",
            SuggestionStatus::Dismissed => "dismissed",
        }
    }
}

impl std::str::FromStr for SuggestionStatus {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pending" => SuggestionStatus::Pending,
            "accepted" => SuggestionStatus::Accepted,
            "dismissed" => SuggestionStatus::Dismissed,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown suggestion status: {other}"
                )))
            }
        })
    }
}

/// A generated suggestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: Option<i64>,
    pub created_at: NaiveDateTime,
    pub kind: SuggestionKind,
    pub text: String,
    /// Stable identity of the trigger (unique in storage) — regeneration on
    /// every tick must not produce duplicates.
    pub dedup_key: String,
    /// Provenance, e.g. "event#5" (F12).
    pub source: Option<String>,
    pub status: SuggestionStatus,
}

/// How far ahead each proactivity level looks when auto-generating. `Passive`
/// never auto-generates (the user can still ask explicitly).
pub fn suggestion_horizon(level: ProactivityLevel) -> Option<i64> {
    match level {
        ProactivityLevel::Passive => None,
        ProactivityLevel::Secretary => Some(1),
        ProactivityLevel::Butler => Some(3),
    }
}

fn hm(t: &NaiveDateTime) -> String {
    t.format("%H:%M").to_string()
}

fn md_hm(t: &NaiveDateTime) -> String {
    t.format("%m-%d %H:%M").to_string()
}

/// Run all rules. `events` may be the full table (filtering happens here);
/// `statuses` are journal `status` entries (habit material), any order.
pub fn generate(
    events: &[Event],
    statuses: &[BehaviorEntry],
    now: NaiveDateTime,
    horizon_days: i64,
) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let until = now + Duration::days(horizon_days);
    let upcoming: Vec<&Event> = events
        .iter()
        .filter(|e| e.start > now && e.start <= until)
        .collect();

    for ev in &upcoming {
        let id = ev.id.unwrap_or(0);
        match ev.kind {
            EventKind::Exam => out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::ExamPrep,
                text: format!(
                    "「{}」{} 开考，建议从今天开始安排复习块，越临近越从容。",
                    ev.title,
                    md_hm(&ev.start)
                ),
                dedup_key: format!("exam_prep:event#{id}"),
                source: Some(format!("event#{id}")),
                status: SuggestionStatus::Pending,
            }),
            EventKind::Deadline => out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::DeadlineCrunch,
                text: format!(
                    "「{}」{} 截止，建议今天先动手，别压到最后一刻。",
                    ev.title,
                    md_hm(&ev.start)
                ),
                dedup_key: format!("deadline_crunch:event#{id}"),
                source: Some(format!("event#{id}")),
                status: SuggestionStatus::Pending,
            }),
            EventKind::Meeting | EventKind::Class => {
                // Early start tomorrow → suggest winding down tonight.
                let tomorrow = (now + Duration::days(1)).date();
                if ev.start.date() == tomorrow && ev.start.hour() < 10 {
                    out.push(Suggestion {
                        id: None,
                        created_at: now,
                        kind: SuggestionKind::EarlyEvent,
                        text: format!(
                            "明早 {} 有「{}」，今晚建议早点休息，提前备好要用的材料。",
                            hm(&ev.start),
                            ev.title
                        ),
                        dedup_key: format!("early_event:event#{id}"),
                        source: Some(format!("event#{id}")),
                        status: SuggestionStatus::Pending,
                    });
                }
            }
            _ => {}
        }
    }

    // Conflicts: two upcoming events starting within an hour of each other.
    for (i, a) in upcoming.iter().enumerate() {
        for b in upcoming.iter().skip(i + 1) {
            let gap = (b.start - a.start).num_minutes().abs();
            if gap <= 60 {
                let (x, y) = (a.id.unwrap_or(0), b.id.unwrap_or(0));
                let (lo, hi) = if x <= y { (x, y) } else { (y, x) };
                out.push(Suggestion {
                    id: None,
                    created_at: now,
                    kind: SuggestionKind::ScheduleConflict,
                    text: format!(
                        "「{}」（{}）与「{}」（{}）时间相近，可能赶不过来，考虑调整其一。",
                        a.title,
                        md_hm(&a.start),
                        b.title,
                        md_hm(&b.start)
                    ),
                    dedup_key: format!("schedule_conflict:event#{lo}-{hi}"),
                    source: Some(format!("event#{lo},event#{hi}")),
                    status: SuggestionStatus::Pending,
                });
            }
        }
    }

    out.extend(detect_habits(statuses, now));
    out
}

/// A recurring status at a similar time of day on ≥3 distinct days → offer a
/// standing reminder. Times must cluster within a 90-minute window.
fn detect_habits(statuses: &[BehaviorEntry], now: NaiveDateTime) -> Vec<Suggestion> {
    use std::collections::HashMap;
    let mut by_content: HashMap<&str, Vec<&BehaviorEntry>> = HashMap::new();
    for e in statuses {
        by_content.entry(e.content.as_str()).or_default().push(e);
    }

    let mut out = Vec::new();
    for (content, entries) in by_content {
        let mut dates: Vec<_> = entries.iter().map(|e| e.ts.date()).collect();
        dates.sort();
        dates.dedup();
        if dates.len() < 3 {
            continue;
        }
        // Cluster around the median instead of a global min/max span: one
        // stray entry (a 23:09 status while the real habit lives at 08:00)
        // must not silently kill the habit forever (2026-07-18 走查发现).
        let mut minutes: Vec<i64> = entries
            .iter()
            .map(|e| (e.ts.hour() * 60 + e.ts.minute()) as i64)
            .collect();
        minutes.sort_unstable();
        let median = minutes[minutes.len() / 2];
        let clustered: Vec<(&&BehaviorEntry, i64)> = entries
            .iter()
            .zip(
                entries
                    .iter()
                    .map(|e| (e.ts.hour() * 60 + e.ts.minute()) as i64),
            )
            .filter(|(_, m)| (m - median).abs() <= 45)
            .collect();
        let mut cluster_dates: Vec<_> = clustered.iter().map(|(e, _)| e.ts.date()).collect();
        cluster_dates.sort();
        cluster_dates.dedup();
        if cluster_dates.len() < 3 {
            continue; // Not clustered enough to call a habit.
        }
        let mean = clustered.iter().map(|(_, m)| m).sum::<i64>() / clustered.len() as i64;
        let typical = format!("{:02}:{:02}", mean / 60, mean % 60);
        out.push(Suggestion {
            id: None,
            created_at: now,
            kind: SuggestionKind::HabitReminder,
            text: format!(
                "最近 {} 天里你有 {} 次在 {} 左右「{}」，要不要设成固定提醒？（采纳即创建，之后可在台账停用）",
                cluster_dates.len(),
                clustered.len(),
                typical,
                content
            ),
            dedup_key: format!("habit_reminder:{content}"),
            // Machine-readable half: accepting the suggestion auto-creates the
            // routine from this (see `routine::parse_habit_source`).
            source: Some(format!("habit:{typical}:{content}")),
            status: SuggestionStatus::Pending,
        });
    }
    out.sort_by(|a, b| a.dedup_key.cmp(&b.dedup_key));
    out
}

// ---- F11 v1: wellness signals (D5) -------------------------------------------
//
// All thresholds are *relative to the personal baseline* (28-day medians from
// `stats::baselines`) — never absolute numbers — and every signal requires its
// metric's data gate (≥14 distinct days) to be open. Output goes through the
// normal suggestion pipeline: gated by `life_suggestions`, deduped per day,
// and (by the 2026-07-14 decision) never an OS notification.

/// Sedentary rule: by this hour of day…
const SEDENTARY_CHECK_HOUR: u32 = 12;
/// …fewer than this fraction of the personal median daily steps.
const SEDENTARY_FRACTION: f64 = 0.15;
/// Sleep-deficit rule: last night below this fraction of the median.
const SLEEP_DEFICIT_FRACTION: f64 = 0.8;
/// Resting-HR rule: this many consecutive days above baseline…
const HR_STREAK_DAYS: usize = 3;
/// …by more than this factor.
const HR_ELEVATED_FACTOR: f64 = 1.10;

/// Run the wellness rules. `samples` may be the full table; `b` are the
/// personal baselines. Deterministic given the inputs, like `generate`.
pub fn generate_wellness(
    samples: &[crate::wearable::HealthSample],
    b: &crate::stats::Baselines,
    now: NaiveDateTime,
) -> Vec<Suggestion> {
    use crate::wearable::HealthMetric;
    let mut out = Vec::new();
    let today = now.date();
    let day_key = today.format("%Y-%m-%d");

    // 1. Sedentary: it's afternoon and today's steps are far below the median.
    if b.gate_open(HealthMetric::Steps) && now.hour() >= SEDENTARY_CHECK_HOUR {
        if let Some(median_steps) = b.daily_steps {
            let today_steps: f64 = samples
                .iter()
                .filter(|s| s.kind == HealthMetric::Steps && s.start.date() == today)
                .map(|s| s.value)
                .sum();
            if today_steps < median_steps * SEDENTARY_FRACTION {
                out.push(Suggestion {
                    id: None,
                    created_at: now,
                    kind: SuggestionKind::Wellness,
                    text: format!(
                        "今天到现在只走了 {today_steps:.0} 步（平时中位数 {median_steps:.0}），要不要起来活动几分钟？"
                    ),
                    dedup_key: format!("wellness_sedentary:{day_key}"),
                    source: Some("baseline:steps".into()),
                    status: SuggestionStatus::Pending,
                });
            }
        }
    }

    // 2. Sleep deficit: last night's total below 80% of the personal median.
    if b.gate_open(HealthMetric::Sleep) {
        if let Some(median_sleep) = b.sleep_minutes {
            let last_night: f64 = samples
                .iter()
                .filter(|s| s.kind == HealthMetric::Sleep && s.end.date() == today)
                .map(|s| s.value)
                .sum();
            if last_night > 0.0 && last_night < median_sleep * SLEEP_DEFICIT_FRACTION {
                out.push(Suggestion {
                    id: None,
                    created_at: now,
                    kind: SuggestionKind::Wellness,
                    text: format!(
                        "昨晚只睡了 {:.1} 小时（平时 {:.1} 小时），今天别安排太满，晚上早点休息。",
                        last_night / 60.0,
                        median_sleep / 60.0
                    ),
                    dedup_key: format!("wellness_sleep:{day_key}"),
                    source: Some("baseline:sleep".into()),
                    status: SuggestionStatus::Pending,
                });
            }
        }
    }

    // 3. Elevated resting HR: daily minimum above baseline×1.10 for 3 straight
    //    days (ending yesterday — today's minimum isn't final yet).
    if b.gate_open(HealthMetric::HeartRate) {
        if let Some(resting) = b.resting_hr {
            let elevated = (1..=HR_STREAK_DAYS).all(|back| {
                let day = today - Duration::days(back as i64);
                let day_min = samples
                    .iter()
                    .filter(|s| s.kind == HealthMetric::HeartRate && s.start.date() == day)
                    .map(|s| s.value)
                    .fold(f64::INFINITY, f64::min);
                day_min.is_finite() && day_min > resting * HR_ELEVATED_FACTOR
            });
            if elevated {
                out.push(Suggestion {
                    id: None,
                    created_at: now,
                    kind: SuggestionKind::Wellness,
                    text: format!(
                        "最近 {HR_STREAK_DAYS} 天静息心率都高于你平时的 {resting:.0} bpm 一成以上，注意休息；持续偏高建议咨询医生。"
                    ),
                    dedup_key: format!("wellness_hr:{day_key}"),
                    source: Some("baseline:heart_rate".into()),
                    status: SuggestionStatus::Pending,
                });
            }
        }
    }
    out
}

/// The anti-nag brake (D4): an active routine older than 7 days with **zero**
/// journal confirmations (a `Status` entry matching its title) in the last 7
/// days → offer to pause it. Deduped per routine per ISO week, so declining
/// stays declined for the week instead of nagging daily.
pub fn generate_routine_pauses(
    routines: &[crate::routine::Routine],
    statuses: &[BehaviorEntry],
    now: NaiveDateTime,
) -> Vec<Suggestion> {
    let week = now.date().format("%G-W%V");
    let cutoff = now - Duration::days(7);
    let mut out = Vec::new();
    for r in routines
        .iter()
        .filter(|r| r.active && r.created_at <= cutoff)
    {
        let Some(id) = r.id else { continue };
        // Only a real confirmation counts. Matching on `content.contains(title)`
        // let any passing mention of the routine's words suppress the pause
        // suggestion — the exact case where the brake is most needed.
        let confirmed = statuses
            .iter()
            .any(|s| s.ts >= cutoff && crate::routine::is_completion_of(s, id));
        if !confirmed {
            out.push(Suggestion {
                id: None,
                created_at: now,
                kind: SuggestionKind::RoutinePause,
                text: format!(
                    "固定提醒「{}（{}）」最近 7 天都没有确认完成过，要不要先暂停它？（采纳即暂停，不删除）",
                    r.title, r.time_of_day
                ),
                dedup_key: format!("routine_pause:{id}:{week}"),
                source: Some(format!("routine#{id}")),
                status: SuggestionStatus::Pending,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::BehaviorKind;
    use chrono::NaiveDate;

    fn dt(d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn ev(id: i64, title: &str, kind: EventKind, start: NaiveDateTime) -> Event {
        let mut e = Event::new(title, kind, start, "", dt(6, 9, 0));
        e.id = Some(id);
        e
    }

    fn status(content: &str, ts: NaiveDateTime) -> BehaviorEntry {
        BehaviorEntry {
            id: None,
            ts,
            kind: BehaviorKind::Status,
            content: content.into(),
            source: None,
        }
    }

    #[test]
    fn exam_and_deadline_within_horizon() {
        let events = vec![
            ev(1, "期末考试", EventKind::Exam, dt(8, 9, 0)),
            ev(2, "交报告", EventKind::Deadline, dt(7, 18, 0)),
            // Outside the 3-day horizon → no suggestion.
            ev(3, "远期考试", EventKind::Exam, dt(15, 9, 0)),
        ];
        let s = generate(&events, &[], dt(6, 10, 0), 3);
        let keys: Vec<&str> = s.iter().map(|x| x.dedup_key.as_str()).collect();
        assert!(keys.contains(&"exam_prep:event#1"));
        assert!(keys.contains(&"deadline_crunch:event#2"));
        assert!(!keys.iter().any(|k| k.contains("event#3")));
    }

    #[test]
    fn early_meeting_tomorrow_only() {
        let events = vec![
            ev(1, "晨会", EventKind::Meeting, dt(7, 8, 30)),
            // Tomorrow but not early → nothing.
            ev(2, "下午会", EventKind::Meeting, dt(7, 15, 0)),
            // Early but day after tomorrow → nothing.
            ev(3, "后天晨会", EventKind::Meeting, dt(8, 8, 30)),
        ];
        let s = generate(&events, &[], dt(6, 10, 0), 3);
        let early: Vec<_> = s
            .iter()
            .filter(|x| x.kind == SuggestionKind::EarlyEvent)
            .collect();
        assert_eq!(early.len(), 1);
        assert_eq!(early[0].dedup_key, "early_event:event#1");
        assert!(early[0].text.contains("08:30"));
    }

    #[test]
    fn conflict_when_starts_within_an_hour() {
        let events = vec![
            ev(1, "评审会", EventKind::Meeting, dt(7, 14, 0)),
            ev(2, "面试", EventKind::Meeting, dt(7, 14, 30)),
            ev(3, "晚课", EventKind::Class, dt(7, 19, 0)),
        ];
        let s = generate(&events, &[], dt(6, 10, 0), 3);
        let conflicts: Vec<_> = s
            .iter()
            .filter(|x| x.kind == SuggestionKind::ScheduleConflict)
            .collect();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].dedup_key, "schedule_conflict:event#1-2");
    }

    #[test]
    fn habit_needs_three_days_and_clustering() {
        // Three distinct days around 07:20 → habit.
        let clustered = vec![
            status("护肤", dt(3, 7, 15)),
            status("护肤", dt(4, 7, 25)),
            status("护肤", dt(5, 7, 20)),
        ];
        let s = generate(&[], &clustered, dt(6, 10, 0), 3);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, SuggestionKind::HabitReminder);
        assert!(s[0].text.contains("07:20"));

        // Only two distinct days → no habit.
        let few = &clustered[..2];
        assert!(generate(&[], few, dt(6, 10, 0), 3).is_empty());

        // Scattered times → no habit.
        let scattered = vec![
            status("刷手机", dt(3, 7, 0)),
            status("刷手机", dt(4, 12, 0)),
            status("刷手机", dt(5, 22, 0)),
        ];
        assert!(generate(&[], &scattered, dt(6, 10, 0), 3).is_empty());

        // A single stray entry outside the cluster must NOT kill the habit
        // (median-window clustering, 2026-07-18): three 07:20-ish days plus
        // one 23:09 outlier still reads as a 07:20 habit.
        let with_outlier = vec![
            status("护肤", dt(2, 23, 9)),
            status("护肤", dt(3, 7, 20)),
            status("护肤", dt(4, 7, 25)),
            status("护肤", dt(5, 7, 20)),
        ];
        let s = generate(&[], &with_outlier, dt(6, 10, 0), 3);
        assert_eq!(s.len(), 1);
        assert!(s[0].text.contains("07:2"));
    }

    #[test]
    fn horizon_by_level() {
        assert_eq!(suggestion_horizon(ProactivityLevel::Passive), None);
        assert_eq!(suggestion_horizon(ProactivityLevel::Secretary), Some(1));
        assert_eq!(suggestion_horizon(ProactivityLevel::Butler), Some(3));
    }
}

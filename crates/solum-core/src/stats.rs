//! Offline data review (ARCHITECTURE.md §6 Phase 6 D2, `pa stats`).
//!
//! Pure aggregation over already-fetched rows — no cloud, no writes. Two jobs:
//! 1. Show the real distributions so the user can固化 the importance rule
//!    table's initial values from evidence, not guesswork (§7 第一条收口).
//! 2. Compute per-user wearable baselines (28-day medians) and check the
//!    "每类数据 ≥ 14 天覆盖" gate that decides whether F11/F13 may start —
//!    the same baselines the wellness rules (D5) consume.

use chrono::{Duration, NaiveDateTime, Timelike};
use serde::Serialize;

use crate::journal::{BehaviorEntry, BehaviorKind};
use crate::model::{Event, EventKind};
use crate::wearable::{HealthMetric, HealthSample};

/// Days of history the baselines look back over.
pub const BASELINE_WINDOW_DAYS: i64 = 28;
/// Minimum distinct days of data per metric before F11/F13 may rely on it.
pub const GATE_MIN_DAYS: usize = 14;

/// Per-user wearable baselines (28-day medians). `None` = not enough data.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Baselines {
    /// Median of daily-minimum heart rate (resting HR estimate), bpm.
    pub resting_hr: Option<f64>,
    /// Median nightly sleep, minutes.
    pub sleep_minutes: Option<f64>,
    /// Median daily step total.
    pub daily_steps: Option<f64>,
    /// Distinct days with ≥1 sample, per metric, inside the window.
    pub hr_days: usize,
    pub sleep_days: usize,
    pub steps_days: usize,
}

impl Baselines {
    /// The F11/F13 data gate: does `metric` have enough coverage to act on?
    pub fn gate_open(&self, metric: HealthMetric) -> bool {
        let days = match metric {
            HealthMetric::HeartRate => self.hr_days,
            HealthMetric::Sleep => self.sleep_days,
            HealthMetric::Steps => self.steps_days,
        };
        days >= GATE_MIN_DAYS
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

/// Group samples of one metric by calendar day → per-day aggregate.
fn per_day<F>(
    samples: &[HealthSample],
    metric: HealthMetric,
    agg: F,
) -> Vec<(chrono::NaiveDate, f64)>
where
    F: Fn(&[f64]) -> f64,
{
    use std::collections::BTreeMap;
    let mut by_day: BTreeMap<chrono::NaiveDate, Vec<f64>> = BTreeMap::new();
    for s in samples.iter().filter(|s| s.kind == metric) {
        by_day.entry(s.start.date()).or_default().push(s.value);
    }
    by_day
        .into_iter()
        .map(|(d, vals)| (d, agg(&vals)))
        .collect()
}

/// Compute baselines from samples (the caller passes everything; windowing
/// happens here so every entry point agrees).
pub fn baselines(samples: &[HealthSample], now: NaiveDateTime) -> Baselines {
    let from = now - Duration::days(BASELINE_WINDOW_DAYS);
    let windowed: Vec<HealthSample> = samples
        .iter()
        .filter(|s| s.start >= from && s.start <= now)
        .cloned()
        .collect();

    let hr_daily_min = per_day(&windowed, HealthMetric::HeartRate, |v| {
        v.iter().cloned().fold(f64::INFINITY, f64::min)
    });
    let sleep_daily = per_day(&windowed, HealthMetric::Sleep, |v| v.iter().sum());
    let steps_daily = per_day(&windowed, HealthMetric::Steps, |v| v.iter().sum());

    Baselines {
        resting_hr: median(hr_daily_min.iter().map(|(_, v)| *v).collect()),
        sleep_minutes: median(sleep_daily.iter().map(|(_, v)| *v).collect()),
        daily_steps: median(steps_daily.iter().map(|(_, v)| *v).collect()),
        hr_days: hr_daily_min.len(),
        sleep_days: sleep_daily.len(),
        steps_days: steps_daily.len(),
    }
}

/// One recurring status activity (habit material) with its typical time.
#[derive(Debug, Clone, Serialize)]
pub struct ActivityCluster {
    pub content: String,
    pub count: usize,
    pub distinct_days: usize,
    /// Mean time of day, "HH:MM".
    pub typical_time: String,
}

/// The full data-review report.
#[derive(Debug, Clone, Serialize)]
pub struct StatsReport {
    pub now: NaiveDateTime,
    /// (kind, count) over all stored events, canonical order, non-zero only.
    pub events_by_kind: Vec<(EventKind, usize)>,
    /// Check-ins asked vs. status answers recorded (rough response rate).
    pub checkins_asked: usize,
    pub status_answers: usize,
    /// Recurring activities, most days first.
    pub activity_clusters: Vec<ActivityCluster>,
    pub baselines: Baselines,
}

/// Build the report from already-fetched rows.
pub fn build_stats(
    events: &[Event],
    behaviors: &[BehaviorEntry],
    samples: &[HealthSample],
    now: NaiveDateTime,
) -> StatsReport {
    let mut events_by_kind = Vec::new();
    for kind in EventKind::all() {
        let n = events.iter().filter(|e| e.kind == kind).count();
        if n > 0 {
            events_by_kind.push((kind, n));
        }
    }

    let checkins_asked = behaviors
        .iter()
        .filter(|b| b.kind == BehaviorKind::CheckinAsked)
        .count();
    let statuses: Vec<&BehaviorEntry> = behaviors
        .iter()
        .filter(|b| b.kind == BehaviorKind::Status)
        .collect();

    use std::collections::BTreeMap;
    let mut by_content: BTreeMap<&str, Vec<&BehaviorEntry>> = BTreeMap::new();
    for e in &statuses {
        by_content.entry(e.content.as_str()).or_default().push(e);
    }
    let mut activity_clusters: Vec<ActivityCluster> = by_content
        .into_iter()
        .filter(|(_, entries)| entries.len() >= 2)
        .map(|(content, entries)| {
            let mut days: Vec<_> = entries.iter().map(|e| e.ts.date()).collect();
            days.sort();
            days.dedup();
            let minutes: Vec<i64> = entries
                .iter()
                .map(|e| (e.ts.hour() * 60 + e.ts.minute()) as i64)
                .collect();
            let mean = minutes.iter().sum::<i64>() / minutes.len() as i64;
            ActivityCluster {
                content: content.to_string(),
                count: entries.len(),
                distinct_days: days.len(),
                typical_time: format!("{:02}:{:02}", mean / 60, mean % 60),
            }
        })
        .collect();
    activity_clusters.sort_by(|a, b| {
        b.distinct_days
            .cmp(&a.distinct_days)
            .then(b.count.cmp(&a.count))
    });

    StatsReport {
        now,
        events_by_kind,
        checkins_asked,
        status_answers: statuses.len(),
        activity_clusters,
        baselines: baselines(samples, now),
    }
}

impl StatsReport {
    /// Readable Chinese report (offline, read-only — nothing here goes to the
    /// cloud).
    pub fn render(&self) -> String {
        let mut out = String::from("数据回看（纯本地统计，只读）\n");

        out.push_str("\n【日程分布】（供固化重要度规则表初值参考）\n");
        if self.events_by_kind.is_empty() {
            out.push_str("  （还没有任何日程记录）\n");
        }
        for (k, n) in &self.events_by_kind {
            out.push_str(&format!("  {:<9} {n} 项\n", k.as_str()));
        }

        out.push_str(&format!(
            "\n【问询应答】问询 {} 次，状态回答 {} 条\n",
            self.checkins_asked, self.status_answers
        ));

        out.push_str("\n【重复活动】（习惯素材）\n");
        if self.activity_clusters.is_empty() {
            out.push_str("  （还没有形成重复模式）\n");
        }
        for c in self.activity_clusters.iter().take(8) {
            out.push_str(&format!(
                "  「{}」{} 天 {} 次，典型时间 {}\n",
                c.content, c.distinct_days, c.count, c.typical_time
            ));
        }

        let b = &self.baselines;
        out.push_str(&format!(
            "\n【穿戴数据基线】（近 {BASELINE_WINDOW_DAYS} 天中位数）\n\
             \x20 静息心率：{}（{} 天有数据）\n\
             \x20 每晚睡眠：{}（{} 天有数据）\n\
             \x20 每日步数：{}（{} 天有数据）\n",
            b.resting_hr
                .map(|v| format!("{v:.0} bpm"))
                .unwrap_or_else(|| "—".into()),
            b.hr_days,
            b.sleep_minutes
                .map(|v| format!("{:.1} 小时", v / 60.0))
                .unwrap_or_else(|| "—".into()),
            b.sleep_days,
            b.daily_steps
                .map(|v| format!("{v:.0} 步"))
                .unwrap_or_else(|| "—".into()),
            b.steps_days,
        ));

        out.push_str(&format!(
            "\n【F11/F13 数据门槛】（每类 ≥ {GATE_MIN_DAYS} 天才可启用相应主动感知）\n\
             \x20 心率 {}  睡眠 {}  步数 {}\n",
            gate_str(b.gate_open(HealthMetric::HeartRate), b.hr_days),
            gate_str(b.gate_open(HealthMetric::Sleep), b.sleep_days),
            gate_str(b.gate_open(HealthMetric::Steps), b.steps_days),
        ));
        out
    }
}

fn gate_str(open: bool, days: usize) -> String {
    if open {
        format!("已达到（{days}天）")
    } else {
        format!("未达到（{days}/{GATE_MIN_DAYS}天）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn hr(d: u32, h: u32, v: f64) -> HealthSample {
        HealthSample::new(HealthMetric::HeartRate, dt(d, h, 0), dt(d, h, 0), v, "hc")
    }

    fn sleep(d: u32, minutes: f64) -> HealthSample {
        HealthSample::new(HealthMetric::Sleep, dt(d, 7, 0), dt(d, 7, 0), minutes, "hc")
    }

    #[test]
    fn baselines_median_of_daily_aggregates() {
        // 3 days of HR: daily minima 60, 62, 64 → resting 62.
        let samples = vec![
            hr(1, 8, 80.0),
            hr(1, 23, 60.0),
            hr(2, 8, 62.0),
            hr(2, 12, 90.0),
            hr(3, 8, 64.0),
            sleep(1, 400.0),
            sleep(2, 420.0),
        ];
        let b = baselines(&samples, dt(4, 10, 0));
        assert_eq!(b.resting_hr, Some(62.0));
        assert_eq!(b.hr_days, 3);
        assert_eq!(b.sleep_minutes, Some(410.0));
        assert_eq!(b.sleep_days, 2);
        assert_eq!(b.daily_steps, None);
        // Gate: 3 days < 14 → closed.
        assert!(!b.gate_open(HealthMetric::HeartRate));
    }

    #[test]
    fn gate_opens_at_fourteen_days() {
        let samples: Vec<HealthSample> = (1..=14).map(|d| hr(d, 8, 60.0)).collect();
        let b = baselines(&samples, dt(15, 10, 0));
        assert!(b.gate_open(HealthMetric::HeartRate));
        assert!(!b.gate_open(HealthMetric::Sleep));
    }

    #[test]
    fn stats_report_clusters_and_renders() {
        let statuses: Vec<BehaviorEntry> = [(1, 7, 15), (2, 7, 25), (3, 7, 20)]
            .iter()
            .map(|&(d, h, mi)| BehaviorEntry {
                id: None,
                ts: dt(d, h, mi),
                kind: BehaviorKind::Status,
                content: "护肤".into(),
                source: None,
            })
            .collect();
        let r = build_stats(&[], &statuses, &[], dt(4, 10, 0));
        assert_eq!(r.activity_clusters.len(), 1);
        assert_eq!(r.activity_clusters[0].distinct_days, 3);
        assert_eq!(r.activity_clusters[0].typical_time, "07:20");
        let text = r.render();
        assert!(text.contains("护肤"));
        assert!(text.contains("F11/F13"));
    }
}

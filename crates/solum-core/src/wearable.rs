//! Wearable Adapter (F5, Phase 4) — structured health samples from a
//! platform health store (ARCHITECTURE.md §3.7). v1 scope is deliberately
//! narrow (2026-07-12 decision): get heart rate / steps / sleep in and
//! stored, nothing more. No proactivity wiring, no cloud call — samples are
//! inert data until F11 (emotion/anomaly sensing) designs a trigger policy
//! on top of them, which is future work.
//!
//! This module owns the domain type and the dedup contract; reading the
//! platform health store is the mobile shell's job (`solum-health-connect`
//! plugin hands over already-parsed samples).

use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::CoreError;
use crate::model::fmt_ts;

/// What a sample measures. The unit is implied by the kind (see
/// [`HealthSample::value`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthMetric {
    HeartRate,
    Steps,
    Sleep,
}

impl HealthMetric {
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthMetric::HeartRate => "heart_rate",
            HealthMetric::Steps => "steps",
            HealthMetric::Sleep => "sleep",
        }
    }

    /// Chinese label + unit for display (ledger/UI).
    pub fn label(&self) -> &'static str {
        match self {
            HealthMetric::HeartRate => "心率",
            HealthMetric::Steps => "步数",
            HealthMetric::Sleep => "睡眠",
        }
    }
}

impl std::str::FromStr for HealthMetric {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "heart_rate" => HealthMetric::HeartRate,
            "steps" => HealthMetric::Steps,
            "sleep" => HealthMetric::Sleep,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown health metric: {other}"
                )))
            }
        })
    }
}

/// One structured reading handed over by a wearable/health-platform adapter.
/// `value`'s unit depends on `kind`: beats-per-minute for heart rate, a step
/// count for steps, minutes for a sleep session's length. For an
/// instantaneous reading (heart rate) `start == end`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthSample {
    pub id: Option<i64>,
    pub kind: HealthMetric,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub value: f64,
    /// Adapter provenance, e.g. "health_connect" (F12 source label).
    pub source: String,
    /// When this row was written locally (agent clock) — distinct from
    /// `start`/`end`, which are the platform's own measurement timestamps.
    pub created_at: NaiveDateTime,
}

impl HealthSample {
    pub fn new(
        kind: HealthMetric,
        start: NaiveDateTime,
        end: NaiveDateTime,
        value: f64,
        source: impl Into<String>,
    ) -> Self {
        HealthSample {
            id: None,
            kind,
            start,
            end,
            value,
            source: source.into(),
            // Overwritten by the orchestrator at insert time with the
            // caller's injected clock; a placeholder here keeps the
            // constructor ergonomic for adapters that don't track "now".
            created_at: start,
        }
    }

    /// Stable key so re-polling an overlapping platform time window doesn't
    /// duplicate rows (same pattern as `suggestions.dedup_key`). Two
    /// readings of the same kind at the same instant are the same reading.
    pub fn dedup_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.kind.as_str(),
            fmt_ts(&self.start),
            fmt_ts(&self.end),
            self.source,
        )
    }
}

/// Privacy-preserving local aggregate shown instead of raw health rows.
/// Values compare the latest seven complete calendar days with the seven days
/// before them. No diagnosis or value judgement is attached to rising/falling data.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthTrendSummary {
    pub kind: HealthMetric,
    pub label: &'static str,
    pub unit: &'static str,
    pub current: f64,
    pub previous: Option<f64>,
    pub change_percent: Option<f64>,
    pub direction: &'static str,
    pub current_days: usize,
    pub previous_days: usize,
    pub window_days: usize,
}

const TREND_WINDOW_DAYS: i64 = 7;
const TREND_MIN_COVERAGE_DAYS: usize = 3;

fn daily_values(
    samples: &[HealthSample],
    metric: HealthMetric,
    from: NaiveDate,
    through: NaiveDate,
) -> Vec<f64> {
    let mut grouped: BTreeMap<NaiveDate, Vec<f64>> = BTreeMap::new();
    for sample in samples {
        let date = sample.start.date();
        if sample.kind == metric
            && date >= from
            && date <= through
            && sample.value.is_finite()
            && sample.value >= 0.0
        {
            grouped.entry(date).or_default().push(sample.value);
        }
    }
    grouped
        .into_values()
        .map(|values| match metric {
            HealthMetric::HeartRate => values.into_iter().fold(f64::INFINITY, f64::min),
            HealthMetric::Steps | HealthMetric::Sleep => values.into_iter().sum(),
        })
        .collect()
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn rounded(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Build bounded, local-only 7-day trends. Metrics without at least one day
/// in the current window are omitted; comparison needs three days of coverage
/// in each window so a single reading is never presented as a trend.
pub fn health_trend_summaries(
    samples: &[HealthSample],
    now: NaiveDateTime,
) -> Vec<HealthTrendSummary> {
    // Exclude today: partial step/sleep totals would otherwise look like a
    // meaningful drop merely because the day is still in progress.
    let current_through = now.date() - Duration::days(1);
    let current_from = current_through - Duration::days(TREND_WINDOW_DAYS - 1);
    let previous_through = current_from - Duration::days(1);
    let previous_from = previous_through - Duration::days(TREND_WINDOW_DAYS - 1);
    let mut summaries = Vec::new();

    for metric in [
        HealthMetric::HeartRate,
        HealthMetric::Steps,
        HealthMetric::Sleep,
    ] {
        let current_values = daily_values(samples, metric, current_from, current_through);
        let previous_values = daily_values(samples, metric, previous_from, previous_through);
        let Some(mut current) = mean(&current_values) else {
            continue;
        };
        let mut previous = mean(&previous_values);
        if metric == HealthMetric::Sleep {
            current /= 60.0;
            previous = previous.map(|value| value / 60.0);
        }
        let change_percent = if current_values.len() >= TREND_MIN_COVERAGE_DAYS
            && previous_values.len() >= TREND_MIN_COVERAGE_DAYS
        {
            previous
                .filter(|value| value.abs() > f64::EPSILON)
                .map(|value| rounded((current - value) / value * 100.0))
        } else {
            None
        };
        let direction = match change_percent {
            Some(value) if value >= 3.0 => "higher",
            Some(value) if value <= -3.0 => "lower",
            Some(_) => "stable",
            None => "no_comparison",
        };
        summaries.push(HealthTrendSummary {
            kind: metric,
            label: metric.label(),
            unit: match metric {
                HealthMetric::HeartRate => "bpm",
                HealthMetric::Steps => "步/日",
                HealthMetric::Sleep => "小时/日",
            },
            current: rounded(current),
            previous: previous.map(rounded),
            change_percent,
            direction,
            current_days: current_values.len(),
            previous_days: previous_values.len(),
            window_days: TREND_WINDOW_DAYS as usize,
        });
    }
    summaries
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 12)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn metric_round_trips() {
        for k in [
            HealthMetric::HeartRate,
            HealthMetric::Steps,
            HealthMetric::Sleep,
        ] {
            assert_eq!(k.as_str().parse::<HealthMetric>().unwrap(), k);
        }
        assert!("nope".parse::<HealthMetric>().is_err());
    }

    #[test]
    fn dedup_key_stable_for_same_reading() {
        let a = HealthSample::new(
            HealthMetric::HeartRate,
            dt(9, 0),
            dt(9, 0),
            72.0,
            "health_connect",
        );
        let b = HealthSample::new(
            HealthMetric::HeartRate,
            dt(9, 0),
            dt(9, 0),
            75.0,
            "health_connect",
        );
        // Same instant + kind + source => same key even if the value differs
        // (a re-read of the same instant should overwrite, not duplicate).
        assert_eq!(a.dedup_key(), b.dedup_key());

        let c = HealthSample::new(
            HealthMetric::Steps,
            dt(9, 0),
            dt(9, 30),
            500.0,
            "health_connect",
        );
        assert_ne!(a.dedup_key(), c.dedup_key());
    }

    fn sample(kind: HealthMetric, day: u32, value: f64) -> HealthSample {
        let at = NaiveDate::from_ymd_opt(2026, 8, day)
            .unwrap()
            .and_hms_opt(8, 0, 0)
            .unwrap();
        HealthSample::new(kind, at, at, value, "health_connect")
    }

    #[test]
    fn local_trends_compare_daily_aggregates_without_exposing_rows() {
        let mut samples = Vec::new();
        for day in 11..=17 {
            samples.push(sample(HealthMetric::Steps, day, 5_000.0));
            samples.push(sample(HealthMetric::Sleep, day, 420.0));
        }
        for day in 18..=24 {
            samples.push(sample(HealthMetric::Steps, day, 6_000.0));
            samples.push(sample(HealthMetric::Sleep, day, 480.0));
        }
        samples.push(sample(HealthMetric::Steps, 25, 1.0));
        let now = NaiveDate::from_ymd_opt(2026, 8, 25)
            .unwrap()
            .and_hms_opt(20, 0, 0)
            .unwrap();
        let trends = health_trend_summaries(&samples, now);
        let steps = trends
            .iter()
            .find(|trend| trend.kind == HealthMetric::Steps)
            .unwrap();
        assert_eq!(steps.current, 6_000.0);
        assert_eq!(steps.previous, Some(5_000.0));
        assert_eq!(steps.change_percent, Some(20.0));
        assert_eq!(steps.direction, "higher");
        let sleep = trends
            .iter()
            .find(|trend| trend.kind == HealthMetric::Sleep)
            .unwrap();
        assert_eq!(sleep.current, 8.0);
        assert_eq!(sleep.previous, Some(7.0));
    }

    #[test]
    fn sparse_data_is_a_summary_but_not_a_claimed_trend() {
        let now = NaiveDate::from_ymd_opt(2026, 8, 25)
            .unwrap()
            .and_hms_opt(20, 0, 0)
            .unwrap();
        let trends = health_trend_summaries(&[sample(HealthMetric::HeartRate, 24, 72.0)], now);
        assert_eq!(trends.len(), 1);
        assert_eq!(trends[0].current_days, 1);
        assert_eq!(trends[0].change_percent, None);
        assert_eq!(trends[0].direction, "no_comparison");
    }
}

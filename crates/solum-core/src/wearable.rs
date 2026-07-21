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

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

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
}

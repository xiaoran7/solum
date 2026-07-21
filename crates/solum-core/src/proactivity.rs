//! Proactivity engine (ARCHITECTURE.md §3.2).
//!
//! Not one global switch — each capability dimension has its own level. Raising
//! a dimension to "butler" makes the agent ask more and act sooner, but it can
//! **never** relax the dangerous-action guard (F7). That invariant is encoded
//! here in [`ProactivityConfig::may_autoexecute`] and enforced independently in
//! [`crate::guard`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::model::RiskLevel;

/// How forward the agent is on a given dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProactivityLevel {
    /// Only acts when asked.
    Passive,
    /// Prepares and suggests, asks before acting.
    Secretary,
    /// Anticipates and acts on safe things autonomously.
    Butler,
}

impl ProactivityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProactivityLevel::Passive => "passive",
            ProactivityLevel::Secretary => "secretary",
            ProactivityLevel::Butler => "butler",
        }
    }
}

impl std::str::FromStr for ProactivityLevel {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_lowercase().as_str() {
            "passive" => ProactivityLevel::Passive,
            "secretary" => ProactivityLevel::Secretary,
            "butler" => ProactivityLevel::Butler,
            other => {
                return Err(CoreError::Invalid(format!(
                    "unknown proactivity level {other:?}"
                )))
            }
        })
    }
}

/// The independently-tunable capability dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProactivityDimension {
    ScheduleReminders,
    LifeSuggestions,
    StatusCheckins,
    WeeklyReview,
}

impl ProactivityDimension {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProactivityDimension::ScheduleReminders => "schedule_reminders",
            ProactivityDimension::LifeSuggestions => "life_suggestions",
            ProactivityDimension::StatusCheckins => "status_checkins",
            ProactivityDimension::WeeklyReview => "weekly_review",
        }
    }

    pub fn all() -> [ProactivityDimension; 4] {
        [
            ProactivityDimension::ScheduleReminders,
            ProactivityDimension::LifeSuggestions,
            ProactivityDimension::StatusCheckins,
            ProactivityDimension::WeeklyReview,
        ]
    }
}

impl std::str::FromStr for ProactivityDimension {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_lowercase().as_str() {
            "schedule_reminders" => ProactivityDimension::ScheduleReminders,
            "life_suggestions" => ProactivityDimension::LifeSuggestions,
            "status_checkins" => ProactivityDimension::StatusCheckins,
            "weekly_review" => ProactivityDimension::WeeklyReview,
            other => return Err(CoreError::Invalid(format!("unknown dimension {other:?}"))),
        })
    }
}

/// Per-dimension proactivity levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityConfig {
    levels: HashMap<ProactivityDimension, ProactivityLevel>,
}

impl ProactivityConfig {
    /// Sensible defaults: remind about the schedule like a secretary, stay
    /// passive elsewhere until the user opts in.
    pub fn defaults() -> Self {
        use ProactivityDimension::*;
        use ProactivityLevel::*;
        ProactivityConfig {
            levels: HashMap::from([
                (ScheduleReminders, Secretary),
                (LifeSuggestions, Passive),
                (StatusCheckins, Passive),
                (WeeklyReview, Passive),
            ]),
        }
    }

    pub fn level(&self, dim: ProactivityDimension) -> ProactivityLevel {
        self.levels
            .get(&dim)
            .copied()
            .unwrap_or(ProactivityLevel::Passive)
    }

    pub fn set(&mut self, dim: ProactivityDimension, level: ProactivityLevel) {
        self.levels.insert(dim, level);
    }

    /// The F7 hard invariant: regardless of any dimension's level, the agent may
    /// only ever auto-execute `Safe` actions. `Sensitive` needs confirmation
    /// (unless the user opted into "notify-only" per dimension elsewhere), and
    /// `Dangerous` can **never** be auto-executed — full stop.
    pub fn may_autoexecute(&self, risk: RiskLevel) -> bool {
        matches!(risk, RiskLevel::Safe)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

impl Default for ProactivityConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let mut c = ProactivityConfig::defaults();
        assert_eq!(
            c.level(ProactivityDimension::ScheduleReminders),
            ProactivityLevel::Secretary
        );
        assert_eq!(
            c.level(ProactivityDimension::WeeklyReview),
            ProactivityLevel::Passive
        );
        c.set(ProactivityDimension::WeeklyReview, ProactivityLevel::Butler);
        assert_eq!(
            c.level(ProactivityDimension::WeeklyReview),
            ProactivityLevel::Butler
        );
    }

    #[test]
    fn butler_never_autoexecutes_dangerous() {
        let mut c = ProactivityConfig::defaults();
        // Crank everything to butler.
        for d in ProactivityDimension::all() {
            c.set(d, ProactivityLevel::Butler);
        }
        assert!(c.may_autoexecute(RiskLevel::Safe));
        assert!(!c.may_autoexecute(RiskLevel::Sensitive));
        assert!(!c.may_autoexecute(RiskLevel::Dangerous));
    }

    #[test]
    fn round_trips_json() {
        let c = ProactivityConfig::defaults();
        assert_eq!(
            ProactivityConfig::from_json(&c.to_json().unwrap()).unwrap(),
            c
        );
    }

    #[test]
    fn parse_level_and_dimension() {
        assert_eq!(
            "butler".parse::<ProactivityLevel>().unwrap(),
            ProactivityLevel::Butler
        );
        assert_eq!(
            "weekly_review".parse::<ProactivityDimension>().unwrap(),
            ProactivityDimension::WeeklyReview
        );
        assert!("nope".parse::<ProactivityLevel>().is_err());
    }
}

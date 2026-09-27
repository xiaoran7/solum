//! `source` -- where an input came from, and the three questions every consumer asks of it in the same order: can it be parsed back, is it AI-eligible by default, and what is it called in the UI. Kept with its `impl` so the invariants the three methods share stay readable in one place.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::classify::CaptureSensitivity;
use super::*;

/// 采集入口的类型学。列不列进 [`capture_connectors`] 是另一回事（见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    Manual,
    Notification,
    Share,
    Calendar,
    Health,
    Email,
    Soulous,
    Location,
}

impl CaptureSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Notification => "notification",
            Self::Share => "share",
            Self::Calendar => "calendar",
            Self::Health => "health",
            Self::Email => "email",
            Self::Soulous => "soulous",
            Self::Location => "location",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "manual" | "conversation" => Self::Manual,
            "notification" => Self::Notification,
            "share" | "system_share" => Self::Share,
            "calendar" => Self::Calendar,
            "health" => Self::Health,
            "email" => Self::Email,
            "soulous" | "official_api" => Self::Soulous,
            "location" => Self::Location,
            _ => return None,
        })
    }

    pub fn allows_ai_by_default(self) -> bool {
        matches!(self, Self::Manual | Self::Share)
    }

    pub fn sensitivity(self) -> CaptureSensitivity {
        match self {
            Self::Manual | Self::Share => CaptureSensitivity::ContentDependent,
            Self::Notification | Self::Calendar | Self::Soulous => CaptureSensitivity::High,
            Self::Health | Self::Email | Self::Location => CaptureSensitivity::VeryHigh,
        }
    }
}

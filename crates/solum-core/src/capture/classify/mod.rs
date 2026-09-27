//! `classify` -- how sensitive an entry is, whether the user has acted on it, whether it works on this device at all, and which platforms could support it. Split from the catalog because these are the vocabulary the catalog is written in, and a reader checking one connector's availability needs all four types before they need any connector.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSensitivity {
    ContentDependent,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Pending,
    Accepted,
    Dismissed,
    Merged,
    Expired,
}

/// 入口当前**在这台设备上**能不能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureAvailability {
    /// 现在就能用。
    Ready,
    /// 机制成立，但本仓还没接这条线。
    Connector,
    /// 需要另一端配合（浏览器扩展等）。
    Companion,
    /// 这个平台上不可能有。
    Unsupported,
}

impl CaptureAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Connector => "connector",
            Self::Companion => "companion",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformSupport {
    Available,
    PermissionRequired,
    ConfigurationRequired,
    Planned,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CapturePlatformMatrix {
    pub android: PlatformSupport,
    pub ios: PlatformSupport,
    pub harmonyos: PlatformSupport,
}

pub(super) fn platform_matrix(source: CaptureSource) -> CapturePlatformMatrix {
    use PlatformSupport::{
        Available, ConfigurationRequired, PermissionRequired, Planned, Unsupported,
    };
    match source {
        CaptureSource::Manual => CapturePlatformMatrix {
            android: Available,
            ios: Available,
            harmonyos: Available,
        },
        CaptureSource::Notification => CapturePlatformMatrix {
            android: PermissionRequired,
            ios: Planned,
            harmonyos: Unsupported,
        },
        CaptureSource::Share => CapturePlatformMatrix {
            android: Available,
            ios: Planned,
            harmonyos: Available,
        },
        CaptureSource::Calendar => CapturePlatformMatrix {
            android: PermissionRequired,
            ios: Planned,
            harmonyos: PermissionRequired,
        },
        CaptureSource::Health => CapturePlatformMatrix {
            android: PermissionRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Email => CapturePlatformMatrix {
            android: ConfigurationRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Soulous => CapturePlatformMatrix {
            android: ConfigurationRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Location => CapturePlatformMatrix {
            android: Planned,
            ios: Planned,
            harmonyos: Planned,
        },
    }
}

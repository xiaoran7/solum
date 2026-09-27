//! `config` -- what counts as a notification worth keeping, and the rules that decide it: the capture mode (all / allowlist / not-allowed), the config those two knobs live in, the priority rules, and the small enums (`CaptureLane`, `CaptureState`) the store persists. `normalized()` is the fail-closed door -- an allowlist that cannot be satisfied must reject, never widen.
//!
//! Part of the `notification_intelligence` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub const MIN_BATCH_INTERVAL_MINUTES: u16 = 15;
pub const MAX_BATCH_INTERVAL_MINUTES: u16 = 30;
pub const DEDUP_WINDOW_MINUTES: i64 = 10;

fn default_batch_interval_minutes() -> u16 {
    MIN_BATCH_INTERVAL_MINUTES
}

/// Which installed apps may enter the device-local notification pipeline.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCaptureMode {
    /// Capture every third-party app (the product default). Android's own
    /// notification-access permission remains the outer, user-controlled gate.
    #[default]
    All,
    /// Capture only packages explicitly listed in `allowed_packages`.
    Allowlist,
}

/// Device-local control plane for third-party notification capture. Capture is
/// all-apps by default; the user can switch to an explicit allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationIntelligenceConfig {
    #[serde(default)]
    pub capture_mode: NotificationCaptureMode,
    /// Apps whose notifications may be **read** in allowlist mode.
    #[serde(default)]
    pub allowed_packages: Vec<String>,
    /// Apps whose notifications may additionally **write to the calendar**
    /// without asking, when local deterministic parsing finds a time.
    ///
    /// Deliberately a second, separate list. "I let you read this app's
    /// notifications" and "I let this app put things in my calendar" are
    /// different grants, and the UI only ever asked for the first one — so
    /// treating capture consent as write consent was consent the user never
    /// gave. Empty by default: capture-only is the safe baseline, and an app
    /// has to be promoted explicitly and individually.
    ///
    /// Determinism does not fix this. It makes the *content* traceable to the
    /// source text, which is what stops a model inventing an appointment; it
    /// says nothing about whether the source itself deserves write access.
    #[serde(default)]
    pub auto_event_packages: Vec<String>,
    #[serde(default = "default_batch_interval_minutes")]
    pub batch_interval_minutes: u16,
    #[serde(default)]
    pub filter_rules: Vec<NotificationFilterRule>,
}

/// Most events one app may auto-create in a rolling day before Solum stops
/// trusting it and routes the rest to review.
///
/// This is a **containment** limit, not an authorization mechanism: the grant
/// above is what authorizes writes; this only bounds the damage when a granted
/// app misbehaves or the user's expectation of "occasionally" turns out to be
/// wrong. A normal app posts a handful of schedulable notifications a day.
pub const MAX_AUTO_EVENTS_PER_APP_PER_DAY: usize = 20;

impl Default for NotificationIntelligenceConfig {
    fn default() -> Self {
        Self {
            capture_mode: NotificationCaptureMode::All,
            allowed_packages: Vec::new(),
            auto_event_packages: Vec::new(),
            batch_interval_minutes: default_batch_interval_minutes(),
            filter_rules: Vec::new(),
        }
    }
}

impl NotificationIntelligenceConfig {
    pub fn normalized(mut self) -> Result<Self> {
        self.allowed_packages = self
            .allowed_packages
            .into_iter()
            .map(|pkg| pkg.trim().to_string())
            .filter(|pkg| !pkg.is_empty())
            .collect();
        self.allowed_packages.sort();
        self.allowed_packages.dedup();
        self.auto_event_packages = self
            .auto_event_packages
            .into_iter()
            .map(|pkg| pkg.trim().to_string())
            .filter(|pkg| !pkg.is_empty())
            .collect();
        self.auto_event_packages.sort();
        self.auto_event_packages.dedup();
        // Auto-create implies capture: an app that cannot be read cannot write.
        // Enforced here rather than trusted from the caller, so a hand-edited
        // config or a stale UI cannot produce the nonsensical combination.
        if self.capture_mode == NotificationCaptureMode::Allowlist {
            self.auto_event_packages
                .retain(|pkg| self.allowed_packages.contains(pkg));
        }
        if !(MIN_BATCH_INTERVAL_MINUTES..=MAX_BATCH_INTERVAL_MINUTES)
            .contains(&self.batch_interval_minutes)
        {
            return Err(CoreError::Invalid(format!(
                "通知批处理间隔必须在 {MIN_BATCH_INTERVAL_MINUTES}–{MAX_BATCH_INTERVAL_MINUTES} 分钟之间"
            )));
        }
        for rule in &mut self.filter_rules {
            rule.normalize()?;
        }
        self.filter_rules.sort_by(|a, b| a.id.cmp(&b.id));
        self.filter_rules.dedup_by(|a, b| a.id == b.id);
        Ok(self)
    }

    /// May this app's notifications be read at all?
    pub fn allows(&self, package_name: &str) -> bool {
        self.capture_mode == NotificationCaptureMode::All
            || self.allowed_packages.iter().any(|pkg| pkg == package_name)
    }

    /// May this app's notifications create calendar entries without asking?
    /// Strictly narrower than [`Self::allows`].
    pub fn allows_auto_event(&self, package_name: &str) -> bool {
        self.allows(package_name)
            && self
                .auto_event_packages
                .iter()
                .any(|pkg| pkg == package_name)
    }
}

/// A confirmed local filtering rule. It is a convenience rule only: every
/// filtered capture remains visible and can be restored from F12.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationFilterRule {
    pub id: String,
    pub pattern: String,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub matcher: NotificationMatchKind,
    #[serde(default)]
    pub reason: String,
}

impl NotificationFilterRule {
    pub fn normalize(&mut self) -> Result<()> {
        self.id = self.id.trim().to_string();
        self.pattern = self.pattern.trim().to_string();
        self.package_name = self
            .package_name
            .as_deref()
            .map(str::trim)
            .filter(|pkg| !pkg.is_empty())
            .map(ToOwned::to_owned);
        self.reason = self.reason.trim().to_string();
        if self.id.is_empty() || self.pattern.is_empty() {
            return Err(CoreError::Invalid("通知过滤规则需要 id 和模式".into()));
        }
        if self.pattern.chars().count() > 160 {
            return Err(CoreError::Invalid("通知过滤规则模式不能超过 160 字".into()));
        }
        if self.matcher == NotificationMatchKind::Regex {
            RegexBuilder::new(&self.pattern)
                .case_insensitive(true)
                .build()
                .map_err(|e| CoreError::Invalid(format!("通知过滤正则无效: {e}")))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureLane {
    Urgent,
    Batch,
}

impl CaptureLane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::Batch => "batch",
        }
    }
}

impl std::str::FromStr for CaptureLane {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "urgent" => Ok(Self::Urgent),
            "batch" => Ok(Self::Batch),
            _ => Err(CoreError::Invalid(format!("未知通知车道: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Queued,
    EventCreated,
    Filtered,
    Deduplicated,
    NeedsReview,
    Resolved,
}

impl CaptureState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::EventCreated => "event_created",
            Self::Filtered => "filtered",
            Self::Deduplicated => "deduplicated",
            Self::NeedsReview => "needs_review",
            Self::Resolved => "resolved",
        }
    }
}

impl std::str::FromStr for CaptureState {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "queued" => Ok(Self::Queued),
            "event_created" => Ok(Self::EventCreated),
            "filtered" => Ok(Self::Filtered),
            "deduplicated" => Ok(Self::Deduplicated),
            "needs_review" => Ok(Self::NeedsReview),
            "resolved" => Ok(Self::Resolved),
            _ => Err(CoreError::Invalid(format!("未知通知处理状态: {s}"))),
        }
    }
}

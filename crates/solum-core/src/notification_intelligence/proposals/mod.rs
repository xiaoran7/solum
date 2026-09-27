//! `proposals` -- the decide-then-ask half: filter and action proposals, the starter rules new installs begin with, and `decide_intake`. Nothing here acts on a notification -- the most it does is write a row the user has to confirm. `NotificationActionKind` is deliberately two variants (cancel, reschedule) because those are the only two intents the local calendar lookup can resolve unambiguously.
//!
//! Part of the `notification_intelligence` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationFilterProposal {
    pub id: Option<i64>,
    pub package_name: Option<String>,
    pub pattern: String,
    pub matcher: NotificationMatchKind,
    pub reason: String,
    pub state: FilterProposalState,
    pub created_at: NaiveDateTime,
}

/// The only existing-record changes an LLM may describe. This is deliberately
/// an intent, not an executable action: it contains a human title hint but
/// never a database id. Rust resolves the real local target before F12 can
/// offer a confirmation tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationActionKind {
    CancelEvent,
    RescheduleEvent,
}

impl NotificationActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CancelEvent => "cancel_event",
            Self::RescheduleEvent => "reschedule_event",
        }
    }
}

impl std::str::FromStr for NotificationActionKind {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "cancel_event" => Ok(Self::CancelEvent),
            "reschedule_event" => Ok(Self::RescheduleEvent),
            _ => Err(CoreError::Invalid(format!("未知通知动作意图: {value}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionProposalState {
    Pending,
    Accepted,
    Dismissed,
    /// Expired, or the event it described has since changed or been deleted.
    /// Terminal: the card can no longer be acted on, only re-proposed.
    Stale,
}

impl ActionProposalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
            Self::Stale => "stale",
        }
    }
}

impl std::str::FromStr for ActionProposalState {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "dismissed" => Ok(Self::Dismissed),
            "stale" => Ok(Self::Stale),
            _ => Err(CoreError::Invalid(format!("未知通知动作提议状态: {value}"))),
        }
    }
}

/// A Rust-resolved, still-pending human confirmation. `event_id` appears only
/// after local matching and is never supplied by the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationActionProposal {
    pub id: Option<i64>,
    pub capture_id: i64,
    pub kind: NotificationActionKind,
    pub event_id: i64,
    pub event_title: String,
    /// Stable identity of the event as resolved at proposal time. `event_id`
    /// alone is a row number: it says *which row*, not *which event*, and a
    /// row can be deleted and its id reused. Checked before the action runs.
    pub event_guid: String,
    /// The event's start when the user was shown this card. Together with the
    /// guid this is the snapshot the confirmation is bound to — if either has
    /// changed (the user edited it, sync merged a peer's change), the card is
    /// describing a state that no longer exists and must not be acted on.
    pub event_start: NaiveDateTime,
    pub new_start: Option<NaiveDateTime>,
    pub reason: String,
    pub state: ActionProposalState,
    pub created_at: NaiveDateTime,
}

/// A pending action card goes stale this long after it was created. Cancelling
/// or moving a real appointment on the strength of a day-old notification —
/// against an event that has had a day to change — is not a confirmation the
/// user meaningfully gave.
pub const ACTION_PROPOSAL_TTL_HOURS: i64 = 12;

/// Untrusted LLM data before local target resolution. It intentionally cannot
/// carry a database id or any arbitrary command/argument surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationActionIntent {
    pub kind: NotificationActionKind,
    pub target: String,
    pub new_start: Option<NaiveDateTime>,
    pub reason: String,
}

/// Deterministic routing decision, evaluated before any LLM call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeDecision {
    NotAllowed,
    Filtered { rule_id: String, reason: String },
    Lane(CaptureLane),
}

pub fn decide_intake(
    capture: &NotificationCapture,
    config: &NotificationIntelligenceConfig,
    priority_rules: &[NotificationPriorityRule],
) -> IntakeDecision {
    if !config.allows(&capture.package_name) {
        return IntakeDecision::NotAllowed;
    }
    let text = capture.text();
    if let Some(rule) = config.filter_rules.iter().find(|rule| {
        rule_matches(
            rule.package_name.as_deref(),
            rule.matcher,
            &rule.pattern,
            &capture.package_name,
            &text,
        )
    }) {
        return IntakeDecision::Filtered {
            rule_id: rule.id.clone(),
            reason: rule.reason.clone(),
        };
    }
    let urgent = priority_rules.iter().any(|rule| {
        rule.priority > 0
            && rule_matches(
                rule.package_name.as_deref(),
                rule.matcher,
                &rule.pattern,
                &capture.package_name,
                &text,
            )
    });
    IntakeDecision::Lane(if urgent {
        CaptureLane::Urgent
    } else {
        CaptureLane::Batch
    })
}

fn rule_matches(
    package_scope: Option<&str>,
    matcher: NotificationMatchKind,
    pattern: &str,
    package_name: &str,
    text: &str,
) -> bool {
    if package_scope.is_some_and(|scope| scope != package_name) {
        return false;
    }
    match matcher {
        NotificationMatchKind::Substring => text.to_lowercase().contains(&pattern.to_lowercase()),
        NotificationMatchKind::Regex => RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map(|regex| regex.is_match(text))
            .unwrap_or(false),
    }
}

/// Stable, dependency-free FNV-1a fingerprint for the exact deterministic
/// dedup key: normalized package + normalized content. This is not a security
/// digest; it is only an inexpensive local equality key.
pub fn content_hash(package_name: &str, text: &str) -> String {
    let normalized = format!(
        "{}\u{1f}{}",
        package_name.trim(),
        text.split_whitespace().collect::<String>()
    )
    .to_lowercase();
    let hash = normalized
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}

/// Declarative starter preset specification for applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppStarterRuleSpec {
    pub package_pattern: &'static str,
    pub patterns: &'static [&'static str],
}

/// Known application-specific starter rule registry.
/// Extracted as a declarative policy layer so domain logic does not hard-code
/// proprietary application packages into business branches.
pub static APP_STARTER_SPECS: &[AppStarterRuleSpec] = &[
    AppStarterRuleSpec {
        package_pattern: "com.tencent.mm",
        patterns: &["@我", "@所有人"],
    },
    AppStarterRuleSpec {
        package_pattern: "com.tencent.mobileqq",
        patterns: &["@我", "有人@我"],
    },
    AppStarterRuleSpec {
        package_pattern: "com.alibaba.android.rimet",
        patterns: &["DING", "紧急", "@"],
    },
];

/// Fallback starter patterns for any other allow-listed application.
pub static DEFAULT_STARTER_PATTERNS: &[&str] = &["@我", "@全体成员"];

/// Resolve starter priority pattern list for a package without hardcoded match branching.
pub fn starter_patterns_for_package(package_name: &str) -> &'static [&'static str] {
    APP_STARTER_SPECS
        .iter()
        .find(|spec| spec.package_pattern == package_name)
        .map(|spec| spec.patterns)
        .unwrap_or(DEFAULT_STARTER_PATTERNS)
}

/// Package-specific best-effort starter rules. They are copied when an app is
/// allow-listed, so users can edit or delete them; they are not an authority.
pub fn priority_presets(package_name: &str) -> Vec<NotificationPriorityRule> {
    let patterns = starter_patterns_for_package(package_name);
    patterns
        .iter()
        .map(|pattern| NotificationPriorityRule {
            id: format!("preset:{package_name}:{pattern}"),
            pattern: (*pattern).to_string(),
            package_name: Some(package_name.to_string()),
            priority: 1,
            matcher: NotificationMatchKind::Substring,
        })
        .collect()
}

//! `types` -- the wire protocol itself: the version and action whitelist the validator enforces, the component catalog the renderer knows how to draw, and the envelope that carries one component tree plus its actions.
//!
//! Part of the `genui` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub const UI_VERSION: u32 = 1;

/// Commands a generated action may request (§3.9 动作白名单 v1). All of them
/// map to existing orchestrator capabilities — GenUI adds no new write path.
pub const ALLOWED_ACTIONS: &[&str] = &[
    "ingest",
    "reminder_dismiss",
    "reminder_fire",
    "suggestion_set",
    "checkin_answer",
    "proactivity_set",
    "persona_import_save",
    "guard_request",
    "event_reschedule",
    "event_cancel",
    "routine_set_active",
];

/// The subset the *cloud reasoner* is told about. The LLM only sees data it
/// could plausibly act on (the current utterance); it has no real row ids, so
/// id-bearing actions stay with the offline template builders.
pub const LLM_ACTIONS: &[&str] = &["ingest", "checkin_answer"];

/// Hard caps: an envelope outside these bounds is rejected wholesale.
pub(super) const MAX_COMPONENTS: usize = 12;
pub(super) const MAX_BUTTONS: usize = 4;
pub(super) const MAX_CHOICE_OPTIONS: usize = 6;
pub(super) const MAX_FORM_FIELDS: usize = 8;
pub(super) const MAX_TEXT_LEN: usize = 4000;
pub(super) const MAX_LABEL_LEN: usize = 40;

/// A requested action: a whitelisted command plus a JSON-object argument bag.
/// The frontend forwards it verbatim to the matching Tauri command, which
/// re-parses and re-validates the args like any other invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAction {
    pub command: String,
    #[serde(default = "empty_args")]
    pub args: serde_json::Value,
}

pub(super) fn empty_args() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

/// Visual weight of a button. The renderer maps these to its existing CSS —
/// the description cannot pick colors or sizes (防 UI 注入：不能把危险动作画成主按钮).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    #[default]
    Normal,
    Primary,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiButton {
    pub label: String,
    pub action: UiAction,
    #[serde(default)]
    pub style: ButtonStyle,
}

/// One editable field in a [`UiComponent::Form`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiField {
    /// Key in the submitted args object.
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    /// Pre-filled value (stringly typed; `toggle` uses "true"/"false").
    #[serde(default)]
    pub value: Option<String>,
    /// Required for `select`, forbidden otherwise.
    #[serde(default)]
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Select,
    Datetime,
    Toggle,
}

/// The v1 component catalog (§3.9): seven kinds, nothing else parses.
/// `deny_unknown_fields` keeps the LLM from smuggling extra keys past review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiComponent {
    /// Plain paragraph text — the reply body. No markup of any kind.
    Text { content: String },
    /// Display snapshot of an extracted event (F1 result preview).
    EventCard {
        title: String,
        kind: String,
        start: String,
        #[serde(default)]
        location: Option<String>,
    },
    /// Display snapshot of a scheduled reminder.
    ReminderCard {
        event_title: String,
        fire_at: String,
        status: String,
    },
    /// Display snapshot of a daily fixed reminder.
    RoutineCard {
        title: String,
        time_of_day: String,
        active: bool,
    },
    /// Display snapshot of a suggestion (F10).
    SuggestionCard { content: String, kind: String },
    /// A row of 1–4 action buttons.
    ButtonGroup { buttons: Vec<UiButton> },
    /// In-place editing of a few fields; submit fires one action with the
    /// field values merged into its args.
    Form {
        fields: Vec<UiField>,
        submit: UiButton,
    },
    /// Single-pick quick answers (tap = submit). 2–6 options.
    Choice { options: Vec<UiButton> },
}

/// What the agent hands the renderer: a version header plus a component list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiEnvelope {
    pub version: u32,
    pub components: Vec<UiComponent>,
}

impl UiEnvelope {
    pub fn new(components: Vec<UiComponent>) -> Self {
        UiEnvelope {
            version: UI_VERSION,
            components,
        }
    }

    /// The concatenated `Text` contents — the plain-text degradation of this
    /// envelope, and what callers use for `message` fields / logs.
    pub fn text_summary(&self) -> String {
        self.components
            .iter()
            .filter_map(|c| match c {
                UiComponent::Text { content } => Some(content.trim()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

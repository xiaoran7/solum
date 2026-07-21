//! Generative UI protocol (F18, ARCHITECTURE.md §3.9) — the component catalog,
//! the action whitelist, and the defensive validator.
//!
//! Four hard rules, mirrored from the architecture doc:
//! 1. A UI description is **data, not code**: a JSON component tree constrained
//!    to this catalog. No scripts, no styling, no free-form layout.
//! 2. Generated UI can only *request* actions. Every action must name a
//!    whitelisted command with schema-checked args; the Rust command layer
//!    re-validates everything, and `dangerous` work still goes through the
//!    §3.3 guard — a generated confirm button is an entrance, not a token.
//! 3. Anything that fails validation degrades to plain text (F16): callers use
//!    [`parse_envelope`], which returns `None` instead of erroring.
//! 4. Envelopes are never persisted (对话即焚): they are one-shot renders of
//!    durable data. What lands in storage is the *result* of actions, through
//!    the existing tables/audit/ledger. See docs/MISC.md 2026-07-14.
//!
//! The renderer (solum-app's frontend) only understands this protocol and does
//! not care whether an envelope came from the offline template builders below
//! or from the cloud reasoner (`llm::chat_reply_ui`).

use serde::{Deserialize, Serialize};

use crate::brief::Brief;
use crate::model::{fmt_ts, Event, Notification};
use crate::persona::PersonaDraft;
use crate::routine::Routine;
use crate::suggest::Suggestion;

/// Protocol version. Bump only with a renderer upgrade; the validator rejects
/// anything else so an old renderer never sees components it can't draw.
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
const MAX_COMPONENTS: usize = 12;
const MAX_BUTTONS: usize = 4;
const MAX_CHOICE_OPTIONS: usize = 6;
const MAX_FORM_FIELDS: usize = 8;
const MAX_TEXT_LEN: usize = 4000;
const MAX_LABEL_LEN: usize = 40;

/// A requested action: a whitelisted command plus a JSON-object argument bag.
/// The frontend forwards it verbatim to the matching Tauri command, which
/// re-parses and re-validates the args like any other invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAction {
    pub command: String,
    #[serde(default = "empty_args")]
    pub args: serde_json::Value,
}

fn empty_args() -> serde_json::Value {
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

// ---- validation --------------------------------------------------------------

/// Validate an envelope against the catalog bounds and the action whitelist.
/// `allowed` narrows the command set further (e.g. [`LLM_ACTIONS`] for
/// cloud-generated envelopes).
pub fn validate(env: &UiEnvelope, allowed: &[&str]) -> Result<(), String> {
    if env.version != UI_VERSION {
        return Err(format!("不支持的协议版本 {}", env.version));
    }
    if env.components.is_empty() || env.components.len() > MAX_COMPONENTS {
        return Err(format!("组件数量必须在 1..={MAX_COMPONENTS}"));
    }
    for c in &env.components {
        validate_component(c, allowed)?;
    }
    Ok(())
}

fn validate_component(c: &UiComponent, allowed: &[&str]) -> Result<(), String> {
    match c {
        UiComponent::Text { content } => {
            if content.trim().is_empty() || content.len() > MAX_TEXT_LEN {
                return Err("text.content 为空或过长".into());
            }
        }
        UiComponent::EventCard {
            title, kind, start, ..
        } => {
            if title.trim().is_empty() || kind.trim().is_empty() || start.trim().is_empty() {
                return Err("event_card 缺少 title/kind/start".into());
            }
        }
        UiComponent::ReminderCard {
            event_title,
            fire_at,
            ..
        } => {
            if event_title.trim().is_empty() || fire_at.trim().is_empty() {
                return Err("reminder_card 缺少 event_title/fire_at".into());
            }
        }
        UiComponent::RoutineCard {
            title, time_of_day, ..
        } => {
            if title.trim().is_empty() || time_of_day.trim().is_empty() {
                return Err("routine_card 缺少 title/time_of_day".into());
            }
        }
        UiComponent::SuggestionCard { content, .. } => {
            if content.trim().is_empty() {
                return Err("suggestion_card.content 为空".into());
            }
        }
        UiComponent::ButtonGroup { buttons } => {
            if buttons.is_empty() || buttons.len() > MAX_BUTTONS {
                return Err(format!("button_group 按钮数必须在 1..={MAX_BUTTONS}"));
            }
            for b in buttons {
                validate_button(b, allowed)?;
            }
        }
        UiComponent::Form { fields, submit } => {
            if fields.is_empty() || fields.len() > MAX_FORM_FIELDS {
                return Err(format!("form 字段数必须在 1..={MAX_FORM_FIELDS}"));
            }
            for f in fields {
                if f.name.trim().is_empty() || f.label.trim().is_empty() {
                    return Err("form 字段缺少 name/label".into());
                }
                match f.kind {
                    FieldKind::Select if f.options.as_ref().is_none_or(|o| o.is_empty()) => {
                        return Err(format!("select 字段 {} 缺少 options", f.name));
                    }
                    _ => {}
                }
            }
            validate_button(submit, allowed)?;
        }
        UiComponent::Choice { options } => {
            if options.len() < 2 || options.len() > MAX_CHOICE_OPTIONS {
                return Err(format!("choice 选项数必须在 2..={MAX_CHOICE_OPTIONS}"));
            }
            for b in options {
                validate_button(b, allowed)?;
            }
        }
    }
    Ok(())
}

fn validate_button(b: &UiButton, allowed: &[&str]) -> Result<(), String> {
    if b.label.trim().is_empty() || b.label.chars().count() > MAX_LABEL_LEN {
        return Err("按钮 label 为空或过长".into());
    }
    validate_action(&b.action, allowed)
}

/// The action gate: whitelisted command + per-command args schema. This runs
/// again in the shell before dispatch — the whitelist narrows the entrance,
/// each Tauri command's own parsing stays the real parameter check.
pub fn validate_action(a: &UiAction, allowed: &[&str]) -> Result<(), String> {
    if !allowed.contains(&a.command.as_str()) {
        return Err(format!("动作 {} 不在白名单", a.command));
    }
    let args = a
        .args
        .as_object()
        .ok_or_else(|| format!("动作 {} 的 args 必须是对象", a.command))?;
    let need_str = |k: &str| -> Result<(), String> {
        match args.get(k).and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => Ok(()),
            _ => Err(format!("动作 {} 缺少字符串参数 {k}", a.command)),
        }
    };
    let need_int = |k: &str| -> Result<(), String> {
        args.get(k)
            .and_then(|v| v.as_i64())
            .map(|_| ())
            .ok_or_else(|| format!("动作 {} 缺少整数参数 {k}", a.command))
    };
    let need_bool = |k: &str| -> Result<(), String> {
        args.get(k)
            .and_then(|v| v.as_bool())
            .map(|_| ())
            .ok_or_else(|| format!("动作 {} 缺少布尔参数 {k}", a.command))
    };
    match a.command.as_str() {
        "ingest" | "checkin_answer" => need_str("text"),
        "reminder_dismiss" | "event_cancel" => need_int("id"),
        "event_reschedule" => {
            need_int("id")?;
            need_str("start")
        }
        "routine_set_active" => {
            need_int("id")?;
            need_bool("active")
        }
        "reminder_fire" => Ok(()),
        "suggestion_set" => {
            need_int("id")?;
            match args.get("status").and_then(|v| v.as_str()) {
                Some("accepted") | Some("dismissed") => Ok(()),
                _ => Err("suggestion_set.status 必须是 accepted|dismissed".into()),
            }
        }
        "proactivity_set" => {
            need_str("dimension")?;
            match args.get("level").and_then(|v| v.as_str()) {
                Some("passive") | Some("secretary") | Some("butler") => Ok(()),
                _ => Err("proactivity_set.level 必须是 passive|secretary|butler".into()),
            }
        }
        // Args come from the form fields the user just edited; the command's
        // own normalization (PersonaDraft::normalized) is the real check.
        "persona_import_save" => Ok(()),
        "guard_request" => {
            need_str("tool")?;
            need_str("args")
        }
        other => Err(format!("动作 {other} 无参数校验规则")), // unreachable given the whitelist
    }
}

// ---- defensive parsing (the F16 degradation seam) ------------------------------

/// Parse a (possibly LLM-authored) envelope. Fences and stray prose are
/// stripped like `llm::parse_event_json`; strict deserialization plus
/// [`validate`] gate the result. Any failure → `None`, caller falls back to
/// plain text — a generated envelope must never be able to fail a conversation.
pub fn parse_envelope(raw: &str, allowed: &[&str]) -> Option<UiEnvelope> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = cleaned.find('{')?;
    let end = cleaned.rfind('}')?;
    let env: UiEnvelope = serde_json::from_str(&cleaned[start..=end]).ok()?;
    validate(&env, allowed).ok()?;
    Some(env)
}

// ---- offline template builders (F16: the rules-based path) ----------------------
//
// Each builder returns an envelope that passes `validate(_, ALLOWED_ACTIONS)`
// by construction (debug_assert'd); ids come from persisted rows, never made up.

fn checked(env: UiEnvelope) -> UiEnvelope {
    debug_assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
    env
}

/// After `ingest` extracts and persists an event: the confirmation card. The
/// event is already saved (existing v1 behavior); the buttons let the user act
/// on the derived reminders without leaving the conversation.
pub fn event_ingested(ev: &Event, notifs: &[Notification]) -> UiEnvelope {
    let mut components = vec![UiComponent::EventCard {
        title: ev.title.clone(),
        kind: ev.kind.as_str().to_string(),
        start: fmt_ts(&ev.start),
        location: ev.location.clone(),
    }];
    let buttons: Vec<UiButton> = notifs
        .iter()
        .filter_map(|n| {
            let id = n.id?;
            Some(UiButton {
                // 「提前0」读起来别扭：零提前量的就是到点提醒本身。
                label: if matches!(n.lead_label.as_str(), "0" | "0m") {
                    "取消这条到点提醒".to_string()
                } else {
                    format!("取消提前{}的提醒", n.lead_label)
                },
                action: UiAction {
                    command: "reminder_dismiss".into(),
                    args: serde_json::json!({ "id": id }),
                },
                style: ButtonStyle::Normal,
            })
        })
        .take(MAX_BUTTONS)
        .collect();
    if !buttons.is_empty() {
        components.push(UiComponent::ButtonGroup { buttons });
    }
    checked(UiEnvelope::new(components))
}

/// After an explicit daily phrase creates a routine, expose the durable result
/// in the conversation and offer only the reversible pause action. The id is
/// read from the just-persisted routine; a transient routine is never rendered
/// as actionable UI.
pub fn routine_ingested(routine: &Routine) -> Option<UiEnvelope> {
    let id = routine.id?;
    Some(checked(UiEnvelope::new(vec![
        UiComponent::RoutineCard {
            title: routine.title.clone(),
            time_of_day: routine.time_of_day.clone(),
            active: routine.active,
        },
        UiComponent::ButtonGroup {
            buttons: vec![UiButton {
                label: "暂停此固定提醒".into(),
                action: UiAction {
                    command: "routine_set_active".into(),
                    args: serde_json::json!({ "id": id, "active": false }),
                },
                style: ButtonStyle::Normal,
            }],
        },
    ])))
}

/// Short "「title」 07-21 15:00" label for pick/confirm buttons, kept inside
/// [`MAX_LABEL_LEN`] by truncating the title, never the time.
fn event_pick_label(ev: &Event) -> String {
    let title: String = ev.title.chars().take(12).collect();
    let ellipsis = if ev.title.chars().count() > 12 {
        "…"
    } else {
        ""
    };
    format!("「{title}{ellipsis}」 {}", ev.start.format("%m-%d %H:%M"))
}

/// Cancel confirmation (F1 扩展): deletion never happens straight off an
/// utterance — the user must tap the (danger-styled) button naming the exact
/// event. Multiple candidates render one button each: the tap doubles as the
/// disambiguation. Only persisted events (with ids) render; `None` when
/// nothing is actionable.
pub fn cancel_confirm(prompt: &str, candidates: &[Event]) -> Option<UiEnvelope> {
    let buttons: Vec<UiButton> = candidates
        .iter()
        .filter_map(|ev| {
            let id = ev.id?;
            Some(UiButton {
                label: event_pick_label(ev),
                action: UiAction {
                    command: "event_cancel".into(),
                    args: serde_json::json!({ "id": id }),
                },
                style: ButtonStyle::Danger,
            })
        })
        .take(MAX_BUTTONS)
        .collect();
    if buttons.is_empty() {
        return None;
    }
    Some(checked(UiEnvelope::new(vec![
        UiComponent::Text {
            content: prompt.to_string(),
        },
        UiComponent::ButtonGroup { buttons },
    ])))
}

/// Reschedule disambiguation: several events matched, each button carries the
/// event id plus its own pre-composed new start (the "keep original date /
/// clock" fallback differs per event). `None` unless ≥2 actionable candidates
/// — a unique match is applied directly, no envelope needed.
pub fn reschedule_pick(
    prompt: &str,
    candidates: &[(Event, chrono::NaiveDateTime)],
) -> Option<UiEnvelope> {
    let options: Vec<UiButton> = candidates
        .iter()
        .filter_map(|(ev, new_start)| {
            let id = ev.id?;
            Some(UiButton {
                label: event_pick_label(ev),
                action: UiAction {
                    command: "event_reschedule".into(),
                    args: serde_json::json!({ "id": id, "start": fmt_ts(new_start) }),
                },
                style: ButtonStyle::Normal,
            })
        })
        .take(MAX_CHOICE_OPTIONS)
        .collect();
    if options.len() < 2 {
        return None;
    }
    Some(checked(UiEnvelope::new(vec![
        UiComponent::Text {
            content: prompt.to_string(),
        },
        UiComponent::Choice { options },
    ])))
}

/// A status check-in (F3/F4) as tap-to-answer quick options. Free-text input
/// through the normal chat box keeps working — this is an accelerator.
pub fn checkin_prompt(question: &str) -> UiEnvelope {
    let opt = |label: &str, answer: &str| UiButton {
        label: label.to_string(),
        action: UiAction {
            command: "checkin_answer".into(),
            args: serde_json::json!({ "text": answer }),
        },
        style: ButtonStyle::Normal,
    };
    checked(UiEnvelope::new(vec![
        UiComponent::Text {
            content: question.to_string(),
        },
        UiComponent::Choice {
            options: vec![
                opt("在工作", "我在工作"),
                opt("在学习", "我在学习"),
                opt("在休息", "我在休息"),
                opt("在路上", "我在路上"),
            ],
        },
    ]))
}

/// Freshly generated suggestions (F10) with accept/dismiss right in the flow.
/// Only persisted suggestions (with ids) render; returns `None` when nothing
/// is actionable.
pub fn suggestions_prompt(fresh: &[Suggestion]) -> Option<UiEnvelope> {
    let mut components = Vec::new();
    for s in fresh {
        let Some(id) = s.id else { continue };
        components.push(UiComponent::SuggestionCard {
            content: s.text.clone(),
            kind: s.kind.as_str().to_string(),
        });
        components.push(UiComponent::ButtonGroup {
            buttons: vec![
                UiButton {
                    label: "采纳".into(),
                    action: UiAction {
                        command: "suggestion_set".into(),
                        args: serde_json::json!({ "id": id, "status": "accepted" }),
                    },
                    style: ButtonStyle::Primary,
                },
                UiButton {
                    label: "忽略".into(),
                    action: UiAction {
                        command: "suggestion_set".into(),
                        args: serde_json::json!({ "id": id, "status": "dismissed" }),
                    },
                    style: ButtonStyle::Normal,
                },
            ],
        });
        // Two components per suggestion; stop before busting the envelope cap.
        if components.len() + 2 > MAX_COMPONENTS {
            break;
        }
    }
    if components.is_empty() {
        return None;
    }
    Some(checked(UiEnvelope::new(components)))
}

/// Today's read-only focus brief as a compact F18 card. It deliberately
/// composes the existing catalog only: an overview text, event/reminder
/// snapshots, and actionable suggestion cards. The brief is a point-in-time
/// projection, so rendering it introduces no new write path.
pub fn daily_brief_prompt(brief: &Brief) -> Option<UiEnvelope> {
    if brief.events_today.is_empty()
        && brief.due_reminders.is_empty()
        && brief.upcoming_reminders.is_empty()
        && brief.top_suggestions.is_empty()
    {
        return None;
    }

    let mut components = vec![UiComponent::Text {
        content: format!(
            "🧭 今日聚焦（{}）：日程 {} 项，到点提醒 {} 条，即将提醒 {} 条，待处理建议 {} 条。",
            brief.date,
            brief.events_today.len(),
            brief.due_reminders.len(),
            brief.upcoming_reminders.len(),
            brief.top_suggestions.len(),
        ),
    }];

    for event in &brief.events_today {
        if components.len() + 1 > MAX_COMPONENTS {
            break;
        }
        components.push(UiComponent::EventCard {
            title: event.title.clone(),
            kind: event.kind.as_str().to_string(),
            start: fmt_ts(&event.start),
            location: event.location.clone(),
        });
    }

    for reminder in &brief.due_reminders {
        if components.len() + 1 > MAX_COMPONENTS {
            break;
        }
        let event_title = brief
            .events_today
            .iter()
            .find(|event| event.id == Some(reminder.event_id))
            .map(|event| event.title.clone())
            .unwrap_or_else(|| format!("event#{}", reminder.event_id));
        components.push(UiComponent::ReminderCard {
            event_title,
            fire_at: fmt_ts(&reminder.fire_at),
            status: reminder.status.as_str().to_string(),
        });
    }

    for suggestion in &brief.top_suggestions {
        let Some(id) = suggestion.id else { continue };
        if components.len() + 2 > MAX_COMPONENTS {
            break;
        }
        components.push(UiComponent::SuggestionCard {
            content: suggestion.text.clone(),
            kind: suggestion.kind.as_str().to_string(),
        });
        components.push(UiComponent::ButtonGroup {
            buttons: vec![
                UiButton {
                    label: "采纳".into(),
                    action: UiAction {
                        command: "suggestion_set".into(),
                        args: serde_json::json!({ "id": id, "status": "accepted" }),
                    },
                    style: ButtonStyle::Primary,
                },
                UiButton {
                    label: "忽略".into(),
                    action: UiAction {
                        command: "suggestion_set".into(),
                        args: serde_json::json!({ "id": id, "status": "dismissed" }),
                    },
                    style: ButtonStyle::Normal,
                },
            ],
        });
    }

    Some(checked(UiEnvelope::new(components)))
}

/// D3: the chat entrance for a dangerous tool — one danger-styled button
/// carrying a `guard_request` action. Strictly an entrance (§3.9 rule 2):
/// clicking it only *starts* the §3.3 flow (preview → confirm → one-time
/// token); nothing executes from the button itself.
pub fn guard_entry(explain: &str, label: &str, tool: &str, tool_args: &str) -> UiEnvelope {
    checked(UiEnvelope::new(vec![
        UiComponent::Text {
            content: explain.to_string(),
        },
        UiComponent::ButtonGroup {
            buttons: vec![UiButton {
                label: label.to_string(),
                action: UiAction {
                    command: "guard_request".into(),
                    args: serde_json::json!({ "tool": tool, "args": tool_args }),
                },
                style: ButtonStyle::Danger,
            }],
        },
    ]))
}

/// The F9 import draft as an in-place edit form (先预览再确认 in one surface):
/// submit maps straight onto `persona_import_save`.
pub fn persona_draft_form(draft: &PersonaDraft, note: Option<&str>) -> UiEnvelope {
    let field = |name: &str, label: &str, value: Option<String>| UiField {
        name: name.to_string(),
        label: label.to_string(),
        kind: FieldKind::Text,
        value,
        options: None,
    };
    checked(UiEnvelope::new(vec![
        UiComponent::Text {
            content: "以下是本地提取的人格初稿，可修改后保存（保存前不落库）：".into(),
        },
        UiComponent::Form {
            fields: vec![
                field("nickname", "称呼你为", draft.nickname.clone()),
                field("tone", "语气", Some(draft.tone.clone())),
                field(
                    "catchphrases",
                    "口头禅（逗号分隔）",
                    Some(draft.catchphrases.join(", ")),
                ),
                field("style_notes", "其他备注", draft.style_notes.clone()),
                field("note", "版本说明", note.map(String::from)),
            ],
            submit: UiButton {
                label: "确认保存为导入版本".into(),
                action: UiAction {
                    command: "persona_import_save".into(),
                    args: empty_args(),
                },
                style: ButtonStyle::Primary,
            },
        },
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventKind;
    use crate::routine::Routine;
    use chrono::NaiveDate;

    fn now() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 14)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn text_env(json: &str) -> Option<UiEnvelope> {
        parse_envelope(json, ALLOWED_ACTIONS)
    }

    #[test]
    fn parses_minimal_text_envelope_with_fences() {
        let raw = "```json\n{\"version\":1,\"components\":[{\"type\":\"text\",\"content\":\"你好\"}]}\n```";
        let env = text_env(raw).expect("valid");
        assert_eq!(env.text_summary(), "你好");
    }

    #[test]
    fn rejects_unknown_component_version_and_junk() {
        // Unknown component type.
        assert!(
            text_env(r#"{"version":1,"components":[{"type":"iframe","content":"x"}]}"#).is_none()
        );
        // Unknown extra field on a known component (deny_unknown_fields).
        assert!(text_env(
            r#"{"version":1,"components":[{"type":"text","content":"x","style":"red"}]}"#
        )
        .is_none());
        // Wrong version.
        assert!(
            text_env(r#"{"version":2,"components":[{"type":"text","content":"x"}]}"#).is_none()
        );
        // Empty components / not JSON at all.
        assert!(text_env(r#"{"version":1,"components":[]}"#).is_none());
        assert!(text_env("hello there").is_none());
    }

    #[test]
    fn action_whitelist_and_arg_schemas_enforced() {
        // Non-whitelisted command.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"删库","action":{"command":"drop_database","args":{}}}]}]}"#;
        assert!(text_env(raw).is_none());
        // Whitelisted command, wrong args shape.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"取消","action":{"command":"reminder_dismiss","args":{"id":"not-a-number"}}}]}]}"#;
        assert!(text_env(raw).is_none());
        // suggestion_set with an invalid status.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"x","action":{"command":"suggestion_set","args":{"id":1,"status":"deleted"}}}]}]}"#;
        assert!(text_env(raw).is_none());
        // A routine action needs a real boolean, not a stringly value.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"暂停","action":{"command":"routine_set_active","args":{"id":1,"active":"false"}}}]}]}"#;
        assert!(text_env(raw).is_none());
        // Correct one passes.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"取消","action":{"command":"reminder_dismiss","args":{"id":3}}}]}]}"#;
        assert!(text_env(raw).is_some());
    }

    #[test]
    fn llm_action_subset_is_narrower() {
        assert_eq!(LLM_ACTIONS, ["ingest", "checkin_answer"]);
        // reminder_dismiss is fine for offline templates but not for the LLM.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"取消","action":{"command":"reminder_dismiss","args":{"id":3}}}]}]}"#;
        assert!(parse_envelope(raw, ALLOWED_ACTIONS).is_some());
        assert!(parse_envelope(raw, LLM_ACTIONS).is_none());
        // ingest is allowed in both.
        let raw = r#"{"version":1,"components":[{"type":"button_group","buttons":[
            {"label":"记下来","action":{"command":"ingest","args":{"text":"明天9点开会"}}}]}]}"#;
        assert!(parse_envelope(raw, LLM_ACTIONS).is_some());
    }

    #[test]
    fn bounds_enforced() {
        // 5 buttons > MAX_BUTTONS.
        let btn = r#"{"label":"a","action":{"command":"reminder_fire","args":{}}}"#;
        let raw = format!(
            r#"{{"version":1,"components":[{{"type":"button_group","buttons":[{}]}}]}}"#,
            [btn; 5].join(",")
        );
        assert!(text_env(&raw).is_none());
        // Choice needs >= 2 options.
        let raw =
            format!(r#"{{"version":1,"components":[{{"type":"choice","options":[{btn}]}}]}}"#);
        assert!(text_env(&raw).is_none());
        // Select field without options.
        let raw = r#"{"version":1,"components":[{"type":"form","fields":[
            {"name":"x","label":"X","type":"select"}],
            "submit":{"label":"存","action":{"command":"persona_import_save","args":{}}}}]}"#;
        assert!(text_env(raw).is_none());
    }

    #[test]
    fn template_event_ingested_validates_and_carries_ids() {
        let mut ev = Event::new("开会", EventKind::Meeting, now(), "raw", now());
        ev.id = Some(7);
        let n = Notification {
            id: Some(42),
            event_id: 7,
            fire_at: now(),
            lead_label: "30m".into(),
            channels: vec![],
            status: crate::model::NotificationStatus::Pending,
            created_at: now(),
            fired_at: None,
        };
        let env = event_ingested(&ev, &[n]);
        assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""type":"event_card""#));
        assert!(json.contains(r#""id":42"#));
        // Round-trips through the defensive parser.
        assert!(parse_envelope(&json, ALLOWED_ACTIONS).is_some());
    }

    #[test]
    fn template_routine_ingested_is_actionable_only_when_persisted() {
        let routine = Routine {
            id: Some(9),
            title: "刷牙".into(),
            time_of_day: "07:00".into(),
            source: Some("raw_input#1".into()),
            active: true,
            created_at: now(),
            scheduled_until: None,
        };
        let env = routine_ingested(&routine).expect("persisted routine renders a card");
        assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""type":"routine_card""#));
        assert!(json.contains(r#""command":"routine_set_active""#));
        assert!(json.contains(r#""active":false"#));

        assert!(routine_ingested(&Routine {
            id: None,
            ..routine
        })
        .is_none());
    }

    #[test]
    fn template_checkin_and_persona_form_validate() {
        let env = checkin_prompt("现在在做什么？");
        assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
        assert_eq!(env.text_summary(), "现在在做什么？");

        let draft = PersonaDraft {
            nickname: None,
            tone: "干练".into(),
            catchphrases: vec!["稳了".into()],
            style_notes: None,
        };
        let env = persona_draft_form(&draft, Some("从聊天记录导入"));
        assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("persona_import_save"));
    }

    #[test]
    fn template_suggestions_skip_unpersisted() {
        use crate::suggest::{SuggestionKind, SuggestionStatus};
        let s = |id: Option<i64>| Suggestion {
            id,
            kind: SuggestionKind::ExamPrep,
            text: "开始复习".into(),
            dedup_key: "exam_prep:1".into(),
            source: None,
            status: SuggestionStatus::Pending,
            created_at: now(),
        };
        assert!(suggestions_prompt(&[s(None)]).is_none());
        let env = suggestions_prompt(&[s(Some(5))]).unwrap();
        assert!(validate(&env, ALLOWED_ACTIONS).is_ok());
        assert!(serde_json::to_string(&env).unwrap().contains(r#""id":5"#));
    }

    #[test]
    fn daily_brief_prompt_round_trips_and_validates() {
        use crate::brief::Brief;
        use crate::model::{Channel, NotificationStatus};
        use crate::suggest::{SuggestionKind, SuggestionStatus};

        let mut event = Event::new("晨会", EventKind::Meeting, now(), "raw", now());
        event.id = Some(7);
        let reminder = Notification {
            id: Some(12),
            event_id: 7,
            fire_at: now(),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: now(),
            fired_at: None,
        };
        let suggestion = Suggestion {
            id: Some(3),
            created_at: now(),
            kind: SuggestionKind::ExamPrep,
            text: "先安排复习块".into(),
            dedup_key: "exam_prep:event#8".into(),
            source: Some("event#8".into()),
            status: SuggestionStatus::Pending,
        };
        let brief = Brief {
            date: now().date(),
            events_today: vec![event],
            due_reminders: vec![reminder],
            upcoming_reminders: vec![],
            top_suggestions: vec![suggestion],
        };

        let env = daily_brief_prompt(&brief).expect("brief has content");
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""type":"event_card""#));
        assert!(json.contains(r#""type":"reminder_card""#));
        assert!(json.contains(r#""type":"suggestion_card""#));
        let round_trip: UiEnvelope = serde_json::from_str(&json).unwrap();
        assert!(validate(&round_trip, ALLOWED_ACTIONS).is_ok());
    }

    #[test]
    fn daily_brief_prompt_skips_an_empty_day() {
        let brief = Brief {
            date: now().date(),
            events_today: vec![],
            due_reminders: vec![],
            upcoming_reminders: vec![],
            top_suggestions: vec![],
        };
        assert!(daily_brief_prompt(&brief).is_none());
    }
}

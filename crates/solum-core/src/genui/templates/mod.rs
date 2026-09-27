//! `templates` -- the offline, rules-based builders -- the F16 path that needs no model at all, and the reason a UI envelope can exist with the device offline. Each returns an envelope for one trigger (an ingested event, a check-in, a daily brief, a guard decision).
//!
//! Part of the `genui` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::types::{empty_args, MAX_BUTTONS, MAX_CHOICE_OPTIONS, MAX_COMPONENTS};
use super::*;

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

//! `validation` -- the catalog bounds check -- every component, field and button against the limits above, and every action against the whitelist. This is the seam the architecture doc calls the defence between generated UI and the command layer.
//!
//! Part of the `genui` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::types::{
    MAX_BUTTONS, MAX_CHOICE_OPTIONS, MAX_COMPONENTS, MAX_FORM_FIELDS, MAX_LABEL_LEN, MAX_TEXT_LEN,
};
use super::*;

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
        "ingest" | "checkin_answer" => {
            if args.len() != 1 {
                return Err(format!("动作 {} 只允许 text 参数", a.command));
            }
            need_str("text")?;
            if args["text"]
                .as_str()
                .is_some_and(|text| text.chars().count() > MAX_TEXT_LEN)
            {
                return Err(format!("动作 {} 的 text 过长", a.command));
            }
            Ok(())
        }
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

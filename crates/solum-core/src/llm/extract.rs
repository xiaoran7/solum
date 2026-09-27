//! extract. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

use super::*;

/// LLM fallback extraction for event-shaped utterances the rule-based
/// extractor couldn't parse. The model returns a strict tool call; a legacy
/// JSON text response remains accepted during rollout. Anything
/// malformed degrades to `Ok(None)` rather than failing ingest.
pub fn llm_extract(r: &dyn Reasoner, text: &str, now: NaiveDateTime) -> Result<Option<Event>> {
    let system = format!(
        "你是日程与提醒抽取器。{}请深入理解用户语境：\
         1. 若包含真实、确定的日程或定时提醒安排（例如“X分钟后提醒我洗澡”、“明天下午3点开会”），\
            请准确按当前时刻换算相对时间（未指定时刻时默认 09:00:00），调用 create_event；\
         2. 若用户表达的是假设、玩笑、反讽、情绪宣泄（例如“等我中了一千万记得提醒我买车”）或非排期闲聊，严禁调用 create_event 工具。",
        now_line(now)
    );
    let response = r.complete_with_tools(&system, text, &[event_tool_definition()])?;
    if response.tool_calls.len() == 1 && response.tool_calls[0].name == TOOL_CREATE_EVENT {
        return Ok(parse_event_value(
            &response.tool_calls[0].arguments,
            text,
            now,
        ));
    }
    Ok(response
        .content
        .as_deref()
        .and_then(|raw| parse_event_json(raw, text, now)))
}

/// Defensive parse of the model's JSON (fences stripped, fields validated).
pub(super) fn parse_event_json(raw: &str, raw_input: &str, now: NaiveDateTime) -> Option<Event> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    // Take the outermost {...} in case the model added stray prose.
    let start = cleaned.find('{')?;
    let end = cleaned.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(&cleaned[start..=end]).ok()?;
    parse_event_value(&v, raw_input, now)
}

pub(super) fn parse_event_value(
    v: &serde_json::Value,
    raw_input: &str,
    now: NaiveDateTime,
) -> Option<Event> {
    if v["none"].as_bool() == Some(true) {
        return None;
    }
    let title = v["title"].as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    let kind: EventKind = v["kind"].as_str()?.parse().ok()?;
    // The model only ever gets told about the six built-ins, so a `custom:`
    // kind here is a hallucination, not a category the user made. Accepting it
    // would let a stray "party" invent a new category on disk. Categories are
    // only ever created deliberately, from the rule table.
    if kind.is_custom() {
        return None;
    }
    let start_s = v["start"].as_str()?;
    // Accept with or without seconds.
    let start_ts = parse_ts(start_s)
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(start_s, "%Y-%m-%dT%H:%M").ok())?;
    let mut ev = Event::new(title, kind, start_ts, raw_input, now);
    ev.location = v["location"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    ev.people = v["people"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some(ev)
}

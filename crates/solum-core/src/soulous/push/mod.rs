//! `push` -- outbound event push: the minimal structured projection that leaves the device, and the preview that shows it before it goes.
//!
//! Part of the `soulous` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// The minimum, structured projection Solum may send to the user's own Soulous
/// server. Never add `people`, `raw_input`, notification text, or any other
/// provenance-bearing field here: those remain L1 local-only data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ExternalContextRequest {
    pub(super) source: &'static str,
    #[serde(rename = "type")]
    pub(super) context_type: &'static str,
    #[serde(rename = "externalId")]
    pub(super) external_id: String,
    #[serde(rename = "occurredAt")]
    pub(super) occurred_at: String,
    pub(super) payload: ScheduleEventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ScheduleEventPayload {
    pub(super) title: String,
    pub(super) kind: String,
    pub(super) start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushOutcome {
    pub external_id: String,
    pub title: String,
    pub refreshed_tokens: bool,
}

pub(super) fn outbound_event_request(
    store: &Store,
    event_id: i64,
) -> Result<ExternalContextRequest> {
    let event = store.get_event(event_id)?;
    let (guid, local_only) = store.event_guid_and_local_only(event_id)?;
    if local_only {
        return Err(CoreError::Soulous(
            "此日程来自第三方通知，按隐私规则不得推送到 Soulous".into(),
        ));
    }
    Ok(ExternalContextRequest {
        source: SOLUM_SOURCE,
        context_type: SCHEDULE_EVENT_TYPE,
        external_id: guid,
        occurred_at: crate::model::fmt_ts(&event.created_at),
        payload: ScheduleEventPayload {
            title: event.title.trim().to_string(),
            kind: event.kind.as_str().to_string(),
            start: crate::model::fmt_ts(&event.start),
            end: event.end.map(|at| crate::model::fmt_ts(&at)),
            location: event.location.and_then(|value| {
                let value = value.trim().to_string();
                (!value.is_empty()).then_some(value)
            }),
        },
    })
}

/// Human-readable effect preview for the Sensitive Tool confirmation. This is
/// deliberately generated from the same restricted projection that will be
/// serialized, so the UI cannot promise a narrower data surface than the
/// actual request.
pub fn preview_event_push(store: &Store, event_id: i64) -> Result<String> {
    let request = outbound_event_request(store, event_id)?;
    let location = request
        .payload
        .location
        .as_deref()
        .map(|value| format!("；地点：{value}"))
        .unwrap_or_default();
    Ok(format!(
        "将向你自己的 Soulous 推送日程「{}」（{}，{}{}）。仅发送标题、类型、时间和地点；不会发送参与人、原始输入、通知文本或 Solum 记忆。",
        request.payload.title,
        request.payload.kind,
        request.payload.start,
        location
    ))
}

pub(super) fn token_tail(token: &str) -> String {
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!("…{tail}")
    }
}

//! parse. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

use super::*;

/// Pull `choices[0].message.content` out of a chat-completions response.
/// Some thinking-model gateways inline the chain of thought as a leading
/// `<think>…</think>` block instead of a separate `reasoning_content` field —
/// strip it so callers only ever see the final answer.
pub(crate) fn parse_chat_content(json: &serde_json::Value) -> Result<String> {
    parse_reasoner_response(json)?
        .content
        .ok_or_else(|| CoreError::Llm("响应缺少可见文本".into()))
}

/// Parse the bounded OpenAI-compatible response surface used by Solum. Error
/// messages deliberately omit the response body because tool arguments may
/// contain personal data.
pub(crate) fn parse_reasoner_response(json: &serde_json::Value) -> Result<ReasonerResponse> {
    let message = json
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| CoreError::Llm("响应缺少 choices[0].message".into()))?;
    let content = match message.get("content") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) => {
            let visible = strip_think_block(value).trim().to_string();
            (!visible.is_empty()).then_some(visible)
        }
        Some(_) => return Err(CoreError::Llm("响应 content 类型无效".into())),
    };
    let raw_calls = match message.get("tool_calls") {
        None | Some(serde_json::Value::Null) => &[][..],
        Some(serde_json::Value::Array(calls)) if calls.len() <= 4 => calls.as_slice(),
        Some(serde_json::Value::Array(_)) => {
            return Err(CoreError::Llm("响应 tool_calls 超过 4 个".into()))
        }
        Some(_) => return Err(CoreError::Llm("响应 tool_calls 类型无效".into())),
    };
    let mut tool_calls = Vec::with_capacity(raw_calls.len());
    for call in raw_calls {
        let function = call
            .get("function")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| CoreError::Llm("工具调用缺少 function".into()))?;
        let name = function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 64)
            .ok_or_else(|| CoreError::Llm("工具调用 name 无效".into()))?;
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(CoreError::Llm("工具调用 name 无效".into()));
        }
        let arguments = match function.get("arguments") {
            Some(serde_json::Value::String(raw)) if raw.len() <= 32_000 => {
                serde_json::from_str::<serde_json::Value>(raw)
                    .map_err(|_| CoreError::Llm("工具调用 arguments 不是有效 JSON".into()))?
            }
            Some(value @ serde_json::Value::Object(_)) => value.clone(),
            _ => return Err(CoreError::Llm("工具调用 arguments 无效".into())),
        };
        if !arguments.is_object() {
            return Err(CoreError::Llm("工具调用 arguments 必须是对象".into()));
        }
        tool_calls.push(ReasonerToolCall {
            name: name.to_string(),
            arguments,
        });
    }
    if content.is_none() && tool_calls.is_empty() {
        return Err(CoreError::Llm("响应既无文本也无工具调用".into()));
    }
    Ok(ReasonerResponse {
        content,
        tool_calls,
    })
}

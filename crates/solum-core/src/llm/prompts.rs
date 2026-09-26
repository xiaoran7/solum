//! prompts. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

use super::*;

// ---- gateway prompts (minimal context by construction) ----------------------

pub(super) fn now_line(now: NaiveDateTime) -> String {
    const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
    format!(
        "现在是 {}，星期{}。",
        now.format("%Y-%m-%d %H:%M"),
        WEEKDAYS[now.weekday().num_days_from_monday() as usize]
    )
}

/// One user↔assistant exchange, kept in shell/orchestrator memory only —
/// never persisted, never synced (§3.5 短期上下文 / §3.10 M1).
#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub user: String,
    pub assistant: String,
}

/// Locally-assembled context for a chat call (§3.10). Both halves are
/// hard-capped upstream: history to [`MAX_HISTORY_TURNS`] turns by the
/// orchestrator, snippets to `recall::MAX_SNIPPETS`/`MAX_TOTAL_CHARS` by the
/// recall module.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatContext<'a> {
    pub history: &'a [ChatTurn],
    pub snippets: &'a [crate::recall::Snippet],
}

/// Hard cap on how many recent turns travel with a chat call.
pub const MAX_HISTORY_TURNS: usize = 4;

/// The "已知背景 + 最近对话" block appended to the chat system prompt. Empty
/// context → empty string (prompt identical to the pre-recall era).
fn context_block(ctx: &ChatContext) -> String {
    let mut s = String::new();
    if !ctx.snippets.is_empty() {
        s.push_str("\n已知背景（来自本地记忆检索，可能不全，除此之外不要假装知道别的）：");
        for sn in ctx.snippets {
            s.push_str(&format!("\n- [{}] {}", sn.layer.label(), sn.content));
        }
    }
    if !ctx.history.is_empty() {
        s.push_str("\n最近对话（从旧到新）：");
        for t in ctx.history.iter().rev().take(MAX_HISTORY_TURNS).rev() {
            s.push_str(&format!("\n用户：{}\n你：{}", t.user, t.assistant));
        }
    }
    s
}

/// A conversational reply to a chat-intent utterance. Context sent: the one
/// utterance + the clock + the user-authored persona style + the locally
/// selected [`ChatContext`] (recall snippets + recent turns). Nothing else.
pub fn chat_reply(
    r: &dyn Reasoner,
    text: &str,
    now: NaiveDateTime,
    persona: Option<&crate::persona::PersonaProfile>,
    ctx: &ChatContext,
) -> Result<String> {
    let style = persona
        .map(|p| format!("\n{}", p.style_prompt()))
        .unwrap_or_default();
    let system = format!(
        "你是用户的个人助理息壤，语气自然、简体中文、不超过三句话。\
         除了下面给出的背景，不要假装知道其他信息；\
         如果这句话在请求危险操作，提醒需要人工确认。{}{}{}",
        now_line(now),
        style,
        context_block(ctx)
    );
    r.complete(&system, text)
}

/// The chat system prompt (F18, §3.9). **Default is plain prose**; executable
/// follow-ups use the supplied function tools. Tool calls remain requests for
/// a user-confirmed button and never execute inside the model path.
fn chat_ui_system(
    now: NaiveDateTime,
    persona: Option<&crate::persona::PersonaProfile>,
    ctx: &ChatContext,
) -> String {
    let style = persona
        .map(|p| format!("\n{}", p.style_prompt()))
        .unwrap_or_default();
    format!(
        "你是用户的个人助理息壤，语气自然、简体中文、不超过三句话。除了下面给出的背景，\
         不要假装知道其他信息；如果这句话在请求危险操作，提醒需要人工确认。{}{}{}\n\
         默认直接用纯文本回复。只有当你确实要建议可执行的后续动作时才调用工具：\
         ingest 用于建议把一件事记成日程；checkin_answer 用于给出快捷状态回答。\
         工具调用只会生成等待用户点按确认的按钮，不会直接执行。不要自行输出工具 JSON，\
         不要调用未提供的工具；没有可执行动作时不要调用工具。",
        now_line(now),
        style,
        context_block(ctx)
    )
}

/// Turn one raw chat reply into `(message, ui)` (F18 envelope-or-prose):
/// - envelope parses & validates → message is its text summary, ui is `Some`;
/// - reply is plain prose → it *is* the message, ui is `None`;
/// - reply looks like attempted-but-broken JSON → canned message, ui `None`
///   (never show the user a blob of malformed JSON).
///
/// Pure and infallible — shared by the blocking and streaming chat calls.
fn reconcile_chat_reply(raw: &str) -> (String, Option<crate::genui::UiEnvelope>) {
    if let Some(env) = crate::genui::parse_envelope(raw, crate::genui::LLM_ACTIONS) {
        let msg = env.text_summary();
        let msg = if msg.is_empty() {
            "已收到。".to_string()
        } else {
            msg
        };
        // A pure-text envelope renders identically to a plain message — skip
        // the ui payload so the frontend takes the simple path.
        let interactive = env
            .components
            .iter()
            .any(|c| !matches!(c, crate::genui::UiComponent::Text { .. }));
        return (msg, interactive.then_some(env));
    }
    let trimmed = raw.trim();
    if trimmed.starts_with('{') || trimmed.starts_with("```") {
        // Attempted JSON that failed validation: degrade without leaking it.
        return (
            "已记录。（云端回复格式异常，已忽略其交互内容）".into(),
            None,
        );
    }
    if trimmed.is_empty() {
        return ("已记录。".into(), None);
    }
    (trimmed.to_string(), None)
}

/// Chat reply with generative UI (F18, §3.9): one call, envelope-or-prose.
///
/// Context sent is identical to [`chat_reply`]: the one utterance, the clock,
/// the persona style, the local [`ChatContext`]. The UI description is
/// *returned* data — no new upstream beyond the locally-capped context.
pub fn chat_reply_ui(
    r: &dyn Reasoner,
    text: &str,
    now: NaiveDateTime,
    persona: Option<&crate::persona::PersonaProfile>,
    ctx: &ChatContext,
) -> Result<(String, Option<crate::genui::UiEnvelope>)> {
    let system = chat_ui_system(now, persona, ctx);
    let response = r.complete_with_tools(&system, text, &chat_tool_definitions())?;
    Ok(reconcile_tool_reply(response))
}

fn reconcile_tool_reply(response: ReasonerResponse) -> (String, Option<crate::genui::UiEnvelope>) {
    if response.tool_calls.is_empty() {
        return reconcile_chat_reply(response.content.as_deref().unwrap_or(""));
    }
    let message = response
        .content
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("我已准备好这个操作，请确认后继续。")
        .to_string();
    let mut buttons = Vec::with_capacity(response.tool_calls.len());
    for call in response.tool_calls {
        let label = match call.name.as_str() {
            TOOL_INGEST => "加入日程",
            TOOL_CHECKIN_ANSWER => "记录状态",
            _ => return ("已收到。（模型请求了不受支持的操作，已忽略）".into(), None),
        };
        let action = crate::genui::UiAction {
            command: call.name,
            args: call.arguments,
        };
        if crate::genui::validate_action(&action, crate::genui::LLM_ACTIONS).is_err() {
            return ("已收到。（模型返回的操作参数无效，已忽略）".into(), None);
        }
        buttons.push(crate::genui::UiButton {
            label: label.into(),
            action,
            style: crate::genui::ButtonStyle::Primary,
        });
    }
    let envelope = crate::genui::UiEnvelope::new(vec![
        crate::genui::UiComponent::Text {
            content: message.clone(),
        },
        crate::genui::UiComponent::ButtonGroup { buttons },
    ]);
    if crate::genui::validate(&envelope, crate::genui::LLM_ACTIONS).is_err() {
        return (
            "已收到。（模型返回的操作无法安全展示，已忽略）".into(),
            None,
        );
    }
    (message, Some(envelope))
}

/// Tool-aware variant used by the streaming UI path. OpenAI tool-call deltas
/// differ between gateways, so this request is deliberately non-streaming and
/// emits safe prose once after the complete response has been validated.
pub fn chat_reply_ui_streaming(
    r: &dyn Reasoner,
    text: &str,
    now: NaiveDateTime,
    persona: Option<&crate::persona::PersonaProfile>,
    ctx: &ChatContext,
    on_visible: &mut dyn FnMut(&str),
) -> Result<(String, Option<crate::genui::UiEnvelope>)> {
    let system = chat_ui_system(now, persona, ctx);
    let response = r.complete_with_tools(&system, text, &chat_tool_definitions())?;
    let (message, ui) = reconcile_tool_reply(response);
    on_visible(&message);
    Ok((message, ui))
}

/// Rewrite the offline review digest in the persona's voice (the F14 cloud
/// step). The digest numbers are the facts; the model may only rephrase.
/// Returns `Ok(None)` when the reply drops or mangles a count — callers fall
/// back to the offline render, which stays the reliability floor (F16).
pub fn rewrite_digest(
    r: &dyn Reasoner,
    digest: &crate::review::Digest,
    persona: Option<&crate::persona::PersonaProfile>,
) -> Result<Option<String>> {
    let style = persona
        .map(|p| p.style_prompt())
        .unwrap_or_else(|| "[人格风格设定]\n- 语气：自然、亲切的个人助理。".into());
    let system = format!(
        "你是用户的个人助理息壤。用户会发来一份由本地系统统计好的复盘简报，\
         请按下面的风格设定把它改写成一段更有人味的总结（简体中文，不超过五句话，不用列表）。\
         所有数字必须原样保留为阿拉伯数字，不得增删或改动任何事实，\
         不要编造简报里没有的事件，也不要替数字猜测原因。\n{style}"
    );
    // Only the numeric core is sent upstream — the 观察/记忆 extras are
    // rendered locally and appended by the caller (D6: fact contents and
    // behavior observations never travel to the cloud).
    let reply = r.complete(&system, &digest.render_core())?;
    let reply = reply.trim().to_string();
    Ok((!reply.is_empty() && digest_counts_preserved(digest, &reply)).then_some(reply))
}

/// Every non-zero count from the digest must survive the rewrite verbatim
/// (as Arabic numerals). Substring matching is deliberately lenient — the
/// point is catching dropped or invented facts, not policing phrasing.
pub(super) fn digest_counts_preserved(d: &crate::review::Digest, reply: &str) -> bool {
    let mut counts = vec![
        d.raw_inputs,
        d.events_total,
        d.notifications_planned,
        d.notifications_fired,
        d.dangerous_attempts,
        d.dangerous_refused,
    ];
    counts.extend(d.events_by_kind.iter().map(|(_, n)| *n));
    counts.retain(|&n| n > 0);
    counts.sort_unstable();
    counts.dedup();
    counts.iter().all(|n| reply.contains(&n.to_string()))
}

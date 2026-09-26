//! tests. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

use super::*;
use chrono::NaiveDate;

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, 6)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

#[test]
fn config_from_json_and_masking() {
    let c = LlmConfig::from_json(
        r#"{"base_url":"https://x/v1","api_key":"tp-secret1234","model":"mimo-v2.5"}"#,
    )
    .unwrap();
    assert_eq!(c.model, "mimo-v2.5");
    let masked = c.masked_summary();
    assert!(masked.contains("…1234"));
    assert!(!masked.contains("secret"));
    // Model defaults when omitted; missing key is an error.
    let d = LlmConfig::from_json(r#"{"base_url":"https://x/v1","api_key":"k"}"#).unwrap();
    assert_eq!(d.model, "gemini-3.8-flash-high");
    assert!(LlmConfig::from_json(r#"{"base_url":"","api_key":"k"}"#).is_err());
    // Omitted knobs keep the historic defaults…
    assert_eq!(d.temperature, Some(0.3));
    assert_eq!(d.max_tokens, None);
    assert_eq!(d.timeout_secs, 30);
    // …explicit null temperature means "don't send the field" (gpt-5).
    let e = LlmConfig::from_json(
        r#"{"base_url":"https://x/v1","api_key":"k","temperature":null,"max_tokens":512,"timeout_secs":60}"#,
    )
    .unwrap();
    assert_eq!(e.temperature, None);
    assert_eq!(e.max_tokens, Some(512));
    assert_eq!(e.timeout_secs, 60);
}

#[test]
fn parses_chat_content() {
    let ok = serde_json::json!({"choices":[{"message":{"content":" 你好！ "}}]});
    assert_eq!(parse_chat_content(&ok).unwrap(), "你好！");
    let bad = serde_json::json!({"error":"nope"});
    assert!(parse_chat_content(&bad).is_err());
    // Inlined chain-of-thought is stripped; an unclosed tag is left alone.
    let think =
        serde_json::json!({"choices":[{"message":{"content":"<think>盘算…</think>\n答案"}}]});
    assert_eq!(parse_chat_content(&think).unwrap(), "答案");
    let only_think =
        serde_json::json!({"choices":[{"message":{"content":"<think>只有思考</think>"}}]});
    assert!(parse_chat_content(&only_think).is_err());
    assert_eq!(strip_think_block("<think>没闭合"), "<think>没闭合");
}

#[test]
fn parses_openai_tool_calls_without_coercing_arguments() {
    let response = serde_json::json!({"choices":[{"message":{
        "content": null,
        "tool_calls":[{"id":"call_1","type":"function","function":{
            "name":"ingest","arguments":"{\"text\":\"明天九点开会\"}"
        }}]
    }}]});
    let parsed = parse_reasoner_response(&response).unwrap();
    assert!(parsed.content.is_none());
    assert_eq!(parsed.tool_calls[0].name, TOOL_INGEST);
    assert_eq!(parsed.tool_calls[0].arguments["text"], "明天九点开会");

    let invalid = serde_json::json!({"choices":[{"message":{"content":null,"tool_calls":[{
        "type":"function","function":{"name":"ingest","arguments":"not-json"}
    }]}}]});
    assert!(parse_reasoner_response(&invalid).is_err());
}

#[test]
fn parses_event_json_with_fences_and_noise() {
    let raw = "```json\n{\"title\":\"陪妈妈复查\",\"kind\":\"reminder\",\
               \"start\":\"2026-07-08T18:30:00\",\"location\":\"医院\",\"people\":[\"妈妈\"]}\n```";
    let ev = parse_event_json(raw, "orig", now()).unwrap();
    assert_eq!(ev.title, "陪妈妈复查");
    assert_eq!(ev.kind, EventKind::Reminder);
    assert_eq!(ev.location.as_deref(), Some("医院"));
    assert_eq!(ev.people, vec!["妈妈".to_string()]);

    // Minute-precision timestamps are accepted too.
    let raw2 = r#"{"title":"x","kind":"other","start":"2026-07-08T18:30"}"#;
    assert!(parse_event_json(raw2, "orig", now()).is_some());
}

/// Scripted reasoner that records the system prompt it was given.
struct EchoReasoner {
    reply: String,
    seen_system: std::cell::RefCell<String>,
}
impl Reasoner for EchoReasoner {
    fn complete(&self, system: &str, _user: &str) -> Result<String> {
        *self.seen_system.borrow_mut() = system.to_string();
        Ok(self.reply.clone())
    }
}

struct NativeToolReasoner {
    response: ReasonerResponse,
    seen_tools: std::cell::RefCell<Vec<serde_json::Value>>,
}
impl Reasoner for NativeToolReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        Ok(self.response.content.clone().unwrap_or_default())
    }

    fn complete_with_tools(
        &self,
        _system: &str,
        _user: &str,
        tools: &[serde_json::Value],
    ) -> Result<ReasonerResponse> {
        *self.seen_tools.borrow_mut() = tools.to_vec();
        Ok(self.response.clone())
    }
}

#[test]
fn native_chat_tool_call_becomes_confirmable_ui() {
    let reasoner = NativeToolReasoner {
        response: ReasonerResponse {
            content: Some("要把它加入日程吗？".into()),
            tool_calls: vec![ReasonerToolCall {
                name: TOOL_INGEST.into(),
                arguments: serde_json::json!({"text":"明天九点开会"}),
            }],
        },
        seen_tools: Default::default(),
    };
    let (message, ui) =
        chat_reply_ui(&reasoner, "明天有会", now(), None, &ChatContext::default()).unwrap();
    assert_eq!(message, "要把它加入日程吗？");
    let envelope = ui.expect("tool call renders a confirmation button");
    assert!(crate::genui::validate(&envelope, crate::genui::LLM_ACTIONS).is_ok());
    assert_eq!(reasoner.seen_tools.borrow().len(), 2);
}

#[test]
fn native_event_tool_call_uses_defensive_event_parser() {
    let reasoner = NativeToolReasoner {
        response: ReasonerResponse {
            content: None,
            tool_calls: vec![ReasonerToolCall {
                name: TOOL_CREATE_EVENT.into(),
                arguments: serde_json::json!({
                    "title":"复查", "kind":"reminder", "start":"2026-07-08T18:30:00",
                    "location":"医院", "people":["妈妈"]
                }),
            }],
        },
        seen_tools: Default::default(),
    };
    let event = llm_extract(&reasoner, "陪妈妈复查", now())
        .unwrap()
        .unwrap();
    assert_eq!(event.title, "复查");
    assert_eq!(
        reasoner.seen_tools.borrow()[0]["function"]["name"],
        TOOL_CREATE_EVENT
    );
}

fn persona() -> crate::persona::PersonaProfile {
    crate::persona::PersonaProfile {
        version: 1,
        created_at: now(),
        source: "manual".into(),
        note: None,
        draft: crate::persona::PersonaDraft {
            nickname: Some("老板".into()),
            tone: "干练".into(),
            catchphrases: vec![],
            style_notes: None,
        },
    }
}

fn digest() -> crate::review::Digest {
    crate::review::Digest {
        from: now(),
        to: now(),
        raw_inputs: 5,
        events_total: 3,
        events_by_kind: vec![(EventKind::Meeting, 3)],
        notifications_planned: 2,
        notifications_fired: 1,
        dangerous_attempts: 0,
        dangerous_refused: 0,
        top_activities: vec![],
        wellness_count: 0,
        new_facts: vec![],
        soulous: crate::soulous::ReviewMaterial {
            courses: 0,
            exams: 0,
            open_tasks: 0,
            checkin_days: 0,
            focus_minutes: 0,
        },
    }
}

#[test]
fn chat_reply_carries_persona_style() {
    let r = EchoReasoner {
        reply: "好的老板。".into(),
        seen_system: Default::default(),
    };
    chat_reply(&r, "你好", now(), Some(&persona()), &ChatContext::default()).unwrap();
    let sys = r.seen_system.borrow();
    assert!(sys.contains("称呼用户为「老板」"));
    assert!(sys.contains("语气：干练"));
}

#[test]
fn chat_context_block_carries_snippets_and_history() {
    // M1+M3: snippets and recent turns land in the system prompt…
    let r = EchoReasoner {
        reply: "好的。".into(),
        seen_system: Default::default(),
    };
    let snippets = vec![crate::recall::Snippet {
        layer: crate::recall::SnippetLayer::Fact,
        id: 1,
        content: "我不吃辣".into(),
        score: 1.0,
    }];
    let history = vec![ChatTurn {
        user: "晚上吃什么".into(),
        assistant: "想吃火锅吗？".into(),
    }];
    let ctx = ChatContext {
        history: &history,
        snippets: &snippets,
    };
    chat_reply(&r, "随便", now(), None, &ctx).unwrap();
    {
        let sys = r.seen_system.borrow();
        assert!(sys.contains("已知背景"));
        assert!(sys.contains("我不吃辣"));
        assert!(sys.contains("最近对话"));
        assert!(sys.contains("想吃火锅吗？"));
    }
    // …and an empty context adds nothing (prompt stays minimal).
    chat_reply(&r, "随便", now(), None, &ChatContext::default()).unwrap();
    {
        let sys = r.seen_system.borrow();
        assert!(!sys.contains("已知背景"));
        assert!(!sys.contains("最近对话"));
    }
    // The UI variant carries the same block.
    chat_reply_ui(&r, "随便", now(), None, &ctx).unwrap();
    assert!(r.seen_system.borrow().contains("我不吃辣"));
}

#[test]
fn history_is_capped_at_max_turns() {
    let r = EchoReasoner {
        reply: "好的。".into(),
        seen_system: Default::default(),
    };
    let history: Vec<ChatTurn> = (0..10)
        .map(|i| ChatTurn {
            user: format!("问题{i}"),
            assistant: format!("回答{i}"),
        })
        .collect();
    let ctx = ChatContext {
        history: &history,
        snippets: &[],
    };
    chat_reply(&r, "hi", now(), None, &ctx).unwrap();
    let sys = r.seen_system.borrow();
    // Only the newest MAX_HISTORY_TURNS turns survive.
    assert!(sys.contains("问题9"));
    assert!(sys.contains("问题6"));
    assert!(!sys.contains("问题5"));
}

#[test]
fn rewrite_sends_core_only() {
    // D6: fact contents / observations never travel upstream.
    let mut d = digest();
    d.new_facts = vec!["我不吃辣".into()];
    d.top_activities = vec![("护肤".into(), 3)];
    struct CaptureUser {
        seen_user: std::cell::RefCell<String>,
    }
    impl Reasoner for CaptureUser {
        fn complete(&self, _system: &str, user: &str) -> Result<String> {
            *self.seen_user.borrow_mut() = user.to_string();
            Ok("5 3 2 1".into())
        }
    }
    let cap = CaptureUser {
        seen_user: Default::default(),
    };
    rewrite_digest(&cap, &d, None).unwrap();
    let sent = cap.seen_user.borrow();
    assert!(!sent.contains("我不吃辣"));
    assert!(!sent.contains("护肤"));
}

#[test]
fn rewrite_accepts_when_counts_survive() {
    let r = EchoReasoner {
        reply: "老板，这周你跟我说了 5 件事，我排进了 3 场会，2 次提醒里有 1 次已经按时喊你了。"
            .into(),
        seen_system: Default::default(),
    };
    let out = rewrite_digest(&r, &digest(), Some(&persona())).unwrap();
    assert!(out.is_some());
    assert!(r.seen_system.borrow().contains("称呼用户为「老板」"));
}

#[test]
fn rewrite_rejects_dropped_or_mangled_counts() {
    // "3" and "5" missing → the model paraphrased the facts away.
    let r = EchoReasoner {
        reply: "这周挺充实的，我帮你安排了几场会，提醒了 2 次，其中 1 次准点。".into(),
        seen_system: Default::default(),
    };
    assert!(rewrite_digest(&r, &digest(), None).unwrap().is_none());
    // Empty reply is rejected too.
    let r = EchoReasoner {
        reply: "  ".into(),
        seen_system: Default::default(),
    };
    assert!(rewrite_digest(&r, &digest(), None).unwrap().is_none());
}

#[test]
fn zero_counts_are_not_required_in_rewrite() {
    // dangerous_attempts=0 → "0" need not appear; all non-zero counts do.
    let d = digest();
    assert!(super::digest_counts_preserved(
        &d,
        "5 条输入、3 场会、2 次提醒、1 次触发"
    ));
    assert!(!super::digest_counts_preserved(&d, "5 条输入、3 场会"));
}

#[test]
fn chat_reply_ui_parses_prose_and_broken_json() {
    // Pure-text envelope: message extracted, ui skipped (nothing interactive).
    let r = EchoReasoner {
        reply: r#"{"version":1,"components":[{"type":"text","content":"你好呀"}]}"#.into(),
        seen_system: Default::default(),
    };
    let (msg, ui) = chat_reply_ui(&r, "你好", now(), None, &ChatContext::default()).unwrap();
    assert_eq!(msg, "你好呀");
    assert!(ui.is_none());

    // Interactive envelope with fences survives.
    let r = EchoReasoner {
        reply: "```json\n{\"version\":1,\"components\":[{\"type\":\"text\",\"content\":\"记吗？\"},{\"type\":\"choice\",\"options\":[{\"label\":\"记\",\"action\":{\"command\":\"ingest\",\"args\":{\"text\":\"明天9点开会\"}}},{\"label\":\"算了\",\"action\":{\"command\":\"checkin_answer\",\"args\":{\"text\":\"我在忙\"}}}]}]}\n```".into(),
        seen_system: Default::default(),
    };
    let (msg, ui) =
        chat_reply_ui(&r, "明天好像要开会", now(), None, &ChatContext::default()).unwrap();
    assert_eq!(msg, "记吗？");
    assert!(ui.is_some());

    // Plain prose is the message itself.
    let r = EchoReasoner {
        reply: "在的，怎么了？".into(),
        seen_system: Default::default(),
    };
    let (msg, ui) = chat_reply_ui(&r, "在吗", now(), None, &ChatContext::default()).unwrap();
    assert_eq!(msg, "在的，怎么了？");
    assert!(ui.is_none());

    // Broken JSON never reaches the user raw.
    let r = EchoReasoner {
        reply: r#"{"version":1,"components":[{"type":"iframe"#.into(),
        seen_system: Default::default(),
    };
    let (msg, ui) = chat_reply_ui(&r, "在吗", now(), None, &ChatContext::default()).unwrap();
    assert!(msg.contains("格式异常"));
    assert!(ui.is_none());

    // Persona still rides along in the system prompt.
    let r = EchoReasoner {
        reply: "好的老板。".into(),
        seen_system: Default::default(),
    };
    chat_reply_ui(&r, "你好", now(), Some(&persona()), &ChatContext::default()).unwrap();
    assert!(r.seen_system.borrow().contains("称呼用户为「老板」"));
}

#[test]
fn event_json_rejects_garbage() {
    assert!(parse_event_json(r#"{"none":true}"#, "orig", now()).is_none());
    assert!(parse_event_json("not json at all", "orig", now()).is_none());
    assert!(parse_event_json(
        r#"{"title":"x","kind":"party","start":"2026-07-08T18:30:00"}"#,
        "o",
        now()
    )
    .is_none());
    assert!(parse_event_json(
        r#"{"title":"x","kind":"other","start":"someday"}"#,
        "o",
        now()
    )
    .is_none());
    assert!(parse_event_json(
        r#"{"title":"","kind":"other","start":"2026-07-08T18:30:00"}"#,
        "o",
        now()
    )
    .is_none());
}

// ---- streaming (§3.6 第 7 条) ------------------------------------------

/// A reasoner whose `complete_streaming` emits scripted chunks in order, so
/// the sniff-router / think-filter can be exercised without a socket.
/// Mirrors `LlmReasoner`: think-filter the visible stream, return the
/// think-stripped full text.
struct ChunkedReasoner {
    chunks: Vec<String>,
}
impl Reasoner for ChunkedReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        Ok(strip_think_block(&self.chunks.concat()).trim().to_string())
    }
    fn complete_streaming(
        &self,
        _system: &str,
        _user: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<String> {
        let mut full = String::new();
        let mut filter = ThinkFilter::default();
        for c in &self.chunks {
            full.push_str(c);
            let vis = filter.push(c);
            if !vis.is_empty() {
                on_token(&vis);
            }
        }
        Ok(strip_think_block(&full).trim().to_string())
    }
}

#[test]
fn sse_line_classifies() {
    assert!(matches!(sse_line("data: [DONE]"), SseLine::Done));
    assert!(matches!(sse_line("data:[DONE]"), SseLine::Done));
    assert!(matches!(sse_line(""), SseLine::Ignore));
    assert!(matches!(sse_line(": keep-alive"), SseLine::Ignore));
    assert!(matches!(sse_line("event: message"), SseLine::Ignore));
    // Role-only opener and reasoning-only chunk carry no visible content.
    assert!(matches!(
        sse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
        SseLine::Ignore
    ));
    assert!(matches!(
        sse_line(r#"data: {"choices":[{"delta":{"reasoning_content":"盘算"}}]}"#),
        SseLine::Ignore
    ));
    match sse_line(r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#) {
        SseLine::Content(s) => assert_eq!(s, "你好"),
        _ => panic!("expected content"),
    }
}

#[test]
fn think_filter_strips_leading_block_across_chunks() {
    // A leading <think>…</think> split across delta boundaries is dropped.
    let mut f = ThinkFilter::default();
    assert_eq!(f.push("<thi"), "");
    assert_eq!(f.push("nk>盘"), "");
    assert_eq!(f.push("算中</thi"), "");
    assert_eq!(f.push("nk>你好"), "你好");
    assert_eq!(f.push("，在吗"), "，在吗"); // passthrough afterwards

    // Ordinary content that merely starts with '<' is not swallowed.
    let mut g = ThinkFilter::default();
    assert_eq!(g.push("<3 你好"), "<3 你好");

    // No think block at all → straight passthrough.
    let mut h = ThinkFilter::default();
    assert_eq!(h.push("在的"), "在的");
    assert_eq!(h.push("，怎么了"), "，怎么了");
}

#[test]
fn streaming_prose_flows_json_is_suppressed() {
    // Prose reply: every visible chunk streams; final message == full prose.
    let r = ChunkedReasoner {
        chunks: vec!["多喝".into(), "水就好".into()],
    };
    let mut seen = String::new();
    let (msg, ui) = chat_reply_ui_streaming(
        &r,
        "怎么办",
        now(),
        None,
        &ChatContext::default(),
        &mut |d| seen.push_str(d),
    )
    .unwrap();
    assert_eq!(seen, "多喝水就好");
    assert_eq!(msg, "多喝水就好");
    assert!(ui.is_none());

    // Legacy JSON envelope reply is reconciled whole and emits only its
    // safe text summary once after validation.
    let r = ChunkedReasoner {
        chunks: vec![
            "{\"version\":1,\"components\":[{\"type\":\"text\",\"content\":\"记吗？\"},".into(),
            "{\"type\":\"choice\",\"options\":[{\"label\":\"记\",\"action\":{\"command\":\"ingest\",\"args\":{\"text\":\"明天9点开会\"}}},{\"label\":\"算了\",\"action\":{\"command\":\"checkin_answer\",\"args\":{\"text\":\"我在忙\"}}}]}]}".into(),
        ],
    };
    let mut seen = String::new();
    let (msg, ui) = chat_reply_ui_streaming(
        &r,
        "明天开会",
        now(),
        None,
        &ChatContext::default(),
        &mut |d| seen.push_str(d),
    )
    .unwrap();
    assert_eq!(seen, "记吗？");
    assert_eq!(msg, "记吗？");
    assert!(ui.is_some());
}

#[test]
fn default_streaming_impl_emits_once() {
    // A reasoner without a streaming backend still drives the callback via
    // the default trait impl: the whole reply as a single token.
    let r = EchoReasoner {
        reply: "在的".into(),
        seen_system: Default::default(),
    };
    let mut calls = 0;
    let mut seen = String::new();
    let full = r
        .complete_streaming("sys", "hi", &mut |d| {
            calls += 1;
            seen.push_str(d);
        })
        .unwrap();
    assert_eq!(full, "在的");
    assert_eq!(seen, "在的");
    assert_eq!(calls, 1);
}

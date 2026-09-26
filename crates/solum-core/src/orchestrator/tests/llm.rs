//! llm. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};
// The canned reasoner lives with the widget tests that introduced it.
use super::widgets::FakeReasoner;

pub(super) struct CountingReasoner(pub(super) std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl crate::extract::Reasoner for CountingReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(r#"{"decisions":[{"index":0,"kind":"keep"}]}"#.into())
    }
}

#[test]
fn chat_uses_reasoner_when_present() {
    let mut o = Orchestrator::in_memory().unwrap();
    // Unauthenticated / offline without reasoner: explicitly errors when invoking cloud chat.
    let err = o.ingest("今天天气怎么样", now()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "cloud reasoner error: 使用云端智能能力请先登录 Solum 账号"
    );
    o.set_reasoner(Box::new(FakeReasoner {
        reply: "天气我看不到，但我在。".into(),
    }));
    let out = o.ingest("今天天气怎么样", now()).unwrap();
    assert_eq!(out.intent, Intent::Chat);
    assert_eq!(out.message, "天气我看不到，但我在。");
}

/// A reasoner that streams its reply in scripted chunks (§3.6 第 7 条).
struct StreamReasoner {
    chunks: Vec<String>,
}
impl crate::extract::Reasoner for StreamReasoner {
    fn complete(&self, _s: &str, _u: &str) -> Result<String> {
        Ok(self.chunks.concat())
    }
    fn complete_streaming(&self, _s: &str, _u: &str, on: &mut dyn FnMut(&str)) -> Result<String> {
        let mut full = String::new();
        for c in &self.chunks {
            full.push_str(c);
            on(c);
        }
        Ok(full)
    }
}

#[test]
fn ingest_streaming_forwards_chat_prose() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(StreamReasoner {
        chunks: vec!["天气我".into(), "看不到，但我在。".into()],
    }));
    let mut seen = String::new();
    let out = o
        .ingest_streaming("今天天气怎么样", now(), &mut |d| seen.push_str(d))
        .unwrap();
    assert_eq!(out.intent, Intent::Chat);
    assert_eq!(seen, "天气我看不到，但我在。");
    assert_eq!(out.message, "天气我看不到，但我在。");
    assert!(out.ui.is_none());
}

#[test]
fn ingest_streaming_suppresses_envelope_json() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(StreamReasoner {
            chunks: vec![
                "{\"version\":1,\"components\":[{\"type\":\"text\",\"content\":\"记吗？\"},".into(),
                "{\"type\":\"choice\",\"options\":[{\"label\":\"记\",\"action\":{\"command\":\"ingest\",\"args\":{\"text\":\"明天9点开会\"}}},{\"label\":\"算了\",\"action\":{\"command\":\"checkin_answer\",\"args\":{\"text\":\"我在忙\"}}}]}]}".into(),
            ],
        }));
    let mut seen = String::new();
    // A chat-routed utterance whose (scripted) reply is a legacy GenUI
    // envelope: only its validated text summary is emitted once.
    let out = o
        .ingest_streaming("今天天气怎么样", now(), &mut |d| seen.push_str(d))
        .unwrap();
    assert_eq!(seen, "记吗？");
    assert_eq!(out.message, "记吗？");
    assert!(out.ui.is_some());
}

#[test]
fn llm_fallback_extraction_persists_event() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"title":"和Lily喝咖啡","kind":"reminder","start":"2026-12-24T15:00:00","location":null,"people":["Lily"]}"#.into(),
        }));
    // "圣诞节前一天" carries no rule-parsable time token at all, so the
    // offline extractor yields None and the cloud fallback takes over.
    let out = o.ingest("圣诞节前一天提醒我和Lily喝咖啡", now()).unwrap();
    let ev = out.event.expect("LLM fallback event");
    assert_eq!(ev.start, dt(2026, 12, 24, 15, 0));
    assert!(out.message.contains("云端解析"));
    // Persisted like any other event, reminders included.
    assert_eq!(o.agenda(now()).unwrap().len(), 1);
    assert!(!out.notifications.is_empty());
}

#[test]
fn llm_garbage_degrades_to_plain_record() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(FakeReasoner {
        reply: "抱歉我不知道你在说什么".into(),
    }));
    let out = o.ingest("圣诞节前一天提醒我和Lily喝咖啡", now()).unwrap();
    assert!(out.event.is_none());
    assert!(out.message.contains("已作为普通输入记录"));
}

/// Reasoner that records the system prompt (to assert persona injection).
pub(super) struct SpyReasoner {
    pub(super) reply: String,
    pub(super) seen_system: std::sync::Arc<std::sync::Mutex<String>>,
}
impl crate::extract::Reasoner for SpyReasoner {
    fn complete(&self, system: &str, _user: &str) -> Result<String> {
        *self.seen_system.lock().unwrap() = system.to_string();
        Ok(self.reply.clone())
    }
}

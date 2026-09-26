//! persona. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::now;
use super::llm::SpyReasoner;
// The canned reasoner lives with the widget tests that introduced it.
use super::widgets::FakeReasoner;

fn manual_persona() -> crate::persona::PersonaDraft {
    crate::persona::PersonaDraft {
        nickname: Some("老板".into()),
        tone: "干练".into(),
        ..Default::default()
    }
}

#[test]
fn persona_versions_rollback_and_survive_reopen() {
    let mut o = Orchestrator::in_memory().unwrap();
    assert!(o.persona().is_none());
    let v1 = o
        .set_persona(manual_persona(), Some("初版".into()), now())
        .unwrap();
    assert_eq!(v1.version, 1);
    let mut d2 = manual_persona();
    d2.tone = "活泼".into();
    let v2 = o.set_persona(d2, None, now()).unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(o.persona().unwrap().draft.tone, "活泼");

    o.rollback_persona(1).unwrap();
    assert_eq!(o.persona().unwrap().draft.tone, "干练");
    assert_eq!(o.persona_versions().unwrap().len(), 2);

    // All-empty drafts are rejected before touching the store.
    assert!(o.set_persona(Default::default(), None, now()).is_err());

    o.clear_persona().unwrap();
    assert!(o.persona().is_none());
    assert!(o.persona_versions().unwrap().is_empty());
}

#[test]
fn persona_import_preview_then_save() {
    let mut o = Orchestrator::in_memory().unwrap();
    let log = (0..25)
        .map(|i| format!("小王: 问题{i}\n我: 稳了稳了哈哈哈{i}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Preview is pure: nothing stored yet.
    let report = o.preview_persona_import(&log, "我").unwrap();
    assert_eq!(report.my_messages, 25);
    assert!(o.persona().is_none());

    // User confirms (possibly after editing) → saved with source=import.
    let mut draft = report.suggested.clone();
    draft.nickname = Some("老板".into());
    let p = o
        .import_persona(draft, Some("从聊天记录导入".into()), now())
        .unwrap();
    assert_eq!(p.source, "import");
    assert_eq!(o.persona().unwrap().version, p.version);
    assert!(o
        .persona()
        .unwrap()
        .draft
        .style_notes
        .as_deref()
        .unwrap()
        .contains("本地提取"));

    // Bad nickname → helpful error, still nothing extra saved.
    let err = format!(
        "{}",
        o.preview_persona_import(&log, "不存在的人").unwrap_err()
    );
    assert!(err.contains("小王"));
}

#[test]
fn chat_carries_active_persona_to_cloud() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_persona(manual_persona(), None, now()).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    o.set_reasoner(Box::new(SpyReasoner {
        reply: "收到，老板。".into(),
        seen_system: seen.clone(),
    }));
    let out = o.ingest("今天天气怎么样", now()).unwrap();
    assert_eq!(out.message, "收到，老板。");
    assert!(seen.lock().unwrap().contains("称呼用户为「老板」"));
}

#[test]
fn review_text_styles_and_degrades() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开会", now()).unwrap();

    // Offline: plain digest, styled=false.
    let (text, styled) = o.review_text(now() - Duration::days(7), now()).unwrap();
    assert!(!styled);
    assert!(text.contains("复盘简报"));

    // Cloud reply that keeps the counts → styled=true.
    o.set_reasoner(Box::new(FakeReasoner {
        reply: "这周你交代了 1 件事，我排了 1 场会并计划了 1 次提醒。".into(),
    }));
    let (text, styled) = o.review_text(now() - Duration::days(7), now()).unwrap();
    assert!(styled);
    assert!(text.contains("1 场会"));

    // Cloud reply that mangles the numbers → fall back to the plain render.
    o.set_reasoner(Box::new(FakeReasoner {
        reply: "这周啥也没发生。".into(),
    }));
    let (text, styled) = o.review_text(now() - Duration::days(7), now()).unwrap();
    assert!(!styled);
    assert!(text.contains("复盘简报"));
}

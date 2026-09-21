use solum_core::model::MemoryLayer;
use solum_core::Orchestrator;

use crate::helpers::{confirm_and_run_tool, parse_flexible};

#[test]
fn deletion_confirmation_cancels_by_default_and_audits_explicit_approval() {
    let now = parse_flexible("2026-07-06T10:00:00").unwrap();
    for tool in ["memory_forget", "persona_clear"] {
        let mut o = Orchestrator::in_memory().unwrap();
        o.ingest("明天下午3点开会", now).unwrap();
        o.set_persona(
            solum_core::persona::PersonaDraft {
                tone: "简洁".into(),
                ..Default::default()
            },
            None,
            now,
        )
        .unwrap();
        let event = o
            .ledger()
            .unwrap()
            .into_iter()
            .find(|row| row.layer == MemoryLayer::Event)
            .unwrap();
        let args = if tool == "memory_forget" {
            serde_json::json!({"layer": "event", "id": event.id}).to_string()
        } else {
            "{}".into()
        };
        for answer in ["", "\n", "no\n"] {
            confirm_and_run_tool(
                &mut o,
                tool,
                &args,
                &mut answer.as_bytes(),
                &mut Vec::new(),
                || Ok(now),
            )
            .unwrap();
            assert!(o
                .ledger()
                .unwrap()
                .iter()
                .any(|row| row.layer == MemoryLayer::Event));
            assert_eq!(o.persona_versions().unwrap().len(), 1);
            assert!(o.audit_log().unwrap().is_empty());
        }
        let mut output = Vec::new();
        confirm_and_run_tool(
            &mut o,
            tool,
            &args,
            &mut "yes\n".as_bytes(),
            &mut output,
            || Ok(now),
        )
        .unwrap();
        assert!(String::from_utf8(output).unwrap().contains("输入 yes"));
        if tool == "memory_forget" {
            assert!(!o
                .ledger()
                .unwrap()
                .iter()
                .any(|row| row.layer == MemoryLayer::Event));
        } else {
            assert!(o.persona_versions().unwrap().is_empty());
            assert!(o.persona().is_none());
        }
        let audit = o.audit_log().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].tool, tool);
        assert_eq!(audit[0].decision, "executed");
        assert!(audit[0].token_id.is_some());
    }
}

#[test]
fn deletion_confirmation_rechecks_time_after_input() {
    let now = parse_flexible("2026-07-06T10:00:00").unwrap();
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_persona(
        solum_core::persona::PersonaDraft {
            tone: "简洁".into(),
            ..Default::default()
        },
        None,
        now,
    )
    .unwrap();
    let mut calls = 0;
    let result = confirm_and_run_tool(
        &mut o,
        "persona_clear",
        "{}",
        &mut "yes\n".as_bytes(),
        &mut Vec::new(),
        || {
            calls += 1;
            Ok(if calls == 1 {
                now
            } else {
                now + chrono::Duration::minutes(11)
            })
        },
    );
    assert!(result.is_err());
    assert_eq!(o.persona_versions().unwrap().len(), 1);
}

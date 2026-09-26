//! import. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::{dt, now};

/// Restoring must go through the Guard like any other broad write, and
/// the preview must state the real numbers before the user confirms.
#[test]
fn importing_a_backup_needs_the_guard_and_previews_real_counts() {
    let mut source = Orchestrator::in_memory().unwrap();
    source.ingest("明天下午3点开会", now()).unwrap();
    let doc = crate::export::build_export(&source.store, now()).unwrap();
    let args = serde_json::json!({ "document": doc }).to_string();

    let mut target = Orchestrator::in_memory().unwrap();
    // No token: refused, and nothing written.
    assert!(target.run_tool("data_import", &args, None, now()).is_err());
    assert!(target.store.list_events().unwrap().is_empty());

    let pending = target
        .request_confirmation("data_import", &args, now())
        .unwrap();
    let preview = &pending.request.effect_preview;
    assert!(preview.contains("events 1 条"), "{preview}");
    assert!(preview.contains("不会删除"), "{preview}");

    let token = target.confirm(&pending.id, now()).unwrap();
    let message = target
        .run_tool("data_import", &args, Some(token), now())
        .unwrap();
    assert!(message.contains("写入"), "{message}");
    assert_eq!(target.store.list_events().unwrap().len(), 1);
    assert_eq!(target.audit_log().unwrap().len(), 2);
}

#[test]
fn import_audit_never_retains_document_body_or_error_fields() {
    const PRIVATE: &str = "synthetic-private-import-marker";
    let mut source = Orchestrator::in_memory().unwrap();
    source.ingest("明天下午3点开会", now()).unwrap();
    let mut doc = crate::export::build_export(&source.store, now()).unwrap();
    // An ignored presentation field must not be persisted by auditing.
    doc["private_note"] = serde_json::json!(PRIVATE.repeat(1024));
    let args = serde_json::json!({ "document": doc }).to_string();
    let mut target = Orchestrator::in_memory().unwrap();
    assert!(target.run_tool("data_import", &args, None, now()).is_err());
    let pending = target
        .request_confirmation("data_import", &args, now())
        .unwrap();
    assert!(pending.request.summary.len() <= 128);
    assert!(!pending.request.summary.contains(PRIVATE));
    let token = target.confirm(&pending.id, now()).unwrap();
    // Redaction must not weaken exact argument binding.
    let changed = format!("{args} ");
    assert!(target
        .run_tool("data_import", &changed, Some(token.clone()), now())
        .is_err());
    target
        .run_tool("data_import", &args, Some(token), now())
        .unwrap();

    let mut bad = doc.clone();
    bad["_restore"][PRIVATE] = serde_json::json!([]);
    let bad_args = serde_json::json!({ "document": bad }).to_string();
    let pending = target
        .request_confirmation("data_import", &bad_args, now())
        .unwrap();
    let token = target.confirm(&pending.id, now()).unwrap();
    let error = target
        .run_tool("data_import", &bad_args, Some(token), now())
        .unwrap_err();
    assert!(
        error.to_string().contains(PRIVATE),
        "caller keeps actionable diagnostics"
    );

    let audit = target.audit_log().unwrap();
    assert_eq!(audit.len(), 4);
    for row in audit {
        assert!(row.summary.len() <= 128);
        assert!(row.detail.len() <= 256);
        assert!(!row.summary.contains(PRIVATE));
        assert!(!row.detail.contains(PRIVATE));
    }
    let exported = crate::export::build_export(&target.store, now()).unwrap();
    assert!(!exported.to_string().contains(PRIVATE));
    let malformed = format!("{{{PRIVATE}");
    assert!(target
        .run_tool("data_import", &malformed, None, now())
        .is_err());
    assert!(!target.export_json(now()).unwrap().contains(PRIVATE));
}

fn spend_draft() -> crate::widget::WidgetDefinitionDraft {
    serde_json::from_value(serde_json::json!({
        "name": "开销", "icon": "doc",
        "fields": [
            { "name": "item", "label": "项目", "type": "text", "required": true },
            { "name": "amount", "label": "金额", "type": "number", "required": false },
            { "name": "paid", "label": "已付", "type": "bool", "required": false },
            { "name": "when", "label": "时间", "type": "datetime", "required": false }
        ],
        "views": [
            { "type": "form", "fields": ["item", "amount", "paid", "when"] },
            { "type": "list", "fields": ["item", "amount"] },
            { "type": "table", "fields": ["item", "amount", "paid"], "sort_by": "amount" },
            { "type": "stat", "fields": ["amount", "paid", "item"] }
        ]
    }))
    .unwrap()
}

/// The stat operator is fixed by field type, so the same widget always
/// reports the same kind of number: numbers sum, booleans count trues,
/// everything else counts filled-in cells.
#[test]
fn stat_view_aggregates_by_field_type() {
    let o = Orchestrator::in_memory().unwrap();
    let definition = o
        .store
        .insert_widget_definition(&spend_draft(), now())
        .unwrap();
    for (item, amount, paid) in [
        ("午饭", 23.5, true),
        ("咖啡", 18.0, false),
        ("地铁", 4.0, true),
    ] {
        o.store
            .insert_widget_record(
                definition.id,
                &serde_json::json!({ "item": item, "amount": amount, "paid": paid }),
                now(),
            )
            .unwrap();
    }
    let records = o.store.list_widget_records(definition.id).unwrap();
    let stats = definition.schema.stats(&records);
    let by = |name: &str| stats.iter().find(|s| s.field == name).unwrap();
    assert_eq!(by("amount").value, 45.5);
    assert_eq!(by("amount").op, crate::widget::StatOp::Sum);
    assert_eq!(by("paid").value, 2.0);
    assert_eq!(by("paid").op, crate::widget::StatOp::CountTrue);
    assert_eq!(by("item").value, 3.0);
    assert_eq!(by("item").op, crate::widget::StatOp::CountFilled);

    // A widget with no stat view reports nothing rather than inventing one.
    let plain: crate::widget::WidgetDefinitionDraft = serde_json::from_value(serde_json::json!({
        "name": "笔记", "icon": "doc",
        "fields": [{ "name": "body", "label": "正文", "type": "text", "required": true }],
        "views": [
            { "type": "form", "fields": ["body"] },
            { "type": "list", "fields": ["body"] }
        ]
    }))
    .unwrap();
    assert!(plain.schema().stats(&records).is_empty());
}

/// `limit` bounds the records written, not how far down the schedule the
/// import may look, and every skip has to come back with a reason. Before
/// this, a run of unmappable entries at the top of the schedule consumed
/// the whole limit and the call returned a bare `0` — indistinguishable
/// from "you have no schedule at all".
#[test]
fn the_import_limit_counts_records_written_and_skips_explain_themselves() {
    let mut o = Orchestrator::in_memory().unwrap();
    // A widget whose only text field is required and whose title cannot be
    // filled from an event that maps to nothing usable.
    let draft: crate::widget::WidgetDefinitionDraft = serde_json::from_value(serde_json::json!({
        "name": "开销记录", "icon": "journal",
        "fields": [
            { "name": "item", "label": "项目", "type": "text", "required": true },
            { "name": "amount", "label": "金额", "type": "number", "required": true }
        ],
        "views": [
            { "type": "form", "fields": ["item", "amount"] },
            { "type": "list", "fields": ["item", "amount"] }
        ]
    }))
    .unwrap();
    let definition = o.store.insert_widget_definition(&draft, now()).unwrap();

    for utterance in ["明天下午3点开会", "后天上午9点体检", "7月28号下午2点面试"]
    {
        o.ingest(utterance, now()).unwrap();
    }

    // `amount` is required and no event can fill it, so everything skips —
    // and the caller learns that, instead of seeing a bare zero.
    let outcome = o
        .import_events_into_widget(definition.id, 2, now())
        .unwrap();
    assert_eq!(outcome.imported, 0);
    assert_eq!(
        outcome.skipped, 3,
        "the limit must not stop the scan at 2 when nothing was written"
    );
    assert!(!outcome.reasons.is_empty(), "a skip must say why");
    assert!(
        outcome.reasons[0].reason.contains("amount"),
        "the reason must name the field that could not be filled: {:?}",
        outcome.reasons[0]
    );
    assert!(
        !outcome.reasons[0].reason.contains("invalid input"),
        "the error Display prefix must not reach the user: {:?}",
        outcome.reasons[0]
    );

    // With a schema an event *can* satisfy, the limit caps writes.
    let ok_draft: crate::widget::WidgetDefinitionDraft =
        serde_json::from_value(serde_json::json!({
            "name": "日程副本", "icon": "calendar",
            "fields": [{ "name": "item", "label": "项目", "type": "text", "required": true }],
            "views": [
                { "type": "form", "fields": ["item"] },
                { "type": "list", "fields": ["item"] }
            ]
        }))
        .unwrap();
    let ok = o.store.insert_widget_definition(&ok_draft, now()).unwrap();
    let outcome = o.import_events_into_widget(ok.id, 2, now()).unwrap();
    assert_eq!(outcome.imported, 2, "limit caps what gets written");
    assert_eq!(outcome.skipped, 0);
    assert_eq!(o.store.list_widget_records(ok.id).unwrap().len(), 2);
}

/// Both event bridges are snapshots (设计稿 ⑦). Importing copies the
/// schedule in; promoting copies a record out; neither creates a live link.
#[test]
fn events_import_and_record_promotion_are_snapshots() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.ingest("明天下午3点开会", now()).unwrap();
    let definition = o
        .store
        .insert_widget_definition(&spend_draft(), now())
        .unwrap();

    let imported = o
        .import_events_into_widget(definition.id, 10, now())
        .unwrap();
    assert_eq!(imported.imported, 1);
    assert_eq!(imported.skipped, 0);
    let records = o.store.list_widget_records(definition.id).unwrap();
    assert_eq!(records[0].data["item"], "开会");
    assert!(records[0].data["when"].is_string());

    // Editing the source event must not reach the copy.
    let event_id = o.store.list_events().unwrap()[0].id.unwrap();
    o.store
        .update_event_times(event_id, dt(2026, 7, 30, 9, 0), None)
        .unwrap();
    let after = o.store.list_widget_records(definition.id).unwrap();
    assert_eq!(
        after[0].data["when"], records[0].data["when"],
        "快照被源事件改动带跑了"
    );

    // Promote a hand-entered record into a real schedule entry.
    let record = o
        .add_widget_record(
            definition.id,
            serde_json::json!({ "item": "牙医", "when": "2026-08-01T10:30" }),
            now(),
        )
        .unwrap();
    let before = o.store.list_events().unwrap().len();
    let promoted = o
        .promote_widget_record(definition.id, record.id, now())
        .unwrap();
    assert_eq!(promoted.title, "牙医");
    assert_eq!(o.store.list_events().unwrap().len(), before + 1);
    // It went through the normal path, so it has reminders and provenance.
    assert!(!o.store.list_notifications().unwrap().is_empty());
    assert!(o
        .store
        .list_raw_inputs()
        .unwrap()
        .iter()
        .any(|r| r.text.contains("[组件·开销]")));

    // A record with no usable title cannot be promoted — better a clear
    // refusal than an event called "".
    let blank = o
        .add_widget_record(definition.id, serde_json::json!({ "item": "  " }), now())
        .unwrap();
    assert!(o
        .promote_widget_record(definition.id, blank.id, now())
        .is_err());
}

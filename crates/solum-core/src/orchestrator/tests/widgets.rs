//! widgets. Part of the `orchestrator::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `orchestrator/tests.rs`.

use super::*;
// The date fixtures moved to `head.rs` when the suite was split.
use super::head::now;

/// Scripted reasoner for offline tests of the cloud seams.
pub(super) struct FakeReasoner {
    pub(super) reply: String,
}
impl crate::extract::Reasoner for FakeReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        Ok(self.reply.clone())
    }
}

#[test]
fn widget_creation_requires_preview_confirmation_and_record_crud_stays_offline() {
    let schema = r#"{"name":"课程记录","icon":"calendar","fields":[{"name":"course","label":"课程","type":"text","required":true},{"name":"starts_at","label":"开始","type":"time","required":true}],"views":[{"type":"form","fields":["course","starts_at"]},{"type":"list","fields":["course","starts_at"],"sort_by":"starts_at"}]}"#;
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(FakeReasoner {
        reply: schema.into(),
    }));

    // The ambiguous F1-shaped phrase is captured by the widget route,
    // but a model output is only a pending preview, never a write.
    let out = o.ingest("帮我弄一个日程表组件", now()).unwrap();
    assert_eq!(out.intent, Intent::CreateWidget);
    assert!(o.widget_definitions().unwrap().is_empty());
    let preview = out.widget_preview.expect("validated preview");
    let definition = o
        .confirm_widget_preview(&preview.preview_id, now())
        .unwrap();
    assert_eq!(o.widget_definitions().unwrap().len(), 1);

    let record = o
        .add_widget_record(
            definition.id,
            serde_json::json!({"course":"数学", "starts_at":"09:00"}),
            now(),
        )
        .unwrap();
    o.update_widget_record(
        definition.id,
        record.id,
        serde_json::json!({"course":"高数", "starts_at":"10:00"}),
    )
    .unwrap();
    assert_eq!(
        o.widget_records(definition.id).unwrap()[0].data["course"],
        "高数"
    );
    o.delete_widget_record(definition.id, record.id).unwrap();
    assert!(o.widget_records(definition.id).unwrap().is_empty());
}

#[test]
fn offline_ambiguous_widget_request_is_explicitly_unavailable_not_an_event() {
    let mut o = Orchestrator::in_memory().unwrap();
    let out = o.ingest("帮我做一个收支追踪表", now()).unwrap();
    assert_eq!(out.intent, Intent::CreateWidget);
    assert!(out.event.is_none());
    assert!(out.widget_preview.is_none());
    assert!(out.message.contains("创建组件需要已配置且可用的云端模型"));
    assert!(o.widget_definitions().unwrap().is_empty());
}

#[test]
fn widget_schema_rejection_is_logged_and_deletion_needs_the_guard() {
    let mut o = Orchestrator::in_memory().unwrap();
    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"name":"坏组件","icon":"doc","fields":[{"name":"body","label":"正文","type":"html"}],"views":[{"type":"form","fields":["body"]},{"type":"list","fields":["body"]}]}"#.into(),
        }));
    let rejected = o.ingest("创建一个笔记组件", now()).unwrap();
    assert_eq!(rejected.intent, Intent::CreateWidget);
    assert!(rejected.widget_preview.is_none());
    assert!(o.widget_definitions().unwrap().is_empty());
    assert_eq!(o.store.list_widget_schema_rejections().unwrap().len(), 1);

    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"name":"笔记","icon":"doc","fields":[{"name":"body","label":"正文","type":"text","required":true}],"views":[{"type":"form","fields":["body"]},{"type":"list","fields":["body"]}]}"#.into(),
        }));
    let preview = o
        .ingest("创建一个笔记组件", now())
        .unwrap()
        .widget_preview
        .unwrap();
    let definition = o
        .confirm_widget_preview(&preview.preview_id, now())
        .unwrap();
    o.add_widget_record(definition.id, serde_json::json!({"body":"一条记录"}), now())
        .unwrap();
    let args = serde_json::json!({"widget_id": definition.id}).to_string();
    assert!(o.run_tool("widget_delete", &args, None, now()).is_err());
    assert_eq!(o.widget_definitions().unwrap().len(), 1);
    let pending = o
        .request_confirmation("widget_delete", &args, now())
        .unwrap();
    assert!(pending.request.effect_preview.contains("1 条记录"));
    let token = o.confirm(&pending.id, now()).unwrap();
    o.run_tool("widget_delete", &args, Some(token), now())
        .unwrap();
    assert!(o.widget_definitions().unwrap().is_empty());
    assert_eq!(o.audit_log().unwrap().len(), 2);
}

/// 设计稿 ⑥ 把「组件总数 ≤ 8」与字段/视图上限并列。它无法从单份 schema 判断，
/// 所以走存储边界；撞上限的请求照样进拒绝日志，「用户想建第 9 个」是第二步要看的信号。
#[test]
fn widget_count_is_capped_and_the_refusal_is_logged() {
    let mut o = Orchestrator::in_memory().unwrap();
    let draft = |n: usize| {
        serde_json::from_value::<crate::widget::WidgetDefinitionDraft>(serde_json::json!({
                "name": format!("组件{n}"), "icon": "doc",
                "fields": [{ "name": "body", "label": "正文", "type": "text", "required": true }],
                "views": [{ "type": "form", "fields": ["body"] }, { "type": "list", "fields": ["body"] }]
            }))
            .unwrap()
    };
    for n in 0..crate::widget::MAX_WIDGETS {
        o.store.insert_widget_definition(&draft(n), now()).unwrap();
    }
    assert_eq!(
        o.widget_definitions().unwrap().len(),
        crate::widget::MAX_WIDGETS
    );

    // 第 9 个：预览照常生成（上限不是 schema 问题），确认时才被拒。
    o.set_reasoner(Box::new(FakeReasoner {
            reply: r#"{"name":"第九个","icon":"doc","fields":[{"name":"body","label":"正文","type":"text","required":true}],"views":[{"type":"form","fields":["body"]},{"type":"list","fields":["body"]}]}"#.into(),
        }));
    let preview = o
        .ingest("创建一个笔记组件", now())
        .unwrap()
        .widget_preview
        .unwrap();
    let error = o
        .confirm_widget_preview(&preview.preview_id, now())
        .unwrap_err();
    assert!(error.to_string().contains("上限"), "{error}");
    assert_eq!(
        o.widget_definitions().unwrap().len(),
        crate::widget::MAX_WIDGETS
    );
    assert_eq!(o.store.list_widget_schema_rejections().unwrap().len(), 1);

    // 删掉一个就应当重新腾出位置——上限是并发容量，不是终身配额。
    let victim = o.widget_definitions().unwrap()[0].id;
    o.store.delete_widget_definition(victim).unwrap();
    let preview = o
        .ingest("创建一个笔记组件", now())
        .unwrap()
        .widget_preview
        .unwrap();
    o.confirm_widget_preview(&preview.preview_id, now())
        .unwrap();
    assert_eq!(
        o.widget_definitions().unwrap().len(),
        crate::widget::MAX_WIDGETS
    );
}

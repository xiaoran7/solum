//! migrations. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

#[test]
pub(crate) fn migrates_and_versions() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
}

pub(super) fn course_draft() -> WidgetDefinitionDraft {
    serde_json::from_value(serde_json::json!({
        "name": "课程记录",
        "icon": "calendar",
        "fields": [
            { "name": "course", "label": "课程", "type": "text", "required": true },
            { "name": "start_time", "label": "开始时刻", "type": "time", "required": true }
        ],
        "views": [
            { "type": "form", "fields": ["course", "start_time"] },
            { "type": "list", "fields": ["start_time", "course"], "sort_by": "start_time" }
        ]
    }))
    .unwrap()
}

/// v13 reversed the first slice's "widgets never sync" invariant. The old
/// assertions here (no guid column, zero triggers) asserted a contract that
/// no longer holds, so they are replaced rather than kept.
#[test]
pub(crate) fn widgets_sync_and_deletion_writes_delete_ops_for_children() {
    let s = Store::open_in_memory().unwrap();
    let definition = s
        .insert_widget_definition(&course_draft(), dt(2026, 7, 19, 10, 0))
        .unwrap();
    s.insert_widget_record(
        definition.id,
        &serde_json::json!({ "course": "数学", "start_time": "09:05" }),
        dt(2026, 7, 19, 10, 1),
    )
    .unwrap();

    for table in ["widget_defs", "widget_fields", "widget_records"] {
        let columns: Vec<String> = s
            .conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            columns.iter().any(|column| column == "guid"),
            "{table} must carry a guid to sync"
        );
    }
    // The view lists survive the round trip through rows, including the
    // list view's independent field order and its sort field.
    let loaded = s.get_widget_definition(definition.id).unwrap();
    assert_eq!(loaded.schema.fields.len(), 2);
    let list = loaded
        .schema
        .views
        .iter()
        .find(|v| v.view_type == WidgetViewType::List)
        .unwrap();
    assert_eq!(list.fields, vec!["start_time", "course"]);
    assert_eq!(list.sort_by.as_deref(), Some("start_time"));

    // Deleting must emit a delete op per child row. SQLite does not fire
    // triggers for FK cascade deletions, so a cascade-only implementation
    // would leave other devices holding orphans — assert the ops exist.
    let ops_before: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sync_oplog WHERE op = 'delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        s.delete_widget_definition(definition.id).unwrap(),
        ("课程记录".into(), 1)
    );
    let deletes: Vec<String> = s
        .conn
        .prepare("SELECT tbl FROM sync_oplog WHERE op = 'delete' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let deletes = &deletes[ops_before as usize..];
    assert_eq!(deletes.iter().filter(|t| *t == "widget_records").count(), 1);
    assert_eq!(deletes.iter().filter(|t| *t == "widget_fields").count(), 2);
    assert_eq!(deletes.iter().filter(|t| *t == "widget_defs").count(), 1);
}

/// A v12 database stored the whole schema in one JSON column. Upgrading
/// must turn it into rows without losing a field, a view membership, the
/// per-view order, or the sort field.
#[test]
pub(crate) fn v12_schema_json_becomes_rows_without_losing_anything() {
    let s = Store::open_in_memory().unwrap();
    // Rebuild the v12 shape: drop the v13 columns/tables and restore the
    // single JSON column, then let migrate() do the upgrade.
    s.conn
        .execute_batch(
            "DROP TABLE widget_records; DROP TABLE widget_fields; DROP TABLE widget_defs;
                 CREATE TABLE widget_defs (
                   id INTEGER PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL,
                   schema_json TEXT NOT NULL, created_at TEXT NOT NULL);",
        )
        .unwrap();
    let legacy = serde_json::json!({
        "fields": [
            { "name": "course", "label": "课程", "type": "text", "required": true },
            { "name": "start_time", "label": "开始时刻", "type": "time", "required": true },
            { "name": "room", "label": "教室", "type": "text", "required": false }
        ],
        "views": [
            { "type": "form", "fields": ["course", "start_time", "room"] },
            { "type": "list", "fields": ["start_time", "course"], "sort_by": "start_time" }
        ]
    });
    s.conn
        .execute(
            "INSERT INTO widget_defs(name, icon, schema_json, created_at)
                 VALUES ('课程记录', 'calendar', ?1, '2026-07-19T10:00:00')",
            params![legacy.to_string()],
        )
        .unwrap();

    s.migrate().unwrap();

    let loaded = &s.list_widget_definitions().unwrap()[0];
    let names: Vec<&str> = loaded
        .schema
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["course", "start_time", "room"]);
    assert!(loaded.schema.fields[0].required);
    let form = loaded
        .schema
        .views
        .iter()
        .find(|v| v.view_type == WidgetViewType::Form)
        .unwrap();
    let list = loaded
        .schema
        .views
        .iter()
        .find(|v| v.view_type == WidgetViewType::List)
        .unwrap();
    assert_eq!(form.fields, vec!["course", "start_time", "room"]);
    // The list view kept both its own order and its exclusion of `room`.
    assert_eq!(list.fields, vec!["start_time", "course"]);
    assert_eq!(list.sort_by.as_deref(), Some("start_time"));
    // And the migrated rows are sync-ready, or the upgrade would leave
    // this device silently unable to share its existing widgets.
    let field_guids: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM widget_fields WHERE guid IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(field_guids, 3);
}

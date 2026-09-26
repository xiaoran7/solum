//! widget sync. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::migrations::course_draft;
use super::sync_core::dt;

/// Ship every locally-originated op from `from` into `to`, the way one
/// sync round would.
pub(super) fn push_all(from: &Store, to: &Store) {
    let ops: Vec<crate::sync::SyncOp> = from
        .local_ops_after(0)
        .unwrap()
        .into_iter()
        .map(|(_, op)| op)
        .collect();
    to.apply_remote_ops(&ops).unwrap();
}

pub(super) fn optional_field(name: &str, label: &str) -> WidgetField {
    WidgetField {
        name: name.into(),
        label: label.into(),
        field_type: crate::widget::WidgetFieldType::Text,
        required: false,
        options: vec![],
    }
}

/// The whole reason the schema is stored as rows. Two devices each add a
/// different field to the same widget while apart; after syncing both ways
/// **both fields must exist on both devices**. Under the previous
/// single-`schema_json`-column design this is exactly the case row-level
/// LWW silently destroyed — the later write replaced the whole schema.
#[test]
pub(crate) fn concurrent_field_additions_on_two_devices_merge_as_a_union() {
    let phone = Store::open_in_memory().unwrap();
    let desktop = Store::open_in_memory().unwrap();
    phone.set_meta("device_id", "phone").unwrap();
    desktop.set_meta("device_id", "desktop").unwrap();

    let created = phone
        .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
        .unwrap();
    phone
        .insert_widget_record(
            created.id,
            &serde_json::json!({ "course": "数学", "start_time": "09:05" }),
            dt(2026, 7, 20, 9, 1),
        )
        .unwrap();
    push_all(&phone, &desktop);

    let mirrored = desktop.list_widget_definitions().unwrap();
    assert_eq!(mirrored.len(), 1, "definition must reach the other device");
    assert_eq!(
        desktop.list_widget_records(mirrored[0].id).unwrap().len(),
        1
    );

    // Apart: each device adds its own field.
    phone
        .add_widget_field(
            created.id,
            &optional_field("feel", "感受"),
            dt(2026, 7, 20, 10, 0),
        )
        .unwrap();
    desktop
        .add_widget_field(
            mirrored[0].id,
            &optional_field("notes", "备注"),
            dt(2026, 7, 20, 10, 1),
        )
        .unwrap();

    push_all(&phone, &desktop);
    push_all(&desktop, &phone);

    for (label, store, id) in [
        ("phone", &phone, created.id),
        ("desktop", &desktop, mirrored[0].id),
    ] {
        let names: Vec<String> = store
            .get_widget_definition(id)
            .unwrap()
            .schema
            .fields
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert!(
            names.contains(&"feel".to_string()),
            "{label} lost feel: {names:?}"
        );
        assert!(
            names.contains(&"notes".to_string()),
            "{label} lost notes: {names:?}"
        );
        assert_eq!(names.len(), 4, "{label}: {names:?}");
    }

    // The pre-existing record still validates against the grown schema —
    // added fields are optional, so no record migration is needed.
    let definition = phone.get_widget_definition(created.id).unwrap();
    let record = &phone.list_widget_records(created.id).unwrap()[0];
    definition.schema.validate_record(&record.data).unwrap();
}

/// Every view slot must survive the trip to another device. The union
/// merge that [`concurrent_field_additions_on_two_devices_merge_as_a_union`]
/// checks was already right; what was missing is that the *payload* has to
/// carry `table_ord` / `stat_ord` at all. Asserting only that field names
/// arrive is what let the table and stat views vanish across sync while
/// every test stayed green — so this asserts the rendered view lists.
#[test]
pub(crate) fn all_four_view_slots_and_the_canonical_order_survive_sync() {
    let phone = Store::open_in_memory().unwrap();
    let desktop = Store::open_in_memory().unwrap();
    phone.set_meta("device_id", "phone").unwrap();
    desktop.set_meta("device_id", "desktop").unwrap();

    let draft: WidgetDefinitionDraft = serde_json::from_value(serde_json::json!({
        "name": "开销记录",
        "icon": "journal",
        "fields": [
            { "name": "item",   "label": "项目", "type": "text", "required": true },
            { "name": "amount", "label": "金额", "type": "number" },
            { "name": "paid",   "label": "已付", "type": "bool" },
            { "name": "when",   "label": "时间", "type": "datetime" }
        ],
        // Deliberately four different orders, so a slot that silently falls
        // back to a default cannot coincidentally match.
        "views": [
            { "type": "form",  "fields": ["item", "amount", "paid", "when"] },
            { "type": "list",  "fields": ["when", "item", "amount"], "sort_by": "when" },
            { "type": "table", "fields": ["when", "item", "amount", "paid"], "sort_by": "amount" },
            { "type": "stat",  "fields": ["amount", "paid"] }
        ]
    }))
    .unwrap();

    let created = phone
        .insert_widget_definition(&draft, dt(2026, 7, 20, 9, 0))
        .unwrap();
    push_all(&phone, &desktop);

    let mirrored = desktop.list_widget_definitions().unwrap();
    assert_eq!(mirrored.len(), 1);
    let there = desktop.get_widget_definition(mirrored[0].id).unwrap();
    let here = phone.get_widget_definition(created.id).unwrap();

    for view_type in WidgetViewType::ALL {
        let pick = |definition: &crate::widget::WidgetDefinition| {
            definition
                .schema
                .views
                .iter()
                .find(|v| v.view_type == *view_type)
                .map(|v| (v.fields.clone(), v.sort_by.clone()))
        };
        assert_eq!(
            pick(&there),
            pick(&here),
            "{} view differs after sync",
            view_type.as_str()
        );
        assert!(
            pick(&there).is_some(),
            "{} view did not survive sync at all",
            view_type.as_str()
        );
    }

    // The canonical field order is a synced property too: leaving `ord` out
    // of the payload lands every arriving field on 0, so the tie breaks on
    // the random guid and the two devices disagree.
    let names = |definition: &crate::widget::WidgetDefinition| {
        definition
            .schema
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&there), names(&here));
    assert_eq!(names(&here), vec!["item", "amount", "paid", "when"]);
}

/// A half-applied v13 split used to be unrecoverable: the field inserts
/// committed, `schema_json` stayed, and the next open re-inserted the same
/// names straight into `UNIQUE(widget_id, name)` — every subsequent open
/// failed the same way. The rewrite is one transaction, so a failure must
/// leave the database exactly as it was.
#[test]
pub(crate) fn a_failed_v13_split_rolls_back_instead_of_wedging_the_database() {
    let s = Store::open_in_memory().unwrap();
    s.conn
        .execute_batch(
            "DROP TABLE widget_records; DROP TABLE widget_fields; DROP TABLE widget_defs;
                 CREATE TABLE widget_defs (
                    id INTEGER PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL,
                    schema_json TEXT NOT NULL, created_at TEXT NOT NULL);",
        )
        .unwrap();
    let schema = serde_json::json!({
        "fields": [
            { "name": "course", "label": "课程", "type": "text", "required": true },
            { "name": "start_time", "label": "开始时刻", "type": "time", "required": true }
        ],
        "views": [
            { "type": "form", "fields": ["course", "start_time"] },
            { "type": "list", "fields": ["start_time", "course"] }
        ]
    })
    .to_string();
    s.conn
        .execute(
            "INSERT INTO widget_defs(id, name, icon, schema_json, created_at)
                 VALUES (1, '课程记录', 'calendar', ?1, '2026-07-19T10:00:00')",
            params![schema],
        )
        .unwrap();
    // Recreate the field table and pre-seed the row the split will collide
    // with, which is what a crashed first attempt leaves behind.
    s.migrate().unwrap();
    s.conn
        .execute("ALTER TABLE widget_defs ADD COLUMN schema_json TEXT", [])
        .unwrap();
    s.conn
        .execute(
            "UPDATE widget_defs SET schema_json = ?1 WHERE id = 1",
            params![schema],
        )
        .unwrap();
    // Drop only the *first* field, so the retry inserts one row
    // successfully and then collides on the second. Colliding on the very
    // first insert would leave nothing committed either way and the test
    // would pass with or without the transaction.
    s.conn
        .execute("DELETE FROM widget_fields WHERE name = 'course'", [])
        .unwrap();
    let before: i64 = s
        .conn
        .query_row("SELECT COUNT(*) FROM widget_fields", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, 1);

    assert!(
        s.migrate_widget_schema_to_rows().is_err(),
        "the duplicate names must still be an error"
    );

    // Rolled back: no half-written fields, and the column is still there,
    // so a later build can retry the whole split from a clean state.
    let after: i64 = s
        .conn
        .query_row("SELECT COUNT(*) FROM widget_fields", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, before, "a failed split must not commit any row");
    assert!(s.has_column("widget_defs", "schema_json").unwrap());
}

/// The v15 repair only speaks for rows this device actually knows about.
/// A device that merely received damaged rows holds the column defaults,
/// and must stay quiet rather than push its own copy back over the good
/// one when both devices upgrade.
#[test]
pub(crate) fn the_v15_view_slot_repair_only_re_emits_rows_that_carry_information() {
    let s = Store::open_in_memory().unwrap();
    s.insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
        .unwrap();
    // Emulate a device that received these rows before the payload carried
    // the slots: everything sits on the defaults.
    s.conn
        .execute(
            "UPDATE widget_fields SET ord = 0, table_ord = NULL, stat_ord = NULL",
            [],
        )
        .unwrap();
    s.conn.execute("DELETE FROM sync_oplog", []).unwrap();
    s.rebroadcast_widget_fields_view_slots().unwrap();
    let emitted: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sync_oplog WHERE tbl = 'widget_fields'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(emitted, 0, "a receiving device must not rebroadcast");

    // The authoring device does have something to say.
    s.conn
        .execute(
            "UPDATE widget_fields SET ord = 1 WHERE name = 'start_time'",
            [],
        )
        .unwrap();
    s.conn.execute("DELETE FROM sync_oplog", []).unwrap();
    s.rebroadcast_widget_fields_view_slots().unwrap();
    let emitted: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sync_oplog WHERE tbl = 'widget_fields'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(emitted, 1, "the row holding a non-default ord must re-emit");
}

/// The one case a grow-only set cannot resolve on its own: the same field
/// name added on both devices. Resolution must be deterministic and must
/// not silently discard the loser.
#[test]
pub(crate) fn same_named_concurrent_fields_resolve_deterministically_and_are_logged() {
    let phone = Store::open_in_memory().unwrap();
    let desktop = Store::open_in_memory().unwrap();
    phone.set_meta("device_id", "phone").unwrap();
    desktop.set_meta("device_id", "desktop").unwrap();

    let created = phone
        .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
        .unwrap();
    push_all(&phone, &desktop);
    let mirrored_id = desktop.list_widget_definitions().unwrap()[0].id;

    phone
        .add_widget_field(
            created.id,
            &optional_field("notes", "手机备注"),
            dt(2026, 7, 20, 10, 0),
        )
        .unwrap();
    desktop
        .add_widget_field(
            mirrored_id,
            &optional_field("notes", "桌面备注"),
            dt(2026, 7, 20, 10, 1),
        )
        .unwrap();

    push_all(&phone, &desktop);
    push_all(&desktop, &phone);

    // Exactly one `notes` survives on each device, and both devices agree
    // on which one — that is what "deterministic" has to mean here.
    let label_on = |store: &Store, id: i64| {
        store
            .get_widget_definition(id)
            .unwrap()
            .schema
            .fields
            .into_iter()
            .filter(|f| f.name == "notes")
            .map(|f| f.label)
            .collect::<Vec<_>>()
    };
    let phone_labels = label_on(&phone, created.id);
    let desktop_labels = label_on(&desktop, mirrored_id);
    assert_eq!(phone_labels.len(), 1, "{phone_labels:?}");
    assert_eq!(phone_labels, desktop_labels, "devices disagree");

    // The discarded one is visible, not silently gone.
    let rejections = phone.list_widget_schema_rejections().unwrap().len()
        + desktop.list_widget_schema_rejections().unwrap().len();
    assert!(rejections >= 1, "the dropped duplicate must be logged");
}

/// Merging may push a widget past MAX_FIELDS. That state is legal — the
/// user created every one of those fields — but it must stop accepting
/// new ones rather than truncate.
#[test]
pub(crate) fn a_merged_over_cap_schema_still_works_but_refuses_further_fields() {
    let s = Store::open_in_memory().unwrap();
    let definition = s
        .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
        .unwrap();
    // Force the over-cap state the way a merge would: straight into rows.
    for n in 0..(crate::widget::MAX_FIELDS + 2) {
        s.conn
            .execute(
                "INSERT INTO widget_fields(widget_id, name, label, field_type, required,
                       options_json, ord, form_ord, list_ord, created_at, guid)
                     VALUES (?1, ?2, ?3, 'text', 0, '[]', ?4, NULL, NULL, ?5, ?6)",
                params![
                    definition.id,
                    format!("merged_{n}"),
                    format!("合并字段{n}"),
                    100 + n as i64,
                    fmt_ts(&dt(2026, 7, 20, 11, 0)),
                    new_guid()
                ],
            )
            .unwrap();
    }
    let loaded = s.get_widget_definition(definition.id).unwrap();
    assert!(loaded.schema.fields.len() > crate::widget::MAX_FIELDS);
    // Still usable: records write and validate normally.
    s.insert_widget_record(
        definition.id,
        &serde_json::json!({ "course": "数学", "start_time": "09:05" }),
        dt(2026, 7, 20, 11, 1),
    )
    .unwrap();
    // But closed to growth.
    let error = s
        .add_widget_field(
            definition.id,
            &optional_field("one_more", "再来一个"),
            dt(2026, 7, 20, 11, 2),
        )
        .unwrap_err();
    assert!(error.to_string().contains("上限"), "{error}");
}

/// Added fields must be optional; a required one could never be satisfied
/// by records that already exist.
#[test]
pub(crate) fn added_fields_must_be_optional() {
    let s = Store::open_in_memory().unwrap();
    let definition = s
        .insert_widget_definition(&course_draft(), dt(2026, 7, 20, 9, 0))
        .unwrap();
    let mut field = optional_field("notes", "备注");
    field.required = true;
    let error = s
        .add_widget_field(definition.id, &field, dt(2026, 7, 20, 10, 0))
        .unwrap_err();
    assert!(error.to_string().contains("可空"), "{error}");
    assert!(s
        .add_widget_field(
            definition.id,
            &optional_field("notes", "备注"),
            dt(2026, 7, 20, 10, 0)
        )
        .is_ok());
}

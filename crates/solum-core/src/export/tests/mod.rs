//! `tests` -- undefined
//!
//! Part of the `export` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// Import clock for tests. Comfortably after every fixture's export time,
/// so the "a backup cannot be from the future" cap never distorts what a
/// test is actually about.
fn import_now() -> chrono::NaiveDateTime {
    dt(30, 12)
}
use crate::model::{Event, EventKind};
use chrono::NaiveDate;

fn dt(d: u32, h: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, d)
        .unwrap()
        .and_hms_opt(h, 0, 0)
        .unwrap()
}

#[test]
fn every_sqlite_table_has_an_explicit_backup_disposition() {
    use std::collections::BTreeSet;

    let unique = format!(
        "solum-backup-coverage-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let path = std::env::temp_dir().join(unique);
    {
        let store = Store::open(path.to_str().unwrap()).unwrap();
        assert!(store.schema_version().unwrap() > 0);
    }

    let connection = rusqlite::Connection::open(&path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let actual: BTreeSet<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    drop(statement);
    drop(connection);

    let declared: BTreeSet<String> = SQLITE_BACKUP_COVERAGE
        .iter()
        .map(|entry| entry.source.to_string())
        .collect();
    assert_eq!(
        actual, declared,
        "every new SQLite table must choose a backup disposition"
    );
    assert_eq!(
        declared.len(),
        SQLITE_BACKUP_COVERAGE.len(),
        "coverage registry contains a duplicate table"
    );

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn v2_restore_tables_match_the_audited_restorable_set() {
    use std::collections::BTreeSet;

    let store = Store::open_in_memory().unwrap();
    let restore = store.export_restore_rows().unwrap();
    let actual: BTreeSet<&str> = restore.keys().map(String::as_str).collect();
    let mut expected: BTreeSet<&str> = SQLITE_BACKUP_COVERAGE
        .iter()
        .filter(|entry| entry.coverage == BackupCoverage::RestorableV2)
        .map(|entry| entry.source)
        .collect();
    // `meta` is field-classified: only rule_table, proactivity and
    // persona_active are present in v2, never the whole table.
    expected.insert("meta");
    assert_eq!(actual, expected);

    let platform_sources: BTreeSet<&str> = PLATFORM_BACKUP_COVERAGE
        .iter()
        .map(|entry| entry.source)
        .collect();
    assert_eq!(
        platform_sources.len(),
        PLATFORM_BACKUP_COVERAGE.len(),
        "platform coverage registry contains a duplicate source"
    );
}

#[test]
fn export_contains_every_user_visible_layer() {
    let store = Store::open_in_memory().unwrap();
    let now = dt(6, 10);
    let raw = store
        .insert_raw_input("明天下午3点开会", "ingest_event", now)
        .unwrap();
    let ev = Event::new(
        "开会",
        EventKind::Meeting,
        dt(7, 15),
        "明天下午3点开会",
        now,
    );
    store.insert_event(&ev, Some(raw)).unwrap();

    let doc = build_export(&store, now).unwrap();
    assert_eq!(doc["format"], FORMAT);
    assert_eq!(doc["version"], EXPORT_VERSION);
    assert_eq!(doc["exported_at"], fmt_ts(&now));
    // 每一层都必须在场（就算是空数组）——漏一层就是导出承诺打折。
    for key in [
        "raw_inputs",
        "events",
        "notifications",
        "behavior_log",
        "suggestions",
        "memory_facts",
        "routines",
        "health_samples",
        "soulous_facts",
        "persona_versions",
        "importance_rules",
        "proactivity",
        "audit_log",
        "widget_defs",
        "widget_records",
    ] {
        assert!(!doc[key].is_null(), "missing export layer: {key}");
    }
    assert_eq!(doc["raw_inputs"].as_array().unwrap().len(), 1);
    assert_eq!(doc["events"].as_array().unwrap().len(), 1);
    assert_eq!(doc["events"][0]["title"], "开会");
}

/// An empty-array key satisfies the presence check above, so assert on
/// actual content here.
#[test]
fn export_carries_widget_definitions_and_their_records() {
    let store = Store::open_in_memory().unwrap();
    let now = dt(6, 10);
    let draft: crate::widget::WidgetDefinitionDraft = serde_json::from_value(json!({
        "name": "收支记录",
        "icon": "doc",
        "fields": [
            { "name": "item", "label": "项目", "type": "text", "required": true },
            { "name": "amount", "label": "金额", "type": "number", "required": true }
        ],
        "views": [
            { "type": "form", "fields": ["item", "amount"] },
            { "type": "list", "fields": ["item", "amount"], "sort_by": "amount" }
        ]
    }))
    .unwrap();
    let definition = store.insert_widget_definition(&draft, now).unwrap();
    store
        .insert_widget_record(definition.id, &json!({ "item": "午饭", "amount": 23 }), now)
        .unwrap();

    let doc = build_export(&store, now).unwrap();
    assert_eq!(doc["widget_defs"].as_array().unwrap().len(), 1);
    assert_eq!(doc["widget_defs"][0]["name"], "收支记录");
    // The schema travels with the definition, or the records are just
    // anonymous JSON blobs to whoever reads the export later.
    assert_eq!(doc["widget_defs"][0]["schema"]["fields"][0]["name"], "item");
    assert_eq!(doc["widget_records"].as_array().unwrap().len(), 1);
    assert_eq!(doc["widget_records"][0]["widget_id"], definition.id);
    assert_eq!(doc["widget_records"][0]["data"]["item"], "午饭");

    // `_restore` is what a restore actually reads, and it shares
    // `SYNC_PAYLOADS` with the sync triggers — so a slot missing there is
    // missing from backups too. Assert every view slot and the canonical
    // order are present as keys, not just that the layer exists.
    let field = &doc["_restore"]["widget_fields"][0];
    for key in ["ord", "form_ord", "list_ord", "table_ord", "stat_ord"] {
        assert!(
            field.get(key).is_some(),
            "_restore.widget_fields lost {key}: {field}"
        );
    }
}

fn seeded_store() -> (Store, NaiveDateTime) {
    let store = Store::open_in_memory().unwrap();
    let now = dt(6, 10);
    let raw = store
        .insert_raw_input("明天下午3点开会", "ingest_event", now)
        .unwrap();
    let ev = Event::new(
        "开会",
        EventKind::Meeting,
        dt(7, 15),
        "明天下午3点开会",
        now,
    );
    store.insert_event(&ev, Some(raw)).unwrap();
    let draft: crate::widget::WidgetDefinitionDraft = serde_json::from_value(json!({
        "name": "收支记录",
        "icon": "doc",
        "fields": [{ "name": "item", "label": "项目", "type": "text", "required": true }],
        "views": [
            { "type": "form", "fields": ["item"] },
            { "type": "list", "fields": ["item"] }
        ]
    }))
    .unwrap();
    let definition = store.insert_widget_definition(&draft, now).unwrap();
    store
        .insert_widget_record(definition.id, &json!({ "item": "午饭" }), now)
        .unwrap();
    (store, now)
}

/// A backup is only a backup if it restores. Export from one store,
/// import into an empty one, and the data must actually be there —
/// including the FK link from the event back to its raw input.
#[test]
fn an_export_restores_into_an_empty_store_including_fk_links() {
    let (source, now) = seeded_store();
    let doc = build_export(&source, now).unwrap();

    let restored = Store::open_in_memory().unwrap();
    let (plan, counts) = import_document(&restored, &doc, import_now()).unwrap();
    assert_eq!(plan.version, EXPORT_VERSION);
    assert!(counts.applied > 0);

    let events = restored.list_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "开会");
    assert_eq!(restored.list_raw_inputs().unwrap().len(), 1);
    // The event must point back at the restored raw input, not at
    // nothing — that link is exactly what a guid-less export could not
    // carry. Re-exporting surfaces it: the FK is emitted as a guid.
    let round_trip = build_export(&restored, now).unwrap();
    let linked = &round_trip["_restore"]["events"][0]["raw_input_guid"];
    assert!(
        linked.is_string(),
        "event lost its raw input link: {linked:?}"
    );
    assert_eq!(
        linked, &doc["_restore"]["events"][0]["raw_input_guid"],
        "the restored link points at a different raw input"
    );

    let widgets = restored.list_widget_definitions().unwrap();
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].name, "收支记录");
    assert_eq!(widgets[0].schema.fields.len(), 1);
    assert_eq!(
        restored.list_widget_records(widgets[0].id).unwrap().len(),
        1
    );
}

/// Importing twice must not double anything — the LWW gate treats the
/// second pass as re-delivery of ops it already has.
/// P2 regression: `exported_at` is only the file's own claim, and it is
/// used as the LWW stamp. A doctored far-future stamp does not merely win
/// once — it is written onto the rows, so it outranks every local edit the
/// user makes *from then on*. The record becomes permanently frozen at the
/// attacker's version, and nothing in the UI explains why edits do not
/// stick.
///
/// Such a document is refused outright rather than repaired: clamping its
/// stamp down to "now" would leave old content stamped fresh, which still
/// beats every edit made before the import.
#[test]
fn a_backup_claiming_a_future_export_time_is_refused() {
    let (source, now) = seeded_store();
    let mut doc = build_export(&source, now).unwrap();
    doc["exported_at"] = json!("2099-01-01T00:00:00");

    let device = Store::open_in_memory().unwrap();
    let err = import_document(&device, &doc, import_now()).unwrap_err();
    assert!(format!("{err}").contains("在未来"), "got {err}");
    assert!(
        device.list_events().unwrap().is_empty(),
        "a refused import must not have written anything"
    );
}

/// P2 regression: unknown tables are refused outright. Letting them flow
/// into `sync_quarantine` meant a crafted backup could fill it and evict
/// real cross-device data that was genuinely waiting for an upgrade.
#[test]
fn a_backup_with_an_unknown_table_is_refused_not_quarantined() {
    let (source, now) = seeded_store();
    let mut doc = build_export(&source, now).unwrap();
    doc["_restore"]["definitely_not_a_solum_table"] =
        json!([{ "guid": "x", "whatever": "payload" }]);

    let device = Store::open_in_memory().unwrap();
    let err = import_document(&device, &doc, import_now()).unwrap_err();
    assert!(format!("{err}").contains("不认识的表"), "got {err}");
    assert_eq!(
        device.sync_quarantine_stats().unwrap(),
        (0, 0),
        "a refused import must not have parked anything"
    );
}

#[test]
fn importing_the_same_document_twice_is_a_no_op() {
    let (source, now) = seeded_store();
    let doc = build_export(&source, now).unwrap();
    let restored = Store::open_in_memory().unwrap();

    import_document(&restored, &doc, import_now()).unwrap();
    let after_first = restored.list_events().unwrap().len();
    let (_, second) = import_document(&restored, &doc, import_now()).unwrap();

    assert_eq!(restored.list_events().unwrap().len(), after_first);
    assert_eq!(restored.list_raw_inputs().unwrap().len(), 1);
    assert_eq!(second.applied, 0, "second import should apply nothing");
}

/// Restoring an old backup must not resurrect stale content over edits
/// made since. The backup's rows are stamped with the export time, so
/// LWW lets the newer local row win.
#[test]
fn an_older_backup_does_not_clobber_newer_local_edits() {
    let (source, now) = seeded_store();
    let doc = build_export(&source, now).unwrap();

    let device = Store::open_in_memory().unwrap();
    import_document(&device, &doc, import_now()).unwrap();
    let event_id = device.list_events().unwrap()[0].id.unwrap();
    device
        .update_event_times(event_id, dt(9, 16), None)
        .unwrap();
    let after_edit = device.list_events().unwrap()[0].start;

    // Re-import the *old* document on top of the newer local edit.
    import_document(&device, &doc, import_now()).unwrap();
    assert_eq!(
        device.list_events().unwrap()[0].start,
        after_edit,
        "an old backup overwrote a newer local edit"
    );
}

/// A v1 file has no restorable section. Say so plainly instead of
/// importing nothing and reporting success.
#[test]
fn a_pre_v2_document_is_refused_with_an_explanation() {
    let store = Store::open_in_memory().unwrap();
    let legacy = json!({ "format": "pa-export", "version": 1, "events": [] });
    let error = plan_import(&legacy).unwrap_err().to_string();
    assert!(error.contains("v1"), "{error}");
    assert!(error.contains("人工查阅"), "{error}");
    assert!(import_document(&store, &legacy, import_now()).is_err());

    let alien = json!({ "format": "something-else", "version": 2 });
    assert!(plan_import(&alien).is_err());
}

/// The 2026-07-20 rename changed the format *name* and nothing else. A
/// backup taken before it must still restore — refusing it would silently
/// void every file the user already exported, which is exactly the
/// "备份不能还原就不叫备份" failure v2 was created to fix.
#[test]
fn a_backup_written_before_the_rename_still_restores() {
    let source = Store::open_in_memory().unwrap();
    let now = dt(6, 10);
    let raw = source
        .insert_raw_input("明天下午3点开会", "ingest_event", now)
        .unwrap();
    let ev = Event::new(
        "开会",
        EventKind::Meeting,
        dt(7, 15),
        "明天下午3点开会",
        now,
    );
    source.insert_event(&ev, Some(raw)).unwrap();

    // Exactly what the pre-rename build wrote: same bytes, old name.
    let mut doc = build_export(&source, now).unwrap();
    doc["format"] = json!(LEGACY_FORMAT);

    let target = Store::open_in_memory().unwrap();
    let (plan, counts) = import_document(&target, &doc, import_now()).unwrap();
    assert_eq!(plan.version, EXPORT_VERSION);
    assert!(counts.applied > 0, "legacy backup restored no rows");
    assert_eq!(target.list_events().unwrap().len(), 1);
    assert_eq!(target.list_events().unwrap()[0].title, "开会");
}

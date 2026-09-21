use super::*;
use crate::guard::Decision;
use crate::model::{EventKind, RiskLevel};
use chrono::NaiveDate;

    /// A private scratch directory; avoids pulling in a dev-dependency just to
    /// exercise three filesystem moves.
    pub(crate) fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solum-adopt-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writer-vs-writer collisions between the shell's two long-lived
    /// connections (`orch`, `sync_store`) must wait, not error: with SQLite's
    /// default zero timeout a losing write surfaced as SQLITE_BUSY mid-`fire_due`
    /// and vanished into the ticker's best-effort logging, silently dropping
    /// half of a reminder fire. If this value drifts back to 0, that loss
    /// quietly comes back.
    #[test]
    pub(crate) fn open_sets_a_busy_timeout_so_writer_collisions_wait_instead_of_erroring() {
        let dir = scratch("busy-timeout");
        let store = Store::open(dir.join("solum.sqlite").to_str().unwrap()).unwrap();
        let ms: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            ms, 5000,
            "busy_timeout drifted from the configured 5s writer-wait"
        );
    }

    /// A crash between parking an oversized op and advancing the push cursor
    /// makes the next round re-read and re-park the same op. That must be a
    /// no-op, not a growing pile of hold rows.
    #[test]
    pub(crate) fn hold_push_op_is_idempotent_per_oplog_id() {
        let store = Store::open_in_memory().unwrap();
        let op = crate::sync::SyncOp {
            tbl: "events".into(),
            guid: "e".repeat(32),
            op: "upsert".into(),
            payload: Some(r#"{"title":"超大"}"#.into()),
            hlc: "2026-08-21T10:00:00.000".into(),
            origin: "self".into(),
        };
        store.hold_push_op(7, &op, "单条超限").unwrap();
        store.hold_push_op(7, &op, "单条超限").unwrap();
        assert_eq!(store.push_hold_count().unwrap(), 1);
        store.hold_push_op(8, &op, "单条超限").unwrap();
        assert_eq!(store.push_hold_count().unwrap(), 2);
    }

    /// The P1 this guards: data committed but still living in the WAL must
    /// survive adoption. The old file-by-file rename could move the main
    /// database and leave the WAL behind, and because the next launch then saw
    /// a database already at the new path it never retried — the committed
    /// rows were simply gone, silently and permanently.
    #[test]
    pub(crate) fn adopting_a_pre_rename_database_keeps_data_still_in_the_wal() {
        let dir = scratch("moves");
        let legacy = dir.join("pa.sqlite");
        let current = dir.join("solum.sqlite");

        // A real WAL-mode database with a committed row that has *not* been
        // checkpointed into the main file yet.
        {
            let store = Store::open(legacy.to_str().unwrap()).unwrap();
            store
                .insert_event(
                    &Event::new(
                        "改名前的会议",
                        crate::model::EventKind::Meeting,
                        dt(2026, 7, 9, 15, 0),
                        "src",
                        dt(2026, 7, 7, 10, 0),
                    ),
                    None,
                )
                .unwrap();
            assert!(
                dir.join("pa.sqlite-wal").exists(),
                "precondition: the commit is in the WAL"
            );
        }

        assert!(adopt_legacy_db(&legacy, &current).unwrap());
        assert!(!legacy.exists(), "legacy database is moved, not copied");

        // The adopted database must still contain the row.
        let adopted = Store::open(current.to_str().unwrap()).unwrap();
        let titles: Vec<String> = adopted
            .list_events()
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(titles, vec!["改名前的会议".to_string()]);
        drop(adopted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dangerous case: adopting must never clobber a database the user has
    /// already built up under the new name.
    #[test]
    pub(crate) fn adopting_never_overwrites_an_existing_database() {
        let dir = scratch("keeps");
        let legacy = dir.join("pa.sqlite");
        let current = dir.join("solum.sqlite");
        std::fs::write(&legacy, b"old").unwrap();
        std::fs::write(&current, b"new").unwrap();

        assert!(!adopt_legacy_db(&legacy, &current).unwrap());

        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        assert_eq!(
            std::fs::read(&legacy).unwrap(),
            b"old",
            "the legacy file stays put for the user to inspect"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    pub(crate) fn adopting_is_a_no_op_on_a_clean_install() {
        let dir = scratch("clean");
        assert!(!adopt_legacy_db(&dir.join("pa.sqlite"), &dir.join("solum.sqlite")).unwrap());
        assert!(!dir.join("solum.sqlite").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(crate) fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    /// P2 regression: a peer stamping ops years ahead used to win LWW forever,
    /// freezing that row against every later local correction. Such an op is
    /// now quarantined — held and visible, not applied and not lost.
    #[test]
    pub(crate) fn an_op_stamped_far_in_the_future_is_quarantined_not_applied() {
        let s = Store::open_in_memory().unwrap();
        let counts = s
            .apply_remote_ops(&[crate::sync::SyncOp {
                tbl: "events".into(),
                guid: "guid-future".into(),
                op: "upsert".into(),
                payload: Some(
                    serde_json::json!({
                        "guid": "guid-future",
                        "title": "来自未来的会议",
                        "kind": "meeting",
                        "start": "2026-07-09T15:00:00",
                        "created_at": "2026-07-07T10:00:00",
                    })
                    .to_string(),
                ),
                hlc: "2099-01-01T00:00:00.000".into(),
                origin: "hostile-peer".into(),
            }])
            .unwrap();

        assert_eq!(counts.applied, 0);
        assert_eq!(counts.quarantined, 1);
        assert!(s.list_events().unwrap().is_empty());
        assert_eq!(s.sync_quarantine_stats().unwrap().0, 1, "held, not dropped");

        // …and the hostile stamp must not have dragged this device's own
        // logical clock into the future with it.
        let hlc_last = s.get_meta("hlc_last").unwrap().unwrap_or_default();
        assert!(
            hlc_last.as_str() < "2099",
            "local clock absorbed the forged stamp: {hlc_last}"
        );
    }

    /// Follow-up review P2: `MAX(wall, hlc_last)` is only *non-decreasing*.
    /// Two updates to the same row inside one millisecond got the identical
    /// `(hlc, origin)`, and `apply_remote_ops` requires strictly newer — so a
    /// peer applied the first and silently dropped the second, diverging
    /// forever. Empirically `strftime('now')` twice in one statement always
    /// returns the same value, so this was certain, not rare.
    #[test]
    pub(crate) fn rapid_updates_to_one_row_get_strictly_increasing_stamps() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_event(&sample_event(), None).unwrap();
        // Hammer the same row; all of these land within a millisecond or two.
        for h in 0..20 {
            s.update_event_times(id, dt(2026, 7, 7, h % 24, 0), None)
                .unwrap();
        }
        let mut stamps = s
            .conn
            .prepare("SELECT hlc FROM sync_oplog WHERE tbl = 'events' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(stamps.len() >= 20, "expected one op per update");
        let total = stamps.len();
        stamps.dedup();
        assert_eq!(
            stamps.len(),
            total,
            "duplicate (hlc, origin) means a peer would silently drop an update"
        );
    }

    /// Follow-up review P2: a peer with a mismatched key can emit unreadable
    /// blobs indefinitely. Keeping every one of them would let it fill this
    /// device's disk.
    #[test]
    pub(crate) fn parked_unopenable_blobs_are_bounded_and_the_loss_is_visible() {
        let s = Store::open_in_memory().unwrap();
        for seq in 1..=(MAX_BAD_BLOBS + 25) {
            s.hold_bad_blob(seq, "peer", "AAAA", "wrong key").unwrap();
        }
        let (held, dropped) = s.bad_blob_stats().unwrap();
        assert!(held <= MAX_BAD_BLOBS, "held {held} exceeds the cap");
        assert_eq!(dropped, 25, "the dropped count must be visible, not silent");

        // An oversized payload keeps the record but not the bytes.
        let huge = "A".repeat(MAX_BAD_BLOB_BYTES + 10);
        s.hold_bad_blob(99_999, "peer", &huge, "wrong key").unwrap();
        let kept: String = s
            .conn
            .query_row(
                "SELECT blob_b64 FROM sync_bad_blobs WHERE seq = 99999",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(kept.is_empty(), "oversized ciphertext should not be stored");
    }

    /// P3 regression: `dedup_key` excludes the value on purpose, so a platform
    /// correcting a reading re-sends the same key with a new number. Under
    /// `INSERT OR IGNORE` that correction was dropped and the first number
    /// kept forever — the opposite of what `dedup_key`'s doc comment claims.
    #[test]
    pub(crate) fn a_corrected_health_reading_replaces_the_original() {
        let s = Store::open_in_memory().unwrap();
        let mut sample = crate::wearable::HealthSample::new(
            crate::wearable::HealthMetric::Steps,
            dt(2026, 7, 12, 8, 0),
            dt(2026, 7, 12, 9, 0),
            1000.0,
            "health_connect",
        );
        assert!(s.insert_health_sample_if_new(&sample).unwrap().is_some());

        // Same interval, same source, corrected value.
        sample.value = 1234.0;
        s.insert_health_sample_if_new(&sample).unwrap();

        let stored = s.list_health_samples().unwrap();
        assert_eq!(stored.len(), 1, "a correction must not add a second row");
        assert_eq!(stored[0].value, 1234.0, "the correction should have landed");
    }

    pub(crate) fn sample_event() -> Event {
        let mut ev = Event::new(
            "开会",
            EventKind::Meeting,
            dt(2026, 7, 7, 15, 0),
            "明天下午3点开会",
            dt(2026, 7, 6, 10, 0),
        );
        ev.location = Some("会议室".into());
        ev.people = vec!["张伟".into()];
        ev
    }

    #[test]
    pub(crate) fn migrates_and_versions() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    pub(crate) fn course_draft() -> WidgetDefinitionDraft {
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

    /// Ship every locally-originated op from `from` into `to`, the way one
    /// sync round would.
    pub(crate) fn push_all(from: &Store, to: &Store) {
        let ops: Vec<crate::sync::SyncOp> = from
            .local_ops_after(0)
            .unwrap()
            .into_iter()
            .map(|(_, op)| op)
            .collect();
        to.apply_remote_ops(&ops).unwrap();
    }

    pub(crate) fn optional_field(name: &str, label: &str) -> WidgetField {
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

    /// Park an op the way an older build would have: it named a table that
    /// build did not know.
    pub(crate) fn park(s: &Store, tbl: &str, guid: &str, payload: &str, hlc: &str) {
        s.conn
            .execute(
                "INSERT INTO sync_quarantine(tbl, guid, op, payload, hlc, origin, reason, held_at)
                 VALUES (?1, ?2, 'upsert', ?3, ?4, 'peer', 'unknown sync table', ?4)",
                params![tbl, guid, payload, hlc],
            )
            .unwrap();
    }

    pub(crate) fn event_payload(title: &str) -> String {
        serde_json::json!({
            "title": title,
            "kind": "meeting",
            "start": crate::model::fmt_ts(&dt(2026, 7, 9, 15, 0)),
            "people_json": "[]",
            "created_at": crate::model::fmt_ts(&dt(2026, 7, 7, 10, 0)),
        })
        .to_string()
    }

    /// The whole reason quarantine beats "just skip it": an op held by an
    /// older build must still land after the upgrade that teaches us the
    /// table. Skipping would have advanced the cursor past it forever.
    #[test]
    pub(crate) fn quarantined_ops_replay_once_this_build_knows_the_table() {
        let s = Store::open_in_memory().unwrap();
        park(
            &s,
            "events",
            &"a".repeat(32),
            &event_payload("升级后补上的会议"),
            "2026-07-07T10:00:00.000",
        );
        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));

        // Re-running migrate is what an app restart after an upgrade does.
        s.migrate().unwrap();

        let titles: Vec<String> = s
            .list_events()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["升级后补上的会议".to_string()]);
        assert_eq!(s.sync_quarantine_stats().unwrap(), (0, 0));
    }

    /// A table *this* build still does not know stays parked — it is not
    /// dropped just because a migration ran for other reasons.
    ///
    /// This used to use `widget_defs` as the stand-in for a future table.
    /// That table shipped in v13, so the name has to be one no build knows,
    /// or the test silently starts asserting the opposite of its own name.
    #[test]
    pub(crate) fn a_still_unknown_table_stays_parked_across_migrations() {
        let s = Store::open_in_memory().unwrap();
        park(
            &s,
            "a_table_from_a_future_version",
            &"b".repeat(32),
            r#"{"name":"日程表"}"#,
            "2026-07-07T10:00:00.000",
        );

        s.migrate().unwrap();
        s.migrate().unwrap();

        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
    }

    /// A known table with an unusable payload must not take the batch down
    /// with it, and the savepoint must leave nothing half-written.
    #[test]
    pub(crate) fn a_malformed_known_table_op_is_quarantined_without_partial_writes() {
        let s = Store::open_in_memory().unwrap();
        let good = crate::sync::SyncOp {
            tbl: "events".into(),
            guid: "c".repeat(32),
            op: "upsert".into(),
            payload: Some(event_payload("完好的会议")),
            hlc: "2026-07-07T10:00:00.000".into(),
            origin: "peer".into(),
        };
        // `kind` is required; without it the arm fails while building params.
        let broken = crate::sync::SyncOp {
            tbl: "events".into(),
            guid: "d".repeat(32),
            op: "upsert".into(),
            payload: Some(r#"{"title":"缺字段的会议"}"#.into()),
            hlc: "2026-07-07T10:00:01.000".into(),
            origin: "peer".into(),
        };

        let counts = s.apply_remote_ops(&[good, broken]).unwrap();

        assert_eq!(
            counts,
            MergeCounts {
                applied: 1,
                skipped: 0,
                quarantined: 1
            }
        );
        // The good sibling survived; the broken one wrote nothing.
        let titles: Vec<String> = s
            .list_events()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["完好的会议".to_string()]);
        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
    }

    /// The same op arriving twice must be held once — replaying it twice
    /// would double-apply after an upgrade.
    #[test]
    pub(crate) fn re_holding_the_same_op_does_not_duplicate_it() {
        let s = Store::open_in_memory().unwrap();
        let op = crate::sync::SyncOp {
            tbl: "widget_defs".into(),
            guid: "e".repeat(32),
            op: "upsert".into(),
            payload: Some(r#"{"name":"日程表"}"#.into()),
            hlc: "2026-07-07T10:00:00.000".into(),
            origin: "peer".into(),
        };
        s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
        s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
        assert_eq!(s.sync_quarantine_stats().unwrap(), (1, 0));
    }

    /// A device that never upgrades must not grow the hold list without end;
    /// the overflow is dropped *and counted*, never silently.
    #[test]
    pub(crate) fn quarantine_overflow_drops_oldest_and_counts_it() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..(MAX_QUARANTINE_OPS + 3) {
            let op = crate::sync::SyncOp {
                tbl: "widget_defs".into(),
                guid: format!("{i:032}"),
                op: "upsert".into(),
                payload: Some("{}".into()),
                hlc: format!("2026-07-07T10:00:{:02}.000", i % 60),
                origin: "peer".into(),
            };
            s.apply_remote_ops(std::slice::from_ref(&op)).unwrap();
        }
        assert_eq!(
            s.sync_quarantine_stats().unwrap(),
            (MAX_QUARANTINE_OPS, 3),
            "held should cap out, and the drops must be visible"
        );
    }

    #[test]
    pub(crate) fn phase9_legacy_capture_migration_runs_once() {
        let s = Store::open_in_memory().unwrap();
        let add_notification = |event_id| {
            s.insert_notification(&Notification {
                id: None,
                event_id,
                fire_at: dt(2026, 7, 7, 14, 30),
                lead_label: "30m".into(),
                channels: vec![Channel::Push],
                status: NotificationStatus::Pending,
                created_at: dt(2026, 7, 6, 10, 0),
                fired_at: None,
            })
            .unwrap()
        };

        // Simulate a v6 database that had notification-source rows before
        // this setting existed. The migration preserves the old local-only
        // boundary and retracts its unsent oplog entries.
        let old_raw = s
            .insert_raw_input(
                "[通知·legacy] 明天下午3点开会",
                "event",
                dt(2026, 7, 6, 10, 0),
            )
            .unwrap();
        let old_event = s
            .insert_event_with_scope(&sample_event(), Some(old_raw), false, None)
            .unwrap();
        let old_notification = add_notification(old_event);
        s.conn
            .execute(
                "UPDATE meta SET value = '6' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        s.migrate().unwrap();
        assert!(s.raw_input_is_local_only(old_raw).unwrap());
        assert_eq!(
            s.list_raw_inputs()
                .unwrap()
                .into_iter()
                .find(|row| row.id == old_raw)
                .unwrap()
                .source_id,
            "notification"
        );
        assert!(s.event_is_local_only(old_event).unwrap());
        assert_eq!(
            s.conn
                .query_row(
                    "SELECT local_only FROM notifications WHERE id = ?1",
                    params![old_notification],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        // v10: the legacy rows keep their local_only stamp (no cloud LLM) but
        // are bootstrapped into the oplog, since sync no longer depends on it.
        assert!(
            !s.local_ops_after(0).unwrap().is_empty(),
            "v9→v10 must bootstrap previously unsynced captures into the oplog"
        );

        // A later open is not allowed to re-mark captures made under an
        // enabled setting; their scope is chosen at ingestion time.
        let new_raw = s
            .insert_raw_input("[通知·new] 明天下午4点开会", "event", dt(2026, 7, 6, 10, 0))
            .unwrap();
        let new_event = s
            .insert_event_with_scope(&sample_event(), Some(new_raw), false, None)
            .unwrap();
        let new_notification = add_notification(new_event);
        // A later schema-only upgrade must not re-run the Phase 9 privacy
        // migration over captures whose scope was already chosen at ingest.
        s.conn
            .execute(
                "UPDATE meta SET value = '8' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        s.migrate().unwrap();
        assert!(!s.raw_input_is_local_only(new_raw).unwrap());
        assert_eq!(
            s.list_raw_inputs()
                .unwrap()
                .into_iter()
                .find(|row| row.id == new_raw)
                .unwrap()
                .source_id,
            "notification"
        );
        assert!(!s.event_is_local_only(new_event).unwrap());
        assert_eq!(
            s.conn
                .query_row(
                    "SELECT local_only FROM notifications WHERE id = ?1",
                    params![new_notification],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    pub(crate) fn event_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let id = s.insert_event(&sample_event(), None).unwrap();
        let got = s.get_event(id).unwrap();
        assert_eq!(got.title, "开会");
        assert_eq!(got.kind, EventKind::Meeting);
        assert_eq!(got.location.as_deref(), Some("会议室"));
        assert_eq!(got.people, vec!["张伟".to_string()]);
        assert_eq!(got.start, dt(2026, 7, 7, 15, 0));
    }

    #[test]
    pub(crate) fn notification_due_and_fire() {
        let s = Store::open_in_memory().unwrap();
        let ev_id = s.insert_event(&sample_event(), None).unwrap();
        let n = Notification {
            id: None,
            event_id: ev_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        };
        let nid = s.insert_notification(&n).unwrap();
        assert!(s
            .due_notifications(dt(2026, 7, 7, 14, 0))
            .unwrap()
            .is_empty());
        assert_eq!(
            s.due_notifications(dt(2026, 7, 7, 14, 30)).unwrap().len(),
            1
        );
        s.mark_fired(nid, dt(2026, 7, 7, 14, 30)).unwrap();
        assert!(s
            .due_notifications(dt(2026, 7, 7, 15, 0))
            .unwrap()
            .is_empty());
    }

    #[test]
    pub(crate) fn snooze_rearms_pending_and_fired_but_not_dismissed() {
        let s = Store::open_in_memory().unwrap();
        let ev_id = s.insert_event(&sample_event(), None).unwrap();
        let n = Notification {
            id: None,
            event_id: ev_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        };
        let nid = s.insert_notification(&n).unwrap();

        // pending → postpone: not due at the old time, due at the new one.
        s.snooze_notification(nid, dt(2026, 7, 7, 14, 45)).unwrap();
        assert!(s
            .due_notifications(dt(2026, 7, 7, 14, 30))
            .unwrap()
            .is_empty());
        assert_eq!(
            s.due_notifications(dt(2026, 7, 7, 14, 45)).unwrap().len(),
            1
        );

        // fired → re-arm: back to pending with fired_at cleared.
        s.mark_fired(nid, dt(2026, 7, 7, 14, 45)).unwrap();
        s.snooze_notification(nid, dt(2026, 7, 7, 14, 55)).unwrap();
        let got = s
            .list_notifications()
            .unwrap()
            .into_iter()
            .find(|x| x.id == Some(nid))
            .unwrap();
        assert_eq!(got.status, NotificationStatus::Pending);
        assert_eq!(got.fire_at, dt(2026, 7, 7, 14, 55));
        assert!(got.fired_at.is_none());

        // dismissed stays cancelled — snooze must not resurrect it.
        s.dismiss_notification(nid).unwrap();
        assert!(s.snooze_notification(nid, dt(2026, 7, 7, 15, 5)).is_err());
    }

    #[test]
    pub(crate) fn fact_update_edits_in_place_and_rejects_duplicates() {
        let s = Store::open_in_memory().unwrap();
        let now = dt(2026, 7, 6, 10, 0);
        let fact = |content: &str| crate::memory::MemoryFact {
            id: None,
            content: content.into(),
            source: "manual".into(),
            created_at: now,
            last_used_at: None,
        };
        let (f1, f2) = (fact("我对花生过敏"), fact("我不吃辣"));
        let id1 = s.insert_fact_if_new(&f1).unwrap().unwrap();
        s.insert_fact_if_new(&f2).unwrap().unwrap();

        s.update_fact(id1, " 我对花生和腰果都过敏 ").unwrap();
        let facts = s.list_facts().unwrap();
        assert!(facts.iter().any(|f| f.content == "我对花生和腰果都过敏"));

        // Editing into another fact's exact wording is rejected, not merged.
        assert!(s.update_fact(id1, "我不吃辣").is_err());
        // Empty content is rejected.
        assert!(s.update_fact(id1, "   ").is_err());
        // Missing row.
        assert!(s.update_fact(9999, "x").is_err());
    }

    #[test]
    pub(crate) fn audit_is_appendable_and_readable() {
        let s = Store::open_in_memory().unwrap();
        let e = AuditEntry {
            ts: dt(2026, 7, 6, 10, 0),
            tool: "delete_file".into(),
            risk: RiskLevel::Dangerous,
            summary: "delete_file(/x)".into(),
            decision: Decision::Refused,
            token_id: None,
            detail: "no token".into(),
        };
        s.append_audit(&e).unwrap();
        let rows = s.list_audit().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, "refused");
        assert_eq!(rows[0].risk, "dangerous");
    }

    #[test]
    pub(crate) fn config_defaults_and_persist() {
        let s = Store::open_in_memory().unwrap();
        // Defaults when nothing saved.
        assert_eq!(s.load_rule_table().unwrap(), RuleTable::default_table());
        assert!(!s.notif_cloud_enabled().unwrap());
        s.set_notif_cloud_enabled(true).unwrap();
        assert!(s.notif_cloud_enabled().unwrap());
        // This is deliberately device-local; changing it cannot create a
        // synced meta document that flips another device's privacy choice.
        assert!(s.local_ops_after(0).unwrap().is_empty());
        let mut c = ProactivityConfig::defaults();
        c.set(
            crate::proactivity::ProactivityDimension::WeeklyReview,
            crate::proactivity::ProactivityLevel::Butler,
        );
        s.save_proactivity(&c).unwrap();
        assert_eq!(s.load_proactivity().unwrap(), c);
    }

    #[test]
    pub(crate) fn persona_versions_rollback_and_clear() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.active_persona().unwrap().is_none());

        let d1 = crate::persona::PersonaDraft {
            tone: "温和".into(),
            ..Default::default()
        };
        let d2 = crate::persona::PersonaDraft {
            tone: "干练".into(),
            nickname: Some("老板".into()),
            ..Default::default()
        };
        let v1 = s
            .insert_persona_version(&d1, "manual", None, dt(2026, 7, 6, 10, 0))
            .unwrap();
        let v2 = s
            .insert_persona_version(&d2, "manual", Some("改干练".into()), dt(2026, 7, 6, 11, 0))
            .unwrap();
        assert_eq!((v1.version, v2.version), (1, 2));

        // Latest write is active; history is newest-first and complete.
        assert_eq!(s.active_persona().unwrap().unwrap().draft.tone, "干练");
        let versions = s.list_persona_versions().unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 2);

        // Rollback moves the pointer without touching history.
        let back = s.set_active_persona(1).unwrap();
        assert_eq!(back.draft.tone, "温和");
        assert_eq!(s.active_persona().unwrap().unwrap().version, 1);
        assert_eq!(s.list_persona_versions().unwrap().len(), 2);
        assert!(s.set_active_persona(99).is_err());

        // Clear removes everything; the next save restarts at v1.
        s.clear_persona().unwrap();
        assert!(s.active_persona().unwrap().is_none());
        assert!(s.list_persona_versions().unwrap().is_empty());
        let again = s
            .insert_persona_version(&d1, "manual", None, dt(2026, 7, 6, 12, 0))
            .unwrap();
        assert_eq!(again.version, 1);
    }

    #[test]
    pub(crate) fn ledger_lists_and_cascade_deletes() {
        let s = Store::open_in_memory().unwrap();
        let raw = s
            .insert_raw_input("明天下午3点开会", "ingest_event", dt(2026, 7, 6, 10, 0))
            .unwrap();
        let ev_id = s.insert_event(&sample_event(), Some(raw)).unwrap();
        let n = Notification {
            id: None,
            event_id: ev_id,
            fire_at: dt(2026, 7, 7, 14, 30),
            lead_label: "30m".into(),
            channels: vec![Channel::Push],
            status: NotificationStatus::Pending,
            created_at: dt(2026, 7, 6, 10, 0),
            fired_at: None,
        };
        s.insert_notification(&n).unwrap();

        let ledger = s.memory_ledger().unwrap();
        // raw input + event + notification = 3 entries.
        assert_eq!(ledger.len(), 3);

        // Deleting the raw input cascades to event and notification.
        s.delete_memory(MemoryLayer::RawInput, raw).unwrap();
        assert!(s.memory_ledger().unwrap().is_empty());
        assert!(s.list_events().unwrap().is_empty());
        assert!(s.list_notifications().unwrap().is_empty());
    }

    /// CoreError must keep the SQLite result code: callers (and incident
    /// logs) need BUSY vs CONSTRAINT vs READONLY to decide retry vs report,
    /// and `Display` alone drops that distinction. Uses the same trigger
    /// fault-injection as the orchestrator rollback tests to force a real
    /// constraint violation through the `From` impl.
    #[test]
    pub(crate) fn storage_errors_keep_the_sqlite_error_code() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute_batch(
                "CREATE TRIGGER test_tripwire_events BEFORE INSERT ON events
                 BEGIN SELECT RAISE(ABORT, 'injected test failure'); END;",
            )
            .unwrap();
        let err = s
            .insert_event_with_scope(
                &crate::model::Event {
                    id: None,
                    title: "会被拦截".into(),
                    kind: crate::model::EventKind::Meeting,
                    start: dt(2026, 7, 7, 15, 0),
                    end: None,
                    location: None,
                    people: Vec::new(),
                    raw_input: String::new(),
                    created_at: dt(2026, 7, 6, 10, 0),
                },
                None,
                false,
                None,
            )
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("ConstraintViolation"),
            "error must carry the SQLite code, got: {text}"
        );
        assert!(
            matches!(err, CoreError::Storage(_)),
            "variant stays Storage for existing matchers"
        );
    }

    /// A corrupt people_json row must not silently read as "no participants":
    /// list_events keeps going (one bad row cannot blank the whole ledger)
    /// but the corruption is visible on stderr with the row id.
    #[test]
    pub(crate) fn corrupt_people_json_is_reported_not_silently_swallowed() {
        let s = Store::open_in_memory().unwrap();
        let raw = s
            .insert_raw_input(
                "明天下午3点开损坏JSON会议",
                "schedule_event",
                dt(2026, 7, 6, 10, 0),
            )
            .unwrap();
        s.conn
            .execute_batch(&format!(
                "INSERT INTO events(title, kind, start, people_json, raw_input_id, created_at)
                 VALUES ('损坏JSON会议', 'meeting', '2026-07-07T15:00:00', '{{oops', {raw}, '2026-07-06T10:00:00');"
            ))
            .unwrap();

        let events = s.list_events().unwrap();
        assert_eq!(events.len(), 1, "the bad row still surfaces");
        assert!(events[0].people.is_empty(), "fallback stays empty");
    }

    /// Same policy for a notification whose channels_json is corrupt.
    #[test]
    pub(crate) fn corrupt_channels_json_is_reported_not_silently_swallowed() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute_batch(
                "INSERT INTO events(title, kind, start, created_at)
                 VALUES ('渠道损坏会议', 'meeting', '2026-07-07T15:00:00', '2026-07-06T10:00:00');
                 INSERT INTO notifications(event_id, fire_at, lead_label, channels_json, status, created_at)
                 VALUES (1, '2026-07-07T14:00:00', '1h', '[broken', 'pending', '2026-07-06T10:00:00');",
            )
            .unwrap();

        let notifications = s.list_notifications().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].channels.is_empty());
    }

    /// F12 ledger paging: pages walk every row exactly once in the same
    /// newest-first order the full ledger produces, cursors chain cleanly,
    /// and an exhausted layer returns no cursor.
    #[test]
    pub(crate) fn ledger_pages_match_the_full_ledger_order() {
        let s = Store::open_in_memory().unwrap();
        // Seed three layers with known timestamps, including same-second
        // ties that exercise the id tiebreak.
        for i in 0..5 {
            s.insert_raw_input(
                &format!("原始输入{i}"),
                "schedule_event",
                dt(2026, 7, 6, 10, i % 2),
            )
            .unwrap();
        }
        for i in 0..4 {
            s.insert_fact_if_new(&crate::memory::MemoryFact {
                id: None,
                content: format!("事实{i}"),
                source: "chat".into(),
                created_at: dt(2026, 7, 6, 9, i),
                last_used_at: None,
            })
            .unwrap()
            .unwrap();
        }
        s.conn
            .execute_batch(
                "INSERT INTO behavior_log(ts, kind, content) VALUES
                 ('2026-07-06T08:00:00', 'status', '状态一'),
                 ('2026-07-06T08:00:00', 'status', '状态二'),
                 ('2026-07-06T08:01:00', 'reminder_fired', '触发一');",
            )
            .unwrap();

        let full = s.memory_ledger().unwrap();

        for (layer, expected_len) in [
            (MemoryLayer::RawInput, 5usize),
            (MemoryLayer::Fact, 4),
            (MemoryLayer::Behavior, 3),
        ] {
            let expected: Vec<(String, i64)> = full
                .iter()
                .filter(|e| e.layer == layer)
                .map(|e| (e.summary.clone(), e.id))
                .collect();
            assert_eq!(expected.len(), expected_len);

            // Page through two-at-a-time and compare against the full view.
            let mut seen = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let (entries, next) = s.memory_ledger_page(layer, 2, cursor.as_deref()).unwrap();
                assert!(entries.len() <= 2);
                for e in &entries {
                    seen.push((e.summary.clone(), e.id));
                }
                match next {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            assert_eq!(seen, expected, "paged {layer:?} must equal full order");
        }

        // A page size larger than the table just returns everything at once.
        let (all, next) = s.memory_ledger_page(MemoryLayer::Fact, 500, None).unwrap();
        assert_eq!(all.len(), 4);
        assert!(next.is_none());
    }

    /// Cursor validation is fail-closed: garbage input must be rejected, not
    /// silently read as "first page".
    #[test]
    pub(crate) fn ledger_page_rejects_malformed_cursors_and_limits() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.memory_ledger_page(MemoryLayer::Event, 0, None).is_err());
        assert!(s.memory_ledger_page(MemoryLayer::Event, 501, None).is_err());
        assert!(s
            .memory_ledger_page(MemoryLayer::Event, 10, Some("not-a-cursor"))
            .is_err());
        assert!(s
            .memory_ledger_page(MemoryLayer::Event, 10, Some("2026-07-06T10:00:00|x"))
            .is_err());
    }

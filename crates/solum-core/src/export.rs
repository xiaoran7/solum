//! 一键全量数据导出（F12 延伸 / §4 承诺兑现）。
//!
//! 定位说"数据完全归你"，§4 明确审计日志用户可导出——这里把承诺落成代码：
//! 本机 SQLite 里用户可见的每一层（原始输入 / 事件 / 提醒 / 行为日志 / 建议 /
//! 语义记忆 / routine / 穿戴 / 人格全部版本 / 规则表 / 主动度 / 审计日志）
//! 聚合成一份可读 JSON。纯只读、纯本地、不上云；导出的用途（备份 / 迁移 /
//! 离机审查）由用户自己决定。
//!
//! 不导出的只有同步内务数据（oplog / 设备游标）——那是传输机制不是记忆，
//! 且换设备重放 oplog 本就不成立。

use chrono::NaiveDateTime;
use serde_json::json;

use crate::error::{CoreError, Result};
use crate::model::fmt_ts;
use crate::store::Store;

/// 当前导出格式版本。字段增删时 +1，方便未来的导入端按版本兼容。
///
/// v2（2026-07-20）：新增 `_restore` 段。v1 的各层是给人看的，**不带 guid**，
/// 因此无法被还原——重复导入会整份翻倍，事件与其原始输入的关联也接不回来。
/// 备份只有在能还原时才叫备份，故补上一段行级的、带 guid 的线上形状。
pub const EXPORT_VERSION: u32 = 2;

/// 能被 [`import_document`] 还原的最低格式版本。
pub const MIN_RESTORABLE_VERSION: u32 = 2;

/// 导出文档的格式标识。2026-07-20 改名后写出的是 [`FORMAT`]。
pub const FORMAT: &str = "solum-export";
/// 改名前写出的标识。**读取时必须继续接受**——否则用户改名前导出的备份
/// 会在导入端被判为"不是导出文档"而整份拒绝，等于把已有备份作废。
/// 格式本身一个字节都没变，变的只有这个名字。
pub const LEGACY_FORMAT: &str = "pa-export";

/// Aggregate everything the user owns into one JSON document.
pub fn build_export(store: &Store, now: NaiveDateTime) -> Result<serde_json::Value> {
    Ok(json!({
        "format": FORMAT,
        "version": EXPORT_VERSION,
        "exported_at": fmt_ts(&now),
        "schema_version": store.schema_version()?,
        "device_id": store.device_id()?,
        "raw_inputs": store.list_raw_inputs()?,
        "events": store.list_events()?,
        "notifications": store.list_notifications()?,
        "behavior_log": store.list_behavior()?,
        "suggestions": store.list_suggestions()?,
        "memory_facts": store.list_facts()?,
        "routines": store.list_routines()?,
        "health_samples": store.list_health_samples()?,
        // A separate fact source, intentionally not `memory_facts` / recall.
        "soulous_facts": store.list_soulous_facts()?,
        "persona_versions": store.list_persona_versions()?,
        // Definitions ship with their assembled schema so the records below
        // stay readable rather than being anonymous JSON blobs.
        "widget_defs": store.list_widget_definitions()?,
        "widget_records": store.list_all_widget_records()?,
        "importance_rules": store.load_rule_table()?,
        "proactivity": store.load_proactivity()?,
        "audit_log": store.list_audit()?,
        // Row-level, guid-bearing shape. The layers above stay as they are —
        // they are the readable artifact §4 promised — and this section is
        // what makes the file replayable. Yes, the content appears twice;
        // for a personal backup that is a cheap price for not having to
        // choose between "readable" and "restorable".
        "_restore": store.export_restore_rows()?,
    }))
}

/// What restoring a document would do, without doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    pub version: u32,
    pub exported_at: NaiveDateTime,
    pub origin: String,
    /// `(table, row count)`, only non-empty tables.
    pub counts: Vec<(String, usize)>,
}

impl ImportPlan {
    pub fn total(&self) -> usize {
        self.counts.iter().map(|(_, n)| n).sum()
    }
}

fn restore_section(doc: &serde_json::Value) -> Result<&serde_json::Value> {
    let format = doc.get("format").and_then(|v| v.as_str());
    if !matches!(format, Some(FORMAT) | Some(LEGACY_FORMAT)) {
        return Err(CoreError::Invalid(format!("不是 {FORMAT} 文档")));
    }
    let version = doc
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    if version < MIN_RESTORABLE_VERSION {
        return Err(CoreError::Invalid(format!(
            "导出格式 v{version} 不含可还原数据（v{MIN_RESTORABLE_VERSION} 起才有）；\
             该文件仍可人工查阅，但无法自动导入"
        )));
    }
    if version > EXPORT_VERSION {
        return Err(CoreError::Invalid(format!(
            "导出文件版本 v{version} 比本机（v{EXPORT_VERSION}）新，请先升级再导入"
        )));
    }
    doc.get("_restore")
        .ok_or_else(|| CoreError::Invalid("文档缺少 _restore 段".into()))
}

pub fn plan_import(doc: &serde_json::Value) -> Result<ImportPlan> {
    let restore = restore_section(doc)?;
    Ok(ImportPlan {
        version: doc
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        exported_at: doc
            .get("exported_at")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| crate::model::parse_ts(s).ok())
            .ok_or_else(|| CoreError::Invalid("文档缺少可解析的 exported_at".into()))?,
        origin: doc
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("import")
            .to_string(),
        counts: Store::count_restore_rows(restore),
    })
}

/// Merge a document's rows into this store. Idempotent, and never destructive:
/// nothing is deleted, and rows the user has edited more recently than the
/// backup keep their local version (see `Store::import_restore_rows`).
pub fn import_document(
    store: &Store,
    doc: &serde_json::Value,
    now: chrono::NaiveDateTime,
) -> Result<(ImportPlan, crate::store::MergeCounts)> {
    let plan = plan_import(doc)?;
    let restore = restore_section(doc)?;
    let counts = store.import_restore_rows(
        restore,
        plan.exported_at,
        &format!("import:{}", plan.origin),
        now,
    )?;
    Ok((plan, counts))
}

#[cfg(test)]
mod tests {
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
}

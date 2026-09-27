//! `build` -- the read side: walk every user-visible layer of the local store into one readable JSON document. Pure read, pure local, nothing leaves the machine.
//!
//! Part of the `export` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

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

//! audit config. Part of the `store::tests` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `store/tests.rs`.

use super::*;
// Shared fixtures moved out when the suite was split.
use super::sync_core::dt;

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

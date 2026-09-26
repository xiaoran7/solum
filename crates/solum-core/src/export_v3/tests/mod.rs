//! `tests` -- the suite for all of the above.
//!
//! Part of the `export_v3` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::is_lower_hex;
use super::*;

use crate::guard::{AuditEntry, Decision};
use crate::model::{Event, EventKind, RiskLevel};
use crate::notification_intelligence::{
    CaptureLane, NotificationActionProposal, NotificationCapture,
};
use chrono::NaiveDate;

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 8, 23)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

pub(crate) fn sample_platform() -> PlatformPortableData {
    PlatformPortableData {
        chat: PortableChat {
            sessions: vec![PortableChatSession {
                id: "session-public-fixture".into(),
                title: "Synthetic session".into(),
                updated_at: "2026-08-23T02:00:00.000Z".into(),
                messages: vec![
                    PortableChatMessage {
                        role: PortableChatRole::User,
                        content: "Plan a synthetic meeting".into(),
                        intent: "chat".into(),
                    },
                    PortableChatMessage {
                        role: PortableChatRole::Assistant,
                        content: "Synthetic reply".into(),
                        intent: String::new(),
                    },
                ],
            }],
            drafts: vec![PortableChatDraft {
                session_id: "session-public-fixture".into(),
                content: "Synthetic draft".into(),
            }],
        },
        documents: vec![DocumentManifest {
            id: "document-public-fixture".into(),
            name: "synthetic.txt".into(),
            media_type: "text/plain".into(),
            plaintext_size: 17,
            sha256: "00".repeat(32),
        }],
        pending_notifications: vec![PendingNotificationInput {
            package_name: "dev.solum.synthetic".into(),
            title: "Synthetic notification".into(),
            body: "No personal data".into(),
            received_at: "2026-08-23T02:00:00.000Z".into(),
            failed: true,
        }],
        llm: Some(PortableLlmSettings {
            base_url: "https://example.com/v1".into(),
            model: "synthetic-model".into(),
            temperature: Some(0.3),
            max_tokens: Some(512),
            timeout_secs: 30,
        }),
        external_restore_intents: ExternalRestoreIntents {
            email_accounts: vec![EmailRestoreIntent {
                id: "mail-public-fixture".into(),
                label: "Synthetic mail".into(),
                provider: "custom".into(),
                address: "fixture@example.invalid".into(),
            }],
            soulous: Some(SoulousRestoreIntent {
                server_url: "https://example.invalid".into(),
                push_schedule_events: false,
            }),
            health_sources: vec!["synthetic-health".into()],
            background_reminders: true,
        },
    }
}

#[test]
fn portable_store_snapshot_is_stable_and_tracks_logical_changes() {
    let store = Store::open(":memory:").unwrap();
    let first = snapshot_portable_store(&store).unwrap();
    assert_eq!(first, snapshot_portable_store(&store).unwrap());
    assert_eq!(first.fingerprint.len(), 64);

    store.set_notif_cloud_enabled(true).unwrap();
    let changed = snapshot_portable_store(&store).unwrap();
    assert_ne!(first.fingerprint, changed.fingerprint);
}

#[test]
fn v3_builds_a_strict_recovery_plan_without_secrets_or_partial_store_import() {
    let source = Store::open_in_memory().unwrap();
    let document = build_portable_export(&source, now(), sample_platform()).unwrap();
    assert_eq!(document["version"], PORTABLE_EXPORT_VERSION);

    let plan = crate::export::plan_import(&document).unwrap();
    assert_eq!(plan.version, PORTABLE_EXPORT_VERSION);
    assert!(plan.counts.contains(&("portable.chat_sessions".into(), 1)));
    assert!(plan.counts.contains(&("portable.documents".into(), 1)));
    let serialized = serde_json::to_string(&document).unwrap();
    for forbidden in ["api_key", "access_token", "refresh_token", "password"] {
        assert!(
            !serialized.contains(forbidden),
            "v3 leaked field {forbidden}"
        );
    }

    let target = Store::open_in_memory().unwrap();
    let error = crate::export::import_document(&target, &document, now()).unwrap_err();
    assert!(error.to_string().contains("平台临时库恢复事务"));
    assert!(target.list_raw_inputs().unwrap().is_empty());
}

#[test]
fn v3_stable_ids_and_safe_restore_states_are_deterministic() {
    let store = Store::open_in_memory().unwrap();
    let capture = store
        .insert_notification_capture(
            &NotificationCapture {
                package_name: "dev.solum.synthetic".into(),
                title: "Synthetic".into(),
                body: "Fixture".into(),
                received_at: now(),
            },
            true,
            CaptureLane::Batch,
            CaptureState::Queued,
            None,
        )
        .unwrap();
    let event = Event::new(
        "Synthetic event",
        EventKind::Meeting,
        now(),
        "fixture",
        now(),
    );
    let event_id = store.insert_event(&event, None).unwrap();
    let event_guid = store.event_guid_and_local_only(event_id).unwrap().0;
    store
        .insert_notification_action_proposal(&NotificationActionProposal {
            id: None,
            capture_id: capture.id.unwrap(),
            kind: NotificationActionKind::CancelEvent,
            event_id,
            event_title: event.title,
            event_guid,
            event_start: event.start,
            new_start: None,
            reason: "Synthetic reason".into(),
            state: ActionProposalState::Pending,
            created_at: now(),
        })
        .unwrap();
    store
        .append_audit(&AuditEntry {
            ts: now(),
            tool: "synthetic_tool".into(),
            risk: RiskLevel::Safe,
            summary: "Synthetic audit".into(),
            decision: Decision::Executed,
            token_id: None,
            detail: "fixture".into(),
        })
        .unwrap();

    let first = build_portable_export(&store, now(), sample_platform()).unwrap();
    let second = build_portable_export(&store, now(), sample_platform()).unwrap();
    assert_eq!(first["_portable"], second["_portable"]);
    let capture = &first["_portable"]["notification_journal"]["captures"][0];
    assert_eq!(capture["source_state"], "queued");
    assert_eq!(capture["restore_state"], "needs_review");
    assert!(is_lower_hex(capture["id"].as_str().unwrap(), 64));
    assert!(is_lower_hex(
        first["_portable"]["audit_log"][0]["id"].as_str().unwrap(),
        64
    ));
    let action = &first["_portable"]["notification_journal"]["action_proposals"][0];
    assert_eq!(action["source_state"], "pending");
    assert_eq!(action["restore_state"], "stale");
}

#[test]
fn v3_rejects_unknown_fields_identity_tampering_and_limits() {
    let store = Store::open_in_memory().unwrap();
    let document = build_portable_export(&store, now(), sample_platform()).unwrap();

    let mut unknown = document.clone();
    unknown["_portable"]["secret"] = json!("must be rejected");
    assert!(validate_portable_document(&unknown).is_err());

    let mut secret_field = document.clone();
    secret_field["_portable"]["agent_settings"]["llm"]["api_key"] =
        json!("fixture-secret-must-not-be-accepted");
    assert!(validate_portable_document(&secret_field).is_err());

    let mut changed_id = document.clone();
    changed_id["_portable"]["pending_notifications"][0]["id"] = json!("00".repeat(32));
    assert!(validate_portable_document(&changed_id).is_err());

    let mut oversized = document.clone();
    oversized["_portable"]["documents"][0]["plaintext_size"] = json!(MAX_DOCUMENT_BYTES + 1);
    assert!(validate_portable_document(&oversized).is_err());

    let mut dangling_draft = document;
    dangling_draft["_portable"]["chat"]["drafts"][0]["session_id"] = json!("missing-session");
    assert!(validate_portable_document(&dangling_draft).is_err());
}

//! Validated construction of a fresh `solum-export` v3 recovery image.
//!
//! This module deliberately stops before publishing a live platform profile.
//! It gives Android a transactionally complete temporary SQLite Store plus one
//! validated hand-off bundle for WebView, attachment and spool staging.

use std::collections::BTreeMap;

use chrono::NaiveDateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};
use crate::export::{plan_import, ImportPlan};
use crate::export_v3::{
    DocumentManifest, PortableChat, PortableLlmSettings, PortablePendingNotification,
    PortableRestoreIntents, PortableSection, PORTABLE_EXPORT_VERSION,
};
use crate::store::{MergeCounts, Store};

/// One authenticated attachment plaintext supplied by the backup coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredAttachment {
    pub id: String,
    pub plaintext: Vec<u8>,
}

/// A document manifest paired with the exact bytes whose size and digest were
/// authenticated by both the v3 payload and attachment container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredDocument {
    pub manifest: DocumentManifest,
    pub plaintext: Vec<u8>,
}

/// Platform-owned state that may be staged only after the temporary Store has
/// committed. Permission-bearing values remain restore intents, not settings.
#[derive(Debug, Clone, PartialEq)]
pub struct PlatformRestoreBundle {
    pub chat: PortableChat,
    pub documents: Vec<RestoredDocument>,
    pub pending_notifications: Vec<PortablePendingNotification>,
    pub llm: Option<PortableLlmSettings>,
    pub restore_intents: PortableRestoreIntents,
}

/// Fully validated input. Fields stay private so callers cannot bypass the
/// attachment-set check between preparation and Store recovery.
#[derive(Debug)]
pub struct PreparedPortableRestore {
    plan: ImportPlan,
    restore: Value,
    section: PortableSection,
    documents: Vec<RestoredDocument>,
}

impl PreparedPortableRestore {
    pub fn plan(&self) -> &ImportPlan {
        &self.plan
    }
}

/// Revalidate v3 and require an exact, bounded attachment set before any Store
/// is touched. The caller should pass only plaintext returned by
/// `attachment::open_attachment` under the matching backup binding.
pub fn prepare_portable_restore(
    mut document: Value,
    attachments: Vec<RestoredAttachment>,
) -> Result<PreparedPortableRestore> {
    let plan = plan_import(&document)?;
    if plan.version != PORTABLE_EXPORT_VERSION {
        return Err(invalid("portable restore requires solum-export v3"));
    }
    let object = document
        .as_object_mut()
        .ok_or_else(|| invalid("portable restore document must be an object"))?;
    let section: PortableSection = serde_json::from_value(
        object
            .remove("_portable")
            .ok_or_else(|| invalid("portable restore is missing _portable"))?,
    )
    .map_err(|_| invalid("portable restore has an invalid _portable shape"))?;
    let restore = object
        .remove("_restore")
        .ok_or_else(|| invalid("portable restore is missing _restore"))?;

    if attachments.len() > crate::attachment::MAX_ATTACHMENTS {
        return Err(invalid("portable attachment set exceeds the count limit"));
    }
    let mut total = 0u64;
    let mut supplied = BTreeMap::new();
    for item in attachments {
        if item.plaintext.len() > crate::attachment::MAX_ATTACHMENT_BYTES {
            return Err(invalid("portable attachment exceeds the size limit"));
        }
        total = total
            .checked_add(item.plaintext.len() as u64)
            .ok_or_else(|| invalid("portable attachment total overflows"))?;
        if total > crate::attachment::MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(invalid(
                "portable attachment set exceeds the total size limit",
            ));
        }
        if supplied.insert(item.id, item.plaintext).is_some() {
            return Err(invalid("portable attachment set contains a duplicate id"));
        }
    }

    let mut documents = Vec::with_capacity(section.documents.len());
    for manifest in &section.documents {
        let plaintext = supplied
            .remove(&manifest.id)
            .ok_or_else(|| invalid(format!("portable attachment {} is missing", manifest.id)))?;
        if plaintext.len() as u64 != manifest.plaintext_size
            || lower_hex(&Sha256::digest(&plaintext)) != manifest.sha256
        {
            return Err(invalid(format!(
                "portable attachment {} does not match its manifest",
                manifest.id
            )));
        }
        documents.push(RestoredDocument {
            manifest: manifest.clone(),
            plaintext,
        });
    }
    if let Some((id, _)) = supplied.into_iter().next() {
        return Err(invalid(format!(
            "portable attachment set contains unexpected id {id}"
        )));
    }

    Ok(PreparedPortableRestore {
        plan,
        restore,
        section,
        documents,
    })
}

/// Populate a newly-created temporary Store in one outer transaction. On any
/// error the Store remains empty and no platform hand-off bundle is returned.
pub fn restore_portable_into_empty_store(
    store: &Store,
    prepared: PreparedPortableRestore,
    now: NaiveDateTime,
) -> Result<(ImportPlan, MergeCounts, PlatformRestoreBundle)> {
    let offered_rows: usize = Store::count_restore_rows(&prepared.restore)
        .into_iter()
        .map(|(_, count)| count)
        .sum();
    let origin = format!("restore:{}", prepared.plan.origin);
    let counts = store.with_transaction(|temporary| {
        temporary.ensure_empty_for_portable_restore()?;
        let counts = temporary.import_restore_rows(
            &prepared.restore,
            prepared.plan.exported_at,
            &origin,
            now,
        )?;
        if counts.applied != offered_rows || counts.skipped != 0 || counts.quarantined != 0 {
            return Err(invalid(
                "portable restore could not apply every guid-bearing row",
            ));
        }
        temporary.import_portable_section(&prepared.section)?;
        Ok(counts)
    })?;

    let bundle = PlatformRestoreBundle {
        chat: prepared.section.chat,
        documents: prepared.documents,
        pending_notifications: prepared.section.pending_notifications,
        llm: prepared.section.agent_settings.llm,
        restore_intents: prepared.section.restore_intents,
    };
    Ok((prepared.plan, counts, bundle))
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    use crate::export_v3::{
        build_portable_export, ExternalRestoreIntents, PendingNotificationInput,
        PlatformPortableData, PortableChatDraft, PortableChatMessage, PortableChatRole,
        PortableChatSession,
    };
    use crate::guard::{AuditEntry, Decision};
    use crate::model::RiskLevel;
    use crate::notification_intelligence::{
        CaptureLane, CaptureState, NotificationCapture, NotificationIntelligenceConfig,
    };

    fn dt(day: u32, hour: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap()
    }

    fn fixture() -> (Value, Vec<RestoredAttachment>) {
        let source = Store::open_in_memory().unwrap();
        source
            .save_notification_intelligence_config(&NotificationIntelligenceConfig {
                capture_mode: crate::notification_intelligence::NotificationCaptureMode::Allowlist,
                allowed_packages: vec!["com.example.calendar".into()],
                auto_event_packages: vec!["com.example.calendar".into()],
                batch_interval_minutes: 20,
                filter_rules: Vec::new(),
            })
            .unwrap();
        source.set_notif_cloud_enabled(true).unwrap();
        source
            .append_audit(&AuditEntry {
                ts: dt(22, 7),
                tool: "fixture_tool".into(),
                risk: RiskLevel::Safe,
                summary: "Synthetic audit".into(),
                decision: Decision::Executed,
                token_id: None,
                detail: "Synthetic detail".into(),
            })
            .unwrap();
        source
            .insert_notification_capture(
                &NotificationCapture {
                    package_name: "com.example.calendar".into(),
                    title: "Synthetic title".into(),
                    body: "Synthetic body".into(),
                    received_at: dt(22, 8),
                },
                true,
                CaptureLane::Batch,
                CaptureState::Queued,
                Some("synthetic fixture"),
            )
            .unwrap();

        let plaintext = b"public synthetic document bytes".to_vec();
        let sha256 = lower_hex(&Sha256::digest(&plaintext));
        let document = build_portable_export(
            &source,
            dt(22, 12),
            PlatformPortableData {
                chat: PortableChat {
                    sessions: vec![PortableChatSession {
                        id: "session-1".into(),
                        title: "Synthetic session".into(),
                        updated_at: "2026-08-22T12:00:00+08:00".into(),
                        messages: vec![PortableChatMessage {
                            role: PortableChatRole::User,
                            content: "Synthetic message".into(),
                            intent: String::new(),
                        }],
                    }],
                    drafts: vec![PortableChatDraft {
                        session_id: "session-1".into(),
                        content: "Synthetic draft".into(),
                    }],
                },
                documents: vec![DocumentManifest {
                    id: "document-1".into(),
                    name: "fixture.txt".into(),
                    media_type: "text/plain".into(),
                    plaintext_size: plaintext.len() as u64,
                    sha256,
                }],
                pending_notifications: vec![PendingNotificationInput {
                    package_name: "com.example.pending".into(),
                    title: "Pending fixture".into(),
                    body: "Synthetic pending body".into(),
                    received_at: "2026-08-22T11:00:00+08:00".into(),
                    failed: false,
                }],
                llm: None,
                external_restore_intents: ExternalRestoreIntents::default(),
            },
        )
        .unwrap();
        (
            document,
            vec![RestoredAttachment {
                id: "document-1".into(),
                plaintext,
            }],
        )
    }

    #[test]
    fn restores_fresh_store_and_returns_complete_platform_bundle() {
        let (document, attachments) = fixture();
        let prepared = prepare_portable_restore(document, attachments).unwrap();
        let target = Store::open_in_memory().unwrap();
        let (plan, counts, bundle) =
            restore_portable_into_empty_store(&target, prepared, dt(23, 12)).unwrap();

        assert_eq!(plan.version, PORTABLE_EXPORT_VERSION);
        assert!(counts.applied > 0);
        let captures = target.list_notification_captures().unwrap();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].state, CaptureState::NeedsReview);
        let config = target.notification_intelligence_config().unwrap();
        assert_eq!(config.batch_interval_minutes, 20);
        assert!(config.allowed_packages.is_empty());
        assert!(config.auto_event_packages.is_empty());
        assert!(!target.notif_cloud_enabled().unwrap());
        let audit = target.list_audit().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].summary, "Synthetic audit");
        assert!(bundle.restore_intents.notification_cloud_was_enabled);
        assert_eq!(
            bundle.restore_intents.notification_packages,
            vec!["com.example.calendar"]
        );
        assert_eq!(bundle.chat.sessions.len(), 1);
        assert_eq!(bundle.pending_notifications.len(), 1);
        assert_eq!(bundle.documents.len(), 1);
        assert_eq!(
            bundle.documents[0].plaintext,
            b"public synthetic document bytes"
        );
    }

    #[test]
    fn attachment_set_must_match_manifest_exactly() {
        let (document, attachments) = fixture();
        assert!(prepare_portable_restore(document.clone(), Vec::new()).is_err());

        let mut wrong = attachments.clone();
        wrong[0].plaintext[0] ^= 1;
        assert!(prepare_portable_restore(document.clone(), wrong).is_err());

        let mut extra = attachments;
        extra.push(RestoredAttachment {
            id: "unexpected".into(),
            plaintext: Vec::new(),
        });
        assert!(prepare_portable_restore(document, extra).is_err());
    }

    #[test]
    fn refuses_non_empty_store_without_changing_existing_data() {
        let (document, attachments) = fixture();
        let prepared = prepare_portable_restore(document, attachments).unwrap();
        let target = Store::open_in_memory().unwrap();
        target
            .insert_raw_input("existing", "note", dt(23, 8))
            .unwrap();

        assert!(restore_portable_into_empty_store(&target, prepared, dt(23, 12)).is_err());
        let rows = target.list_raw_inputs().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "existing");
        assert!(target.list_notification_captures().unwrap().is_empty());
    }

    #[test]
    fn portable_failure_rolls_back_guid_rows() {
        let (mut document, attachments) = fixture();
        let capture = &mut document["_portable"]["notification_journal"]["captures"][0];
        capture["local_only"] = Value::Bool(false);
        let prepared = prepare_portable_restore(document, attachments).unwrap();
        let target = Store::open_in_memory().unwrap();

        assert!(restore_portable_into_empty_store(&target, prepared, dt(23, 12)).is_err());
        assert!(target.list_raw_inputs().unwrap().is_empty());
        assert!(target.list_notification_captures().unwrap().is_empty());
    }

    #[test]
    fn duplicate_guid_row_is_not_accepted_as_a_complete_restore() {
        let (mut document, attachments) = fixture();
        let duplicate = document["_restore"]["raw_inputs"][0].clone();
        document["_restore"]["raw_inputs"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        let prepared = prepare_portable_restore(document, attachments).unwrap();
        let target = Store::open_in_memory().unwrap();

        assert!(restore_portable_into_empty_store(&target, prepared, dt(23, 12)).is_err());
        assert!(target.list_raw_inputs().unwrap().is_empty());
    }
}

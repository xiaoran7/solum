//! `build` -- assembling the portable document: the store snapshot, the export itself, the whole-document check, and the private journal/audit/pending builders.
//!
//! Part of the `export_v3` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{invalid, is_lower_hex, next_stable_id};
use super::types::{
    PortableActionProposalRow, PortableAgentSettings, PortableAuditRow, PortableCaptureRow,
    PortableFilterProposalRow, PortableNotificationFilterRule, PortableNotificationJournal,
    PortableNotificationSettings, PortableRecoveryPlan, PortableStoreSnapshot,
};
use super::validate::{validate_platform_input, validate_section};
use super::*;

/// Produce a deterministic logical snapshot of every SQLite-backed domain
/// represented by the portable export. Platform-owned data is deliberately
/// empty: callers fingerprint that storage independently in its owning
/// runtime. This function does not expose row contents to its caller.
pub fn snapshot_portable_store(store: &Store) -> Result<PortableStoreSnapshot> {
    let fixed_time = NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .ok_or_else(|| invalid("portable snapshot timestamp is invalid"))?;
    let document = build_portable_export(
        store,
        fixed_time,
        PlatformPortableData {
            chat: PortableChat::default(),
            documents: Vec::new(),
            pending_notifications: Vec::new(),
            llm: None,
            external_restore_intents: ExternalRestoreIntents::default(),
        },
    )?;
    let plan = crate::export::plan_import(&document)?;
    let encoded = serde_json::to_vec(&document)?;
    let fingerprint = Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(PortableStoreSnapshot {
        counts: plan.counts,
        fingerprint,
    })
}

/// Build v3 from the current Store plus a same-profile platform snapshot.
pub fn build_portable_export(
    store: &Store,
    now: NaiveDateTime,
    platform: PlatformPortableData,
) -> Result<Value> {
    validate_platform_input(&platform)?;
    let notification_config = store.notification_intelligence_config()?;
    let section = PortableSection {
        format: PORTABLE_FORMAT.to_string(),
        version: PORTABLE_SCHEMA_VERSION,
        notification_journal: build_notification_journal(store)?,
        audit_log: build_audit_log(store)?,
        chat: platform.chat,
        documents: platform.documents,
        pending_notifications: build_pending_notifications(platform.pending_notifications)?,
        agent_settings: PortableAgentSettings {
            llm: platform.llm,
            notification: PortableNotificationSettings {
                batch_interval_minutes: notification_config.batch_interval_minutes,
                filter_rules: notification_config
                    .filter_rules
                    .into_iter()
                    .map(|rule| PortableNotificationFilterRule {
                        id: rule.id,
                        pattern: rule.pattern,
                        package_name: rule.package_name,
                        matcher: rule.matcher,
                        reason: rule.reason,
                    })
                    .collect(),
            },
        },
        restore_intents: PortableRestoreIntents {
            notification_cloud_was_enabled: store.notif_cloud_enabled()?,
            notification_packages: notification_config.allowed_packages,
            auto_event_packages: notification_config.auto_event_packages,
            email_accounts: platform.external_restore_intents.email_accounts,
            soulous: platform.external_restore_intents.soulous,
            health_sources: platform.external_restore_intents.health_sources,
            background_reminders: platform.external_restore_intents.background_reminders,
        },
    };

    let mut document = crate::export::build_export(store, now)?;
    document["version"] = json!(PORTABLE_EXPORT_VERSION);
    document["_portable"] = serde_json::to_value(section)?;
    validate_portable_document(&document)?;
    Ok(document)
}

pub fn validate_portable_document(document: &Value) -> Result<PortableRecoveryPlan> {
    if document.get("format").and_then(Value::as_str) != Some(crate::export::FORMAT)
        || document.get("version").and_then(Value::as_u64) != Some(PORTABLE_EXPORT_VERSION as u64)
    {
        return Err(invalid("document is not solum-export v3"));
    }
    let section: PortableSection = serde_json::from_value(
        document
            .get("_portable")
            .cloned()
            .ok_or_else(|| invalid("missing _portable"))?,
    )
    .map_err(|_| invalid("invalid _portable shape"))?;
    if section.format != PORTABLE_FORMAT || section.version != PORTABLE_SCHEMA_VERSION {
        return Err(invalid("unsupported _portable format or version"));
    }
    validate_section(&section, document.get("_restore"))?;

    let counts = vec![
        (
            "portable.notification_captures".into(),
            section.notification_journal.captures.len(),
        ),
        (
            "portable.filter_proposals".into(),
            section.notification_journal.filter_proposals.len(),
        ),
        (
            "portable.action_proposals".into(),
            section.notification_journal.action_proposals.len(),
        ),
        ("portable.audit_log".into(), section.audit_log.len()),
        ("portable.chat_sessions".into(), section.chat.sessions.len()),
        ("portable.chat_drafts".into(), section.chat.drafts.len()),
        ("portable.documents".into(), section.documents.len()),
        (
            "portable.pending_notifications".into(),
            section.pending_notifications.len(),
        ),
    ]
    .into_iter()
    .filter(|(_, count)| *count != 0)
    .collect();
    Ok(PortableRecoveryPlan {
        counts,
        attachment_bytes: section
            .documents
            .iter()
            .map(|item| item.plaintext_size)
            .sum(),
    })
}

pub(super) fn build_notification_journal(store: &Store) -> Result<PortableNotificationJournal> {
    let mut captures = store.list_notification_captures()?;
    captures.sort_by_key(|row| row.id.unwrap_or_default());
    if captures.len() > MAX_JOURNAL_ROWS {
        return Err(invalid("notification capture count exceeds limit"));
    }
    let mut capture_ids = HashMap::new();
    let mut capture_ordinals = BTreeMap::new();
    let mut portable_captures = Vec::with_capacity(captures.len());
    for capture in captures {
        let local_id = capture
            .id
            .ok_or_else(|| invalid("capture missing local id"))?;
        let raw_input_guid = store.raw_input_guid_for_backup(capture.raw_input_id)?;
        let event_guid = capture
            .event_id
            .map(|id| store.event_guid_for_backup(id))
            .transpose()?
            .flatten();
        let identity = json!({ "raw_input_guid": raw_input_guid });
        let id = next_stable_id("capture", &identity, &mut capture_ordinals)?;
        capture_ids.insert(local_id, id.clone());
        let restore_state = if capture.state == CaptureState::Queued {
            CaptureState::NeedsReview
        } else {
            capture.state
        };
        portable_captures.push(PortableCaptureRow {
            id,
            raw_input_guid,
            event_guid,
            package_name: capture.package_name,
            title: capture.title,
            body: capture.body,
            received_at: fmt_ts(&capture.received_at),
            content_hash: capture.content_hash,
            local_only: capture.local_only,
            lane: capture.lane.as_str().to_string(),
            source_state: capture.state,
            restore_state,
            reason: capture.reason,
            created_at: fmt_ts(&capture.created_at),
        });
    }

    let mut filters = store.list_notification_filter_proposals()?;
    filters.sort_by_key(|row| row.id.unwrap_or_default());
    if filters.len() > MAX_JOURNAL_ROWS {
        return Err(invalid("filter proposal count exceeds limit"));
    }
    let mut filter_ordinals = BTreeMap::new();
    let mut portable_filters = Vec::with_capacity(filters.len());
    for row in filters {
        let created_at = fmt_ts(&row.created_at);
        let identity = json!({
            "package_name": row.package_name,
            "pattern": row.pattern,
            "matcher": row.matcher,
            "reason": row.reason,
            "created_at": created_at,
        });
        portable_filters.push(PortableFilterProposalRow {
            id: next_stable_id("filter-proposal", &identity, &mut filter_ordinals)?,
            package_name: row.package_name,
            pattern: row.pattern,
            matcher: row.matcher,
            reason: row.reason,
            state: row.state.as_str().to_string(),
            created_at,
        });
    }

    let mut actions = store.list_notification_action_proposals()?;
    actions.sort_by_key(|row| row.id.unwrap_or_default());
    if actions.len() > MAX_JOURNAL_ROWS {
        return Err(invalid("action proposal count exceeds limit"));
    }
    let mut action_ordinals = BTreeMap::new();
    let mut portable_actions = Vec::with_capacity(actions.len());
    for row in actions {
        let capture_id = capture_ids
            .get(&row.capture_id)
            .cloned()
            .ok_or_else(|| invalid("action proposal has no capture"))?;
        let event_guid = store
            .event_guid_for_backup(row.event_id)?
            .or_else(|| is_lower_hex(&row.event_guid, 32).then(|| row.event_guid.clone()));
        let event_start = fmt_ts(&row.event_start);
        let new_start = row.new_start.map(|value| fmt_ts(&value));
        let created_at = fmt_ts(&row.created_at);
        let identity = json!({
            "capture_id": capture_id,
            "kind": row.kind,
            "event_guid": event_guid,
            "event_title": row.event_title,
            "event_start": event_start,
            "new_start": new_start,
            "reason": row.reason,
            "created_at": created_at,
        });
        let restore_state = if row.state == ActionProposalState::Pending {
            ActionProposalState::Stale
        } else {
            row.state
        };
        portable_actions.push(PortableActionProposalRow {
            id: next_stable_id("action-proposal", &identity, &mut action_ordinals)?,
            capture_id,
            kind: row.kind,
            event_guid,
            event_title: row.event_title,
            event_start,
            new_start,
            reason: row.reason,
            source_state: row.state,
            restore_state,
            created_at,
        });
    }

    Ok(PortableNotificationJournal {
        captures: portable_captures,
        filter_proposals: portable_filters,
        action_proposals: portable_actions,
    })
}

pub(super) fn build_audit_log(store: &Store) -> Result<Vec<PortableAuditRow>> {
    let rows = store.list_audit()?;
    if rows.len() > MAX_AUDIT_ROWS {
        return Err(invalid("audit row count exceeds limit"));
    }
    let mut ordinals = BTreeMap::new();
    rows.into_iter()
        .map(|row| {
            let identity = json!({
                "ts": row.ts,
                "tool": row.tool,
                "risk": row.risk,
                "summary": row.summary,
                "decision": row.decision,
                "token_id": row.token_id,
                "detail": row.detail,
            });
            Ok(PortableAuditRow {
                id: next_stable_id("audit", &identity, &mut ordinals)?,
                ts: row.ts,
                tool: row.tool,
                risk: row.risk,
                summary: row.summary,
                decision: row.decision,
                token_id: row.token_id,
                detail: row.detail,
            })
        })
        .collect()
}

pub(super) fn build_pending_notifications(
    rows: Vec<PendingNotificationInput>,
) -> Result<Vec<PortablePendingNotification>> {
    if rows.len() > MAX_PENDING_NOTIFICATIONS {
        return Err(invalid("pending notification count exceeds limit"));
    }
    let mut ordinals = BTreeMap::new();
    rows.into_iter()
        .map(|row| {
            let identity = json!({
                "package_name": row.package_name,
                "title": row.title,
                "body": row.body,
                "received_at": row.received_at,
            });
            Ok(PortablePendingNotification {
                id: next_stable_id("pending-notification", &identity, &mut ordinals)?,
                package_name: row.package_name,
                title: row.title,
                body: row.body,
                received_at: row.received_at,
                failed: row.failed,
            })
        })
        .collect()
}

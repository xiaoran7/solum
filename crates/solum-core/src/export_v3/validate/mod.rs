//! `validate` -- the per-section validators: chat, documents, pending notifications, settings, restore intents, journal and audit rows.
//!
//! Part of the `export_v3` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{
    invalid, is_lower_hex, next_stable_id, restore_guids, validate_local_id, validate_rfc3339,
    validate_string_set,
};
use super::types::{
    PortableAuditRow, PortableFilterProposalRow, PortableNotificationJournal,
    PortableNotificationSettings,
};
use super::*;

pub(super) fn validate_platform_input(platform: &PlatformPortableData) -> Result<()> {
    validate_chat(&platform.chat)?;
    validate_documents(&platform.documents)?;
    validate_pending_inputs(&platform.pending_notifications)?;
    if let Some(llm) = &platform.llm {
        validate_llm(llm)?;
    }
    validate_external_intents(&platform.external_restore_intents)
}

pub(super) fn validate_section(section: &PortableSection, restore: Option<&Value>) -> Result<()> {
    validate_chat(&section.chat)?;
    validate_documents(&section.documents)?;
    if let Some(llm) = &section.agent_settings.llm {
        validate_llm(llm)?;
    }
    validate_notification_settings(&section.agent_settings.notification)?;
    validate_restore_intents(&section.restore_intents)?;

    let restore = restore
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("_restore must be an object"))?;
    let raw_guids = restore_guids(restore.get("raw_inputs"))?;
    validate_journal(&section.notification_journal, &raw_guids)?;
    validate_audit(&section.audit_log)?;
    validate_pending_rows(&section.pending_notifications)
}

pub(crate) fn validate_chat(chat: &PortableChat) -> Result<()> {
    if chat.sessions.len() > MAX_CHAT_SESSIONS || chat.drafts.len() > MAX_CHAT_SESSIONS {
        return Err(invalid("chat count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut total_chars = 0usize;
    for session in &chat.sessions {
        validate_local_id(&session.id)?;
        if !ids.insert(session.id.as_str()) {
            return Err(invalid("duplicate chat session id"));
        }
        validate_rfc3339(&session.updated_at)?;
        if session.title.chars().count() > MAX_TITLE_CHARS
            || session.messages.len() > MAX_MESSAGES_PER_SESSION
        {
            return Err(invalid("chat session exceeds limit"));
        }
        total_chars = total_chars.saturating_add(session.title.chars().count());
        for message in &session.messages {
            let length = message.content.chars().count();
            if length > MAX_MESSAGE_CHARS || message.intent.chars().count() > 128 {
                return Err(invalid("chat message exceeds limit"));
            }
            total_chars = total_chars.saturating_add(length);
        }
    }
    let mut draft_ids = BTreeSet::new();
    for draft in &chat.drafts {
        if !ids.contains(draft.session_id.as_str())
            || !draft_ids.insert(draft.session_id.as_str())
            || draft.content.chars().count() > MAX_MESSAGE_CHARS
        {
            return Err(invalid("invalid chat draft"));
        }
        total_chars = total_chars.saturating_add(draft.content.chars().count());
    }
    if total_chars > MAX_CHAT_TEXT_CHARS {
        return Err(invalid("chat text exceeds total limit"));
    }
    Ok(())
}

pub(crate) fn validate_documents(documents: &[DocumentManifest]) -> Result<()> {
    if documents.len() > MAX_DOCUMENTS {
        return Err(invalid("document count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut total = 0u64;
    for document in documents {
        validate_local_id(&document.id)?;
        if !ids.insert(document.id.as_str())
            || document.name.is_empty()
            || document.name.chars().count() > 255
            || document.media_type.chars().count() > 127
            || document.plaintext_size > MAX_DOCUMENT_BYTES
            || !is_lower_hex(&document.sha256, 64)
        {
            return Err(invalid("invalid document manifest"));
        }
        total = total
            .checked_add(document.plaintext_size)
            .ok_or_else(|| invalid("document bytes overflow"))?;
    }
    if total > MAX_DOCUMENT_TOTAL_BYTES {
        return Err(invalid("document bytes exceed total limit"));
    }
    Ok(())
}

pub(super) fn validate_pending_inputs(rows: &[PendingNotificationInput]) -> Result<()> {
    if rows.len() > MAX_PENDING_NOTIFICATIONS {
        return Err(invalid("pending notification count exceeds limit"));
    }
    for row in rows {
        validate_pending_fields(&row.package_name, &row.title, &row.body, &row.received_at)?;
    }
    Ok(())
}

pub(super) fn validate_pending_rows(rows: &[PortablePendingNotification]) -> Result<()> {
    if rows.len() > MAX_PENDING_NOTIFICATIONS {
        return Err(invalid("pending notification count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in rows {
        validate_pending_fields(&row.package_name, &row.title, &row.body, &row.received_at)?;
        let identity = json!({
            "package_name": row.package_name,
            "title": row.title,
            "body": row.body,
            "received_at": row.received_at,
        });
        let expected = next_stable_id("pending-notification", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid pending notification id"));
        }
    }
    Ok(())
}

pub(super) fn validate_pending_fields(
    package: &str,
    title: &str,
    body: &str,
    received_at: &str,
) -> Result<()> {
    if package.is_empty()
        || package.chars().count() > 255
        || title.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
        || body.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
    {
        return Err(invalid("pending notification field exceeds limit"));
    }
    validate_rfc3339(received_at)
}

pub(crate) fn validate_llm(llm: &PortableLlmSettings) -> Result<()> {
    crate::net::validate_endpoint(&llm.base_url, "backup LLM base_url")?;
    if llm.model.trim().is_empty()
        || llm.model.chars().count() > 200
        || llm.timeout_secs < 3
        || llm.timeout_secs > 600
        || llm.max_tokens == Some(0)
        || llm.temperature.is_some_and(|value| !value.is_finite())
    {
        return Err(invalid("invalid LLM settings"));
    }
    Ok(())
}

pub(super) fn validate_external_intents(intents: &ExternalRestoreIntents) -> Result<()> {
    validate_email_intents(&intents.email_accounts)?;
    validate_soulous_intent(intents.soulous.as_ref())?;
    validate_string_set(&intents.health_sources, 32, 128)
}

pub(crate) fn validate_restore_intents(intents: &PortableRestoreIntents) -> Result<()> {
    validate_string_set(&intents.notification_packages, 256, 255)?;
    validate_string_set(&intents.auto_event_packages, 256, 255)?;
    if !intents
        .auto_event_packages
        .iter()
        .all(|item| intents.notification_packages.contains(item))
    {
        return Err(invalid("auto-event intent is not a capture intent"));
    }
    validate_email_intents(&intents.email_accounts)?;
    validate_soulous_intent(intents.soulous.as_ref())?;
    validate_string_set(&intents.health_sources, 32, 128)
}

pub(super) fn validate_email_intents(accounts: &[EmailRestoreIntent]) -> Result<()> {
    if accounts.len() > 12 {
        return Err(invalid("email intent count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    for account in accounts {
        validate_local_id(&account.id)?;
        if !ids.insert(account.id.as_str())
            || account.label.is_empty()
            || account.label.chars().count() > 80
            || account.provider.is_empty()
            || account.provider.chars().count() > 40
            || account.address.is_empty()
            || account.address.chars().count() > 320
        {
            return Err(invalid("invalid email restore intent"));
        }
    }
    Ok(())
}

pub(super) fn validate_soulous_intent(intent: Option<&SoulousRestoreIntent>) -> Result<()> {
    if let Some(intent) = intent {
        crate::net::validate_endpoint(&intent.server_url, "backup Soulous server_url")?;
    }
    Ok(())
}

pub(super) fn validate_notification_settings(
    settings: &PortableNotificationSettings,
) -> Result<()> {
    if !(crate::notification_intelligence::MIN_BATCH_INTERVAL_MINUTES
        ..=crate::notification_intelligence::MAX_BATCH_INTERVAL_MINUTES)
        .contains(&settings.batch_interval_minutes)
        || settings.filter_rules.len() > 1_000
    {
        return Err(invalid("invalid notification settings"));
    }
    let mut ids = BTreeSet::new();
    for rule in &settings.filter_rules {
        validate_local_id(&rule.id)?;
        if !ids.insert(rule.id.as_str())
            || rule.pattern.is_empty()
            || rule.pattern.chars().count() > 160
            || rule.reason.chars().count() > 500
        {
            return Err(invalid("invalid notification filter rule"));
        }
    }
    Ok(())
}

pub(super) fn validate_journal(
    journal: &PortableNotificationJournal,
    raw_guids: &BTreeSet<String>,
) -> Result<()> {
    if journal.captures.len() > MAX_JOURNAL_ROWS
        || journal.filter_proposals.len() > MAX_JOURNAL_ROWS
        || journal.action_proposals.len() > MAX_JOURNAL_ROWS
    {
        return Err(invalid("notification journal count exceeds limit"));
    }
    let mut capture_ids = BTreeSet::new();
    let mut capture_ordinals = BTreeMap::new();
    for row in &journal.captures {
        if !raw_guids.contains(&row.raw_input_guid)
            || !is_lower_hex(&row.raw_input_guid, 32)
            || row
                .event_guid
                .as_ref()
                .is_some_and(|id| !is_lower_hex(id, 32))
            || row.package_name.is_empty()
            || row.title.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
            || row.body.chars().count() > MAX_NOTIFICATION_FIELD_CHARS
            || !is_lower_hex(&row.content_hash, 16)
            || !matches!(row.lane.as_str(), "urgent" | "batch")
            || row.restore_state
                != if row.source_state == CaptureState::Queued {
                    CaptureState::NeedsReview
                } else {
                    row.source_state
                }
        {
            return Err(invalid("invalid portable capture"));
        }
        parse_ts(&row.received_at)?;
        parse_ts(&row.created_at)?;
        let expected = next_stable_id(
            "capture",
            &json!({ "raw_input_guid": row.raw_input_guid }),
            &mut capture_ordinals,
        )?;
        if row.id != expected || !capture_ids.insert(row.id.as_str()) {
            return Err(invalid("invalid portable capture id"));
        }
    }

    validate_filter_rows(&journal.filter_proposals)?;
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in &journal.action_proposals {
        if !capture_ids.contains(row.capture_id.as_str())
            || row
                .event_guid
                .as_ref()
                .is_some_and(|id| !is_lower_hex(id, 32))
            || row.restore_state
                != if row.source_state == ActionProposalState::Pending {
                    ActionProposalState::Stale
                } else {
                    row.source_state
                }
        {
            return Err(invalid("invalid portable action proposal"));
        }
        parse_ts(&row.event_start)?;
        row.new_start.as_deref().map(parse_ts).transpose()?;
        parse_ts(&row.created_at)?;
        let identity = json!({
            "capture_id": row.capture_id,
            "kind": row.kind,
            "event_guid": row.event_guid,
            "event_title": row.event_title,
            "event_start": row.event_start,
            "new_start": row.new_start,
            "reason": row.reason,
            "created_at": row.created_at,
        });
        let expected = next_stable_id("action-proposal", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid portable action proposal id"));
        }
    }
    Ok(())
}

pub(super) fn validate_filter_rows(rows: &[PortableFilterProposalRow]) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in rows {
        parse_ts(&row.created_at)?;
        if !matches!(row.state.as_str(), "pending" | "accepted" | "dismissed") {
            return Err(invalid("invalid filter proposal state"));
        }
        let identity = json!({
            "package_name": row.package_name,
            "pattern": row.pattern,
            "matcher": row.matcher,
            "reason": row.reason,
            "created_at": row.created_at,
        });
        let expected = next_stable_id("filter-proposal", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid filter proposal id"));
        }
    }
    Ok(())
}

pub(super) fn validate_audit(rows: &[PortableAuditRow]) -> Result<()> {
    if rows.len() > MAX_AUDIT_ROWS {
        return Err(invalid("audit row count exceeds limit"));
    }
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeMap::new();
    for row in rows {
        parse_ts(&row.ts)?;
        let identity = json!({
            "ts": row.ts,
            "tool": row.tool,
            "risk": row.risk,
            "summary": row.summary,
            "decision": row.decision,
            "token_id": row.token_id,
            "detail": row.detail,
        });
        let expected = next_stable_id("audit", &identity, &mut ordinals)?;
        if row.id != expected || !ids.insert(row.id.as_str()) {
            return Err(invalid("invalid audit id"));
        }
    }
    Ok(())
}

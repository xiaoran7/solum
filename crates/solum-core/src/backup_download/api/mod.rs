//! `api` -- the module's entry points: list, delete, and download-and-stage a version.
//!
//! Part of the `backup_download` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::stage::{
    strict_timestamp, valid_lower_hex, valid_uuid, validate_catalog, verify_downloaded_object,
};
use super::*;

/// Fetch and strictly validate complete-only cloud metadata.
pub fn fetch_backup_catalog(
    transport: &dyn BackupCloudReader,
    session: &mut AccountSession,
    limit: usize,
) -> DownloadResult<BackupCatalog> {
    if !(1..=MAX_CATALOG_BACKUPS).contains(&limit) {
        return Err(BackupDownloadError::InvalidCatalog);
    }
    let catalog = transport.list_backups(session, limit)?;
    validate_catalog(&catalog, limit)?;
    Ok(catalog)
}

/// Re-read and validate the complete catalog immediately before deletion,
/// then submit the exact plan ETag the human previewed. The server performs a
/// second atomic comparison under the tenant mutation lock.
pub fn delete_backup_version(
    transport: &(impl BackupCloudReader + BackupCloudDelete),
    session: &mut AccountSession,
    request: &BackupDeleteRequest,
) -> DownloadResult<()> {
    validate_backup_delete_request(request)?;
    let catalog = fetch_backup_catalog(transport, session, MAX_CATALOG_BACKUPS)?;
    let selected = catalog
        .backups
        .iter()
        .find(|version| version.backup_id == request.backup_id)
        .ok_or(BackupDownloadError::BackupNotFound)?;
    if selected.plan_etag != request.plan_etag
        || selected.committed_at != request.committed_at
        || selected.total_bytes != request.total_bytes
        || selected.object_count != request.object_count
    {
        return Err(BackupDownloadError::DeletePrecondition);
    }
    transport.delete_backup(session, &request.backup_id, &request.plan_etag)
}

pub fn validate_backup_delete_request(request: &BackupDeleteRequest) -> DownloadResult<()> {
    if !valid_uuid(&request.backup_id)
        || request.backup_id != request.backup_id.to_ascii_lowercase()
        || !valid_lower_hex(&request.plan_etag, 32)
        || strict_timestamp(&request.committed_at).is_none()
        || request.total_bytes == 0
        || request.total_bytes > MAX_CLOUD_BACKUP_BYTES
        || request.object_count == 0
        || request.object_count > MAX_CLOUD_OBJECTS
    {
        return Err(BackupDownloadError::InvalidCatalog);
    }
    Ok(())
}

/// Download, authenticate and assemble one still-inactive sealed generation.
/// The catalog is revalidated so callers cannot fabricate a partially checked
/// selection. No live Store or active pointer is opened or modified.
pub fn download_and_stage_backup(
    request: BackupStageRequest<'_>,
    transport: &dyn BackupCloudReader,
    session: &mut AccountSession,
) -> DownloadResult<StagedBackupOutcome> {
    validate_catalog(request.catalog, MAX_CATALOG_BACKUPS)?;
    if !valid_uuid(request.user_id)
        || request.user_id != request.user_id.to_ascii_lowercase()
        || session.user_id != request.user_id
    {
        return Err(BackupDownloadError::MetadataMismatch);
    }
    let selected = request
        .catalog
        .backups
        .iter()
        .find(|backup| backup.backup_id == request.backup_id)
        .ok_or(BackupDownloadError::BackupNotFound)?;
    if selected.protocol_version != backup::PROTOCOL_VERSION {
        return Err(BackupDownloadError::UnsupportedVersion(
            selected.protocol_version,
        ));
    }

    let main = selected
        .objects
        .iter()
        .find(|object| object.kind == BackupObjectKind::Main)
        .ok_or(BackupDownloadError::InvalidCatalog)?;
    let main_bytes = transport.download_object(session, &selected.backup_id, main)?;
    verify_downloaded_object(main, &main_bytes)?;
    let verified = backup::open_backup(&main_bytes, request.master_key)
        .map_err(|_| BackupDownloadError::Authentication)?;
    if verified.header.user_id != request.user_id
        || verified.header.device_generation != selected.generation
        || verified.header.backup_id != selected.backup_id
        || verified.header.created_at != selected.created_at
        || verified.header.protocol_version != selected.protocol_version
    {
        return Err(BackupDownloadError::MetadataMismatch);
    }
    if verified.header.payload_version != PORTABLE_EXPORT_VERSION
        || verified.import_plan.version != PORTABLE_EXPORT_VERSION
    {
        return Err(BackupDownloadError::InvalidPayload);
    }

    let manifests: Vec<DocumentManifest> = serde_json::from_value(
        verified
            .document
            .get("_portable")
            .and_then(|value| value.get("documents"))
            .cloned()
            .ok_or(BackupDownloadError::InvalidPayload)?,
    )
    .map_err(|_| BackupDownloadError::InvalidPayload)?;
    if manifests.len() > attachment::MAX_ATTACHMENTS {
        return Err(BackupDownloadError::AttachmentSet);
    }

    let metadata = BackupMetadata {
        user_id: request.user_id.to_string(),
        device_generation: selected.generation,
        backup_id: selected.backup_id.clone(),
        created_at: selected.created_at.clone(),
    };
    let attachment_objects: BTreeMap<_, _> = selected
        .objects
        .iter()
        .filter(|object| object.kind == BackupObjectKind::Attachment)
        .map(|object| (object.object_id.as_str(), object))
        .collect();
    let mut expected_ids = BTreeSet::new();
    let mut manifest_objects = Vec::with_capacity(manifests.len());
    for manifest in &manifests {
        let object_id = attachment_object_id(request.master_key, &selected.backup_id, &manifest.id)
            .map_err(|_| BackupDownloadError::AttachmentSet)?;
        if object_id == main.object_id || !expected_ids.insert(object_id.clone()) {
            return Err(BackupDownloadError::AttachmentSet);
        }
        let object = attachment_objects
            .get(object_id.as_str())
            .copied()
            .ok_or(BackupDownloadError::AttachmentSet)?;
        manifest_objects.push((manifest, object));
    }
    if expected_ids.len() != attachment_objects.len() {
        return Err(BackupDownloadError::AttachmentSet);
    }

    let mut restored = Vec::with_capacity(manifest_objects.len());
    for (manifest, object) in manifest_objects {
        let ciphertext = transport.download_object(session, &selected.backup_id, object)?;
        verify_downloaded_object(object, &ciphertext)?;
        let binding = AttachmentBinding::from_manifest(&metadata, manifest);
        let plaintext = attachment::open_attachment(&ciphertext, &binding, request.master_key)
            .map_err(|_| BackupDownloadError::Authentication)?;
        restored.push(RestoredAttachment {
            id: manifest.id.clone(),
            plaintext,
        });
    }

    let prepared = restore_v3::prepare_portable_restore(verified.document, restored)
        .map_err(|_| BackupDownloadError::InvalidPayload)?;
    let sealed = crate::portable_generation::stage_portable_generation(
        request.profile_root,
        prepared,
        request.now,
    )
    .map_err(|_| BackupDownloadError::Staging)?;
    Ok(StagedBackupOutcome {
        backup_id: selected.backup_id.clone(),
        generation: selected.generation,
        generation_id: sealed.generation_id().to_string(),
        object_count: selected.object_count,
        attachment_count: manifests.len(),
        restored_rows: sealed.counts().applied,
    })
}

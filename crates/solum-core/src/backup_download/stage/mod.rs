//! `stage` -- what download-and-stage does: restore the objects into a fresh generation and validate them.
//!
//! Part of the `backup_download` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) fn validate_catalog(catalog: &BackupCatalog, max_backups: usize) -> DownloadResult<()> {
    if catalog.active_generation == 0
        || catalog.active_generation > MAX_SAFE_INTEGER
        || catalog.backups.len() > max_backups
        || catalog.backups.len() > MAX_CATALOG_BACKUPS
    {
        return Err(BackupDownloadError::InvalidCatalog);
    }
    let mut backup_ids = BTreeSet::new();
    let mut previous: Option<(DateTime<Utc>, &str)> = None;
    for backup in &catalog.backups {
        let committed =
            strict_timestamp(&backup.committed_at).ok_or(BackupDownloadError::InvalidCatalog)?;
        if let Some((previous_time, previous_id)) = previous {
            if committed > previous_time
                || (committed == previous_time && backup.backup_id.as_str() >= previous_id)
            {
                return Err(BackupDownloadError::InvalidCatalog);
            }
        }
        previous = Some((committed, &backup.backup_id));
        if !valid_uuid(&backup.backup_id)
            || backup.backup_id != backup.backup_id.to_ascii_lowercase()
            || !backup_ids.insert(backup.backup_id.as_str())
            || backup.generation == 0
            || backup.generation > MAX_SAFE_INTEGER
            || backup.generation > catalog.active_generation
            || strict_timestamp(&backup.created_at).is_none()
            || strict_timestamp(&backup.created_at).is_some_and(|created| created > committed)
            || backup.protocol_version == 0
            || backup.total_bytes == 0
            || backup.total_bytes > MAX_CLOUD_BACKUP_BYTES
            || backup.object_count != backup.objects.len()
            || backup.objects.is_empty()
            || backup.objects.len() > MAX_CLOUD_OBJECTS
            || !valid_lower_hex(&backup.plan_etag, 32)
        {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let mut object_ids = BTreeSet::new();
        let mut main_count = 0usize;
        let mut total = 0u64;
        let mut previous_object: Option<&str> = None;
        for object in &backup.objects {
            if let Some(previous_id) = previous_object {
                if object.object_id.as_str() <= previous_id {
                    return Err(BackupDownloadError::InvalidCatalog);
                }
            }
            previous_object = Some(&object.object_id);
            if !valid_uuid(&object.object_id)
                || object.object_id != object.object_id.to_ascii_lowercase()
                || !object_ids.insert(object.object_id.as_str())
                || object.bytes == 0
                || object.bytes > MAX_CLOUD_OBJECT_BYTES
                || !valid_lower_hex(&object.sha256, 32)
            {
                return Err(BackupDownloadError::InvalidCatalog);
            }
            if object.kind == BackupObjectKind::Main {
                main_count += 1;
                if object.bytes > backup::MAX_CONTAINER_BYTES as u64 {
                    return Err(BackupDownloadError::InvalidCatalog);
                }
            } else if object.bytes > attachment::MAX_CONTAINER_BYTES as u64 {
                return Err(BackupDownloadError::InvalidCatalog);
            }
            total = total
                .checked_add(object.bytes)
                .ok_or(BackupDownloadError::InvalidCatalog)?;
        }
        if main_count != 1 || total != backup.total_bytes {
            return Err(BackupDownloadError::InvalidCatalog);
        }
        let canonical = BackupPlan {
            backup_id: backup.backup_id.clone(),
            generation: backup.generation,
            created_at: backup.created_at.clone(),
            protocol_version: backup.protocol_version,
            objects: backup.objects.clone(),
        };
        let encoded =
            serde_json::to_vec(&canonical).map_err(|_| BackupDownloadError::InvalidCatalog)?;
        if lower_hex(&Sha256::digest(encoded)) != backup.plan_etag {
            return Err(BackupDownloadError::InvalidCatalog);
        }
    }
    Ok(())
}

pub(super) fn verify_downloaded_object(
    object: &BackupObjectPlan,
    bytes: &[u8],
) -> DownloadResult<()> {
    if bytes.len() as u64 != object.bytes || lower_hex(&Sha256::digest(bytes)) != object.sha256 {
        return Err(BackupDownloadError::TransportIntegrity);
    }
    Ok(())
}

pub(super) fn strict_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&Utc);
    (parsed.to_rfc3339_opts(SecondsFormat::Millis, true) == value).then_some(parsed)
}

pub(super) fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
        && value != "00000000-0000-0000-0000-000000000000"
}

pub(super) fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn lower_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

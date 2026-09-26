//! staging. Part of the `backup_upload` module; see `mod.rs` for the contract.
//!
//! Moved verbatim from the single-file `backup_upload.rs`.

//! Local sealing and staging: ciphertext on disk, object-id derivation, and the shared validators.

use super::types::StagedObject;
use super::*;

pub(super) fn portable_manifests(document: &Value) -> UploadResult<Vec<DocumentManifest>> {
    export_v3::validate_portable_document(document)
        .map_err(|_| BackupUploadError::InvalidSnapshot)?;
    serde_json::from_value(
        document
            .get("_portable")
            .and_then(|value| value.get("documents"))
            .cloned()
            .ok_or(BackupUploadError::InvalidSnapshot)?,
    )
    .map_err(|_| BackupUploadError::InvalidSnapshot)
}

pub(super) fn stage_ciphertext(
    staging_dir: &Path,
    object_id: String,
    kind: BackupObjectKind,
    ciphertext: &[u8],
) -> UploadResult<StagedObject> {
    if ciphertext.is_empty() {
        return Err(BackupUploadError::Encryption);
    }
    let path = staging_dir.join(&object_id);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| BackupUploadError::Staging)?;
    file.write_all(ciphertext)
        .and_then(|_| file.sync_all())
        .map_err(|_| BackupUploadError::Staging)?;
    crate::fsatomic::sync_directory(staging_dir).map_err(|_| BackupUploadError::Staging)?;
    Ok(StagedObject {
        plan: BackupObjectPlan {
            object_id,
            kind,
            bytes: ciphertext.len() as u64,
            sha256: sha256_hex(ciphertext),
        },
        path,
    })
}

pub(super) fn clean_encrypted_staging(root: &Path) -> UploadResult<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(BackupUploadError::Staging),
    };
    for entry in entries {
        let entry = entry.map_err(|_| BackupUploadError::Staging)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let metadata =
            std::fs::symlink_metadata(entry.path()).map_err(|_| BackupUploadError::Staging)?;
        if name.len() == 32
            && name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && metadata.file_type().is_dir()
            && !metadata.file_type().is_symlink()
        {
            std::fs::remove_dir_all(entry.path()).map_err(|_| BackupUploadError::Staging)?;
        }
    }
    Ok(())
}

pub(super) fn map_attachment_error(error: attachment::AttachmentError) -> BackupUploadError {
    match error {
        attachment::AttachmentError::SizeLimit => BackupUploadError::SizeLimit,
        attachment::AttachmentError::RandomSource => BackupUploadError::RandomSource,
        _ => BackupUploadError::Encryption,
    }
}

pub(super) fn random_hex(bytes: usize) -> UploadResult<String> {
    let mut random = vec![0u8; bytes];
    getrandom::getrandom(&mut random).map_err(|_| BackupUploadError::RandomSource)?;
    Ok(hex(&random))
}

pub(super) fn random_uuid_v4() -> UploadResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| BackupUploadError::RandomSource)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
}

/// Derive the server-visible UUID for one manifest attachment without exposing
/// its local id. The random backup id makes the result unlinkable across
/// backup versions; the master key keeps the mapping opaque to the server.
pub fn attachment_object_id(
    master_key: &[u8; 32],
    backup_id: &str,
    attachment_id: &str,
) -> UploadResult<String> {
    if !valid_uuid(backup_id)
        || backup_id != backup_id.to_ascii_lowercase()
        || !valid_attachment_id(attachment_id)
    {
        return Err(BackupUploadError::InvalidSnapshot);
    }
    let backup_len = u32::try_from(backup_id.len()).map_err(|_| BackupUploadError::Encryption)?;
    let attachment_len =
        u32::try_from(attachment_id.len()).map_err(|_| BackupUploadError::Encryption)?;
    let mut info =
        Vec::with_capacity(OBJECT_ID_KDF_INFO.len() + 8 + backup_id.len() + attachment_id.len());
    info.extend_from_slice(OBJECT_ID_KDF_INFO);
    info.extend_from_slice(&backup_len.to_be_bytes());
    info.extend_from_slice(backup_id.as_bytes());
    info.extend_from_slice(&attachment_len.to_be_bytes());
    info.extend_from_slice(attachment_id.as_bytes());

    let hkdf = Hkdf::<Sha256>::new(Some(OBJECT_ID_KDF_SALT), master_key);
    let mut bytes = [0u8; 16];
    hkdf.expand(&info, &mut bytes)
        .map_err(|_| BackupUploadError::Encryption)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
}

fn valid_attachment_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._:".contains(&byte))
}

pub(super) fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
        && value != "00000000-0000-0000-0000-000000000000"
}

pub(super) fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn valid_server_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

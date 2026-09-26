//! `verify` -- re-reading what was staged, plus the small IO helpers the other sections share.
//!
//! Part of the `portable_generation` module; see `mod.rs` for the
//! contract. Moved verbatim from the single-file `portable_generation.rs`.

use super::*;

use super::stage::{ExpectedSpool, SpoolRecord};

pub(super) fn validate_bootstrap(
    root: &Path,
    generation: &str,
    bootstrap: &WebViewBootstrap,
) -> Result<()> {
    if bootstrap.format != BOOTSTRAP_FORMAT
        || bootstrap.version != BOOTSTRAP_VERSION
        || bootstrap.generation != generation
    {
        return Err(invalid("invalid or unsupported WebView bootstrap"));
    }
    crate::export_v3::validate_chat(&bootstrap.chat)?;
    if let Some(llm) = &bootstrap.llm {
        crate::export_v3::validate_llm(llm)?;
    }
    crate::export_v3::validate_restore_intents(&bootstrap.restore_intents)?;
    let manifests: Vec<_> = bootstrap
        .documents
        .iter()
        .map(|document| DocumentManifest {
            id: document.id.clone(),
            name: document.name.clone(),
            media_type: document.media_type.clone(),
            plaintext_size: document.plaintext_size,
            sha256: document.sha256.clone(),
        })
        .collect();
    crate::export_v3::validate_documents(&manifests)?;

    let directory = root.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR);
    ensure_directory(&directory, "WebView document directory")?;
    let mut expected_names = BTreeSet::new();
    for document in &bootstrap.documents {
        let name = document_blob_name(&document.id);
        if document.blob != format!("{WEBVIEW_DOCUMENTS_DIR}/{name}")
            || !expected_names.insert(name)
        {
            return Err(invalid("invalid WebView bootstrap document path"));
        }
        if digest_file(
            &root.join(WEBVIEW_DIR).join(&document.blob),
            document.plaintext_size,
        )? != document.sha256
        {
            return Err(invalid("WebView bootstrap document digest does not match"));
        }
    }
    let actual_names: BTreeSet<String> = std::fs::read_dir(&directory)
        .map_err(|error| storage("WebView document directory", error))?
        .map(|entry| {
            let entry = entry.map_err(|error| storage("WebView document directory", error))?;
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| storage("WebView document", error))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid(
                    "WebView document directory contains an invalid entry",
                ));
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("WebView document name is not UTF-8"))
        })
        .collect::<Result<_>>()?;
    if actual_names != expected_names {
        return Err(invalid("WebView document files do not match bootstrap"));
    }
    Ok(())
}

pub(super) fn expected_spool(bundle: &PlatformRestoreBundle) -> Result<Vec<ExpectedSpool>> {
    bundle
        .pending_notifications
        .iter()
        .map(|row| {
            let received_at = DateTime::parse_from_rfc3339(&row.received_at)
                .map_err(|_| invalid("pending notification timestamp is invalid"))?;
            let mut relative_path = PathBuf::from(NOTIFICATION_SPOOL_DIR);
            if row.failed {
                relative_path.push(FAILED_NOTIFICATION_DIR);
            }
            relative_path.push(format!("restore-{}.json", row.id));
            Ok(ExpectedSpool {
                relative_path,
                record: SpoolRecord {
                    ts: received_at.timestamp_millis(),
                    pkg: row.package_name.clone(),
                    title: row.title.clone(),
                    text: row.body.clone(),
                },
            })
        })
        .collect()
}

pub(super) fn spool_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    let spool = root.join(NOTIFICATION_SPOOL_DIR);
    if !spool.exists() {
        return Ok(paths);
    }
    ensure_directory(&spool, "notification spool directory")?;
    for entry in
        std::fs::read_dir(&spool).map_err(|error| storage("notification spool directory", error))?
    {
        let entry = entry.map_err(|error| storage("notification spool directory", error))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| storage("notification spool entry", error))?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            insert_spool_path(root, &path, &mut paths)?;
        } else if entry.file_name() == FAILED_NOTIFICATION_DIR
            && metadata.is_dir()
            && !metadata.file_type().is_symlink()
        {
            collect_failed_spool_paths(root, &path, &mut paths)?;
        } else {
            return Err(invalid("notification spool contains an invalid entry"));
        }
    }
    Ok(paths)
}

fn collect_failed_spool_paths(
    root: &Path,
    directory: &Path,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .map_err(|error| storage("notification spool directory", error))?
    {
        let entry = entry.map_err(|error| storage("notification spool directory", error))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| storage("notification spool entry", error))?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            insert_spool_path(root, &path, out)?;
        } else {
            return Err(invalid("notification spool contains an invalid entry"));
        }
    }
    Ok(())
}

fn insert_spool_path(root: &Path, path: &Path, out: &mut BTreeSet<PathBuf>) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("notification spool escaped generation root"))?
        .to_path_buf();
    if !out.insert(relative) {
        return Err(invalid("notification spool contains a duplicate entry"));
    }
    Ok(())
}

pub(super) fn document_blob_name(id: &str) -> String {
    format!("{}.blob", lower_hex(&Sha256::digest(id.as_bytes())))
}

pub(super) fn write_new(path: &Path, bytes: &[u8], label: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| storage(label, error))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| storage(label, error))?;
    file.write_all(bytes)
        .map_err(|error| storage(label, error))?;
    file.sync_all().map_err(|error| storage(label, error))
}

pub(super) fn read_bounded_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<T> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| storage(label, error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(invalid(format!("staged {label} is invalid or too large")));
    }
    let bytes = std::fs::read(path).map_err(|error| storage(label, error))?;
    if bytes.len() as u64 > limit {
        return Err(invalid(format!("staged {label} exceeds its size limit")));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid(format!("staged {label} is invalid")))
}

pub(super) fn digest_file(path: &Path, expected_size: u64) -> Result<String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| storage("WebView document", error))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != expected_size
        || expected_size > crate::attachment::MAX_ATTACHMENT_BYTES as u64
    {
        return Err(invalid("staged WebView document has an invalid size"));
    }
    let mut file = std::fs::File::open(path).map_err(|error| storage("WebView document", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read_total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| storage("WebView document", error))?;
        if read == 0 {
            break;
        }
        read_total = read_total
            .checked_add(read as u64)
            .ok_or_else(|| invalid("staged WebView document size overflow"))?;
        if read_total > expected_size {
            return Err(invalid("staged WebView document changed while verifying"));
        }
        hasher.update(&buffer[..read]);
    }
    if read_total != expected_size {
        return Err(invalid("staged WebView document changed while verifying"));
    }
    Ok(lower_hex(&hasher.finalize()))
}

pub(super) fn ensure_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| storage(label, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(format!("staged {label} is invalid")));
    }
    Ok(())
}

pub(super) fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn storage(label: &str, error: std::io::Error) -> CoreError {
    CoreError::Storage(format!("could not stage {label}: {error}"))
}

pub(super) fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Invalid(message.into())
}

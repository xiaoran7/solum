//! `stage` -- staging SQLite, WebView documents, the notification spool and the native policy.
//!
//! Part of the `portable_generation` module; see `mod.rs` for the
//! contract. Moved verbatim from the single-file `portable_generation.rs`.

use super::*;

use super::types::{BootstrapDocument, SealedPortableGeneration};
use super::verify::{
    digest_file, document_blob_name, ensure_directory, expected_spool, invalid, read_bounded_json,
    spool_paths, storage, write_new,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpoolRecord {
    pub(super) ts: i64,
    pub(super) pkg: String,
    pub(super) title: String,
    pub(super) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExpectedSpool {
    pub(super) relative_path: PathBuf,
    pub(super) record: SpoolRecord,
}

/// Restore a fresh SQLite Store and persist every platform-owned handoff into
/// one sealed staging generation. This function never writes the active pointer.
pub fn stage_portable_generation(
    profile_root: &Path,
    prepared: PreparedPortableRestore,
    now: NaiveDateTime,
) -> Result<SealedPortableGeneration> {
    let staged = begin_profile_generation(profile_root)?;
    let sqlite_path = staged.path().join(SQLITE_FILE);
    let sqlite_text = sqlite_path
        .to_str()
        .ok_or_else(|| invalid("temporary generation SQLite path is not UTF-8"))?;
    let store = Store::open(sqlite_text)?;
    let (plan, counts, bundle) = restore_portable_into_empty_store(&store, prepared, now)?;

    let bootstrap = write_platform_bundle(staged.path(), staged.id(), &bundle)?;
    verify_platform_bundle(staged.path(), staged.id(), &bundle, &bootstrap)?;
    store.checkpoint_for_profile_generation()?;
    drop(store);

    let generation = seal_profile_generation(staged)?;
    Ok(SealedPortableGeneration {
        generation,
        plan,
        counts,
    })
}

fn write_platform_bundle(
    root: &Path,
    generation: &str,
    bundle: &PlatformRestoreBundle,
) -> Result<WebViewBootstrap> {
    write_new(
        &root.join(NOTIFICATION_POLICY_FILE),
        EMPTY_NOTIFICATION_POLICY.as_bytes(),
        "notification policy",
    )?;

    let documents_dir = root.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR);
    std::fs::create_dir_all(&documents_dir)
        .map_err(|error| storage("WebView document directory", error))?;
    let mut documents = Vec::with_capacity(bundle.documents.len());
    for document in &bundle.documents {
        let blob = document_blob_name(&document.manifest.id);
        write_new(
            &documents_dir.join(&blob),
            &document.plaintext,
            "WebView document",
        )?;
        documents.push(BootstrapDocument {
            id: document.manifest.id.clone(),
            name: document.manifest.name.clone(),
            media_type: document.manifest.media_type.clone(),
            plaintext_size: document.manifest.plaintext_size,
            sha256: document.manifest.sha256.clone(),
            blob: format!("{WEBVIEW_DOCUMENTS_DIR}/{blob}"),
        });
    }

    let bootstrap = WebViewBootstrap {
        format: BOOTSTRAP_FORMAT.into(),
        version: BOOTSTRAP_VERSION,
        generation: generation.into(),
        chat: bundle.chat.clone(),
        documents,
        llm: bundle.llm.clone(),
        restore_intents: bundle.restore_intents.clone(),
    };
    let encoded = serde_json::to_vec(&bootstrap)?;
    if encoded.len() as u64 > MAX_BOOTSTRAP_BYTES {
        return Err(invalid("WebView bootstrap exceeds its size limit"));
    }
    write_new(
        &root.join(WEBVIEW_DIR).join(WEBVIEW_BOOTSTRAP_FILE),
        &encoded,
        "WebView bootstrap",
    )?;

    for row in &bundle.pending_notifications {
        let received_at = DateTime::parse_from_rfc3339(&row.received_at)
            .map_err(|_| invalid("pending notification timestamp is invalid"))?;
        let record = SpoolRecord {
            ts: received_at.timestamp_millis(),
            pkg: row.package_name.clone(),
            title: row.title.clone(),
            text: row.body.clone(),
        };
        let mut directory = root.join(NOTIFICATION_SPOOL_DIR);
        if row.failed {
            directory.push(FAILED_NOTIFICATION_DIR);
        }
        let path = directory.join(format!("restore-{}.json", row.id));
        let mut encoded = serde_json::to_vec(&record)?;
        encoded.push(b'\n');
        if encoded.len() as u64 > MAX_SPOOL_FILE_BYTES {
            return Err(invalid(
                "pending notification spool entry exceeds its size limit",
            ));
        }
        write_new(&path, &encoded, "pending notification spool")?;
    }
    Ok(bootstrap)
}

fn verify_platform_bundle(
    root: &Path,
    generation: &str,
    bundle: &PlatformRestoreBundle,
    expected_bootstrap: &WebViewBootstrap,
) -> Result<()> {
    if std::fs::read(root.join(NOTIFICATION_POLICY_FILE))
        .map_err(|error| storage("notification policy", error))?
        != EMPTY_NOTIFICATION_POLICY.as_bytes()
    {
        return Err(invalid("staged notification policy is not fail-closed"));
    }

    let bootstrap_path = root.join(WEBVIEW_DIR).join(WEBVIEW_BOOTSTRAP_FILE);
    let actual_bootstrap: WebViewBootstrap =
        read_bounded_json(&bootstrap_path, MAX_BOOTSTRAP_BYTES, "WebView bootstrap")?;
    if actual_bootstrap != *expected_bootstrap
        || actual_bootstrap.format != BOOTSTRAP_FORMAT
        || actual_bootstrap.version != BOOTSTRAP_VERSION
        || actual_bootstrap.generation != generation
    {
        return Err(invalid(
            "staged WebView bootstrap does not match the restore plan",
        ));
    }
    verify_documents(root, &bundle.documents, &actual_bootstrap.documents)?;

    let expected_spool = expected_spool(bundle)?;
    let actual_paths = spool_paths(root)?;
    let expected_paths: BTreeSet<_> = expected_spool
        .iter()
        .map(|item| item.relative_path.clone())
        .collect();
    if actual_paths != expected_paths {
        return Err(invalid(
            "staged notification spool set does not match the restore plan",
        ));
    }
    for expected in expected_spool {
        let actual: SpoolRecord = read_bounded_json(
            &root.join(expected.relative_path),
            MAX_SPOOL_FILE_BYTES,
            "pending notification spool",
        )?;
        if actual != expected.record {
            return Err(invalid(
                "staged notification spool does not match the restore plan",
            ));
        }
    }
    Ok(())
}

fn verify_documents(
    root: &Path,
    restored: &[RestoredDocument],
    documents: &[BootstrapDocument],
) -> Result<()> {
    if restored.len() != documents.len() {
        return Err(invalid("staged WebView document set is incomplete"));
    }
    let directory = root.join(WEBVIEW_DIR).join(WEBVIEW_DOCUMENTS_DIR);
    ensure_directory(&directory, "WebView document directory")?;
    let actual_names: BTreeSet<String> = std::fs::read_dir(&directory)
        .map_err(|error| storage("WebView document directory", error))?
        .map(|entry| {
            let entry = entry.map_err(|error| storage("WebView document directory", error))?;
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| storage("WebView document", error))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid("staged WebView document is not a regular file"));
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("staged WebView document name is not UTF-8"))
        })
        .collect::<Result<_>>()?;
    let expected_names: BTreeSet<_> = documents
        .iter()
        .map(|document| {
            document
                .blob
                .strip_prefix(&format!("{WEBVIEW_DOCUMENTS_DIR}/"))
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    if actual_names != expected_names || expected_names.len() != documents.len() {
        return Err(invalid(
            "staged WebView document files do not match bootstrap",
        ));
    }

    for (source, document) in restored.iter().zip(documents) {
        let expected_blob = format!(
            "{WEBVIEW_DOCUMENTS_DIR}/{}",
            document_blob_name(&source.manifest.id)
        );
        if document.id != source.manifest.id
            || document.name != source.manifest.name
            || document.media_type != source.manifest.media_type
            || document.plaintext_size != source.manifest.plaintext_size
            || document.sha256 != source.manifest.sha256
            || document.blob != expected_blob
        {
            return Err(invalid("staged WebView document metadata is inconsistent"));
        }
        let digest = digest_file(
            &root.join(WEBVIEW_DIR).join(&document.blob),
            source.manifest.plaintext_size,
        )?;
        if digest != source.manifest.sha256 {
            return Err(invalid("staged WebView document digest does not match"));
        }
    }
    Ok(())
}

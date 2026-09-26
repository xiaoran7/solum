//! `reader` -- reading a sealed generation's bootstrap and documents in chunks.
//!
//! Part of the `portable_generation` module; see `mod.rs` for the
//! contract. Moved verbatim from the single-file `portable_generation.rs`.

use super::*;

use super::verify::{ensure_directory, invalid, read_bounded_json, storage, validate_bootstrap};

#[derive(Debug)]
pub struct PortableGenerationReader {
    generation_path: PathBuf,
    bootstrap: WebViewBootstrap,
}

impl PortableGenerationReader {
    /// Open only a sealed, inactive staging generation. Active/final
    /// generations are mutable and are never attachment sources for hydrate.
    pub fn open(profile_root: &Path, generation: &str) -> Result<Self> {
        resume_sealed_profile_generation(profile_root, generation)?;
        let generation_path = profile_root.join(STAGING_DIR).join(generation);
        ensure_directory(&generation_path, "inactive portable generation")?;
        let bootstrap: WebViewBootstrap = read_bounded_json(
            &generation_path
                .join(WEBVIEW_DIR)
                .join(WEBVIEW_BOOTSTRAP_FILE),
            MAX_BOOTSTRAP_BYTES,
            "WebView bootstrap",
        )?;
        validate_bootstrap(&generation_path, generation, &bootstrap)?;
        Ok(Self {
            generation_path,
            bootstrap,
        })
    }

    pub fn bootstrap(&self) -> &WebViewBootstrap {
        &self.bootstrap
    }

    pub fn read_document_chunk(
        &self,
        document_id: &str,
        offset: u64,
    ) -> Result<PortableDocumentChunk> {
        let document = self
            .bootstrap
            .documents
            .iter()
            .find(|document| document.id == document_id)
            .ok_or_else(|| invalid("portable document is not part of this generation"))?;
        if offset > document.plaintext_size {
            return Err(invalid("portable document chunk offset exceeds its size"));
        }
        let path = self.generation_path.join(WEBVIEW_DIR).join(&document.blob);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| storage("WebView document", error))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != document.plaintext_size
        {
            return Err(invalid("portable document changed after reader validation"));
        }
        let remaining = document.plaintext_size - offset;
        let take = remaining.min(WEBVIEW_DOCUMENT_CHUNK_BYTES as u64) as usize;
        let mut bytes = vec![0u8; take];
        let mut file =
            std::fs::File::open(&path).map_err(|error| storage("WebView document", error))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|error| storage("WebView document", error))?;
        file.read_exact(&mut bytes)
            .map_err(|_| invalid("portable document changed while reading a chunk"))?;
        let next = offset + bytes.len() as u64;
        Ok(PortableDocumentChunk {
            generation: self.bootstrap.generation.clone(),
            document_id: document.id.clone(),
            offset,
            total_size: document.plaintext_size,
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            done: next == document.plaintext_size,
        })
    }
}

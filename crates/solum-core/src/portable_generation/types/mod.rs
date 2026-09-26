//! `types` -- the sealed handle, WebView bootstrap and document-chunk shapes.
//!
//! Part of the `portable_generation` module; see `mod.rs` for the
//! contract. Moved verbatim from the single-file `portable_generation.rs`.

use super::*;

#[derive(Debug)]
pub struct SealedPortableGeneration {
    pub(super) generation: SealedProfileGeneration,
    pub(super) plan: ImportPlan,
    pub(super) counts: MergeCounts,
}

impl SealedPortableGeneration {
    pub fn generation_id(&self) -> &str {
        self.generation.id()
    }

    pub fn generation(&self) -> &SealedProfileGeneration {
        &self.generation
    }

    pub fn plan(&self) -> &ImportPlan {
        &self.plan
    }

    pub fn counts(&self) -> &MergeCounts {
        &self.counts
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebViewBootstrap {
    pub format: String,
    pub version: u32,
    pub generation: String,
    pub chat: PortableChat,
    pub documents: Vec<BootstrapDocument>,
    pub llm: Option<PortableLlmSettings>,
    pub restore_intents: PortableRestoreIntents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapDocument {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub plaintext_size: u64,
    pub sha256: String,
    pub blob: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortableDocumentChunk {
    pub generation: String,
    pub document_id: String,
    pub offset: u64,
    pub total_size: u64,
    pub data_base64: String,
    pub done: bool,
}

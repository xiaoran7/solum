//! `types` -- what a profile generation *is*, and how to find the live one: the three handles (staged, sealed, active), the on-disk pointer and manifest shapes, and `resolve_active_data_dir`. The pointer is a small JSON file naming the active generation, and a profile root with no valid pointer falls back to the default data directory rather than failing -- a fresh install has no generations yet, and refusing to start there would make the first run impossible.
//!
//! Part of the `profile_generation` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::io::{path_entry_exists, read_json};
use super::verify::{validate_pointer, validate_published_target};
use super::*;

pub const POINTER_FILE: &str = "active-generation.json";
pub const GENERATIONS_DIR: &str = "generations";
pub const STAGING_DIR: &str = "restore-staging";
pub const MANIFEST_FILE: &str = "generation-manifest.json";
pub const SQLITE_FILE: &str = "solum.sqlite";
pub const GENERATION_LOCK_FILE: &str = "active-generation.lock";

pub(super) const POINTER_FORMAT: &str = "solum-profile-generation";
pub(super) const MANIFEST_FORMAT: &str = "solum-profile-image";
pub(super) const PROTOCOL_VERSION: u32 = 1;
pub(super) const MAX_POINTER_BYTES: u64 = 4096;
pub(super) const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_GENERATION_FILES: usize = 12_000;
pub const MAX_GENERATION_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveProfileData {
    Legacy { path: PathBuf },
    Generation { id: String, path: PathBuf },
}

impl ActiveProfileData {
    pub fn path(&self) -> &Path {
        match self {
            Self::Legacy { path } | Self::Generation { path, .. } => path,
        }
    }

    pub fn generation_id(&self) -> Option<&str> {
        match self {
            Self::Legacy { .. } => None,
            Self::Generation { id, .. } => Some(id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StagedProfileGeneration {
    pub(super) profile_root: PathBuf,
    pub(super) id: String,
    pub(super) path: PathBuf,
}

impl StagedProfileGeneration {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone)]
pub struct SealedProfileGeneration {
    pub(super) profile_root: PathBuf,
    pub(super) id: String,
}

impl SealedProfileGeneration {
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActivePointer {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GenerationManifest {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) generation: String,
    pub(super) artifacts: Vec<GenerationArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GenerationArtifact {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) sha256: String,
}

/// Resolve the sole committed data directory. A malformed pointer is an error,
/// never permission to fall back to legacy or create an empty database.
pub fn resolve_active_data_dir(profile_root: &Path) -> Result<ActiveProfileData> {
    with_generation_lock(profile_root, || {
        resolve_active_data_dir_unlocked(profile_root)
    })
}

pub(crate) fn resolve_active_data_dir_unlocked(profile_root: &Path) -> Result<ActiveProfileData> {
    let pointer_path = profile_root.join(POINTER_FILE);
    if !path_entry_exists(&pointer_path)? {
        return Ok(ActiveProfileData::Legacy {
            path: profile_root.to_path_buf(),
        });
    }
    let pointer: ActivePointer = read_json(&pointer_path, MAX_POINTER_BYTES, "generation pointer")?;
    validate_pointer(&pointer)?;
    let path = profile_root.join(GENERATIONS_DIR).join(&pointer.generation);
    validate_published_target(&path, &pointer.generation)?;
    Ok(ActiveProfileData::Generation {
        id: pointer.generation,
        path,
    })
}

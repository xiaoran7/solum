//! Crash-safe publication of one complete profile data generation.
//!
//! The active pointer is the sole commit record. Staging directories and
//! unreferenced sealed generations are inert until that pointer is durably
//! replaced, so a crash cannot expose a half-restored SQLite/profile image.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

mod io;
mod publish;
mod stage;
#[cfg(test)]
mod tests;
mod types;
mod verify;

#[cfg(target_os = "android")]
use std::os::fd::AsRawFd;

#[cfg(not(target_os = "android"))]
use fs2::FileExt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub(crate) use io::{sync_tree_directories, with_generation_lock};
pub use publish::{publish_profile_generation, resume_sealed_profile_generation};
pub(crate) use publish::{
    publish_profile_generation_unlocked, resume_sealed_profile_generation_unlocked,
};
pub use stage::{begin_profile_generation, seal_profile_generation};
pub(crate) use types::resolve_active_data_dir_unlocked;
pub use types::{
    resolve_active_data_dir, ActiveProfileData, SealedProfileGeneration, StagedProfileGeneration,
    GENERATIONS_DIR, GENERATION_LOCK_FILE, MANIFEST_FILE, MAX_GENERATION_BYTES,
    MAX_GENERATION_FILES, POINTER_FILE, SQLITE_FILE, STAGING_DIR,
};

//! `purge_target` -- which memory layers a bulk purge may touch, and which timestamp column each one uses. The cascade-heavy layers are deliberately not purgeable in v1: batch-deleting schedule data has knock-on effects a preview cannot honestly summarize in one number, so those rows are deleted one at a time from the ledger instead.
//!
//! Part of the `sync` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// Which layers `ledger_purge` may touch, and their timestamp column. The
/// cascade-heavy layers (raw_input/event/notification) are deliberately *not*
/// purgeable in v1 — batch-deleting schedule data has knock-on effects the
/// preview can't honestly summarize in one number.
pub(crate) fn purge_target(layer: MemoryLayer) -> Result<(&'static str, &'static str)> {
    Ok(match layer {
        MemoryLayer::Behavior => ("behavior_log", "ts"),
        MemoryLayer::Suggestion => ("suggestions", "created_at"),
        MemoryLayer::Wearable => ("health_samples", "start"),
        other => {
            return Err(CoreError::Invalid(format!(
                "层 {} 不支持批量清理（请在台账逐条删除）",
                other.as_str()
            )))
        }
    })
}

//! `registry` -- the built-in tool list. Separate from the tools themselves because this is the one place that has to know all of them at once: a tool added here and nowhere else is a tool the orchestrator cannot route to.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::channel::{EventCancelTool, PersonaClearTool, WidgetRecordDeleteTool};
use super::echo::{EchoTool, SimulatedDeleteTool};
use super::ledger::MemoryForgetTool;
use super::previews::{EmailSendTool, SoulousPushEventTool, WidgetDeleteTool};
use super::records::{BackupDeleteTool, DataImportTool, LedgerPurgeTool};
use super::*;

// ---- built-in tools ---------------------------------------------------------

pub(crate) fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(EchoTool),
        Box::new(SimulatedDeleteTool),
        Box::new(LedgerPurgeTool),
        Box::new(WidgetDeleteTool),
        Box::new(DataImportTool),
        Box::new(BackupDeleteTool),
        Box::new(SoulousPushEventTool),
        Box::new(EmailSendTool),
        Box::new(MemoryForgetTool),
        Box::new(PersonaClearTool),
        Box::new(WidgetRecordDeleteTool),
        Box::new(EventCancelTool),
    ]
}

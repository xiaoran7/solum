//! `kind` -- the classification itself, as a type. Every variant carries the one sentence explaining what must be decided before it is chosen — the ordering constraints in the router only read correctly against these.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

/// High-level classification of a user's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Free-form conversation, nothing to schedule.
    Chat,
    /// A request to create a persistent declarative widget. This must be
    /// decided before the event route: “帮我弄一个日程表组件” is not a calendar
    /// entry named “日程表组件”. The definition still requires preview + human
    /// confirmation before it is written.
    CreateWidget,
    /// A schedulable event to ingest.
    IngestEvent,
    /// An answer to a proactive status check-in ("在护肤"/"working").
    StatusAnswer,
    /// "记住我不吃辣" — write a semantic memory fact (§3.10). Rule-matched
    /// only; anything ambiguous stays chat.
    MemoryWrite,
    /// "把明天的会改到下午4点" — move an existing event. Rule-matched only;
    /// must be checked before the schedulable test, or the utterance would
    /// ingest as a brand-new event.
    RescheduleEvent,
    /// "取消明天的会" — cancel an existing event. Routing hint only: the
    /// actual deletion always goes through an explicit confirmation tap.
    CancelEvent,
    /// A request that looks destructive/irreversible — must go through the
    /// guard. This is only a routing hint, not an authorization.
    DangerousCommand,
}

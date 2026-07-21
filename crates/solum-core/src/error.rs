//! Error types for solum-core.

use thiserror::Error;

/// The result type used throughout solum-core.
pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("could not parse a time from input: {0}")]
    TimeParse(String),

    #[error("nothing schedulable was found in the input")]
    NoEvent,

    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// HITL guard rejected an execution attempt. This is a code-level refusal,
    /// not a policy the LLM can talk its way past. See ARCHITECTURE.md §3.3.
    #[error("HITL guard refused execution: {0}")]
    GuardRefused(String),

    #[error("no such record: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    /// Cloud LLM gateway failure (network, auth, or malformed response). The
    /// offline paths must keep working when this occurs (F16).
    #[error("cloud reasoner error: {0}")]
    Llm(String),

    /// Soulous is a user-owned, read-only fact source. Its failure must stay
    /// isolated from the ingest/reminder reliability path (F16).
    #[error("Soulous data source error: {0}")]
    Soulous(String),

    /// Mailbox connector failure. It is isolated from the local ingest and
    /// reminder paths just like every optional external integration.
    #[error("email connector error: {0}")]
    Email(String),
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Storage(e.to_string())
    }
}

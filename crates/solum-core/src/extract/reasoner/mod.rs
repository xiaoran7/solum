//! `reasoner` -- the seam where a cloud model could be layered in over the offline extractor, and the null implementation that keeps it honest: `NullReasoner` fails loudly rather than degrading, so "no reasoner configured" is never mistaken for a model that reasoned and declined.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

/// The cloud reasoning seam (ARCHITECTURE.md §3.6). The gateway
/// ([`crate::llm`]) is responsible for assembling *minimal* context before
/// calling this — the trait itself just performs one completion. `Send`
/// because the orchestrator holding it is shared across threads by the shell.
/// One OpenAI-compatible function call returned by a reasoner. Arguments stay
/// data until the caller validates them against the relevant Solum schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ReasonerToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A completion may contain prose, tool calls, or both.
#[derive(Debug, Clone, PartialEq)]
pub struct ReasonerResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ReasonerToolCall>,
}

pub trait Reasoner: Send {
    fn complete(&self, system: &str, user: &str) -> Result<String>;

    /// OpenAI-compatible tool-calling seam. Offline/test reasoners retain a
    /// text-only default, while network reasoners override this method and
    /// send the supplied JSON Schema tools upstream.
    fn complete_with_tools(
        &self,
        system: &str,
        user: &str,
        _tools: &[serde_json::Value],
    ) -> Result<ReasonerResponse> {
        Ok(ReasonerResponse {
            content: Some(self.complete(system, user)?),
            tool_calls: Vec::new(),
        })
    }

    /// Streaming completion (ARCHITECTURE.md §3.6 第 7 条). Invokes `on_token`
    /// with each visible content delta as it arrives and returns the full
    /// accumulated reply (identical to what [`Self::complete`] would return).
    ///
    /// The default is *non-streaming*: it performs one [`Self::complete`] and
    /// emits the whole reply as a single token. Only the cloud reasoner
    /// overrides this with real SSE — offline/test reasoners keep working
    /// unchanged, and callers that want progress just receive it in one shot.
    fn complete_streaming(
        &self,
        system: &str,
        user: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<String> {
        let full = self.complete(system, user)?;
        on_token(&full);
        Ok(full)
    }
}

/// Offline placeholder reasoner. Present so the type plumbing exists; it never
/// reaches the network, upholding the privacy-first default.
pub struct NullReasoner;

impl Reasoner for NullReasoner {
    fn complete(&self, _system: &str, _user: &str) -> Result<String> {
        Err(crate::error::CoreError::Llm(
            "no cloud reasoner configured (offline mode)".into(),
        ))
    }
}

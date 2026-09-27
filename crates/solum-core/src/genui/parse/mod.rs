//! `parse` -- the F16 degradation seam: parse a cloud reasoner's JSON and return `None` instead of erroring, so a malformed envelope falls back to plain text rather than failing the turn.
//!
//! Part of the `genui` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

// ---- defensive parsing (the F16 degradation seam) ------------------------------

/// Parse a (possibly LLM-authored) envelope. Fences and stray prose are
/// stripped like `llm::parse_event_json`; strict deserialization plus
/// [`validate`] gate the result. Any failure → `None`, caller falls back to
/// plain text — a generated envelope must never be able to fail a conversation.
pub fn parse_envelope(raw: &str, allowed: &[&str]) -> Option<UiEnvelope> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = cleaned.find('{')?;
    let end = cleaned.rfind('}')?;
    let env: UiEnvelope = serde_json::from_str(&cleaned[start..=end]).ok()?;
    validate(&env, allowed).ok()?;
    Some(env)
}

//! Semantic memory facts (ARCHITECTURE.md §3.10) — one-sentence, user-visible
//! facts the agent remembers ("我不吃辣"). Facts are the highest-weight layer
//! the recall pipeline draws on, and a first-class F12 ledger layer: every
//! fact is inspectable, traceable to its source, and deletable.
//!
//! Writes are always user-initiated or user-confirmed (§3.10: unconfirmed
//! automatic memory writes are the same risk class as F15 persona pollution):
//! - `chat`  — the user said "记住…" in conversation (rule-matched intent);
//! - `habit` — accepting a habit suggestion固化 the fact alongside the routine.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// One remembered fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: Option<i64>,
    /// The fact itself, one sentence of user-authored (or user-confirmed) text.
    pub content: String,
    /// Where it came from: "chat" | "habit".
    pub source: String,
    pub created_at: NaiveDateTime,
    /// Last time recall handed this fact to the cloud gateway (for future
    /// relevance tuning; display-only for now).
    pub last_used_at: Option<NaiveDateTime>,
}

/// Extract the fact content from a MemoryWrite utterance: strip the lead-in
/// ("记住/请记住/帮我记住/别忘了") and trailing punctuation. Returns `None`
/// when nothing meaningful remains.
pub fn extract_fact_content(text: &str) -> Option<String> {
    let t = text.trim();
    let mut rest = t;
    // Politeness prefixes first, then the imperative itself.
    for p in ["请", "麻烦", "帮我", "给我"] {
        rest = rest.strip_prefix(p).unwrap_or(rest);
    }
    let mut matched = false;
    for p in ["记住", "记着", "别忘了", "别忘记"] {
        if let Some(r) = rest.strip_prefix(p) {
            rest = r;
            matched = true;
            break;
        }
    }
    if !matched {
        return None;
    }
    let content = rest
        .trim_start_matches(['，', ',', '：', ':', ' '])
        .trim()
        .trim_end_matches(['。', '！', '!', '~'])
        .trim();
    (!content.is_empty()).then(|| content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fact_content() {
        assert_eq!(
            extract_fact_content("记住我不吃辣").as_deref(),
            Some("我不吃辣")
        );
        assert_eq!(
            extract_fact_content("请记住：我对花生过敏。").as_deref(),
            Some("我对花生过敏")
        );
        assert_eq!(
            extract_fact_content("帮我记住 楼下快递柜密码是1234").as_deref(),
            Some("楼下快递柜密码是1234")
        );
        assert_eq!(
            extract_fact_content("别忘了我周五不上班").as_deref(),
            Some("我周五不上班")
        );
        // Not a memory write at all.
        assert_eq!(extract_fact_content("今天天气怎么样"), None);
        // Prefix with nothing behind it.
        assert_eq!(extract_fact_content("记住"), None);
        assert_eq!(extract_fact_content("记住。"), None);
    }
}

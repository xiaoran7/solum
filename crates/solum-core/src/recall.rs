//! Local memory recall v1 (ARCHITECTURE.md §3.10) — the "看着记忆说话" seam.
//!
//! Pure lexical retrieval, deliberately *without* a vector store: the corpus
//! is a few hundred rows at most, so full scan + in-memory scoring beats an
//! index (and FTS5's unicode61 tokenizer is a poor fit for Chinese anyway).
//! Score = character-bigram Dice overlap × recency decay × layer weight
//! (semantic facts > behavior journal > past events).
//!
//! Hard rules (§3.10, enforced here in code, not by prompt discipline):
//! - Selection is entirely local; only the top-k snippets travel upstream.
//! - `MAX_SNIPPETS` / `MAX_TOTAL_CHARS` are hard caps.
//! - Content captured from third-party notifications is **excluded by the
//!   caller** (the orchestrator filters events derived from `[通知·…]` raw
//!   inputs before building the corpus) — "永不出进程" survives recall.
//! - The corpus is read straight from the authoritative store: an F12
//!   deletion removes the row from recall on the very next query.

use std::collections::HashSet;

use chrono::NaiveDateTime;
use serde::Serialize;

/// Hard caps on what may leave the device per query (§3.10).
pub const MAX_SNIPPETS: usize = 5;
pub const MAX_TOTAL_CHARS: usize = 500;

/// Which memory layer a candidate came from — sets its base weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetLayer {
    Fact,
    Behavior,
    Event,
}

impl SnippetLayer {
    fn weight(self) -> f64 {
        match self {
            SnippetLayer::Fact => 3.0,
            SnippetLayer::Behavior => 2.0,
            SnippetLayer::Event => 1.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SnippetLayer::Fact => "记忆",
            SnippetLayer::Behavior => "日志",
            SnippetLayer::Event => "事件",
        }
    }
}

/// One retrieval candidate (built by the orchestrator from store rows).
#[derive(Debug, Clone)]
pub struct Candidate {
    pub layer: SnippetLayer,
    /// Row id in its own table — provenance for `pa recall` display (F12).
    pub id: i64,
    pub content: String,
    pub created_at: NaiveDateTime,
}

/// A selected snippet, scored. `source` is human-readable provenance.
#[derive(Debug, Clone, Serialize)]
pub struct Snippet {
    pub layer: SnippetLayer,
    pub id: i64,
    pub content: String,
    pub score: f64,
}

impl Snippet {
    pub fn source(&self) -> String {
        format!("{}#{}", self.layer.label(), self.id)
    }
}

/// Character bigrams (with unigram fallback for single-char strings), lowercased.
fn bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
        .collect();
    if chars.len() == 1 {
        return HashSet::from([(chars[0], '\0')]);
    }
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Dice coefficient over character bigrams — 0.0 (disjoint) to 1.0 (identical).
fn overlap(a: &HashSet<(char, char)>, b: &HashSet<(char, char)>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    (2 * inter) as f64 / (a.len() + b.len()) as f64
}

/// Recency decay: half weight roughly every two weeks; never fully zero, so an
/// exact-match old fact still beats an unrelated fresh one.
fn recency(created_at: NaiveDateTime, now: NaiveDateTime) -> f64 {
    let days = (now - created_at).num_days().max(0) as f64;
    0.25 + 0.75 / (1.0 + days / 14.0)
}

/// Score the corpus against `query` and return the top snippets within the
/// hard caps. Pure — the caller owns fetching (and filtering) the candidates.
pub fn recall(query: &str, candidates: &[Candidate], now: NaiveDateTime) -> Vec<Snippet> {
    let q = bigrams(query);
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<Snippet> = candidates
        .iter()
        .filter_map(|c| {
            let o = overlap(&q, &bigrams(&c.content));
            if o <= 0.0 {
                return None; // no lexical connection at all → never send it
            }
            Some(Snippet {
                layer: c.layer,
                id: c.id,
                content: c.content.clone(),
                score: o * recency(c.created_at, now) * c.layer.weight(),
            })
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = Vec::new();
    let mut total_chars = 0usize;
    for s in scored {
        let len = s.content.chars().count();
        if out.len() >= MAX_SNIPPETS {
            break;
        }
        // A single verbose top hit must not starve shorter relevant snippets
        // that still fit the hard privacy budget.
        if total_chars + len > MAX_TOTAL_CHARS {
            continue;
        }
        total_chars += len;
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, d)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn cand(layer: SnippetLayer, id: i64, content: &str, d: u32) -> Candidate {
        Candidate {
            layer,
            id,
            content: content.into(),
            created_at: dt(d),
        }
    }

    #[test]
    fn matching_fact_wins_over_unrelated() {
        let corpus = vec![
            cand(SnippetLayer::Fact, 1, "我不吃辣", 1),
            cand(SnippetLayer::Fact, 2, "周五通常休息", 1),
            cand(SnippetLayer::Behavior, 3, "在吃辣火锅", 10),
        ];
        let hits = recall("晚饭想吃辣的吗", &corpus, dt(15));
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, 1, "「吃辣」fact 应排第一，layer 权重压过日志");
        // Totally unrelated fact never leaves the device.
        assert!(hits.iter().all(|s| s.id != 2));
    }

    #[test]
    fn layer_weight_beats_equal_overlap() {
        let corpus = vec![
            cand(SnippetLayer::Event, 1, "去健身房锻炼", 14),
            cand(SnippetLayer::Fact, 2, "去健身房锻炼", 14),
        ];
        let hits = recall("健身房", &corpus, dt(15));
        assert_eq!(hits[0].layer, SnippetLayer::Fact);
    }

    #[test]
    fn recency_breaks_ties() {
        let corpus = vec![
            cand(SnippetLayer::Behavior, 1, "在准备期末考试复习", 1),
            cand(SnippetLayer::Behavior, 2, "在准备期末考试复习", 14),
        ];
        let hits = recall("期末考试", &corpus, dt(15));
        assert_eq!(hits[0].id, 2, "更新的一条应排前");
    }

    #[test]
    fn caps_enforced() {
        let corpus: Vec<Candidate> = (0..20)
            .map(|i| cand(SnippetLayer::Fact, i, "我很喜欢喝咖啡也很喜欢喝茶", 10))
            .collect();
        let hits = recall("咖啡", &corpus, dt(15));
        assert!(hits.len() <= MAX_SNIPPETS);
        let total: usize = hits.iter().map(|s| s.content.chars().count()).sum();
        assert!(total <= MAX_TOTAL_CHARS);
    }

    #[test]
    fn oversized_top_hit_does_not_starve_shorter_matches() {
        let long = "咖啡".repeat(MAX_TOTAL_CHARS / 2 + 1);
        let corpus = vec![
            cand(SnippetLayer::Fact, 1, &long, 10),
            cand(SnippetLayer::Fact, 2, "我喜欢咖啡", 1),
        ];
        let hits = recall("咖啡", &corpus, dt(15));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 2);
    }

    #[test]
    fn empty_query_or_no_overlap_sends_nothing() {
        let corpus = vec![cand(SnippetLayer::Fact, 1, "我不吃辣", 1)];
        assert!(recall("", &corpus, dt(15)).is_empty());
        assert!(recall("weather report", &corpus, dt(15)).is_empty());
    }
}

//! `cancel` -- "cancel that" as prefix or suffix stripping. The empty string means "no description given", which the resolver turns into a candidate list for the user to pick from — the cancel path is the one place where not naming the target is a normal thing to say.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

const CANCEL_PREFIXES: &[&str] = &["取消", "帮我取消", "请取消", "取消掉"];
const CANCEL_SUFFIXES: &[&str] = &["取消了", "取消吧", "不开了", "不去了", "不用去了", "不上了"];

/// Rule-only parse of a cancel utterance; returns the target description
/// ("明天的会"). The empty string means "no description given" — the resolver
/// will list candidates for the user to pick from.
pub fn parse_cancel(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches(['。', '！', '!', '.']);
    let target = CANCEL_PREFIXES
        .iter()
        // Longest prefix first so "取消掉X" doesn't leave a dangling "掉X".
        .max_by_key(|p| if t.starts_with(**p) { p.len() } else { 0 })
        .filter(|p| t.starts_with(**p))
        .map(|p| &t[p.len()..])
        .or_else(|| {
            CANCEL_SUFFIXES
                .iter()
                .find(|s| t.ends_with(**s))
                .map(|s| &t[..t.len() - s.len()])
        })?;
    Some(target.trim().trim_end_matches('的').trim().to_string())
}

//! `clues` -- the extraction itself, kept apart from the shapes it fills: people, place, title, and the body line that looks like the subject. Truncation counts **characters, not bytes** — the bodies are mostly Chinese, and a byte cut lands inside a multi-byte sequence and panics.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

fn first_match(text: &str, re: &Regex, group: usize) -> Option<String> {
    let caps = re.captures(text)?;
    let value = caps.get(group)?.as_str().trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 第一行像样的内容当"事项"。跳过空行和裸 URL。
///
/// 截断按**字符**而不是字节——正文基本都是中文，按字节切会在多字节序列中间
/// 断开，直接 panic。
fn first_matter_line(text: &str) -> Option<String> {
    // 绑在循环外：`lazy_re!` 本身只编译一次（`OnceLock`），但 clippy 的
    // `regex_creation_in_loops` 看不穿宏，会把循环体里的调用报成每轮重编译。
    let bare_url = lazy_re!(r"(?i)^https?://");
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || bare_url.is_match(line) {
            continue;
        }
        let mut chars = line.chars();
        let head: String = chars.by_ref().take(48).collect();
        return Some(if chars.next().is_some() {
            format!("{head}…")
        } else {
            head
        });
    }
    None
}

/// 抽取给人核对的线索。**不是**正式事件解析（那是 [`crate::extract`] 的活）。
pub fn extract_capture_clues(text: &str) -> CaptureClues {
    let normalized = text.trim();
    if normalized.is_empty() {
        return CaptureClues::default();
    }

    let amount = first_match(
        normalized,
        lazy_re!(
            r"(?i)(?:¥|￥|RMB\s*|CNY\s*|人民币\s*)[0-9]+(?:\.[0-9]{1,2})?|[0-9]+(?:\.[0-9]{1,2})?\s*元"
        ),
        0,
    );

    let explicit_location = first_match(
        normalized,
        lazy_re!(
            r"(?:地点|地址|会场|上课地点|取货点|出发地|目的地)\s*[:：]\s*([^\n，,。；;]{2,40})"
        ),
        1,
    );
    let location = match explicit_location {
        Some(v) => Some(v),
        None => first_match(
            normalized,
            lazy_re!(
                r"在\s*([^\n，,。；;]{2,24}?)(?:开会|见面|集合|签到|上课|考试|举办|领取|取货)"
            ),
            1,
        ),
    };

    let time = first_match(
        normalized,
        lazy_re!(concat!(
            r"(?:今天|明天|后天|大后天|(?:本周|这周|下周)[一二三四五六日天]?|",
            r"周[一二三四五六日天]|星期[一二三四五六日天]|",
            r"[0-9]{1,2}月[0-9]{1,2}日|[0-9]{4}[-/.年][0-9]{1,2}[-/.月][0-9]{1,2}日?)",
            r"(?:\s*(?:上午|中午|下午|晚上|凌晨)?\s*[0-9]{1,2}(?::[0-9]{2}|点(?:半|[0-9]{1,2}分)?)?)?"
        )),
        0,
    );

    CaptureClues {
        matter: first_matter_line(normalized),
        time,
        location,
        amount,
    }
}

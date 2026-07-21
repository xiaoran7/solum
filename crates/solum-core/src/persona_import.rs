//! Persona import pipeline (F9, ARCHITECTURE.md §3.4) — strictly local.
//!
//! Parses a chat-log text export, keeps only the *user's own* messages, and
//! distills readable style features (常用语气词/高频短语/句长与标点习惯) into a
//! suggested [`PersonaDraft`]. The other party's messages are only used to
//! tell speakers apart — their content is never analyzed, stored, or sent
//! anywhere. Nothing in this module touches the network or the store: the
//! output is a *draft* the user reviews (and can edit) before it is saved as
//! a persona version with `source = "import"`.
//!
//! Supported export layouts (auto-detected per line):
//! 1. 时间戳头部式（微信/QQ txt 导出）：`2026-07-01 12:30:45 昵称` 单独一行，
//!    后续行是该发言人的消息，直到下一个头部。QQ 的 `昵称(10000)` /
//!    `昵称<mail@x>` 后缀会被剥掉。
//! 2. 行内式：`昵称: 消息` 或 `昵称：消息`，可带 `[12:30]` 之类的前导时间戳。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::persona::PersonaDraft;

/// One attributed message from the chat log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub speaker: String,
    pub text: String,
}

/// What the extractor found, plus the suggested draft. Everything is
/// human-readable (§3.4: 不是黑盒向量) so the user can see exactly where each
/// suggestion came from before confirming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    /// Messages parsed from the whole log (all speakers).
    pub total_messages: usize,
    /// Messages attributed to `me` — the only ones analyzed.
    pub my_messages: usize,
    /// Distinct speakers seen, so the user can spot a wrong `--me` value.
    pub speakers: Vec<String>,
    /// Average characters per own message.
    pub avg_chars: f64,
    /// 语气词 hit counts: (word, number of own messages containing it).
    pub top_particles: Vec<(String, usize)>,
    /// Recurring phrases: (phrase, number of own messages containing it).
    pub top_phrases: Vec<(String, usize)>,
    /// Caveats worth surfacing (e.g. small sample).
    pub notes: Vec<String>,
    /// The persona 初稿 assembled from the stats. User-editable before save.
    pub suggested: PersonaDraft,
}

impl ImportReport {
    /// Render the report for CLI / UI display.
    pub fn render(&self) -> String {
        let mut out = Vec::new();
        out.push(format!(
            "共解析 {} 条消息，其中本人 {} 条（发言人：{}）",
            self.total_messages,
            self.my_messages,
            self.speakers.join("、")
        ));
        out.push(format!("平均每条 {:.1} 字", self.avg_chars));
        if !self.top_particles.is_empty() {
            let items: Vec<String> = self
                .top_particles
                .iter()
                .map(|(w, n)| format!("{w}×{n}"))
                .collect();
            out.push(format!("常用语气词：{}", items.join("、")));
        }
        if !self.top_phrases.is_empty() {
            let items: Vec<String> = self
                .top_phrases
                .iter()
                .map(|(w, n)| format!("{w}×{n}"))
                .collect();
            out.push(format!("高频短语：{}", items.join("、")));
        }
        out.push(format!("推断语气：{}", self.suggested.tone));
        if !self.suggested.catchphrases.is_empty() {
            out.push(format!(
                "建议口头禅：{}",
                self.suggested.catchphrases.join("、")
            ));
        }
        for n in &self.notes {
            out.push(format!("⚠ {n}"));
        }
        out.join("\n")
    }
}

// ---- parsing ----------------------------------------------------------------

/// `2026-07-01 12:30:45 昵称` — header line that switches the current speaker.
static HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}[-/.]\d{1,2}[-/.]\d{1,2}[ \t]+\d{1,2}:\d{2}(?::\d{2})?[ \t]+(.+)$")
        .expect("static regex")
});

/// Leading `[12:30]` / `[2026-07-01 12:30]` timestamp on an inline line.
static LEAD_TS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[[\d\-/.: ]{4,20}\][ \t]*").expect("static regex"));

/// QQ-style speaker suffixes: `昵称(10000)` / `昵称<mail@example.com>`.
static SPEAKER_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[（(][^()（）]*[)）]$|<[^<>]*>$").expect("static regex"));

/// Media/system placeholder tokens that carry no style signal.
static PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(图片|表情|语音|视频|文件|链接|红包|转账|动画表情|合并转发|位置|名片|Photo|Sticker|Voice|Video|File)\]")
        .expect("static regex")
});

/// Lines that are chat-app boilerplate, not speech.
fn is_system_line(line: &str) -> bool {
    line.contains("撤回了一条消息")
        || line.contains("以上是打招呼的内容")
        || line.contains("你已添加了")
        || line.contains("现在可以开始聊天了")
        || line.contains("请使用最新版本手机QQ查看")
}

fn normalize_speaker(raw: &str) -> String {
    SPEAKER_SUFFIX_RE.replace(raw.trim(), "").trim().to_string()
}

/// Max chars a speaker name may have in the inline `昵称: 消息` form — longer
/// prefixes are almost certainly message text containing a colon.
const MAX_INLINE_SPEAKER_CHARS: usize = 20;

/// Parse a chat export into attributed messages. Unattributable lines are
/// dropped rather than guessed at.
pub fn parse_chat_log(raw: &str) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    let mut current_speaker: Option<String> = None;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || is_system_line(line) {
            continue;
        }

        // 1) timestamp header → switch speaker, no message on this line.
        if let Some(cap) = HEADER_RE.captures(line) {
            current_speaker = Some(normalize_speaker(&cap[1]));
            continue;
        }

        // 2) inline `昵称: 消息` (optionally after a `[..]` timestamp).
        let stripped = LEAD_TS_RE.replace(line, "");
        if let Some((name, msg)) = split_inline(&stripped) {
            let speaker = normalize_speaker(name);
            if !speaker.is_empty() {
                let text = clean_text(msg);
                if text.is_empty() {
                    // `昵称：` with nothing after it still switches the speaker.
                    current_speaker = Some(speaker);
                } else {
                    current_speaker = Some(speaker.clone());
                    out.push(ChatMessage { speaker, text });
                }
                continue;
            }
        }

        // 3) continuation line under the current header speaker.
        if let Some(speaker) = &current_speaker {
            let text = clean_text(line);
            if !text.is_empty() {
                out.push(ChatMessage {
                    speaker: speaker.clone(),
                    text,
                });
            }
        }
    }
    out
}

/// Split `昵称: 消息` / `昵称：消息`; `None` when the prefix doesn't look like
/// a name (too long, or no separator).
fn split_inline(line: &str) -> Option<(&str, &str)> {
    let idx = line.find([':', '：'])?;
    let (name, rest) = line.split_at(idx);
    let msg = rest.trim_start_matches([':', '：']);
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_INLINE_SPEAKER_CHARS {
        return None;
    }
    // URLs (`https://…`) and clock times (`12:30`) are not speakers.
    if name.chars().all(|c| c.is_ascii_digit()) || msg.starts_with("//") {
        return None;
    }
    Some((name, msg))
}

fn clean_text(s: &str) -> String {
    PLACEHOLDER_RE.replace_all(s, "").trim().to_string()
}

// ---- extraction ---------------------------------------------------------------

/// 语气词/网络用语 vocabulary the extractor looks for. Substring match per
/// message; repeated-char laughs are normalized first (哈哈哈哈 → 哈哈).
const PARTICLES: &[&str] = &[
    "哈哈",
    "嘿嘿",
    "呵呵",
    "嘻嘻",
    "呜呜",
    "唉",
    "哎",
    "嗯",
    "哦",
    "噢",
    "咦",
    "诶",
    "欸",
    "呀",
    "啦",
    "嘛",
    "捏",
    "叭",
    "咯",
    "哇",
    "卧槽",
    "我去",
    "好家伙",
    "笑死",
    "绝了",
    "离谱",
    "无语",
    "麻了",
    "寄",
    "摆烂",
    "泪目",
    "蚌埠住",
    "属于是",
    "666",
    "233",
    "hhh",
    "emm",
    "yyds",
    "xswl",
    "awsl",
];

/// Structural characters that alone can't be a catchphrase.
const STOP_CHARS: &str = "的了是我你他她它在有和就不都一个这那些么什吗吧呢啊呀也很还没到去来说要会能对好多少上下里外中为与及或但而被把从给让";

/// Run the local extraction over a chat export. `me` is the display name the
/// user goes by in this log (suffixes like `(QQ号)` are ignored on both sides).
pub fn extract_persona(raw: &str, me: &str) -> Result<ImportReport> {
    let me = normalize_speaker(me);
    if me.is_empty() {
        return Err(CoreError::Invalid("请提供你在聊天记录里的昵称".into()));
    }
    let messages = parse_chat_log(raw);
    if messages.is_empty() {
        return Err(CoreError::Invalid(
            "无法从文件中识别出聊天消息。支持的格式：微信/QQ txt 导出（时间戳一行 + 消息一行），或每行「昵称: 消息」".into(),
        ));
    }

    let mut speakers: Vec<String> = Vec::new();
    for m in &messages {
        if !speakers.contains(&m.speaker) {
            speakers.push(m.speaker.clone());
        }
    }

    let mine: Vec<&ChatMessage> = messages.iter().filter(|m| m.speaker == me).collect();
    if mine.is_empty() {
        return Err(CoreError::Invalid(format!(
            "没有找到「{me}」的发言。这份记录里的发言人有：{}",
            speakers.join("、")
        )));
    }

    let n = mine.len();
    let total_chars: usize = mine.iter().map(|m| m.text.chars().count()).sum();
    let avg_chars = total_chars as f64 / n as f64;

    // Normalize repeated laughs so 哈哈哈哈 counts as 哈哈, not 哈哈+哈哈.
    static LAUGH_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(哈|嘿|呵|嘻|呜|h){3,}").expect("static regex"));
    let normalized: Vec<String> = mine
        .iter()
        .map(|m| {
            LAUGH_RE
                .replace_all(&m.text, |c: &regex::Captures| c[1].repeat(2))
                .into_owned()
        })
        .collect();

    // Particles: count messages containing each.
    let min_hits = (n / 25).max(3).min(n); // scale with sample, floor 3
    let mut particles: Vec<(String, usize)> = PARTICLES
        .iter()
        .map(|p| {
            (
                p.to_string(),
                normalized.iter().filter(|t| t.contains(p)).count(),
            )
        })
        .filter(|(_, c)| *c >= min_hits)
        .collect();
    particles.sort_by_key(|p| std::cmp::Reverse(p.1));
    particles.truncate(5);

    // Phrases: CJK n-grams (2..=4 chars) + latin tokens, counted per message.
    let phrases = top_phrases(&normalized, &particles, min_hits);

    // Punctuation / emoji habits → tone description.
    let ends_exclaim = ratio(&normalized, |t| t.ends_with(['！', '!']));
    let ends_question = ratio(&normalized, |t| t.ends_with(['？', '?']));
    let ends_bare = ratio(&normalized, |t| {
        t.chars()
            .last()
            .is_some_and(|c| !c.is_ascii_punctuation() && !"。！？…～”)】".contains(c))
    });
    let has_emoji = ratio(&normalized, |t| t.chars().any(is_emoji));
    let particle_msgs = ratio(&normalized, |t| PARTICLES.iter().any(|p| t.contains(p)));

    let mut tone_parts: Vec<&str> = Vec::new();
    if avg_chars < 10.0 {
        tone_parts.push("消息偏短、说话直接");
    } else if avg_chars > 30.0 {
        tone_parts.push("喜欢成段表达、内容详细");
    }
    if particle_msgs > 0.3 {
        tone_parts.push("爱带语气词，口吻轻松");
    }
    if ends_exclaim > 0.15 {
        tone_parts.push("感叹号用得多，情绪外放");
    }
    if ends_question > 0.2 {
        tone_parts.push("问句多，喜欢确认和追问");
    }
    if has_emoji > 0.2 {
        tone_parts.push("常用表情符号");
    }
    if ends_bare > 0.6 {
        tone_parts.push("句尾常不加标点");
    }
    if tone_parts.is_empty() {
        tone_parts.push("平实自然");
    }
    let tone = tone_parts.join("；");

    let mut catchphrases: Vec<String> = particles.iter().map(|(w, _)| w.clone()).collect();
    for (w, _) in &phrases {
        if !catchphrases.contains(w) {
            catchphrases.push(w.clone());
        }
    }
    catchphrases.truncate(6);

    let mut notes = Vec::new();
    if n < 20 {
        notes.push(format!("本人消息只有 {n} 条，样本偏少，提取结果仅供参考"));
    }

    let suggested = PersonaDraft {
        nickname: None, // 「怎么称呼用户」不是聊天记录能推断的，留给用户填
        tone,
        catchphrases,
        style_notes: Some(format!(
            "由聊天记录本地提取（分析本人消息 {n} 条；对方内容未参与统计，原始记录不入库）"
        )),
    };

    Ok(ImportReport {
        total_messages: messages.len(),
        my_messages: n,
        speakers,
        avg_chars,
        top_particles: particles,
        top_phrases: phrases,
        notes,
        suggested,
    })
}

fn ratio(msgs: &[String], pred: impl Fn(&str) -> bool) -> f64 {
    msgs.iter().filter(|t| pred(t)).count() as f64 / msgs.len().max(1) as f64
}

fn is_emoji(c: char) -> bool {
    matches!(u32::from(c), 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF)
}

fn is_cjk(c: char) -> bool {
    matches!(u32::from(c), 0x4E00..=0x9FFF)
}

/// Recurring phrases: CJK n-grams (2..=4) and latin tokens that appear in at
/// least `min_hits` distinct messages. Longer n-grams swallow their
/// substrings when nearly as frequent, so 「安排上」 wins over 「安排」.
fn top_phrases(
    msgs: &[String],
    particles: &[(String, usize)],
    min_hits: usize,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for msg in msgs {
        let mut seen: Vec<String> = Vec::new();
        // CJK runs → n-grams. 3..=4 only: 2-char grams are dominated by
        // word-boundary fragments (了哈/事了…), and real 2-char habits are
        // already in the PARTICLES vocabulary.
        for run in msg.split(|c: char| !is_cjk(c)).filter(|r| !r.is_empty()) {
            let chars: Vec<char> = run.chars().collect();
            for len in 3..=4usize {
                if chars.len() < len {
                    break;
                }
                for w in chars.windows(len) {
                    let g: String = w.iter().collect();
                    if !seen.contains(&g) {
                        seen.push(g);
                    }
                }
            }
        }
        // latin tokens
        for tok in msg
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() >= 2 && t.len() <= 12 && !t.chars().all(|c| c.is_ascii_digit()))
        {
            let t = tok.to_lowercase();
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        for g in seen {
            *counts.entry(g).or_insert(0) += 1;
        }
    }

    let mut cands: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(g, c)| {
            *c >= min_hits
                && !g.chars().all(|ch| STOP_CHARS.contains(ch))
                && !particles
                    .iter()
                    .any(|(p, _)| p.contains(g.as_str()) || g.contains(p.as_str()))
        })
        .collect();
    // Highest count first; ties prefer clean word edges (a gram starting or
    // ending on a structural particle / laugh char is a boundary fragment,
    // 了稳了哈 vs 稳了稳了), then longer, then lexicographic for determinism.
    // Greedy pick suppresses candidates sharing a 2-char fragment with an
    // already picked phrase — overlapping windows of the same utterance
    // would otherwise flood the top-5 with one habit.
    // Only the *leading* char is penalized: Chinese habits often end on a
    // particle (完事了/绝了), but starting on one (了稳了哈) means the window
    // slid past a word boundary.
    let edge_penalty = |s: &str| -> usize {
        s.chars()
            .next()
            .is_some_and(|c| STOP_CHARS.contains(c) || "哈嘿呵嘻呜".contains(c)) as usize
    };
    cands.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| edge_penalty(&a.0).cmp(&edge_penalty(&b.0)))
            .then_with(|| b.0.chars().count().cmp(&a.0.chars().count()))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut picked: Vec<(String, usize)> = Vec::new();
    for (g, c) in cands {
        if picked.len() >= 5 {
            break;
        }
        let shadowed = picked.iter().any(|(p, _)| shares_bigram(p, &g));
        if !shadowed {
            picked.push((g, c));
        }
    }
    picked
}

/// True when any 2-char CJK window of `a` also occurs in `b` (either
/// direction), or one latin token contains the other.
fn shares_bigram(a: &str, b: &str) -> bool {
    let bigrams = |s: &str| -> Vec<String> {
        let chars: Vec<char> = s.chars().filter(|c| is_cjk(*c)).collect();
        chars.windows(2).map(|w| w.iter().collect()).collect()
    };
    let ab = bigrams(a);
    if ab.is_empty() {
        // latin vs latin: containment either way
        return a.contains(b) || b.contains(a);
    }
    bigrams(b).iter().any(|g| ab.contains(g))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wechat_header_format() {
        let log = "\
2026-07-01 12:30:45 小王
今天考试改到周五了
2026-07-01 12:31:02 我自己
好的收到
稳了稳了
2026-07-01 12:32:00 小王(wxid_abc)
[图片]
";
        let msgs = parse_chat_log(log);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].speaker, "小王");
        assert_eq!(
            msgs[1],
            ChatMessage {
                speaker: "我自己".into(),
                text: "好的收到".into()
            }
        );
        assert_eq!(msgs[2].text, "稳了稳了");
        // 第三条头部只有 [图片]，清洗后为空，被丢弃；后缀 (wxid) 被剥掉
    }

    #[test]
    fn parses_inline_format_with_timestamps() {
        let log = "\
[12:30] 小王: 明天开会吗
[12:31] 我: 开的，安排上了
我: 顺便把材料带上
";
        let msgs = parse_chat_log(log);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].speaker, "我");
        assert_eq!(msgs[2].text, "顺便把材料带上");
    }

    #[test]
    fn skips_system_lines_and_urls() {
        let log = "\
我: 看这个 https://example.com/x
小王撤回了一条消息
我: 哈哈哈哈好
";
        let msgs = parse_chat_log(log);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].text.contains("https"));
        // URL 的 `https://…` 不会被误认成「发言人 https」
        assert_eq!(msgs[0].speaker, "我");
    }

    fn fixture_log() -> String {
        let mut lines = Vec::new();
        for i in 0..30 {
            lines.push(format!("小王: 第{i}条消息问一下"));
            lines.push(format!("我: 稳了稳了哈哈哈 冲就完事了{i}"));
        }
        lines.push("我: 今晚吃啥？".into());
        lines.join("\n")
    }

    #[test]
    fn extracts_particles_phrases_and_tone() {
        let r = extract_persona(&fixture_log(), "我").unwrap();
        assert_eq!(r.my_messages, 31);
        assert_eq!(r.speakers, vec!["小王".to_string(), "我".to_string()]);
        assert!(
            r.top_particles.iter().any(|(w, _)| w == "哈哈"),
            "{:?}",
            r.top_particles
        );
        assert!(
            r.top_phrases.iter().any(|(w, _)| w.contains("稳了")),
            "{:?}",
            r.top_phrases
        );
        assert!(!r.suggested.tone.is_empty());
        assert!(r
            .suggested
            .style_notes
            .as_deref()
            .unwrap()
            .contains("31 条"));
        // 初稿必须能通过 normalized 校验（可直接保存）
        assert!(r.suggested.clone().normalized().is_ok());
        assert!(!r.render().is_empty());
    }

    #[test]
    fn errors_are_actionable() {
        assert!(matches!(
            extract_persona("完全不是聊天记录的一段话", "我"),
            Err(CoreError::Invalid(_))
        ));
        let err = extract_persona("小王: 你好\n小李: 好的", "我").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("小王") && msg.contains("小李"), "{msg}");
        assert!(extract_persona("我: hi", " ").is_err());
    }

    #[test]
    fn small_sample_gets_a_note() {
        let log =
            "我: 哈哈哈今天不错\n我: 嗯嗯好的哈哈\n我: 哈哈稳了\n我: 嗯这个可以哈哈\n我: 走起哈哈";
        let r = extract_persona(log, "我").unwrap();
        assert!(!r.notes.is_empty());
    }
}

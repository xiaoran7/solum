//! `extractor` -- the offline extractor that has to keep working when the cloud is down (F16): decide the kind, then pull location, people and title out of the same sentence, and hand back an `Event` the rest of the system can validate. The impl block stays whole — splitting it would separate the type from the trait it implements.
//!
//! Part of the `extract` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::eventwords::is_schedulable;
use super::*;

/// Turns an utterance into a structured event, if it is schedulable.
pub trait Extractor {
    fn extract(&self, text: &str, now: NaiveDateTime) -> Result<Option<Event>>;
}

/// Fully-offline, deterministic extractor. This is the reliability floor: it
/// keeps working when the cloud is down (F16).
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedExtractor;

impl RuleBasedExtractor {
    pub fn new() -> Self {
        RuleBasedExtractor
    }

    /// Detect the event kind from keywords. Order matters: more specific first.
    pub fn detect_kind(text: &str) -> EventKind {
        Self::detect_kind_with(text, &[])
    }

    /// Kind detection, with the user's own categories considered *after* the
    /// six built-ins. A custom keyword must never steal a classification the
    /// built-ins are sure about ("考试" is an exam even if someone made a
    /// category called "复习"), so the ordering is part of the contract and
    /// must match the Harmony `detectKind`.
    pub fn detect_kind_with(text: &str, custom_kinds: &[CustomKind]) -> EventKind {
        let lower = text.to_lowercase();
        let has = |ws: &[&str]| ws.iter().any(|w| lower.contains(*w));
        if has(&["考试", "期末", "期中", "exam", "test", "quiz"]) {
            EventKind::Exam
        } else if has(&["截止", "deadline", "ddl", "交", "提交", "submit", "due"]) {
            EventKind::Deadline
        } else if has(&[
            "开会",
            "会议",
            "晨会",
            "例会",
            "周会",
            "meeting",
            "面试",
            "interview",
            "电话会",
            "评审",
        ]) {
            EventKind::Meeting
        } else if has(&["上课", "课程", "class", "lecture", "讲座"]) {
            EventKind::Class
        } else if has(&["提醒", "remind", "记得", "别忘", "don't forget"]) {
            EventKind::Reminder
        } else {
            for custom in custom_kinds {
                if custom
                    .keywords
                    .iter()
                    .any(|k| !k.is_empty() && lower.contains(&k.to_lowercase()))
                {
                    return custom.kind();
                }
            }
            EventKind::Other
        }
    }

    fn extract_location(text: &str) -> Option<String> {
        // "在<place>" — a run of Han characters (minus connective/verb chars
        // that would signal the place name has ended) or an ASCII token.
        let re = lazy_re!(
            r"[在@]\s*([\p{Han}&&[^和跟与同的开吃上见聊休去到做交完要有]]{1,8}|[A-Za-z][A-Za-z0-9]{0,19})"
        );
        for caps in re.captures_iter(text) {
            let m0 = caps.get(0)?;
            // Skip "现在/正在/还在" — those aren't locations.
            if let Some(prev) = text[..m0.start()].chars().last() {
                if "现正还也都刚".contains(prev) {
                    continue;
                }
            }
            let loc = caps.get(1)?.as_str().trim();
            let timey = [
                "点", "上午", "下午", "中午", "晚上", "早上", "凌晨", "周", "星期", "礼拜",
            ];
            if loc.is_empty() || timey.iter().any(|t| loc.contains(t)) {
                continue;
            }
            return Some(loc.to_string());
        }
        None
    }

    fn extract_people(text: &str) -> Vec<String> {
        // Names after a connective, stopping before common verbs/particles.
        let re = lazy_re!(
            r"(?:和|跟|与|同)([\p{Han}&&[^开吃上见聊休去到做交完要有的和跟与同一起吧了呢在去]]{1,4})"
        );
        let re_en = lazy_re!(r"(?i)with\s+([A-Z][a-z]+)");
        let mut out = Vec::new();
        for caps in re.captures_iter(text).chain(re_en.captures_iter(text)) {
            if let Some(m) = caps.get(1) {
                let name = m.as_str().trim();
                if !name.is_empty() && !out.iter().any(|n| n == name) {
                    out.push(name.to_string());
                }
            }
        }
        out
    }

    /// Best-effort title: strip the recognized time expression, the extracted
    /// location/people spans (kept in their own fields), and common filler, so
    /// what's left describes the action itself.
    fn clean_title(
        text: &str,
        kind: &EventKind,
        location: Option<&str>,
        people: &[String],
    ) -> String {
        let mut s = text.to_string();
        // Remove location and people fragments we already captured elsewhere.
        if let Some(loc) = location {
            for prefix in ["在", "@"] {
                s = s.replace(&format!("{prefix}{loc}"), " ");
            }
            s = s.replace(loc, " ");
        }
        for p in people {
            for conn in ["和", "跟", "与", "同"] {
                s = s.replace(&format!("{conn}{p}"), " ");
            }
            s = s.replace(p, " ");
        }
        let strips = [
            r"(?:大后天|后天|明天|明日|明晚|明早|今天|今日|今晚|今早|昨天|昨日)",
            r"(?:每天|每日|每早|每晚)",
            r"(?:上午|下午|中午|晚上|早上|早晨|凌晨|傍晚|正午|深夜)",
            r"(?:这|本|下)?(?:周|星期|礼拜)[一二三四五六日天]",
            r"(?:[0-9零一二两三四五六七八九十]+)\s*天\s*(?:后|之后)",
            r"(?:[0-9零一二两三四五六七八九十]+)\s*(?:个|個)?\s*小时\s*(?:后|之后)?",
            r"(?:[0-9零一二两三四五六七八九十]+)\s*分钟\s*(?:后|之后)?",
            r"(?:[0-9零一二两三四五六七八九十]+)\s*月\s*(?:[0-9零一二两三四五六七八九十]+)?\s*(?:号|日)?",
            r"(?:[0-9零一二两三四五六七八九十]+)\s*(?:号|日)",
            r"[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}",
            r"(?:[0-9零一二两三四五六七八九十]+)\s*点(?:半|(?:[0-9零一二两三四五六七八九十]+)\s*分?)?",
            r"\b\d{1,2}:\d{2}\b",
            r"(?i)(?:at\s+)?\d{1,2}(?::\d{2})?\s*(?:am|pm)",
            r"(?i)\b(?:today|tomorrow|tonight|noon|midnight)\b",
            r"(?i)\b(?:next|this)\s+(?:week|month|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
            r"(?i)\bin\s+\d+\s+(?:hours?|minutes?|days?|weeks?)\b",
            r"(?i)\b(?:monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
        ];
        for pat in strips {
            if let Ok(re) = Regex::new(pat) {
                s = re.replace_all(&s, " ").into_owned();
            }
        }
        // Time/frequency stripping can leave leading whitespace before the
        // original filler ("每天 8 点提醒我吃药"). Trim first, but only remove
        // a filler at the title's start so normal wording such as "请假" is
        // never damaged in the middle of an action title.
        s = lazy_re!(
            r"^(?:提醒我|提醒|记一下|记得|帮我记|帮我|请|remind me to|remind me|please)\s*"
        )
        .replace_all(s.trim(), " ")
        .into_owned();
        let s = s
            .trim()
            .trim_matches(|c: char| c.is_whitespace() || "，,。.!！?？、:：;；的了".contains(c))
            .trim()
            .to_string();
        if s.is_empty() {
            default_title(kind.clone())
        } else {
            s
        }
    }
}

fn default_title(kind: EventKind) -> String {
    match kind {
        EventKind::Exam => "考试",
        EventKind::Meeting => "会议",
        EventKind::Class => "上课",
        EventKind::Deadline => "截止",
        EventKind::Reminder => "提醒",
        EventKind::Other => "事件",
        // A user category's name lives in the rule table, not here; the slug
        // is the best label available without it.
        EventKind::Custom(slug) => return slug.clone(),
    }
    .to_string()
}

impl Extractor for RuleBasedExtractor {
    fn extract(&self, text: &str, now: NaiveDateTime) -> Result<Option<Event>> {
        let Some(parsed) = parse_datetime(text, now) else {
            return Ok(None);
        };
        // A bare date word with no explicit time and no event keyword is chat.
        if !is_schedulable(text, now) {
            return Ok(None);
        }
        let kind = Self::detect_kind(text);
        let location = Self::extract_location(text);
        let people = Self::extract_people(text);
        let title = Self::clean_title(text, &kind, location.as_deref(), &people);
        let mut ev = Event::new(title, kind, parsed.start, text, now);
        ev.location = location;
        ev.people = people;
        Ok(Some(ev))
    }
}

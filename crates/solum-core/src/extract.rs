//! Intent routing and natural-language → structured event extraction.
//!
//! Two layers:
//! - [`route_intent`] — a fast, offline classifier that decides what kind of
//!   input we're dealing with (chat / event / status answer / dangerous
//!   command). It never executes anything; dangerous routing is only a hint,
//!   the real enforcement is the HITL guard (see [`crate::guard`]).
//! - [`Extractor`] — turns an event-shaped utterance into a structured
//!   [`Event`]. The default [`RuleBasedExtractor`] works entirely offline; a
//!   cloud [`Reasoner`] can be layered on later behind the same trait.

use std::sync::OnceLock;

use chrono::NaiveDateTime;
use regex::Regex;

use crate::error::Result;
use crate::model::{Event, EventKind};
use crate::time_parse::parse_datetime;

macro_rules! lazy_re {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex compiles"))
    }};
}

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

const DANGER_WORDS: &[&str] = &[
    "删除",
    "删掉",
    "删了",
    "格式化",
    "转账",
    "打款",
    "支付",
    "付款",
    "清空",
    // D3 的对话入口示例就是「把上周的行为日志清掉」——2026-07-18 走查发现
    // 该措辞落进闲聊，云端答非所问；同义的「清理」误报面太大（清理房间），
    // 不收。
    "清掉",
    "清除",
    "抹掉",
    "delete",
    "format",
    "wipe",
    "transfer",
    "pay ",
    "erase",
    "rm -rf",
];

// Keep this aligned with the keywords `RuleBasedExtractor::detect_kind` knows:
// an utterance that would classify to a real kind should also route as an event
// even when it carries only a date (no explicit clock time).
const EVENT_WORDS: &[&str] = &[
    "考试",
    "开会",
    "会议",
    "晨会",
    "例会",
    "周会",
    "上课",
    "课",
    "截止",
    "提醒",
    "约",
    "面试",
    "预约",
    "报告",
    "作业",
    "交",
    "提交",
    "讲座",
    "电话会",
    "评审",
    "期末",
    "期中",
    "deadline",
    "ddl",
    "due",
    "exam",
    "test",
    "quiz",
    "meeting",
    "interview",
    "class",
    "lecture",
    "remind",
    "appointment",
    "submit",
    "report",
    "task",
];

const STATUS_PREFIXES: &[&str] = &["我在", "正在", "在做", "刚在", "i'm ", "im ", "currently"];

const WIDGET_NOUNS: &[&str] = &["组件", "小组件", "widget"];
const WIDGET_CREATE_WORDS: &[&str] = &[
    "创建",
    "新建",
    "做个",
    "做一个",
    "弄个",
    "弄一个",
    "帮我做",
    "帮我弄",
    "create",
    "make",
];

/// A high-confidence offline route for widget creation. The cloud classifier
/// can additionally handle ambiguous chat-shaped requests, but an explicit
/// “组件” request must never fall through to event ingestion when offline.
pub fn requests_widget(text: &str) -> bool {
    let lower = text.to_lowercase();
    WIDGET_NOUNS.iter().any(|noun| lower.contains(noun))
        && WIDGET_CREATE_WORDS.iter().any(|verb| lower.contains(verb))
}

/// Ambiguous phrasing eligible for the opt-in cloud router. This deliberately
/// excludes schedule/event vocabulary: the LLM can assist a user-authored
/// widget request, but never gets to reinterpret ordinary F1 ingestion.
pub fn might_request_widget(text: &str, now: NaiveDateTime) -> bool {
    let lower = text.to_lowercase();
    let creation = WIDGET_CREATE_WORDS.iter().any(|verb| lower.contains(verb));
    let shape = ["表", "清单", "追踪", "记录器", "收支", "tracker", "list"]
        .iter()
        .any(|hint| lower.contains(hint));
    creation && shape && !is_schedulable(text, now)
}

fn has_event_word(lower: &str) -> bool {
    EVENT_WORDS.iter().any(|w| lower.contains(*w))
}

/// Whether an utterance is worth turning into a scheduled event: it must carry
/// an explicit clock time *or* an event keyword. A bare date word ("今天") on
/// its own is chat, not a calendar entry.
fn is_schedulable(text: &str, now: NaiveDateTime) -> bool {
    let explicit_time = parse_datetime(text, now)
        .map(|p| p.explicit_time)
        .unwrap_or(false);
    explicit_time || has_event_word(&text.to_lowercase())
}

/// A parsed "move this event" request. `new_date` / `new_time` are the halves
/// the phrase actually specified; whichever is absent keeps the target event's
/// original value (that resolution needs the event, so it lives in the
/// orchestrator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescheduleRequest {
    /// What the user called the event ("明天的会", "期末考试").
    pub target: String,
    pub new_date: Option<chrono::NaiveDate>,
    pub new_time: Option<(u32, u32)>,
}

const RESCHEDULE_MARKERS: &[&str] = &[
    "改到",
    "改成",
    "改在",
    "推迟到",
    "延到",
    "延期到",
    "提前到",
    "挪到",
    "换到",
    "调到",
];

/// Rule-only parse of a reschedule utterance: `<target> <marker> <new time>`.
/// Returns `None` unless the right-hand side resolves to at least a date or a
/// clock time — "把会改一下" stays chat.
pub fn parse_reschedule(text: &str, now: NaiveDateTime) -> Option<RescheduleRequest> {
    for marker in RESCHEDULE_MARKERS {
        let Some(pos) = text.find(marker) else {
            continue;
        };
        let (new_date, new_time) =
            crate::time_parse::parse_date_time_parts(&text[pos + marker.len()..], now);
        if new_date.is_none() && new_time.is_none() {
            continue;
        }
        let target = text[..pos]
            .trim()
            .trim_start_matches(['把', '将', '帮', '我', '请'])
            .trim_end_matches('的')
            .trim()
            .to_string();
        return Some(RescheduleRequest {
            target,
            new_date,
            new_time,
        });
    }
    None
}

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

/// Classify an utterance. `now` is used so the router can tell whether the text
/// actually contains a resolvable time.
pub fn route_intent(text: &str, now: NaiveDateTime) -> Intent {
    let lower = text.to_lowercase();
    if DANGER_WORDS.iter().any(|w| lower.contains(*w)) {
        return Intent::DangerousCommand;
    }
    // Before the schedulable test: "把明天的会改到4点" carries both an event
    // word and a time, and must not ingest as a new event.
    if parse_reschedule(text, now).is_some() {
        return Intent::RescheduleEvent;
    }
    if parse_cancel(text).is_some() {
        return Intent::CancelEvent;
    }
    if requests_widget(text) {
        return Intent::CreateWidget;
    }
    if is_schedulable(text, now) {
        return Intent::IngestEvent;
    }
    // After the schedulable check on purpose: "记住明天3点开会" is an event
    // ("记住" doubles as a reminder filler), while "记住我不吃辣" is a fact.
    if crate::memory::extract_fact_content(text).is_some() {
        return Intent::MemoryWrite;
    }
    if STATUS_PREFIXES.iter().any(|p| lower.starts_with(*p)) {
        return Intent::StatusAnswer;
    }
    Intent::Chat
}

/// The cloud reasoning seam (ARCHITECTURE.md §3.6). The gateway
/// ([`crate::llm`]) is responsible for assembling *minimal* context before
/// calling this — the trait itself just performs one completion. `Send`
/// because the orchestrator holding it is shared across threads by the shell.
pub trait Reasoner: Send {
    fn complete(&self, system: &str, user: &str) -> Result<String>;

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
        kind: EventKind,
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
            default_title(kind)
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
        let title = Self::clean_title(text, kind, location.as_deref(), &people);
        let mut ev = Event::new(title, kind, parsed.start, text, now);
        ev.location = location;
        ev.people = people;
        Ok(Some(ev))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, 6)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    #[test]
    fn routes_dangerous() {
        assert_eq!(
            route_intent("帮我删除今天的所有文件", now()),
            Intent::DangerousCommand
        );
        assert_eq!(
            route_intent("transfer 500 to Alice", now()),
            Intent::DangerousCommand
        );
        // ARCHITECTURE §6 D3 的对话入口示例措辞必须能入高危路由。
        assert_eq!(
            route_intent("把上周的行为日志清掉", now()),
            Intent::DangerousCommand
        );
    }

    #[test]
    fn routes_event() {
        assert_eq!(route_intent("明天下午3点开会", now()), Intent::IngestEvent);
        assert_eq!(route_intent("下周五要考试", now()), Intent::IngestEvent);
    }

    #[test]
    fn routes_explicit_widget_before_event_ingest() {
        assert_eq!(
            route_intent("帮我弄一个日程表组件", now()),
            Intent::CreateWidget
        );
    }

    #[test]
    fn routes_chat() {
        assert_eq!(route_intent("今天天气怎么样", now()), Intent::Chat);
    }

    #[test]
    fn routes_reschedule_before_event_ingest() {
        // Carries both an event word and a time — must NOT become a new event.
        assert_eq!(
            route_intent("把明天的会改到下午4点", now()),
            Intent::RescheduleEvent
        );
        assert_eq!(
            route_intent("期末考试推迟到下周五", now()),
            Intent::RescheduleEvent
        );
        // A marker without a resolvable new time stays chat.
        assert_eq!(route_intent("把会改一下吧", now()), Intent::Chat);
    }

    #[test]
    fn routes_cancel() {
        assert_eq!(route_intent("取消明天的会", now()), Intent::CancelEvent);
        assert_eq!(route_intent("明天的会不开了", now()), Intent::CancelEvent);
        // Danger words still win: "删掉" routes to the guard, not to cancel.
        assert_eq!(
            route_intent("把明天的会删掉", now()),
            Intent::DangerousCommand
        );
    }

    #[test]
    fn parse_reschedule_splits_target_and_halves() {
        // Time only → date half empty (keeps the event's own date).
        let r = parse_reschedule("把明天的会改到下午4点", now()).unwrap();
        assert_eq!(r.target, "明天的会");
        assert_eq!(r.new_date, None);
        assert_eq!(r.new_time, Some((16, 0)));
        // Date only → time half empty (keeps the event's own clock time).
        let r = parse_reschedule("期末考试推迟到下周五", now()).unwrap();
        assert_eq!(r.target, "期末考试");
        assert_eq!(r.new_date, NaiveDate::from_ymd_opt(2026, 7, 17));
        assert_eq!(r.new_time, None);
        // Both halves present.
        let r = parse_reschedule("将周会挪到明天上午十点", now()).unwrap();
        assert_eq!(r.target, "周会");
        assert_eq!(r.new_date, NaiveDate::from_ymd_opt(2026, 7, 7));
        assert_eq!(r.new_time, Some((10, 0)));
    }

    #[test]
    fn parse_cancel_extracts_target() {
        assert_eq!(parse_cancel("取消明天的会").as_deref(), Some("明天的会"));
        assert_eq!(parse_cancel("明天的会不开了").as_deref(), Some("明天的会"));
        assert_eq!(parse_cancel("今天天气怎么样"), None);
    }

    #[test]
    fn routes_english_deadline_without_clock_time() {
        // Date-only (no explicit time) but an English event keyword → event.
        assert_eq!(
            route_intent("in 3 days submit the report", now()),
            Intent::IngestEvent
        );
        let ex = RuleBasedExtractor::new();
        let ev = ex
            .extract("in 3 days submit the report", now())
            .unwrap()
            .unwrap();
        assert_eq!(ev.kind, EventKind::Deadline);
        assert_eq!(
            ev.start.date(),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 9).unwrap()
        );
    }

    #[test]
    fn routes_status() {
        assert_eq!(route_intent("我在护肤", now()), Intent::StatusAnswer);
    }

    #[test]
    fn extracts_meeting() {
        let ex = RuleBasedExtractor::new();
        let ev = ex
            .extract("明天下午3点在会议室和张伟开会", now())
            .unwrap()
            .unwrap();
        assert_eq!(ev.kind, EventKind::Meeting);
        assert_eq!(
            ev.start,
            NaiveDate::from_ymd_opt(2026, 7, 7)
                .unwrap()
                .and_hms_opt(15, 0, 0)
                .unwrap()
        );
        assert_eq!(ev.location.as_deref(), Some("会议室"));
        assert!(ev.people.contains(&"张伟".to_string()));
        assert!(!ev.title.contains("下午"));
        assert!(!ev.title.contains("3点"));
        // Location/people are stripped from the title (they have their own fields).
        assert_eq!(ev.title, "开会");
    }

    #[test]
    fn extracts_exam_kind() {
        let ex = RuleBasedExtractor::new();
        let ev = ex
            .extract("下周五上午九点期末考试", now())
            .unwrap()
            .unwrap();
        assert_eq!(ev.kind, EventKind::Exam);
    }

    #[test]
    fn non_event_returns_none() {
        let ex = RuleBasedExtractor::new();
        assert!(ex.extract("随便聊聊", now()).unwrap().is_none());
    }

    #[test]
    fn reminder_title_strips_filler() {
        let ex = RuleBasedExtractor::new();
        let ev = ex
            .extract("提醒我明天上午十点交作业", now())
            .unwrap()
            .unwrap();
        // "交作业" is a deadline-ish reminder; title should not carry the time.
        assert!(!ev.title.contains("十点"));
        assert!(ev.title.contains("作业"));
    }
}

//! Natural-language date/time parsing for Chinese and English.
//!
//! This is deliberately dependency-light (regex + hand-rolled Chinese numeral
//! handling) rather than a full NLP model: it must run offline and be fully
//! deterministic so reminders work even when the cloud LLM is unreachable
//! (F16 离线兜底, and the "通知可靠性" non-functional requirement).
//!
//! Everything resolves relative to an injected `now` so the logic is pure and
//! testable.

use std::sync::OnceLock;

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;

/// Compile a regex once and reuse it. Each call site gets its own static cell.
macro_rules! lazy_re {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex compiles"))
    }};
}

const CN_DIGITS: &str = "零一二两三四五六七八九十";

/// Outcome of parsing a time expression from free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedTime {
    pub start: NaiveDateTime,
    /// Whether an explicit clock time was found (vs. a date-only default).
    pub explicit_time: bool,
}

/// Parse a single Chinese numeral run (0..=99) such as "三", "十", "二十五", "两".
fn cn_numeral(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    let digit = |c: char| -> Option<u32> {
        Some(match c {
            '零' => 0,
            '一' => 1,
            '二' | '两' => 2,
            '三' => 3,
            '四' => 4,
            '五' => 5,
            '六' => 6,
            '七' => 7,
            '八' => 8,
            '九' => 9,
            _ => return None,
        })
    };
    let chars: Vec<char> = s.chars().collect();
    if let Some(pos) = chars.iter().position(|&c| c == '十') {
        let tens = if pos == 0 { 1 } else { digit(chars[pos - 1])? };
        let units = if pos + 1 < chars.len() {
            digit(chars[pos + 1])?
        } else {
            0
        };
        return Some(tens * 10 + units);
    }
    if chars.len() == 1 {
        return digit(chars[0]);
    }
    None
}

/// Parse either an ASCII number or a Chinese numeral run.
fn any_numeral(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    cn_numeral(s)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

/// Match a "<number><unit><suffix>" offset such as "三天后" or "2周后".
fn match_offset(text: &str, quick: &[&str], pattern: &str) -> Option<u32> {
    if !contains_any(text, quick) {
        return None;
    }
    let re = Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    any_numeral(caps.get(1)?.as_str())
}

/// Resolve the date component. Returns `(date, explicit)` where `explicit` is
/// false when we fell back to "today".
fn resolve_date(text: &str, now: NaiveDate) -> (NaiveDate, bool) {
    if contains_any(text, &["大后天"]) {
        return (now + Duration::days(3), true);
    }
    if contains_any(text, &["后天", "the day after tomorrow"]) {
        return (now + Duration::days(2), true);
    }
    if contains_any(text, &["明天", "明日", "明晚", "明早", "tomorrow"]) {
        return (now + Duration::days(1), true);
    }
    if contains_any(text, &["昨天", "昨日", "yesterday"]) {
        return (now - Duration::days(1), true);
    }
    if contains_any(
        text,
        &["今天", "今日", "今晚", "今早", "今晨", "today", "tonight"],
    ) {
        return (now, true);
    }

    // "N天后" / "N天之后" / "in N days"
    if let Some(days) = match_offset(
        text,
        &["天后", "天之后"],
        r"([0-9]+|[零一二两三四五六七八九十]+)\s*天\s*(?:后|之后)",
    ) {
        return (now + Duration::days(days as i64), true);
    }
    if let Some(caps) = lazy_re!(r"in\s+(\d+)\s+days?").captures(text) {
        if let Ok(n) = caps[1].parse::<i64>() {
            return (now + Duration::days(n), true);
        }
    }
    // "N周后" / "N星期后" / "in N weeks"
    if let Some(weeks) = match_offset(
        text,
        &["周后", "星期后", "礼拜后"],
        r"([0-9]+|[零一二两三四五六七八九十]+)\s*(?:周|星期|礼拜)\s*(?:后|之后)",
    ) {
        return (now + Duration::days(weeks as i64 * 7), true);
    }
    if let Some(caps) = lazy_re!(r"in\s+(\d+)\s+weeks?").captures(text) {
        if let Ok(n) = caps[1].parse::<i64>() {
            return (now + Duration::days(n * 7), true);
        }
    }

    // Weekday, possibly prefixed by 下/这/本 (next / this).
    if let Some(date) = resolve_weekday(text, now) {
        return (date, true);
    }

    // "下个月" / "next month"
    if contains_any(text, &["下个月", "下月", "next month"]) {
        let base = now.checked_add_months(Months::new(1)).unwrap_or(now);
        if let Some(day) = day_of_month(text) {
            if let Some(d) = base.with_day(day) {
                return (d, true);
            }
        }
        return (base, true);
    }

    // Explicit "M月D号/日".
    if let Some(date) = resolve_month_day(text, now) {
        return (date, true);
    }

    // ISO "YYYY-MM-DD".
    if let Some(caps) = lazy_re!(r"(\d{4})-(\d{1,2})-(\d{1,2})").captures(text) {
        if let (Ok(y), Ok(m), Ok(d)) = (
            caps[1].parse::<i32>(),
            caps[2].parse::<u32>(),
            caps[3].parse::<u32>(),
        ) {
            if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
                return (date, true);
            }
        }
    }

    // Bare "N号/N日" → this month, or next month if already past.
    if let Some(day) = day_of_month(text) {
        if let Some(d) = now.with_day(day) {
            if d >= now {
                return (d, true);
            }
            let nm = now.checked_add_months(Months::new(1)).unwrap_or(now);
            if let Some(d2) = nm.with_day(day) {
                return (d2, true);
            }
        }
    }

    (now, false)
}

fn day_of_month(text: &str) -> Option<u32> {
    let re = lazy_re!(r"([0-9]+|[零一二两三四五六七八九十]+)\s*(?:号|日)");
    let caps = re.captures(text)?;
    let n = any_numeral(caps.get(1)?.as_str())?;
    (1..=31).contains(&n).then_some(n)
}

fn resolve_month_day(text: &str, now: NaiveDate) -> Option<NaiveDate> {
    let re = lazy_re!(
        r"([0-9]+|[一二三四五六七八九十]+)\s*月\s*([0-9]+|[零一二两三四五六七八九十]+)?\s*(?:号|日)?"
    );
    let caps = re.captures(text)?;
    let month = any_numeral(caps.get(1)?.as_str())?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let day = caps
        .get(2)
        .and_then(|m| any_numeral(m.as_str()))
        .unwrap_or(1);
    if !(1..=31).contains(&day) {
        return None;
    }
    let year = now.year();
    let candidate = NaiveDate::from_ymd_opt(year, month, day)?;
    let year = if candidate < now { year + 1 } else { year };
    NaiveDate::from_ymd_opt(year, month, day)
}

fn weekday_index(text: &str) -> Option<u32> {
    let cn = [
        ("一", 0u32),
        ("二", 1),
        ("三", 2),
        ("四", 3),
        ("五", 4),
        ("六", 5),
        ("日", 6),
        ("天", 6),
    ];
    for prefix in ["周", "星期", "礼拜"] {
        if let Some(pos) = text.find(prefix) {
            let rest = &text[pos + prefix.len()..];
            if let Some(next) = rest.chars().next() {
                for (ch, idx) in cn {
                    if next.to_string() == ch {
                        return Some(idx);
                    }
                }
            }
        }
    }
    None
}

fn english_weekday_index(text: &str) -> Option<u32> {
    let days = [
        ("monday", 0u32),
        ("tuesday", 1),
        ("wednesday", 2),
        ("thursday", 3),
        ("friday", 4),
        ("saturday", 5),
        ("sunday", 6),
    ];
    days.iter()
        .find(|(name, _)| text.contains(*name))
        .map(|(_, idx)| *idx)
}

fn resolve_weekday(text: &str, now: NaiveDate) -> Option<NaiveDate> {
    let target = weekday_index(text).or_else(|| english_weekday_index(text))?;
    let cur = now.weekday().num_days_from_monday();
    let next_week = contains_any(text, &["下周", "下星期", "下礼拜", "next week", "next "]);
    let this_week = contains_any(text, &["这周", "本周", "这星期", "this week", "this "]);

    let this_monday = now - Duration::days(cur as i64);
    if next_week {
        Some(this_monday + Duration::days(7 + target as i64))
    } else if this_week {
        Some(this_monday + Duration::days(target as i64))
    } else {
        let ahead = (target as i64 - cur as i64 + 7) % 7;
        Some(now + Duration::days(ahead))
    }
}

/// Resolve the clock-time component if present. Returns `(hour, minute)`.
fn resolve_time(text: &str) -> Option<(u32, u32)> {
    let lower = text.to_lowercase();

    if lower.contains("midnight") || contains_any(text, &["午夜", "半夜"]) {
        return Some((0, 0));
    }

    // 24h "HH:MM".
    if let Some(caps) = lazy_re!(r"\b(\d{1,2}):(\d{2})\b").captures(&lower) {
        if let (Ok(mut h), Ok(m)) = (caps[1].parse::<u32>(), caps[2].parse::<u32>()) {
            if lower.contains("pm") && h < 12 {
                h += 12;
            }
            if lower.contains("am") && h == 12 {
                h = 0;
            }
            if h < 24 && m < 60 {
                return Some((h, m));
            }
        }
    }

    // English "3pm" / "3:30 pm" / "at 3pm".
    if let Some(caps) = lazy_re!(r"(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)").captures(&lower) {
        if let Ok(mut h) = caps[1].parse::<u32>() {
            let m = caps
                .get(2)
                .and_then(|x| x.as_str().parse().ok())
                .unwrap_or(0);
            let pm = &caps[3] == "pm";
            if pm && h < 12 {
                h += 12;
            }
            if !pm && h == 12 {
                h = 0;
            }
            if h < 24 && m < 60 {
                return Some((h, m));
            }
        }
    }

    if let Some((h, m)) = chinese_clock(text) {
        return Some((h, m));
    }

    if contains_any(text, &["中午", "正午"]) || lower.contains("noon") {
        return Some((12, 0));
    }

    None
}

fn chinese_clock(text: &str) -> Option<(u32, u32)> {
    let marker = if text.contains('点') {
        '点'
    } else if text.contains('時') {
        '時'
    } else if text.contains('时') {
        '时'
    } else {
        return None;
    };
    let pos = text.find(marker)?;
    let before = &text[..pos];
    let hour_run: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || CN_DIGITS.contains(*c))
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let mut hour = any_numeral(&hour_run)?;

    let after = &text[pos + marker.len_utf8()..];
    let mut minute = 0u32;
    if after.starts_with('半') {
        minute = 30;
    } else {
        let run: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit() || CN_DIGITS.contains(*c))
            .collect();
        if let Some(n) = any_numeral(&run) {
            if n < 60 {
                minute = n;
            }
        }
    }

    let pm = contains_any(text, &["下午", "傍晚", "晚上", "晚", "夜"]);
    let am = contains_any(
        text,
        &["上午", "早上", "早晨", "清晨", "凌晨", "今早", "明早"],
    );
    let noon = contains_any(text, &["中午", "正午"]);
    // Shift into the afternoon/evening for "下午/晚上 N点" (N<12) or "中午" (N<6).
    if (pm && !am && hour < 12) || (noon && hour < 6) {
        hour += 12;
    }
    if hour >= 24 || minute >= 60 {
        return None;
    }
    Some((hour, minute))
}

/// Split resolution for the reschedule path (F1 扩展): which parts of a phrase
/// carry an explicit date and/or an explicit clock time. "改到4点" must keep
/// the target event's original date, while "改到周五" must keep its original
/// clock time — so the caller needs the two halves separately, not the
/// combined default-filled result of [`parse_datetime`]. Relative offsets
/// ("2小时后") are not handled here.
pub fn parse_date_time_parts(
    text: &str,
    now: NaiveDateTime,
) -> (Option<NaiveDate>, Option<(u32, u32)>) {
    let (date, date_explicit) = resolve_date(text, now.date());
    (date_explicit.then_some(date), resolve_time(text))
}

/// Parse a datetime from free text, relative to `now`.
///
/// Resolution rules:
/// - explicit relative offsets ("in 2 hours" / "2小时后") win outright;
/// - otherwise date and time are resolved independently and combined;
/// - date without time defaults to 09:00;
/// - time without date uses today, rolling to tomorrow if already past.
pub fn parse_datetime(text: &str, now: NaiveDateTime) -> Option<ParsedTime> {
    let lower = text.to_lowercase();

    if let Some(caps) = lazy_re!(r"(?:in\s+)?(\d+)\s*(?:hours?|hrs?)\b").captures(&lower) {
        if let Ok(n) = caps[1].parse::<i64>() {
            return Some(ParsedTime {
                start: now + Duration::hours(n),
                explicit_time: true,
            });
        }
    }
    if let Some(n) = match_offset(
        text,
        &["小时后", "小时之后", "小時後"],
        r"([0-9]+|[零一二两三四五六七八九十]+)\s*(?:个|個)?\s*小[时時]\s*(?:后|後|之后)",
    ) {
        return Some(ParsedTime {
            start: now + Duration::hours(n as i64),
            explicit_time: true,
        });
    }
    if let Some(caps) = lazy_re!(r"(?:in\s+)?(\d+)\s*(?:minutes?|mins?)\b").captures(&lower) {
        if let Ok(n) = caps[1].parse::<i64>() {
            return Some(ParsedTime {
                start: now + Duration::minutes(n),
                explicit_time: true,
            });
        }
    }
    if let Some(n) = match_offset(
        text,
        &["分钟后", "分钟之后", "分鐘後"],
        r"([0-9]+|[零一二两三四五六七八九十]+)\s*分[钟鐘]\s*(?:后|後|之后)",
    ) {
        return Some(ParsedTime {
            start: now + Duration::minutes(n as i64),
            explicit_time: true,
        });
    }

    let (date, date_explicit) = resolve_date(text, now.date());
    let time = resolve_time(text);

    match (date_explicit, time) {
        (_, Some((h, m))) => {
            let t = NaiveTime::from_hms_opt(h, m, 0)?;
            let mut dt = NaiveDateTime::new(date, t);
            if !date_explicit && dt <= now {
                dt += Duration::days(1);
            }
            Some(ParsedTime {
                start: dt,
                explicit_time: true,
            })
        }
        (true, None) => {
            let t = NaiveTime::from_hms_opt(9, 0, 0)?;
            Some(ParsedTime {
                start: NaiveDateTime::new(date, t),
                explicit_time: false,
            })
        }
        (false, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> NaiveDateTime {
        // Monday 2026-07-06 10:00:00
        NaiveDate::from_ymd_opt(2026, 7, 6)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn p(text: &str) -> ParsedTime {
        parse_datetime(text, now()).unwrap_or_else(|| panic!("no time parsed from {text:?}"))
    }

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn cn_numerals() {
        assert_eq!(cn_numeral("三"), Some(3));
        assert_eq!(cn_numeral("十"), Some(10));
        assert_eq!(cn_numeral("十五"), Some(15));
        assert_eq!(cn_numeral("二十"), Some(20));
        assert_eq!(cn_numeral("二十五"), Some(25));
        assert_eq!(cn_numeral("两"), Some(2));
    }

    #[test]
    fn tomorrow_afternoon_cn() {
        let r = p("明天下午3点开会");
        assert_eq!(r.start, dt(2026, 7, 7, 15, 0));
        assert!(r.explicit_time);
    }

    #[test]
    fn tomorrow_evening_chinese_numeral() {
        assert_eq!(p("明晚八点半有约").start, dt(2026, 7, 7, 20, 30));
    }

    #[test]
    fn day_after_tomorrow_morning() {
        assert_eq!(p("后天上午九点考试").start, dt(2026, 7, 8, 9, 0));
    }

    #[test]
    fn n_days_later_cn() {
        let r = p("三天后交报告");
        assert_eq!(r.start, dt(2026, 7, 9, 9, 0));
        assert!(!r.explicit_time);
    }

    #[test]
    fn in_two_hours() {
        assert_eq!(p("in 2 hours standup").start, now() + Duration::hours(2));
    }

    #[test]
    fn in_30_minutes_cn() {
        assert_eq!(p("30分钟后提醒我喝水").start, now() + Duration::minutes(30));
    }

    #[test]
    fn two_hours_later_cn() {
        assert_eq!(p("两小时后开电话会").start, now() + Duration::hours(2));
    }

    #[test]
    fn next_friday_cn() {
        assert_eq!(
            p("下周五要考试").start.date(),
            NaiveDate::from_ymd_opt(2026, 7, 17).unwrap()
        );
    }

    #[test]
    fn this_coming_weekday_cn() {
        assert_eq!(p("周三下午两点上课").start, dt(2026, 7, 8, 14, 0));
    }

    #[test]
    fn english_next_monday_3pm() {
        assert_eq!(
            p("next monday at 3pm meeting").start,
            dt(2026, 7, 13, 15, 0)
        );
    }

    #[test]
    fn iso_datetime() {
        assert_eq!(p("2026-08-01 14:30 团建").start, dt(2026, 8, 1, 14, 30));
    }

    #[test]
    fn month_day_cn() {
        assert_eq!(p("12月31号晚上八点跨年").start, dt(2026, 12, 31, 20, 0));
    }

    #[test]
    fn time_only_rolls_to_tomorrow_when_past() {
        assert_eq!(p("8点提醒我").start, dt(2026, 7, 7, 8, 0));
    }

    #[test]
    fn time_only_today_when_future() {
        assert_eq!(p("下午3点").start, dt(2026, 7, 6, 15, 0));
    }

    #[test]
    fn noon_word() {
        assert_eq!(p("明天中午吃饭").start, dt(2026, 7, 7, 12, 0));
    }

    #[test]
    fn dian_not_misread_as_time() {
        // "有点累" contains 点 but no numeral before it → not a clock time.
        assert!(parse_datetime("有点累想休息", now()).is_none());
    }

    #[test]
    fn plain_chat_has_no_time() {
        assert!(parse_datetime("随便聊聊天气", now()).is_none());
    }

    #[test]
    fn today_word_defaults_to_nine() {
        let r = parse_datetime("我今天心情不错", now()).unwrap();
        assert_eq!(r.start, dt(2026, 7, 6, 9, 0));
        assert!(!r.explicit_time);
    }
}

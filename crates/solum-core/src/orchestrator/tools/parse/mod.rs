//! `parse` -- turning an utterance into something actionable: intent, memory-layer and event-kind labels, purge-shaped dangerous commands with their day range, the daily-recurrence phrases that map to `routines` (and the caveat text for the rest, which have no correct scheduler model), reschedule composition, and the filler words stripped before title matching. Pure — every function here reads text and returns a value.
//!
//! Part of the `tools` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(crate) fn intent_str(intent: Intent) -> &'static str {
    match intent {
        Intent::Chat => "chat",
        Intent::CreateWidget => "create_widget",
        Intent::IngestEvent => "ingest_event",
        Intent::StatusAnswer => "status_answer",
        Intent::MemoryWrite => "memory_write",
        Intent::RescheduleEvent => "reschedule_event",
        Intent::CancelEvent => "cancel_event",
        Intent::DangerousCommand => "dangerous_command",
    }
}

/// D3: recognize a purge-shaped dangerous command and derive its arguments.
/// Requires a purge verb (already guaranteed by the danger routing), a layer
/// keyword, and optionally a time range ("上周/N天前"; absent = 全部).
pub(crate) fn parse_purge_request(
    text: &str,
    now: NaiveDateTime,
) -> Option<(MemoryLayer, NaiveDateTime)> {
    let layer = if text.contains("行为日志") || text.contains("日志") {
        MemoryLayer::Behavior
    } else if text.contains("建议") {
        MemoryLayer::Suggestion
    } else if text.contains("穿戴") || text.contains("健康数据") {
        MemoryLayer::Wearable
    } else {
        return None;
    };
    // "上周的/一周前的" → older than 7 days; "N天前" → older than N days;
    // no range word (or 全部/所有) → everything (before `now`).
    let before = if text.contains("上周") || text.contains("一周") {
        now - Duration::days(7)
    } else if let Some(days) = extract_days_back(text) {
        now - Duration::days(days)
    } else {
        now
    };
    Some((layer, before))
}

/// "3天前 / 三天以前" → 3. Digits and simple Chinese numerals only.
fn extract_days_back(text: &str) -> Option<i64> {
    let idx = text.find("天前").or_else(|| text.find("天以前"))?;
    let head: String = text[..idx]
        .chars()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    // Trailing run of digit-ish chars right before "天前".
    let digits: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || "零一二两三四五六七八九十".contains(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        return None;
    }
    if let Ok(n) = digits.parse::<i64>() {
        return (n > 0).then_some(n);
    }
    let mapped: i64 = match digits.as_str() {
        "一" => 1,
        "两" | "二" => 2,
        "三" => 3,
        "四" => 4,
        "五" => 5,
        "六" => 6,
        "七" => 7,
        "八" => 8,
        "九" => 9,
        "十" => 10,
        _ => return None,
    };
    Some(mapped)
}

pub(crate) fn layer_label(layer: MemoryLayer) -> &'static str {
    match layer {
        MemoryLayer::RawInput => "原始输入",
        MemoryLayer::Event => "日程",
        MemoryLayer::Notification => "提醒",
        MemoryLayer::Behavior => "行为日志",
        MemoryLayer::Suggestion => "建议",
        MemoryLayer::Wearable => "穿戴数据",
        MemoryLayer::Fact => "记忆",
        MemoryLayer::Routine => "固定提醒",
        MemoryLayer::NotificationCapture => "通知捕获",
    }
}

pub(crate) fn summarize_event(ev: &Event, notifs: &[Notification], now: NaiveDateTime) -> String {
    let loc = ev
        .location
        .as_ref()
        .map(|l| format!("，地点：{l}"))
        .unwrap_or_default();
    let people = if ev.people.is_empty() {
        String::new()
    } else {
        format!("，参与：{}", ev.people.join("、"))
    };
    let mut msg = format!(
        "已记录【{}】{}，时间：{}{}{}",
        kind_label(&ev.kind),
        ev.title,
        crate::model::fmt_ts_human(&ev.start),
        loc,
        people
    );
    if notifs.is_empty() {
        msg.push_str("；未设置提前提醒。");
    } else {
        msg.push_str(&format!("；{}", summarize_reminders(notifs, now)));
    }
    msg
}

/// Just the reminder-plan tail of [`summarize_event`], for reschedule replies.
/// A reminder whose fire time is already behind `now`（比如今天才录入三天后的
/// 考试）is called out — otherwise the user thinks a heads-up is still coming.
pub(crate) fn summarize_reminders(notifs: &[Notification], now: NaiveDateTime) -> String {
    if notifs.is_empty() {
        return "未设置提前提醒。".into();
    }
    let times: Vec<String> = notifs
        .iter()
        .map(|n| {
            let when = crate::model::fmt_ts_human(&n.fire_at);
            if n.fire_at <= now {
                format!("{when}（提前{}·已过点，将立即提醒）", n.lead_label)
            } else {
                format!("{when}（提前{}）", n.lead_label)
            }
        })
        .collect();
    format!("提醒计划：{}", times.join("，"))
}

/// Daily phrases are mapped to `routines`; the remaining recurrence phrases
/// still have no correct scheduler model, so say so instead of silently
/// degrading to a one-shot entry.
pub(crate) fn is_daily_recurrence(text: &str) -> bool {
    ["每天", "每日", "每早", "每晚"]
        .iter()
        .any(|word| text.contains(word))
}

pub(crate) fn recurrence_caveat(text: &str) -> Option<&'static str> {
    const WORDS: &[&str] = &["每小时", "每周", "每月"];
    WORDS.iter().any(|w| text.contains(w)).then_some(
        "注意：暂不支持重复提醒，这里只登记了最近的一次（习惯类可通过「固定提醒」实现）。",
    )
}

/// Combine a reschedule request with the target event: whichever half the
/// phrase didn't specify keeps the event's original value ("改到4点" keeps
/// the date, "改到周五" keeps the clock time).
pub(crate) fn compose_new_start(
    ev: &Event,
    req: &crate::extract::RescheduleRequest,
) -> NaiveDateTime {
    let date = req.new_date.unwrap_or_else(|| ev.start.date());
    let time = req
        .new_time
        .and_then(|(h, m)| chrono::NaiveTime::from_hms_opt(h, m, 0))
        .unwrap_or_else(|| ev.start.time());
    NaiveDateTime::new(date, time)
}

/// Filler words stripped from a target description before title matching —
/// they scope the search (dates handle that separately) but never name it.
// Longer phrases first: "下星期五" must strip before "星期五" leaves "下"
// behind, "大后天" before "后天".
pub(crate) const TARGET_FILLERS: &[&str] = &[
    "下星期一",
    "下星期二",
    "下星期三",
    "下星期四",
    "下星期五",
    "下星期六",
    "下星期日",
    "下星期天",
    "星期一",
    "星期二",
    "星期三",
    "星期四",
    "星期五",
    "星期六",
    "星期日",
    "星期天",
    "下周一",
    "下周二",
    "下周三",
    "下周四",
    "下周五",
    "下周六",
    "下周日",
    "下周天",
    "周一",
    "周二",
    "周三",
    "周四",
    "周五",
    "周六",
    "周日",
    "周天",
    "下周",
    "这周",
    "本周",
    "大后天",
    "后天",
    "今天",
    "明天",
    "上午",
    "下午",
    "晚上",
    "早上",
    "中午",
    "那个",
    "那场",
    "这个",
    "这场",
    "的",
];

pub(crate) fn kind_label(kind: &crate::model::EventKind) -> &str {
    kind.label()
}

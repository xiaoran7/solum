//! `tests` -- the parsing, dedup, limit and clue tests — the cases where a change to the intake order would quietly change which drafts a user is shown.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::classify::{platform_matrix, CaptureAvailability, PlatformSupport};
use super::inbox::{MAX_DRAFTS, MAX_DRAFT_CHARS};
use super::*;

#[test]
fn notification_capture_is_android_only() {
    let find = |list: Vec<CaptureConnector>| {
        list.into_iter()
            .find(|c| c.source == CaptureSource::Notification)
            .expect("notification connector is always listed")
    };
    assert_eq!(
        find(capture_connectors(true)).availability,
        CaptureAvailability::Ready
    );
    // 桌面必须是"不支持"，不能是"待接入"——后者暗示以后会有，是假的。
    assert_eq!(
        find(capture_connectors(false)).availability,
        CaptureAvailability::Unsupported
    );
}

#[test]
fn platform_matrix_matches_the_harmony_shell() {
    assert_eq!(
        platform_matrix(CaptureSource::Notification).harmonyos,
        PlatformSupport::Unsupported
    );
    assert_eq!(
        platform_matrix(CaptureSource::Share).harmonyos,
        PlatformSupport::Available
    );
    assert_eq!(
        platform_matrix(CaptureSource::Calendar).harmonyos,
        PlatformSupport::PermissionRequired
    );
    assert_eq!(
        platform_matrix(CaptureSource::Health).harmonyos,
        PlatformSupport::Planned
    );
}

/// 高敏感位置在没有实现按需取点以前不能标成可用。
#[test]
fn unimplemented_entries_are_not_advertised_as_ready() {
    for c in capture_connectors(true) {
        if c.source == CaptureSource::Location {
            assert_ne!(
                c.availability,
                CaptureAvailability::Ready,
                "{} 在本仓尚未实现，不能标成可用",
                c.label
            );
        }
    }
}

#[test]
fn source_strings_round_trip() {
    for c in capture_connectors(true) {
        assert_eq!(CaptureSource::parse(c.source.as_str()), Some(c.source));
    }
    assert_eq!(CaptureSource::parse("no_such_source"), None);
}

#[test]
fn duplicate_text_across_sources_merges_without_losing_provenance() {
    let mut inbox = CaptureInbox::new();
    let first = inbox.push(CaptureSource::Share, "", "明天 10 点开会", 1_000);
    let merged = inbox.push(CaptureSource::Calendar, "", "明天10点开会", 2_000);
    assert_eq!(first.id, merged.id);
    assert_eq!(inbox.len(), 1);
    assert_eq!(merged.also_seen_from, vec![CaptureSource::Calendar]);
    assert!(merged.note.as_deref().unwrap().contains("合并"));
}

#[test]
fn external_candidates_expire_but_manual_drafts_do_not() {
    let mut inbox = CaptureInbox::new();
    inbox.push(CaptureSource::Share, "", "外部候选", 1_000);
    inbox.push(CaptureSource::Manual, "", "手动候选", 1_001);
    assert_eq!(inbox.prune_expired(1_000 + 8 * 24 * 60 * 60 * 1000), 1);
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox.snapshot()[0].source, CaptureSource::Manual);
}

#[test]
fn clues_pick_up_time_location_and_amount() {
    let clues = extract_capture_clues(
        "会员续费提醒\n下周三 下午3点 在实验楼B301 开会\n地点：实验楼B301\n合计 ¥128.50",
    );
    assert_eq!(clues.matter.as_deref(), Some("会员续费提醒"));
    assert_eq!(clues.time.as_deref(), Some("下周三 下午3点"));
    assert_eq!(clues.location.as_deref(), Some("实验楼B301"));
    assert_eq!(clues.amount.as_deref(), Some("¥128.50"));
    assert!(clues.summary().contains("金额：¥128.50"));
}

#[test]
fn natural_location_is_used_only_without_an_explicit_one() {
    let natural = extract_capture_clues("明天在图书馆三楼自习室集合");
    assert_eq!(natural.location.as_deref(), Some("图书馆三楼自习室"));

    let explicit = extract_capture_clues("地址：东风路 8 号\n在别处集合");
    assert_eq!(explicit.location.as_deref(), Some("东风路 8 号"));
}

#[test]
fn amount_written_in_yuan_is_recognised() {
    assert_eq!(
        extract_capture_clues("订单金额 39.9 元").amount.as_deref(),
        Some("39.9 元")
    );
}

#[test]
fn a_bare_url_is_not_the_matter_line() {
    let clues = extract_capture_clues("https://example.com/order/123\n订单已发货");
    assert_eq!(clues.matter.as_deref(), Some("订单已发货"));
}

#[test]
fn empty_input_yields_nothing_and_says_so() {
    let clues = extract_capture_clues("   \n  ");
    assert!(clues.is_empty());
    assert!(clues.summary().contains("暂未提取到明确线索"));
}

/// 按字节切中文会 panic；这条钉住按字符截断。
#[test]
fn a_long_chinese_matter_line_truncates_on_char_boundaries() {
    let line = "考".repeat(80);
    let matter = extract_capture_clues(&line).matter.unwrap();
    assert_eq!(matter.chars().count(), 49, "48 个字 + 省略号");
    assert!(matter.ends_with('…'));
}

#[test]
fn inbox_holds_drafts_until_they_are_removed() {
    let mut inbox = CaptureInbox::new();
    assert!(inbox.is_empty());

    let a = inbox.push(
        CaptureSource::Share,
        "",
        "明天 10 点开会",
        1_700_000_000_000,
    );
    let b = inbox.push(
        CaptureSource::Manual,
        "手写标题",
        "随便写点",
        1_700_000_001_000,
    );
    assert_ne!(a.id, b.id, "id 必须唯一");
    assert_eq!(inbox.len(), 2);

    // 标题留空时用线索里的事项兜底。
    assert_eq!(a.title, "明天 10 点开会");
    assert_eq!(b.title, "手写标题");

    // 最新的排前面。
    assert_eq!(inbox.snapshot()[0].id, b.id);

    assert!(inbox.remove(&a.id));
    assert!(!inbox.remove(&a.id), "删第二次应报告「没删到」");
    assert_eq!(inbox.len(), 1);
    assert!(inbox.get(&b.id).is_some());
}

#[test]
fn an_untitled_draft_without_clues_falls_back_to_the_entry_name() {
    let mut inbox = CaptureInbox::new();
    let d = inbox.push(CaptureSource::Share, "", "   ", 1_700_000_000_000);
    assert_eq!(d.title, "系统分享");
}

#[test]
fn oversized_input_is_truncated_rather_than_rejected() {
    let mut inbox = CaptureInbox::new();
    let huge = "字".repeat(MAX_DRAFT_CHARS + 500);
    let d = inbox.push(CaptureSource::Share, "x", &huge, 1_700_000_000_000);
    assert_eq!(d.text.chars().count(), MAX_DRAFT_CHARS + 1);
    assert!(d.text.ends_with('…'));
}

#[test]
fn the_queue_is_capped_and_drops_the_oldest() {
    let mut inbox = CaptureInbox::new();
    let mut first = String::new();
    for i in 0..(MAX_DRAFTS + 5) {
        let d = inbox.push(
            CaptureSource::Share,
            &format!("第 {i} 条"),
            &format!("正文 {i}"),
            1_700_000_000_000 + i as i64,
        );
        if i == 0 {
            first = d.id;
        }
    }
    assert_eq!(inbox.len(), MAX_DRAFTS);
    assert!(inbox.get(&first).is_none(), "最旧的应被挤出");
}

//! Privacy-minimal email event extraction.
//!
//! Only IMAP envelope metadata is inspected. Bodies and attachments are not
//! fetched, stored, synchronized, backed up, or sent to the LLM. Matches enter
//! the ordinary reviewable capture inbox and become durable only after the
//! user accepts them.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::Write;

use crate::email::EmailSummary;

const MAX_CANDIDATES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailEventKind {
    Security,
    Travel,
    Appointment,
    Deadline,
    Payment,
    Delivery,
}

impl EmailEventKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Security => "账户安全",
            Self::Travel => "出行",
            Self::Appointment => "预约或会议",
            Self::Deadline => "截止或到期",
            Self::Payment => "账单或付款",
            Self::Delivery => "物流或取件",
        }
    }

    fn confidence(self) -> u16 {
        match self {
            Self::Security | Self::Travel => 900,
            Self::Appointment | Self::Deadline | Self::Payment => 860,
            Self::Delivery => 820,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HighValueEmailCandidate {
    pub uid: u32,
    pub kind: EmailEventKind,
    pub title: String,
    pub text: String,
    pub external_id: String,
    pub confidence: u16,
}

fn contains_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|word| text.contains(word))
}

pub fn classify_high_value_subject(subject: &str) -> Option<EmailEventKind> {
    let normalized = subject.trim().to_lowercase();
    if normalized.is_empty()
        || contains_any(
            &normalized,
            &[
                "newsletter",
                "促销",
                "优惠",
                "折扣",
                "新品",
                "推荐",
                "营销",
                "日报",
                "周报",
            ],
        )
    {
        return None;
    }
    let groups: &[(EmailEventKind, &[&str])] = &[
        (
            EmailEventKind::Security,
            &[
                "安全提醒",
                "异地登录",
                "登录活动",
                "密码已",
                "验证码",
                "security alert",
                "new sign-in",
                "password changed",
                "verification code",
            ],
        ),
        (
            EmailEventKind::Travel,
            &[
                "航班",
                "登机",
                "机票",
                "车票",
                "列车",
                "酒店预订",
                "行程确认",
                "flight",
                "boarding",
                "train ticket",
                "hotel reservation",
                "itinerary",
            ],
        ),
        (
            EmailEventKind::Appointment,
            &[
                "会议邀请",
                "会议变更",
                "面试",
                "预约确认",
                "就诊",
                "appointment",
                "interview",
                "meeting invitation",
                "meeting updated",
            ],
        ),
        (
            EmailEventKind::Deadline,
            &[
                "截止",
                "到期",
                "即将过期",
                "提交期限",
                "deadline",
                "due date",
                "expires",
                "expiring",
            ],
        ),
        (
            EmailEventKind::Payment,
            &[
                "账单",
                "还款",
                "付款到期",
                "缴费",
                "扣款失败",
                "invoice",
                "bill due",
                "payment due",
                "payment failed",
            ],
        ),
        (
            EmailEventKind::Delivery,
            &[
                "快递",
                "取件",
                "派送",
                "已签收",
                "delivery",
                "out for delivery",
                "pickup code",
                "parcel",
            ],
        ),
    ];
    groups
        .iter()
        .find_map(|(kind, words)| contains_any(&normalized, words).then_some(*kind))
}

fn stable_external_id(account_id: &str, mailbox: &str, uid: u32) -> String {
    let mut digest = Sha256::new();
    digest.update(b"solum-email-event:v1\0");
    digest.update(account_id.as_bytes());
    digest.update(b"\0");
    digest.update(mailbox.as_bytes());
    digest.update(b"\0");
    digest.update(uid.to_string().as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        // Writing into a String cannot fail.
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    format!("email:{encoded}")
}

fn bounded(value: &str, chars: usize) -> String {
    value.trim().chars().take(chars).collect()
}

pub fn high_value_candidates(
    account_id: &str,
    mailbox: &str,
    summaries: &[EmailSummary],
) -> Vec<HighValueEmailCandidate> {
    summaries
        .iter()
        .filter_map(|summary| {
            let kind = classify_high_value_subject(&summary.subject)?;
            let subject = bounded(&summary.subject, 500);
            let sender = bounded(&summary.from, 320);
            let date = bounded(&summary.date, 128);
            Some(HighValueEmailCandidate {
                uid: summary.uid,
                kind,
                title: subject.clone(),
                text: format!(
                    "邮件事件：{}\n主题：{}\n发件人：{}\n邮件时间：{}",
                    kind.label(),
                    subject,
                    sender,
                    date
                ),
                external_id: stable_external_id(account_id, mailbox, summary.uid),
                confidence: kind.confidence(),
            })
        })
        .take(MAX_CANDIDATES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(uid: u32, subject: &str) -> EmailSummary {
        EmailSummary {
            uid,
            from: "service@example.com".into(),
            subject: subject.into(),
            date: "2026-08-24T12:00:00Z".into(),
            seen: false,
            size: 2048,
        }
    }

    #[test]
    fn extracts_only_high_value_envelope_events() {
        let rows = vec![
            summary(1, "航班 MU5123 行程确认"),
            summary(2, "本周新品优惠 newsletter"),
            summary(3, "信用卡账单即将到期"),
        ];
        let out = high_value_candidates("main", "INBOX", &rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, EmailEventKind::Travel);
        assert_eq!(out[1].kind, EmailEventKind::Deadline);
        assert!(!out[0].text.contains("正文"));
        assert!(out[0].external_id.starts_with("email:"));
    }

    #[test]
    fn stable_id_deduplicates_the_same_mail_without_exposing_account_or_folder() {
        let row = summary(42, "异地登录安全提醒");
        let first = high_value_candidates("private-account", "INBOX", std::slice::from_ref(&row));
        let second = high_value_candidates("private-account", "INBOX", &[row]);
        assert_eq!(first[0].external_id, second[0].external_id);
        assert!(!first[0].external_id.contains("private-account"));
        assert_eq!(first[0].kind, EmailEventKind::Security);
    }

    #[test]
    fn caps_candidates_and_rejects_promotional_false_positives() {
        let rows: Vec<_> = (1..=30).map(|uid| summary(uid, "快递正在派送")).collect();
        assert_eq!(
            high_value_candidates("a", "INBOX", &rows).len(),
            MAX_CANDIDATES
        );
        assert!(classify_high_value_subject("账单优惠促销").is_none());
        assert!(classify_high_value_subject("ordinary hello").is_none());
    }
}

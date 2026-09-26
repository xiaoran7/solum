//! `types` -- the UI projection shapes: account summary, folder, message summary, and the send draft.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{truncate_for_preview, validate_mailbox_address};
use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAccountSummary {
    pub id: String,
    pub label: String,
    pub provider: EmailProvider,
    pub address: String,
    pub auth_kind: String,
    pub secret_tail: String,
    pub oauth_ready: bool,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: SmtpTls,
    pub client_id: String,
    pub tenant: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailFolder {
    pub name: String,
    pub delimiter: Option<char>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSummary {
    pub uid: u32,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub seen: bool,
    pub size: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailMessage {
    pub uid: u32,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub date: String,
    pub text: String,
    pub html: Option<String>,
    pub attachment_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSendDraft {
    pub account_id: String,
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub html: bool,
}

impl EmailSendDraft {
    pub fn parse_json(args: &str) -> Result<Self> {
        let draft: Self = serde_json::from_str(args)
            .map_err(|e| CoreError::Invalid(format!("email_send 参数不是 JSON: {e}")))?;
        draft.validate()?;
        Ok(draft)
    }

    pub fn validate(&self) -> Result<()> {
        if self.account_id.trim().is_empty() || self.account_id.len() > 64 {
            return Err(CoreError::Invalid("邮件必须指定有效账户".into()));
        }
        let total = self.to.len() + self.cc.len() + self.bcc.len();
        if self.to.is_empty() || total > 100 {
            return Err(CoreError::Invalid(
                "邮件需要至少一个收件人，且总收件人不能超过 100 个".into(),
            ));
        }
        for address in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            validate_mailbox_address(address, "收件人")?;
        }
        if self.subject.chars().count() > 500 || self.body.len() > 1_000_000 {
            return Err(CoreError::Invalid("邮件主题或正文超出 v1 大小限制".into()));
        }
        if self.subject.contains(['\r', '\n']) {
            return Err(CoreError::Invalid("邮件主题不能包含换行".into()));
        }
        Ok(())
    }

    /// This is deliberately safe to put in the append-only audit table.
    pub fn audit_summary(&self) -> String {
        format!(
            "邮件发送：账户 {}，收件人 {} 位；内容已脱敏",
            self.account_id,
            self.to.len() + self.cc.len() + self.bcc.len()
        )
    }

    pub fn preview(&self, account: &EmailAccount) -> String {
        let join = |addresses: &[String]| addresses.join("、");
        let body = truncate_for_preview(&self.body, 8_000);
        format!(
            "将从「{} <{}>」发送邮件。\n收件人：{}\n抄送：{}\n密送：{}\n主题：{}\n\n正文：\n{}",
            account.label,
            account.address,
            join(&self.to),
            if self.cc.is_empty() {
                "无".into()
            } else {
                join(&self.cc)
            },
            if self.bcc.is_empty() {
                "无".into()
            } else {
                join(&self.bcc)
            },
            self.subject,
            body,
        )
    }
}

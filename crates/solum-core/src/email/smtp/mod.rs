//! `smtp` -- sending and previewing a draft over SMTP.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::oauth::oauth_access_token;
use super::*;

pub fn send_configured(draft: &EmailSendDraft) -> Result<String> {
    let config = EmailConfig::load()
        .ok_or_else(|| CoreError::NotFound("solum-email.json 邮箱配置".into()))?;
    let account = config.account(&draft.account_id)?;
    send(account, draft)
}

pub fn preview_configured(draft: &EmailSendDraft) -> Result<String> {
    let config = EmailConfig::load()
        .ok_or_else(|| CoreError::NotFound("solum-email.json 邮箱配置".into()))?;
    let account = config.account(&draft.account_id)?;
    draft.validate()?;
    account.validate(true)?;
    Ok(draft.preview(account))
}

pub(super) fn send(account: &EmailAccount, draft: &EmailSendDraft) -> Result<String> {
    draft.validate()?;
    account.validate(true)?;
    let from: Mailbox = account
        .address
        .parse()
        .map_err(|e| CoreError::Invalid(format!("发件邮箱无效: {e}")))?;
    let mut builder = Message::builder().from(from).subject(&draft.subject);
    for recipient in &draft.to {
        builder = builder.to(recipient
            .parse()
            .map_err(|e| CoreError::Invalid(format!("收件人无效: {e}")))?);
    }
    for recipient in &draft.cc {
        builder = builder.cc(recipient
            .parse()
            .map_err(|e| CoreError::Invalid(format!("抄送无效: {e}")))?);
    }
    for recipient in &draft.bcc {
        builder = builder.bcc(
            recipient
                .parse()
                .map_err(|e| CoreError::Invalid(format!("密送无效: {e}")))?,
        );
    }
    let content_type = if draft.html {
        ContentType::parse("text/html; charset=utf-8")
            .map_err(|e| CoreError::Invalid(format!("HTML 邮件格式无效: {e}")))?
    } else {
        ContentType::TEXT_PLAIN
    };
    let message = builder
        .singlepart(
            SinglePart::builder()
                .header(content_type)
                .body(draft.body.clone()),
        )
        .map_err(|e| CoreError::Invalid(format!("构建邮件失败: {e}")))?;
    let secret = match &account.auth {
        EmailAuth::AppPassword { secret } => secret.clone(),
        EmailAuth::OAuth2 { .. } => oauth_access_token(account)?,
    };
    let tls = TlsParameters::new(account.endpoints.smtp_host.clone())
        .map_err(|e| CoreError::Email(format!("创建 SMTP TLS 连接失败: {e}")))?;
    let tls = match account.endpoints.smtp_tls {
        SmtpTls::Wrapper => Tls::Wrapper(tls),
        SmtpTls::StartTls => Tls::Required(tls),
    };
    let mechanism = match account.auth {
        EmailAuth::AppPassword { .. } => Mechanism::Plain,
        EmailAuth::OAuth2 { .. } => Mechanism::Xoauth2,
    };
    let sender = SmtpTransport::relay(&account.endpoints.smtp_host)
        .map_err(|e| CoreError::Email(format!("SMTP 配置无效: {e}")))?
        .port(account.endpoints.smtp_port)
        .tls(tls)
        .credentials(Credentials::new(account.address.clone(), secret))
        .authentication(vec![mechanism])
        .build();
    sender
        .send(&message)
        .map_err(|e| CoreError::Email(format!("SMTP 发送失败: {e}")))?;
    Ok(format!(
        "已发送给 {} 位收件人。",
        draft.to.len() + draft.cc.len() + draft.bcc.len()
    ))
}

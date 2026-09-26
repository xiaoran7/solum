//! `mail` -- the IMAP session: connect, list folders, list/search, fetch, and parse into the UI shapes.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{decode_header, strip_html};
use super::oauth::{oauth_access_token, XOAuth2};
use super::*;

pub(super) type ImapSession = imap::Session<imap::Connection>;

pub(super) fn connect(account: &EmailAccount) -> Result<ImapSession> {
    account.validate(true)?;
    let client = imap::ClientBuilder::new(
        account.endpoints.imap_host.as_str(),
        account.endpoints.imap_port,
    )
    .connect()
    .map_err(|e| CoreError::Email(format!("连接 IMAP 服务器失败: {e}")))?;
    match &account.auth {
        EmailAuth::AppPassword { secret } => client
            .login(&account.address, secret)
            .map_err(|e| CoreError::Email(format!("IMAP 登录失败: {}", e.0))),
        EmailAuth::OAuth2 { .. } => {
            let token = oauth_access_token(account)?;
            let response = format!("user={}\x01auth=Bearer {}\x01\x01", account.address, token);
            client
                .authenticate(
                    "XOAUTH2",
                    &XOAuth2 {
                        response: response.into_bytes(),
                    },
                )
                .map_err(|e| CoreError::Email(format!("IMAP OAuth 登录失败: {}", e.0)))
        }
    }
}

pub fn list_folders(account: &EmailAccount) -> Result<Vec<EmailFolder>> {
    let mut session = connect(account)?;
    let folders = session
        .list(Some(""), Some("*"))
        .map_err(|e| CoreError::Email(format!("读取邮箱文件夹失败: {e}")))?
        .iter()
        .map(|folder| EmailFolder {
            name: folder.name().to_string(),
            delimiter: folder.delimiter().and_then(|value| value.chars().next()),
        })
        .collect();
    let _ = session.logout();
    Ok(folders)
}

pub fn list_messages(
    account: &EmailAccount,
    mailbox: &str,
    limit: usize,
) -> Result<Vec<EmailSummary>> {
    let mut session = connect(account)?;
    let messages = list_messages_session(&mut session, mailbox, limit)?;
    let _ = session.logout();
    Ok(messages)
}

pub fn search_messages(
    account: &EmailAccount,
    mailbox: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<EmailSummary>> {
    let query = query.trim();
    if query.is_empty()
        || query.chars().count() > MAX_SEARCH_QUERY_LEN
        || query.contains(['\r', '\n', '"'])
    {
        return Err(CoreError::Invalid(
            "搜索词需要是 1–160 个字符，且不能包含引号或换行".into(),
        ));
    }
    let mut session = connect(account)?;
    session
        .examine(mailbox)
        .map_err(|e| CoreError::Email(format!("打开邮箱文件夹失败: {e}")))?;
    // Use a fixed IMAP grammar and quote/escape only the user term. Server-side
    // search avoids copying entire mailboxes into process memory.
    let safe = query.replace('\\', "\\\\");
    let ids = session
        .uid_search(format!("OR FROM \"{safe}\" SUBJECT \"{safe}\""))
        .map_err(|e| CoreError::Email(format!("邮箱搜索失败: {e}")))?;
    let messages =
        fetch_summaries_by_uid(&mut session, &ids.into_iter().collect::<Vec<_>>(), limit)?;
    let _ = session.logout();
    Ok(messages)
}

pub fn get_message(account: &EmailAccount, mailbox: &str, uid: u32) -> Result<EmailMessage> {
    if uid == 0 {
        return Err(CoreError::Invalid("邮件 uid 必须为正整数".into()));
    }
    let mut session = connect(account)?;
    session
        .examine(mailbox)
        .map_err(|e| CoreError::Email(format!("打开邮箱文件夹失败: {e}")))?;
    let fetched = session
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
        .map_err(|e| CoreError::Email(format!("读取邮件失败: {e}")))?;
    let mail = fetched
        .iter()
        .next()
        .and_then(|item| item.body())
        .ok_or_else(|| CoreError::NotFound(format!("邮件 uid {uid}")))?;
    if mail.len() > MAX_BODY_BYTES {
        return Err(CoreError::Invalid(
            "邮件过大，v1 最多读取 1.5 MB 正文".into(),
        ));
    }
    let parsed = mailparse::parse_mail(mail)
        .map_err(|e| CoreError::Invalid(format!("邮件 MIME 解析失败: {e}")))?;
    let message = parsed_message(uid, &parsed)?;
    let _ = session.logout();
    Ok(message)
}

pub(super) fn list_messages_session(
    session: &mut ImapSession,
    mailbox: &str,
    limit: usize,
) -> Result<Vec<EmailSummary>> {
    let status = session
        .examine(mailbox)
        .map_err(|e| CoreError::Email(format!("打开邮箱文件夹失败: {e}")))?;
    if status.exists == 0 {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, MAX_LIST_LIMIT) as u32;
    let start = status.exists.saturating_sub(limit).saturating_add(1);
    let fetched = session
        .fetch(
            format!("{start}:*"),
            "(UID FLAGS ENVELOPE INTERNALDATE RFC822.SIZE)",
        )
        .map_err(|e| CoreError::Email(format!("读取邮件列表失败: {e}")))?;
    summaries_from_fetches(&fetched)
}

pub(super) fn fetch_summaries_by_uid(
    session: &mut ImapSession,
    ids: &[u32],
    limit: usize,
) -> Result<Vec<EmailSummary>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    let take = limit.clamp(1, MAX_LIST_LIMIT);
    let selected = ids.into_iter().rev().take(take).collect::<Vec<_>>();
    let set = selected
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let fetched = session
        .uid_fetch(set, "(UID FLAGS ENVELOPE INTERNALDATE RFC822.SIZE)")
        .map_err(|e| CoreError::Email(format!("读取搜索结果失败: {e}")))?;
    summaries_from_fetches(&fetched)
}

// imap 3.x 的 fetch 返回自持有的 `Fetches`（内部是 borrow 自 Vec<u8> 的自引用结构），
// 不再 Deref 成 &[Fetch]，只暴露 iter()——所以这里按容器收参而不是切片。
pub(super) fn summaries_from_fetches(fetches: &imap::types::Fetches) -> Result<Vec<EmailSummary>> {
    let mut summaries = fetches
        .iter()
        .filter_map(|item| {
            let envelope = item.envelope()?;
            let uid = item.uid?;
            let from = envelope
                .from
                .as_ref()
                .map(|addresses| {
                    addresses
                        .iter()
                        .map(|address| {
                            let mailbox = address
                                .mailbox
                                .as_deref()
                                .and_then(|raw| std::str::from_utf8(raw).ok())
                                .unwrap_or_default();
                            let host = address
                                .host
                                .as_deref()
                                .and_then(|raw| std::str::from_utf8(raw).ok())
                                .unwrap_or_default();
                            let email = if host.is_empty() {
                                mailbox.to_string()
                            } else {
                                format!("{mailbox}@{host}")
                            };
                            let name = address
                                .name
                                .as_deref()
                                .and_then(|raw| std::str::from_utf8(raw).ok())
                                .map(decode_header)
                                .unwrap_or_default();
                            if name.is_empty() {
                                email
                            } else {
                                format!("{name} <{email}>")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "（未知发件人）".into());
            let subject = envelope
                .subject
                .as_deref()
                .and_then(|raw| std::str::from_utf8(raw).ok())
                .map(decode_header)
                .unwrap_or_else(|| "（无主题）".into());
            let date = envelope
                .date
                .as_deref()
                .and_then(|raw| std::str::from_utf8(raw).ok())
                .map(decode_header)
                .unwrap_or_default();
            Some(EmailSummary {
                uid,
                from,
                subject,
                date,
                seen: item.flags().iter().any(|flag| flag.to_string() == "\\Seen"),
                size: item.size.unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|item| std::cmp::Reverse(item.uid));
    Ok(summaries)
}

pub(super) fn parsed_message(uid: u32, mail: &ParsedMail<'_>) -> Result<EmailMessage> {
    let header = |name: &str| mail.headers.get_first_value(name).unwrap_or_default();
    let mut plain = None;
    let mut html = None;
    let mut attachments = 0;
    collect_parts(mail, &mut plain, &mut html, &mut attachments)?;
    let text = plain
        .or_else(|| html.as_ref().map(|value| strip_html(value)))
        .unwrap_or_default();
    Ok(EmailMessage {
        uid,
        from: header("From"),
        to: header("To"),
        cc: header("Cc"),
        subject: header("Subject"),
        date: header("Date"),
        text,
        html,
        attachment_count: attachments,
    })
}

pub(super) fn collect_parts(
    part: &ParsedMail<'_>,
    plain: &mut Option<String>,
    html: &mut Option<String>,
    attachments: &mut usize,
) -> Result<()> {
    if part.subparts.is_empty() {
        let disposition = part.get_content_disposition();
        if disposition.disposition == mailparse::DispositionType::Attachment {
            *attachments += 1;
            return Ok(());
        }
        match part.ctype.mimetype.as_str() {
            "text/plain" if plain.is_none() => {
                *plain = Some(
                    part.get_body()
                        .map_err(|e| CoreError::Invalid(format!("读取邮件正文失败: {e}")))?,
                );
            }
            "text/html" if html.is_none() => {
                *html = Some(
                    part.get_body()
                        .map_err(|e| CoreError::Invalid(format!("读取邮件正文失败: {e}")))?,
                );
            }
            _ => {}
        }
    } else {
        for child in &part.subparts {
            collect_parts(child, plain, html, attachments)?;
        }
    }
    Ok(())
}

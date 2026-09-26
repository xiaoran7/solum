//! `helpers` -- host and address validation, masking, header decoding, and HTML stripping.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) fn validate_host(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 253
        || value.contains(['/', '\\', '@', ':', ' ', '\r', '\n'])
    {
        return Err(CoreError::Invalid(format!("{label} 主机名无效")));
    }
    Ok(())
}

pub(super) fn validate_mailbox_address(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > 320 || value.contains(['\r', '\n']) || !value.contains('@')
    {
        return Err(CoreError::Invalid(format!("{label}不是有效邮箱地址")));
    }
    Ok(())
}

pub(super) fn mask_tail(value: &str) -> String {
    let tail = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    if tail.is_empty() {
        String::new()
    } else {
        format!("…{tail}")
    }
}

pub(super) fn random_urlsafe(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf)
        .map_err(|e| CoreError::Invalid(format!("生成 OAuth 随机数失败: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

pub(super) fn decode_header(value: &str) -> String {
    mailparse::parse_header(format!("X: {value}\r\n").as_bytes())
        .ok()
        .map(|(header, _)| header.get_value())
        .unwrap_or_else(|| value.to_string())
}

pub(super) fn truncate_for_preview(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut text = value.chars().take(max).collect::<String>();
    text.push_str("\n\n（预览已截断；发送将使用完整正文）");
    text
}

pub(super) fn strip_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;
    for character in input.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

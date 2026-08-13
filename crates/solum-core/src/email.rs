//! Local-only mailbox connector (F21, ARCHITECTURE.md §3.14).
//!
//! This module deliberately has no `Store` dependency: account credentials and
//! mailbox content must never reach SQLite, sync, exports, recall, or the LLM.
//! The app shell owns the short-lived UI projection; this module opens a fresh
//! encrypted IMAP/SMTP connection for each user-initiated operation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use lettre::message::{header::ContentType, Mailbox, Message, SinglePart};
use lettre::transport::smtp::{
    authentication::{Credentials, Mechanism},
    client::{Tls, TlsParameters},
};
use lettre::{SmtpTransport, Transport};
use mailparse::{MailHeaderMap, ParsedMail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::form_urlencoded;

use crate::error::{CoreError, Result};

const MAX_LIST_LIMIT: usize = 100;
const MAX_SEARCH_QUERY_LEN: usize = 160;
const MAX_BODY_BYTES: usize = 1_500_000;

/// The curated setup profile. `Custom` is still TLS-only; plaintext mail
/// protocols are deliberately not an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailProvider {
    Qq,
    Gmail,
    Microsoft,
    Custom,
}

impl EmailProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Qq => "QQ 邮箱",
            Self::Gmail => "Gmail",
            Self::Microsoft => "Microsoft 365 / Outlook",
            Self::Custom => "自定义 IMAP/SMTP",
        }
    }

    fn oauth_endpoints(self, tenant: &str) -> Option<OAuthEndpoints> {
        match self {
            Self::Gmail => Some(OAuthEndpoints {
                authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
                token: "https://oauth2.googleapis.com/token".into(),
                scopes: vec!["https://mail.google.com/".into()],
            }),
            Self::Microsoft => {
                let tenant = if tenant.trim().is_empty() {
                    "common"
                } else {
                    tenant.trim()
                };
                Some(OAuthEndpoints {
                    authorize: format!(
                        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"
                    ),
                    token: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
                    scopes: vec![
                        "offline_access".into(),
                        "https://outlook.office.com/IMAP.AccessAsUser.All".into(),
                        "https://outlook.office.com/SMTP.Send".into(),
                    ],
                })
            }
            Self::Qq | Self::Custom => None,
        }
    }

    pub fn preset(self) -> Option<EmailEndpoints> {
        match self {
            Self::Qq => Some(EmailEndpoints {
                imap_host: "imap.qq.com".into(),
                imap_port: 993,
                smtp_host: "smtp.qq.com".into(),
                smtp_port: 465,
                smtp_tls: SmtpTls::Wrapper,
            }),
            Self::Gmail => Some(EmailEndpoints {
                imap_host: "imap.gmail.com".into(),
                imap_port: 993,
                smtp_host: "smtp.gmail.com".into(),
                smtp_port: 465,
                smtp_tls: SmtpTls::Wrapper,
            }),
            Self::Microsoft => Some(EmailEndpoints {
                imap_host: "outlook.office365.com".into(),
                imap_port: 993,
                smtp_host: "smtp.office365.com".into(),
                smtp_port: 587,
                smtp_tls: SmtpTls::StartTls,
            }),
            Self::Custom => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTls {
    /// SMTPS: TLS before SMTP, normally port 465.
    Wrapper,
    /// SMTP submission: require STARTTLS, normally port 587.
    StartTls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailEndpoints {
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: SmtpTls,
}

/// Stored secret material. It is intentionally separate from the UI DTOs;
/// [`EmailConfig::summaries`] never copies these fields back across IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmailAuth {
    AppPassword {
        secret: String,
    },
    OAuth2 {
        client_id: String,
        #[serde(default)]
        client_secret: String,
        #[serde(default)]
        refresh_token: String,
        /// Microsoft Entra tenant. Empty means `common`; ignored by Google.
        #[serde(default)]
        tenant: String,
    },
}

impl EmailAuth {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AppPassword { .. } => "app_password",
            Self::OAuth2 { .. } => "oauth2",
        }
    }

    fn secret_tail(&self) -> String {
        match self {
            Self::AppPassword { secret } => mask_tail(secret),
            Self::OAuth2 { refresh_token, .. } => mask_tail(refresh_token),
        }
    }

    fn validate_for_use(&self, provider: EmailProvider) -> Result<()> {
        match self {
            Self::AppPassword { secret } if secret.trim().is_empty() => {
                Err(CoreError::Invalid("邮箱授权码不能为空".into()))
            }
            Self::OAuth2 {
                client_id,
                refresh_token,
                ..
            } => {
                if provider.oauth_endpoints("").is_none() {
                    return Err(CoreError::Invalid(
                        "自定义与 QQ 账户目前使用授权码；OAuth 仅支持 Gmail 和 Microsoft".into(),
                    ));
                }
                if client_id.trim().is_empty() || refresh_token.trim().is_empty() {
                    return Err(CoreError::Invalid(
                        "OAuth 尚未完成授权：需要 client id 和 refresh token".into(),
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAccount {
    pub id: String,
    pub label: String,
    pub provider: EmailProvider,
    pub address: String,
    pub endpoints: EmailEndpoints,
    pub auth: EmailAuth,
}

impl EmailAccount {
    pub fn validate(&self, for_use: bool) -> Result<()> {
        if self.id.trim().is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(CoreError::Invalid(
                "邮箱账户 id 只能是 1–64 位字母、数字、_ 或 -".into(),
            ));
        }
        if self.label.trim().is_empty() || self.label.chars().count() > 80 {
            return Err(CoreError::Invalid("邮箱账户名称需要是 1–80 个字符".into()));
        }
        validate_mailbox_address(&self.address, "发件邮箱")?;
        validate_host(&self.endpoints.imap_host, "IMAP")?;
        validate_host(&self.endpoints.smtp_host, "SMTP")?;
        if self.endpoints.imap_port == 0 || self.endpoints.smtp_port == 0 {
            return Err(CoreError::Invalid("IMAP/SMTP 端口必须在 1–65535".into()));
        }
        if for_use {
            self.auth.validate_for_use(self.provider)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EmailConfig {
    #[serde(default)]
    pub accounts: Vec<EmailAccount>,
}

impl EmailConfig {
    /// Environment override mirrors the existing LLM/Soulous local-config
    /// convention. The desktop fallback is deliberately a gitignored file.
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("SOLUM_EMAIL_CONFIG") {
            return p.into();
        }
        crate::paths::resolve_profile_with_adoption("solum-email.json")
    }

    pub fn load() -> Option<Self> {
        let raw = std::fs::read_to_string(Self::path()).ok()?;
        Self::from_json(&raw).ok()
    }

    pub fn from_json(raw: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(raw)
            .map_err(|e| CoreError::Invalid(format!("邮箱配置不是合法 JSON: {e}")))?;
        config.validate(false)?;
        Ok(config)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.validate(false)?;
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Invalid(format!("序列化邮箱配置失败: {e}")))?;
        // Atomic: a truncated mail config makes the account unreadable, and
        // nothing would tell the user the file was damaged rather than empty.
        crate::fsatomic::write_atomic(path, &raw)
            .map_err(|e| CoreError::Email(format!("写入邮箱配置失败: {e}")))
    }

    pub fn validate(&self, for_use: bool) -> Result<()> {
        if self.accounts.len() > 12 {
            return Err(CoreError::Invalid("最多连接 12 个邮箱账户".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for account in &self.accounts {
            account.validate(for_use)?;
            if !ids.insert(account.id.clone()) {
                return Err(CoreError::Invalid(format!(
                    "邮箱账户 id 重复：{}",
                    account.id
                )));
            }
        }
        Ok(())
    }

    pub fn account(&self, id: &str) -> Result<&EmailAccount> {
        self.accounts
            .iter()
            .find(|account| account.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("邮箱账户 {id}")))
    }

    pub fn account_mut(&mut self, id: &str) -> Result<&mut EmailAccount> {
        self.accounts
            .iter_mut()
            .find(|account| account.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("邮箱账户 {id}")))
    }

    pub fn summaries(&self) -> Vec<EmailAccountSummary> {
        self.accounts
            .iter()
            .map(|account| EmailAccountSummary {
                id: account.id.clone(),
                label: account.label.clone(),
                provider: account.provider,
                address: account.address.clone(),
                auth_kind: account.auth.kind().into(),
                secret_tail: account.auth.secret_tail(),
                oauth_ready: matches!(&account.auth, EmailAuth::OAuth2 { refresh_token, .. } if !refresh_token.trim().is_empty()),
                imap_host: account.endpoints.imap_host.clone(),
                imap_port: account.endpoints.imap_port,
                smtp_host: account.endpoints.smtp_host.clone(),
                smtp_port: account.endpoints.smtp_port,
                smtp_tls: account.endpoints.smtp_tls.clone(),
                client_id: match &account.auth { EmailAuth::OAuth2 { client_id, .. } => client_id.clone(), _ => String::new() },
                tenant: match &account.auth { EmailAuth::OAuth2 { tenant, .. } => tenant.clone(), _ => String::new() },
            })
            .collect()
    }
}

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

#[derive(Debug, Clone)]
struct OAuthEndpoints {
    authorize: String,
    token: String,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStart {
    pub authorization_url: String,
    pub state: String,
    pub code_verifier: String,
}

/// Build a PKCE authorization URL. The shell binds the loopback listener and
/// retains the returned verifier/state in memory only.
pub fn oauth_start(account: &EmailAccount, redirect_uri: &str) -> Result<OAuthStart> {
    let EmailAuth::OAuth2 {
        client_id, tenant, ..
    } = &account.auth
    else {
        return Err(CoreError::Invalid("该账户没有选择 OAuth2".into()));
    };
    if client_id.trim().is_empty() {
        return Err(CoreError::Invalid("请先保存 OAuth client id".into()));
    }
    let endpoints = account
        .provider
        .oauth_endpoints(tenant)
        .ok_or_else(|| CoreError::Invalid("此账户类型不支持 OAuth2".into()))?;
    let state = random_urlsafe(24)?;
    let verifier = random_urlsafe(48)?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut serializer = form_urlencoded::Serializer::new(endpoints.authorize);
    serializer.append_pair("response_type", "code");
    serializer.append_pair("client_id", client_id.trim());
    serializer.append_pair("redirect_uri", redirect_uri);
    serializer.append_pair("scope", &endpoints.scopes.join(" "));
    serializer.append_pair("state", &state);
    serializer.append_pair("code_challenge", &challenge);
    serializer.append_pair("code_challenge_method", "S256");
    serializer.append_pair("access_type", "offline");
    serializer.append_pair("prompt", "consent");
    Ok(OAuthStart {
        authorization_url: serializer.finish(),
        state,
        code_verifier: verifier,
    })
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
}

/// Finish an Authorization Code + PKCE exchange and persist only the refresh
/// token. The caller must compare `state` before reaching this function.
pub fn oauth_finish(
    config: &mut EmailConfig,
    account_id: &str,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Result<()> {
    let account = config.account(account_id)?.clone();
    let EmailAuth::OAuth2 {
        client_id,
        client_secret,
        tenant,
        refresh_token: old_refresh,
    } = account.auth
    else {
        return Err(CoreError::Invalid("该账户没有选择 OAuth2".into()));
    };
    let endpoints = account
        .provider
        .oauth_endpoints(&tenant)
        .ok_or_else(|| CoreError::Invalid("此账户类型不支持 OAuth2".into()))?;
    let mut form = HashMap::new();
    form.insert("grant_type", "authorization_code");
    form.insert("client_id", client_id.as_str());
    form.insert("code", code.trim());
    form.insert("redirect_uri", redirect_uri);
    form.insert("code_verifier", code_verifier);
    if !client_secret.trim().is_empty() {
        form.insert("client_secret", client_secret.trim());
    }
    let token = post_oauth_form(&endpoints.token, &form)?;
    if token.refresh_token.trim().is_empty() && old_refresh.trim().is_empty() {
        return Err(CoreError::Invalid(
            "授权服务没有返回 refresh token；请确认已授予离线访问权限".into(),
        ));
    }
    let _ = token.access_token; // never persist access tokens
    let target = config.account_mut(account_id)?;
    if let EmailAuth::OAuth2 { refresh_token, .. } = &mut target.auth {
        *refresh_token = if token.refresh_token.trim().is_empty() {
            old_refresh
        } else {
            token.refresh_token
        };
    }
    Ok(())
}

fn oauth_access_token(account: &EmailAccount) -> Result<String> {
    let EmailAuth::OAuth2 {
        client_id,
        client_secret,
        refresh_token,
        tenant,
    } = &account.auth
    else {
        return Err(CoreError::Invalid("账户没有 OAuth2 凭据".into()));
    };
    let endpoints = account
        .provider
        .oauth_endpoints(tenant)
        .ok_or_else(|| CoreError::Invalid("此账户类型不支持 OAuth2".into()))?;
    let mut form = HashMap::new();
    form.insert("grant_type", "refresh_token");
    form.insert("client_id", client_id.trim());
    form.insert("refresh_token", refresh_token.trim());
    if !client_secret.trim().is_empty() {
        form.insert("client_secret", client_secret.trim());
    }
    Ok(post_oauth_form(&endpoints.token, &form)?.access_token)
}

fn post_oauth_form(url: &str, form: &HashMap<&str, &str>) -> Result<OAuthTokenResponse> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in form {
        serializer.append_pair(key, value);
    }
    ureq::post(url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send_string(&serializer.finish())
        .map_err(|e| CoreError::Email(format!("OAuth token 请求失败: {e}")))?
        .into_json()
        .map_err(|e| CoreError::Invalid(format!("OAuth token 响应无效: {e}")))
}

struct XOAuth2 {
    response: Vec<u8>,
}

impl imap::Authenticator for XOAuth2 {
    type Response = Vec<u8>;

    fn process(&self, _challenge: &[u8]) -> Self::Response {
        self.response.clone()
    }
}

// imap 3.x 把连接收敛成 ClientBuilder + 后端无关的 `Connection`（本仓编译期选定
// rustls-tls，见 workspace Cargo.toml 的注释：native-tls 会拖进 openssl-sys，Android
// 上没有系统 OpenSSL）。会话类型因此不再拿具体的 TlsStream 参数化。
type ImapSession = imap::Session<imap::Connection>;

fn connect(account: &EmailAccount) -> Result<ImapSession> {
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

fn list_messages_session(
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

fn fetch_summaries_by_uid(
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
fn summaries_from_fetches(fetches: &imap::types::Fetches) -> Result<Vec<EmailSummary>> {
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

fn parsed_message(uid: u32, mail: &ParsedMail<'_>) -> Result<EmailMessage> {
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

fn collect_parts(
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

pub fn send(account: &EmailAccount, draft: &EmailSendDraft) -> Result<String> {
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

fn validate_host(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 253
        || value.contains(['/', '\\', '@', ':', ' ', '\r', '\n'])
    {
        return Err(CoreError::Invalid(format!("{label} 主机名无效")));
    }
    Ok(())
}

fn validate_mailbox_address(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > 320 || value.contains(['\r', '\n']) || !value.contains('@')
    {
        return Err(CoreError::Invalid(format!("{label}不是有效邮箱地址")));
    }
    Ok(())
}

fn mask_tail(value: &str) -> String {
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

fn random_urlsafe(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf)
        .map_err(|e| CoreError::Invalid(format!("生成 OAuth 随机数失败: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn decode_header(value: &str) -> String {
    mailparse::parse_header(format!("X: {value}\r\n").as_bytes())
        .ok()
        .map(|(header, _)| header.get_value())
        .unwrap_or_else(|| value.to_string())
}

fn truncate_for_preview(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut text = value.chars().take(max).collect::<String>();
    text.push_str("\n\n（预览已截断；发送将使用完整正文）");
    text
}

fn strip_html(input: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(provider: EmailProvider, auth: EmailAuth) -> EmailAccount {
        EmailAccount {
            id: "work-mail".into(),
            label: "工作邮箱".into(),
            provider,
            address: "me@example.com".into(),
            endpoints: provider.preset().unwrap_or(EmailEndpoints {
                imap_host: "imap.example.com".into(),
                imap_port: 993,
                smtp_host: "smtp.example.com".into(),
                smtp_port: 465,
                smtp_tls: SmtpTls::Wrapper,
            }),
            auth,
        }
    }

    #[test]
    fn provider_presets_are_tls_only_and_have_expected_hosts() {
        let qq = EmailProvider::Qq.preset().unwrap();
        assert_eq!(qq.imap_host, "imap.qq.com");
        assert_eq!(qq.smtp_port, 465);
        let gmail = EmailProvider::Gmail.preset().unwrap();
        assert_eq!(gmail.imap_host, "imap.gmail.com");
        let microsoft = EmailProvider::Microsoft.preset().unwrap();
        assert_eq!(microsoft.smtp_host, "smtp.office365.com");
        assert_eq!(microsoft.smtp_tls, SmtpTls::StartTls);
    }

    #[test]
    fn summaries_never_include_secrets() {
        let cfg = EmailConfig {
            accounts: vec![account(
                EmailProvider::Qq,
                EmailAuth::AppPassword {
                    secret: "secret-9999".into(),
                },
            )],
        };
        let json = serde_json::to_string(&cfg.summaries()).unwrap();
        assert!(json.contains("…9999"));
        assert!(!json.contains("secret-9999"));
    }

    #[test]
    fn mail_send_audit_summary_excludes_addresses_and_content() {
        let draft = EmailSendDraft {
            account_id: "work-mail".into(),
            to: vec!["private@example.com".into()],
            cc: vec![],
            bcc: vec![],
            subject: "秘密主题".into(),
            body: "绝不能写进审计库的正文".into(),
            html: false,
        };
        let audit = draft.audit_summary();
        assert!(audit.contains("work-mail"));
        assert!(!audit.contains("private@example.com"));
        assert!(!audit.contains("秘密主题"));
        assert!(!audit.contains("正文"));
    }

    #[test]
    fn oauth_url_has_pkce_and_provider_scope() {
        let account = account(
            EmailProvider::Gmail,
            EmailAuth::OAuth2 {
                client_id: "client-id".into(),
                client_secret: String::new(),
                refresh_token: String::new(),
                tenant: String::new(),
            },
        );
        let start = oauth_start(&account, "http://127.0.0.1:12345/callback").unwrap();
        assert!(start
            .authorization_url
            .contains("code_challenge_method=S256"));
        assert!(start.authorization_url.contains("mail.google.com"));
        assert!(start.code_verifier.len() >= 43);
    }
}

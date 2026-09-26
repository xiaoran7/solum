//! `config` -- the provider/TLS/auth enums and the stored account and config shapes.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::helpers::{mask_tail, validate_host, validate_mailbox_address};
use super::oauth::OAuthEndpoints;
use super::*;

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

    pub(super) fn oauth_endpoints(self, tenant: &str) -> Option<OAuthEndpoints> {
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

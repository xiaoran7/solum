//! `tests` -- the suite for all of the above.
//!
//! Part of the `email` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

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

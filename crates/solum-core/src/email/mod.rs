//! Local-only mailbox connector (F21, ARCHITECTURE.md §3.14).
//!
//! This module deliberately has no `Store` dependency: account credentials and
//! mailbox content must never reach SQLite, sync, exports, recall, or the LLM.
//! The app shell owns the short-lived UI projection; this module opens a fresh
//! encrypted IMAP/SMTP connection for each user-initiated operation.
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

mod config;
mod helpers;
mod mail;
mod oauth;
mod smtp;
#[cfg(test)]
mod tests;
mod types;

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

// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use config::{EmailAccount, EmailAuth, EmailConfig, EmailEndpoints, EmailProvider, SmtpTls};
pub use mail::{get_message, list_folders, list_messages, search_messages};
pub use oauth::{oauth_finish, oauth_start, OAuthStart};
pub use smtp::{preview_configured, send_configured};
pub use types::{EmailAccountSummary, EmailFolder, EmailMessage, EmailSendDraft, EmailSummary};

const MAX_LIST_LIMIT: usize = 100;
const MAX_SEARCH_QUERY_LEN: usize = 160;
const MAX_BODY_BYTES: usize = 1_500_000;

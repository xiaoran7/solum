//! http. Part of the `account` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `account.rs`.

use super::*;

/// HTTP seam so login/refresh/retry logic is testable without a socket
/// (same shape as [`crate::soulous::SoulousHttp`]).
pub trait AccountHttp {
    fn post_json(
        &self,
        url: &str,
        access_token: Option<&str>,
        body: Value,
    ) -> std::result::Result<Value, AccountHttpError>;

    fn get_json(
        &self,
        _url: &str,
        _access_token: &str,
    ) -> std::result::Result<Value, AccountHttpError> {
        Err(AccountHttpError::Other(
            "当前 HTTP 实现不支持读取账号状态".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountHttpError {
    Unauthorized,
    Retryable(String),
    Other(String),
}

impl std::fmt::Display for AccountHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountHttpError::Unauthorized => f.write_str("认证已过期"),
            AccountHttpError::Retryable(message) => f.write_str(message),
            AccountHttpError::Other(message) => f.write_str(message),
        }
    }
}

pub struct HttpAccountClient {
    agent: ureq::Agent,
}

impl HttpAccountClient {
    pub fn new() -> Self {
        Self::with_timeout(TIMEOUT_SECS)
    }

    pub(super) fn with_timeout(timeout_secs: u64) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build(),
        }
    }
}

impl Default for HttpAccountClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountHttp for HttpAccountClient {
    fn post_json(
        &self,
        url: &str,
        access_token: Option<&str>,
        body: Value,
    ) -> std::result::Result<Value, AccountHttpError> {
        let mut request = self.agent.post(url);
        if let Some(token) = access_token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let response = request.send_json(body).map_err(map_http_error)?;
        response
            .into_json()
            .map_err(|e| AccountHttpError::Retryable(format!("响应不是 JSON: {e}")))
    }

    fn get_json(
        &self,
        url: &str,
        access_token: &str,
    ) -> std::result::Result<Value, AccountHttpError> {
        let mut request = self.agent.get(url);
        if !access_token.is_empty() {
            request = request.set("Authorization", &format!("Bearer {access_token}"));
        }
        request
            .call()
            .map_err(map_http_error)?
            .into_json()
            .map_err(|e| AccountHttpError::Retryable(format!("响应不是 JSON: {e}")))
    }
}

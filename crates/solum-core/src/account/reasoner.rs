//! reasoner. Part of the `account` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `account.rs`.

use super::*;

/// A [`Reasoner`] backed by the account proxy instead of a direct vendor key.
/// Interior mutability because token rotation happens mid-`&self` call.
pub struct AccountReasoner {
    session: Mutex<AccountSession>,
    agent: ureq::Agent,
}

impl AccountReasoner {
    pub fn new(session: AccountSession) -> Self {
        Self {
            session: Mutex::new(session),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .build(),
        }
    }

    pub fn masked_summary(&self) -> String {
        self.session
            .lock()
            .map(|s| s.masked_summary())
            .unwrap_or_else(|_| "账号状态不可用".to_string())
    }

    fn chat_body(session: &AccountSession, system: &str, user: &str, stream: bool) -> Value {
        json!({
            "model": session.model,
            "stream": stream,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        })
    }

    fn tool_chat_body(
        session: &AccountSession,
        system: &str,
        user: &str,
        tools: &[Value],
    ) -> Value {
        json!({
            "model": session.model,
            "stream": false,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "tools": tools,
            "tool_choice": "auto",
        })
    }

    /// Non-streaming completion via the seam (unit-tested with a fake client).
    pub fn complete_with_client<T: AccountHttp>(
        client: &T,
        session: &mut AccountSession,
        system: &str,
        user: &str,
    ) -> Result<String> {
        let body = Self::chat_body(session, system, user, false);
        let response = post_with_refresh(client, session, "/v1/ai/chat/completions", body)?;
        crate::llm::parse_chat_content(&response)
    }

    /// Send the streaming request once, mapping a 401 to `Unauthorized` so the
    /// caller can refresh and retry (ureq surfaces the status before the SSE
    /// body starts, so auth failures never look like broken streams).
    fn send_streaming(
        &self,
        session: &AccountSession,
        system: &str,
        user: &str,
    ) -> std::result::Result<ureq::Response, AccountHttpError> {
        self.agent
            .post(&format!("{}/v1/ai/chat/completions", session.server_url))
            .set("Authorization", &format!("Bearer {}", session.access_token))
            .set("Accept", "text/event-stream")
            .send_json(Self::chat_body(session, system, user, true))
            .map_err(map_http_error)
    }
}

impl Reasoner for AccountReasoner {
    fn complete(&self, system: &str, user: &str) -> Result<String> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Llm("账号状态锁中毒".into()))?;
        let client = HttpAccountClient::new();
        Self::complete_with_client(&client, &mut session, system, user)
    }

    fn complete_with_tools(
        &self,
        system: &str,
        user: &str,
        tools: &[Value],
    ) -> Result<crate::extract::ReasonerResponse> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Llm("账号状态锁中毒".into()))?;
        let body = Self::tool_chat_body(&session, system, user, tools);
        let client = HttpAccountClient::new();
        let response = post_with_refresh(&client, &mut session, "/v1/ai/chat/completions", body)?;
        crate::llm::parse_reasoner_response(&response)
    }

    /// Streaming with the same think-stripping guarantees as
    /// [`crate::llm::LlmReasoner::complete_streaming`], plus the account
    /// layer's refresh-once-on-401 semantics.
    fn complete_streaming(
        &self,
        system: &str,
        user: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<String> {
        use std::io::BufRead;
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Llm("账号状态锁中毒".into()))?;
        let response = match self.send_streaming(&session, system, user) {
            Ok(response) => response,
            Err(AccountHttpError::Unauthorized) => {
                let client = HttpAccountClient::new();
                let rotated = refresh_and_save_with_client(&client, &session)?;
                *session = rotated;
                self.send_streaming(&session, system, user)
                    .map_err(|e| CoreError::Llm(format!("刷新后重试失败: {e}")))?
            }
            Err(error) => return Err(CoreError::Llm(format!("请求失败: {error}"))),
        };
        drop(session);
        let reader = std::io::BufReader::new(response.into_reader());
        let mut full = String::new();
        let mut filter = crate::llm::ThinkFilter::default();
        for line in reader.lines() {
            let line = line.map_err(|e| CoreError::Llm(format!("流式读取中断: {e}")))?;
            match crate::llm::sse_line(&line) {
                crate::llm::SseLine::Done => break,
                crate::llm::SseLine::Ignore => {}
                crate::llm::SseLine::Content(piece) => {
                    full.push_str(&piece);
                    let visible = filter.push(&piece);
                    if !visible.is_empty() {
                        on_token(&visible);
                    }
                }
            }
        }
        let out = crate::llm::strip_think_block(&full).trim().to_string();
        if out.is_empty() {
            return Err(CoreError::Llm(format!("流式响应为空: {full:?}")));
        }
        Ok(out)
    }
}

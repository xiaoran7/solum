//! reasoner. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

use super::*;

/// A [`Reasoner`] backed by an OpenAI-compatible chat endpoint.
pub struct LlmReasoner {
    config: LlmConfig,
    agent: ureq::Agent,
}

impl LlmReasoner {
    pub fn new(config: LlmConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(config.timeout_secs.max(1)))
            .build();
        LlmReasoner { config, agent }
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }
}

impl Reasoner for LlmReasoner {
    fn complete(&self, system: &str, user: &str) -> Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        // OpenAI gpt-5-family 400s on any non-default temperature — a config
        // of `null` keeps the field out of the request entirely.
        if let Some(t) = self.config.temperature {
            body["temperature"] = t.into();
        }
        if let Some(m) = self.config.max_tokens {
            body["max_tokens"] = m.into();
        }
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.config.api_key))
            .send_json(body)
            .map_err(|e| CoreError::Llm(format!("请求失败: {e}")))?;
        let json: serde_json::Value = resp
            .into_json()
            .map_err(|e| CoreError::Llm(format!("响应不是 JSON: {e}")))?;
        parse_chat_content(&json)
    }

    fn complete_with_tools(
        &self,
        system: &str,
        user: &str,
        tools: &[serde_json::Value],
    ) -> Result<ReasonerResponse> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "tools": tools,
            "tool_choice": "auto",
        });
        if let Some(t) = self.config.temperature {
            body["temperature"] = t.into();
        }
        if let Some(m) = self.config.max_tokens {
            body["max_tokens"] = m.into();
        }
        let response = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.config.api_key))
            .send_json(body)
            .map_err(|error| CoreError::Llm(format!("请求失败: {error}")))?;
        let json: serde_json::Value = response
            .into_json()
            .map_err(|error| CoreError::Llm(format!("响应不是 JSON: {error}")))?;
        parse_reasoner_response(&json)
    }

    /// Streaming variant (§3.6 第 7 条): request `stream:true` and read the
    /// SSE body line by line, forwarding each visible content delta to
    /// `on_token`. Only `choices[0].delta.content` is read — chain-of-thought
    /// (`reasoning_content`, or an inlined leading `<think>` block) never
    /// reaches the caller, matching [`Self::complete`]'s think-stripping.
    /// The returned string is the full think-stripped reply.
    fn complete_streaming(
        &self,
        system: &str,
        user: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<String> {
        use std::io::BufRead;
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut body = serde_json::json!({
            "model": self.config.model,
            "stream": true,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        if let Some(t) = self.config.temperature {
            body["temperature"] = t.into();
        }
        if let Some(m) = self.config.max_tokens {
            body["max_tokens"] = m.into();
        }
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.config.api_key))
            .set("Accept", "text/event-stream")
            .send_json(body)
            .map_err(|e| CoreError::Llm(format!("请求失败: {e}")))?;
        let reader = std::io::BufReader::new(resp.into_reader());
        let mut full = String::new();
        let mut filter = ThinkFilter::default();
        for line in reader.lines() {
            let line = line.map_err(|e| CoreError::Llm(format!("流式读取中断: {e}")))?;
            match sse_line(&line) {
                SseLine::Done => break,
                SseLine::Ignore => {}
                SseLine::Content(piece) => {
                    full.push_str(&piece);
                    let visible = filter.push(&piece);
                    if !visible.is_empty() {
                        on_token(&visible);
                    }
                }
            }
        }
        let out = strip_think_block(&full).trim().to_string();
        if out.is_empty() {
            return Err(CoreError::Llm(format!("流式响应为空: {full:?}")));
        }
        Ok(out)
    }
}

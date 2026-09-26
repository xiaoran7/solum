//! streaming. Part of the `llm` module; see `mod.rs` for the module contract.
//!
//! Moved verbatim from the single-file `llm.rs`.

pub(crate) fn strip_think_block(s: &str) -> &str {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            return &rest[end + "</think>".len()..];
        }
    }
    s
}

// ---- SSE streaming helpers --------------------------------------------------

/// One classified line of an OpenAI-compatible `stream:true` SSE body.
pub(crate) enum SseLine {
    /// A `data: [DONE]` terminator.
    Done,
    /// A content delta (`choices[0].delta.content`).
    Content(String),
    /// Blank lines, `event:`/comment lines, keep-alives, deltas without text
    /// content (role-only openers, `reasoning_content`-only thinking chunks).
    Ignore,
}

/// Classify one raw SSE line. Pure so it can be unit-tested without a socket.
/// `reasoning_content` is deliberately never read — chain-of-thought stays out
/// of the process, same guarantee as [`parse_chat_content`].
pub(crate) fn sse_line(line: &str) -> SseLine {
    let Some(data) = line.strip_prefix("data:") else {
        return SseLine::Ignore;
    };
    let data = data.trim();
    if data.is_empty() {
        return SseLine::Ignore;
    }
    if data == "[DONE]" {
        return SseLine::Done;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
        return SseLine::Ignore;
    };
    match v["choices"][0]["delta"]["content"].as_str() {
        Some(s) if !s.is_empty() => SseLine::Content(s.to_string()),
        _ => SseLine::Ignore,
    }
}

/// Streams content deltas while dropping a *leading* inlined `<think>…</think>`
/// block, handling the tags being split across delta boundaries. Once past any
/// leading think region it becomes a pass-through. Mirrors [`strip_think_block`]
/// but incrementally (that fn still runs on the full accumulation for the
/// returned value; this only shapes what the caller *sees* live).
#[derive(Default)]
pub(crate) struct ThinkFilter {
    /// Content buffered while we can't yet decide think-vs-content.
    buf: String,
    /// Once true, everything is forwarded verbatim.
    passthrough: bool,
}

impl ThinkFilter {
    /// Feed one content piece; return the portion to emit now (may be empty).
    pub(crate) fn push(&mut self, piece: &str) -> String {
        if self.passthrough {
            return piece.to_string();
        }
        self.buf.push_str(piece);
        let trimmed = self.buf.trim_start();
        // Still possibly the opening `<think>` tag, incomplete → wait.
        if !trimmed.is_empty() && trimmed.len() < "<think>".len() && "<think>".starts_with(trimmed)
        {
            return String::new();
        }
        // A confirmed leading `<think>` block: emit only what follows its close.
        if let Some(rest) = trimmed.strip_prefix("<think>") {
            let Some(end) = rest.find("</think>") else {
                return String::new(); // close tag not here yet
            };
            let after = rest[end + "</think>".len()..].to_string();
            self.passthrough = true;
            self.buf.clear();
            return after;
        }
        // Only whitespace so far → can't tell think from content → wait.
        if trimmed.is_empty() {
            return String::new();
        }
        // Definitely ordinary content → flush the buffer and pass through.
        self.passthrough = true;
        std::mem::take(&mut self.buf)
    }
}

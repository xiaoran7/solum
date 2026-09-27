//! `draft` -- one pending item and the clues pulled out of it. The clues are all `Option` on purpose — not-found is not-found, and this layer never guesses. **Not** a substitute for `crate::extract`'s event parsing; it only produces something for a human to check against.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::classify::{CandidateState, CaptureSensitivity};
use super::*;

/// 待确认的一条外部输入。**没有进数据库**——它只活在进程内存里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureDraft {
    pub id: String,
    pub source: CaptureSource,
    pub title: String,
    pub text: String,
    /// Unix 毫秒。
    pub created_at: i64,
    pub occurred_at: i64,
    /// 来源侧稳定 id；没有就不编造。
    pub external_id: Option<String>,
    pub evidence_ref: Option<String>,
    /// 同一正文被多个入口看见时保留全部来源，避免重复候选。
    pub also_seen_from: Vec<CaptureSource>,
    /// 0..=1000。候选阶段只表达解析确信度，不代表真实性。
    pub confidence: u16,
    pub sensitivity: CaptureSensitivity,
    pub expires_at: Option<i64>,
    pub allow_memory: bool,
    pub allow_ai: bool,
    pub allow_backup: bool,
    pub proposed_action: &'static str,
    pub state: CandidateState,
    pub note: Option<String>,
}

/// 抽出来给人核对的线索。字段全是 `Option`——抽不到就是抽不到，不编。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CaptureClues {
    pub matter: Option<String>,
    pub time: Option<String>,
    pub location: Option<String>,
    pub amount: Option<String>,
}

impl CaptureClues {
    pub fn is_empty(&self) -> bool {
        self.matter.is_none()
            && self.time.is_none()
            && self.location.is_none()
            && self.amount.is_none()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = &self.matter {
            parts.push(format!("事项：{v}"));
        }
        if let Some(v) = &self.time {
            parts.push(format!("时间：{v}"));
        }
        if let Some(v) = &self.location {
            parts.push(format!("地点：{v}"));
        }
        if let Some(v) = &self.amount {
            parts.push(format!("金额：{v}"));
        }
        if parts.is_empty() {
            "暂未提取到明确线索，可先编辑原文再交给息壤。".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

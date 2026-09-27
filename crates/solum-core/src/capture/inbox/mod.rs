//! `inbox` -- the in-process queue of pending drafts. Receiving a share is not saving it: nothing reaches disk until the user adopts it in the capture center, a mis-tap can be discarded with no invisible copy left behind, and the whole thing dies with the process — deliberate, because a pending area is not a backup.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::classify::CandidateState;
use super::*;

/// 进程内的待确认队列。
///
/// 收到分享**不等于**永久保存：只有用户在采集中心采用后，壳层才把内容送进
/// 既有 ingest 管道。误点分享目标时可以直接丢弃，磁盘上不留隐形副本；
/// 进程退出即整体消失，这是有意的——待确认区不是收件备份。
#[derive(Debug, Default)]
pub struct CaptureInbox {
    drafts: Vec<CaptureDraft>,
    sequence: u64,
}

/// 单条草稿的正文上限：待确认区是给人扫一眼的，不是文档仓库。
/// 超长输入截断而不是拒绝——拒绝会让用户以为分享失败。
pub const MAX_DRAFT_CHARS: usize = 4000;
/// 队列上限，防止某个producer 出问题时把内存吃光。超限丢最旧的。
pub const MAX_DRAFTS: usize = 50;

impl CaptureInbox {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_id(&mut self, now_ms: i64) -> String {
        self.sequence += 1;
        format!("capture-{now_ms}-{}", self.sequence)
    }

    /// 收下一条外部输入，返回落好 id 的草稿。
    ///
    /// `title` 留空时用线索里的"事项"兜底，再兜底成入口名——待确认区每条都得
    /// 有个能认出来的抬头。
    pub fn push(
        &mut self,
        source: CaptureSource,
        title: &str,
        text: &str,
        now_ms: i64,
    ) -> CaptureDraft {
        self.push_with_external_id(source, title, text, None, now_ms)
    }

    pub fn push_with_external_id(
        &mut self,
        source: CaptureSource,
        title: &str,
        text: &str,
        external_id: Option<&str>,
        now_ms: i64,
    ) -> CaptureDraft {
        self.push_candidate(source, title, text, external_id, now_ms, now_ms)
    }

    pub fn push_candidate(
        &mut self,
        source: CaptureSource,
        title: &str,
        text: &str,
        external_id: Option<&str>,
        occurred_at: i64,
        now_ms: i64,
    ) -> CaptureDraft {
        let text = truncate_chars(text.trim(), MAX_DRAFT_CHARS);
        let normalized = normalize_for_dedup(&text);
        if let Some(existing) = self.drafts.iter_mut().find(|draft| {
            let same_external = external_id.is_some()
                && draft.source == source
                && draft.external_id.as_deref() == external_id;
            let recent_same_text = now_ms.saturating_sub(draft.created_at) <= 48 * 60 * 60 * 1000
                && normalize_for_dedup(&draft.text) == normalized;
            same_external || recent_same_text
        }) {
            if existing.source != source && !existing.also_seen_from.contains(&source) {
                existing.also_seen_from.push(source);
            }
            existing.created_at = now_ms;
            existing.note = Some("已与重复候选合并".into());
            return existing.clone();
        }
        let clues = extract_capture_clues(&text);
        let title = {
            let t = title.trim();
            if !t.is_empty() {
                truncate_chars(t, 48)
            } else {
                clues
                    .matter
                    .clone()
                    .unwrap_or_else(|| capture_source_label(source).to_string())
            }
        };
        let draft = CaptureDraft {
            id: self.next_id(now_ms),
            source,
            title,
            text,
            created_at: now_ms,
            occurred_at,
            external_id: external_id.map(|value| truncate_chars(value.trim(), 160)),
            evidence_ref: external_id
                .map(|value| format!("{}:{}", source.as_str(), truncate_chars(value.trim(), 160))),
            also_seen_from: Vec::new(),
            confidence: match source {
                CaptureSource::Manual => 1000,
                CaptureSource::Calendar | CaptureSource::Health | CaptureSource::Soulous => 900,
                CaptureSource::Email => 800,
                CaptureSource::Share => 750,
                CaptureSource::Notification => 650,
                CaptureSource::Location => 600,
            },
            sensitivity: source.sensitivity(),
            expires_at: (source != CaptureSource::Manual)
                .then_some(now_ms + 7 * 24 * 60 * 60 * 1000),
            allow_memory: true,
            allow_ai: !matches!(
                source,
                CaptureSource::Notification
                    | CaptureSource::Calendar
                    | CaptureSource::Health
                    | CaptureSource::Email
                    | CaptureSource::Soulous
                    | CaptureSource::Location
            ),
            allow_backup: source != CaptureSource::Location,
            proposed_action: if clues.time.is_some() {
                "create_event"
            } else {
                "review"
            },
            state: CandidateState::Pending,
            note: None,
        };
        self.drafts.push(draft.clone());
        if self.drafts.len() > MAX_DRAFTS {
            self.drafts.remove(0);
        }
        draft
    }

    /// 最新的排前面——待确认区要先看到刚进来的那条。
    pub fn snapshot(&self) -> Vec<CaptureDraft> {
        let mut out = self.drafts.clone();
        out.reverse();
        out
    }

    pub fn prune_expired(&mut self, now_ms: i64) -> usize {
        let before = self.drafts.len();
        self.drafts
            .retain(|draft| draft.expires_at.is_none_or(|expiry| expiry > now_ms));
        before - self.drafts.len()
    }

    pub fn get(&self, id: &str) -> Option<&CaptureDraft> {
        self.drafts.iter().find(|d| d.id == id)
    }

    /// 返回是否真的删掉了——壳层据此区分「已丢弃」和「这条早没了」。
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.drafts.len();
        self.drafts.retain(|d| d.id != id);
        self.drafts.len() != before
    }

    pub fn clear(&mut self) {
        self.drafts.clear();
    }

    pub fn len(&self) -> usize {
        self.drafts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn normalize_for_dedup(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

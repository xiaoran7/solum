//! 多入口采集的纯领域层（自 Solum Harmony `core/capture.ets` 回移）。
//!
//! 本模块不碰数据库、不碰网络、不依赖壳层：它只负责三件事——**入口目录**
//! （哪些采集通道存在、各自什么状态）、**待确认草稿队列**，以及给人核对用的
//! **线索抽取**。外部文本先进 [`CaptureInbox`]，用户在界面上确认后才交给既有的
//! intent/extract/store 管道。收到不等于保存，这是这层存在的全部理由。
//!
//! **与鸿蒙版的两处壳层差异**：
//!
//! 1. **入口清单按平台真实能力给状态。** Android 有通知捕获、文字/图片系统分享、
//!    打包的端侧截图 OCR 与系统语音识别入口；HarmonyOS 没有第三方通知监听，但有
//!    系统分享与端侧 OCR。[`capture_connectors`] 只描述当前壳层真正接通的能力，
//!    不用一端的限制覆盖另一端。
//!    鸿蒙清单里的「桌面 Agent」在本仓不列——本仓自己就是那个桌面端。
//! 2. **收件箱不是全局静态类。** 鸿蒙用 ArkUI 静态类 + 监听器回调；本仓由壳层
//!    在 `AppState` 里持有一个实例，界面按需读取，不需要发布订阅。
//!
//! 线索抽取**不替代** [`crate::extract`] 的正式事件解析：它只抽「给人看一眼好核对」
//! 的片段，即使看起来很完整，也仍要用户点确认后才走正式管道。

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

macro_rules! lazy_re {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex compiles"))
    }};
}

/// 采集入口的类型学。列不列进 [`capture_connectors`] 是另一回事（见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    Manual,
    Notification,
    Share,
    Calendar,
    Health,
    Email,
    Soulous,
    Location,
}

impl CaptureSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Notification => "notification",
            Self::Share => "share",
            Self::Calendar => "calendar",
            Self::Health => "health",
            Self::Email => "email",
            Self::Soulous => "soulous",
            Self::Location => "location",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "manual" | "conversation" => Self::Manual,
            "notification" => Self::Notification,
            "share" | "system_share" => Self::Share,
            "calendar" => Self::Calendar,
            "health" => Self::Health,
            "email" => Self::Email,
            "soulous" | "official_api" => Self::Soulous,
            "location" => Self::Location,
            _ => return None,
        })
    }

    pub fn allows_ai_by_default(self) -> bool {
        matches!(self, Self::Manual | Self::Share)
    }

    pub fn sensitivity(self) -> CaptureSensitivity {
        match self {
            Self::Manual | Self::Share => CaptureSensitivity::ContentDependent,
            Self::Notification | Self::Calendar | Self::Soulous => CaptureSensitivity::High,
            Self::Health | Self::Email | Self::Location => CaptureSensitivity::VeryHigh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSensitivity {
    ContentDependent,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Pending,
    Accepted,
    Dismissed,
    Merged,
    Expired,
}

/// 入口当前**在这台设备上**能不能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureAvailability {
    /// 现在就能用。
    Ready,
    /// 机制成立，但本仓还没接这条线。
    Connector,
    /// 需要另一端配合（浏览器扩展等）。
    Companion,
    /// 这个平台上不可能有。
    Unsupported,
}

impl CaptureAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Connector => "connector",
            Self::Companion => "companion",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformSupport {
    Available,
    PermissionRequired,
    ConfigurationRequired,
    Planned,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CapturePlatformMatrix {
    pub android: PlatformSupport,
    pub ios: PlatformSupport,
    pub harmonyos: PlatformSupport,
}

fn platform_matrix(source: CaptureSource) -> CapturePlatformMatrix {
    use PlatformSupport::{Available, ConfigurationRequired, PermissionRequired, Planned};
    match source {
        CaptureSource::Manual => CapturePlatformMatrix {
            android: Available,
            ios: Available,
            harmonyos: Available,
        },
        CaptureSource::Notification => CapturePlatformMatrix {
            android: PermissionRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Share => CapturePlatformMatrix {
            android: Available,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Calendar | CaptureSource::Health => CapturePlatformMatrix {
            android: PermissionRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Email => CapturePlatformMatrix {
            android: ConfigurationRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Soulous => CapturePlatformMatrix {
            android: ConfigurationRequired,
            ios: Planned,
            harmonyos: Planned,
        },
        CaptureSource::Location => CapturePlatformMatrix {
            android: Planned,
            ios: Planned,
            harmonyos: Planned,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureConnector {
    pub source: CaptureSource,
    pub label: &'static str,
    pub availability: CaptureAvailability,
    /// 一句话状态，直接上屏。
    pub status: &'static str,
    pub description: &'static str,
    /// 来源内容的默认敏感度，只用于解释和默认开关，不代替逐条判断。
    pub sensitivity: &'static str,
    pub allow_memory: bool,
    pub allow_ai: bool,
    pub allow_backup: bool,
    pub platforms: CapturePlatformMatrix,
}

/// 这台设备上的采集入口清单。
///
/// `android` 决定通知捕获这条：它是 Android 通知使用权的产物，桌面端没有等价
/// 机制（`solum-notif-access` 的 desktop 实现是空壳），所以桌面必须显示
/// 「不支持」而不是「待接入」——后者会暗示"等下个版本就有了"，那是假的。
pub fn capture_connectors(android: bool) -> Vec<CaptureConnector> {
    vec![
        CaptureConnector {
            source: CaptureSource::Manual,
            label: "手动输入",
            availability: CaptureAvailability::Ready,
            status: "可用",
            description: "直接说「下周三提醒我交材料」。这是不依赖任何系统能力的主入口。",
            sensitivity: "由内容决定",
            allow_memory: true,
            allow_ai: true,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Manual),
        },
        CaptureConnector {
            source: CaptureSource::Notification,
            label: "第三方通知",
            availability: if android {
                CaptureAvailability::Ready
            } else {
                CaptureAvailability::Unsupported
            },
            status: if android {
                "可用"
            } else {
                "桌面无此机制"
            },
            description:
                "需在系统里授予通知使用权，并逐个应用授权；「读取」与「允许建日程」是两条独立授权。",
            sensitivity: "高",
            allow_memory: true,
            allow_ai: false,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Notification),
        },
        CaptureConnector {
            source: CaptureSource::Share,
            label: "系统分享",
            availability: if android {
                CaptureAvailability::Ready
            } else {
                CaptureAvailability::Unsupported
            },
            status: if android {
                "可用"
            } else {
                "桌面无系统分享目标"
            },
            description: "接收文字或图片分享；截图仅在本机 OCR，识别文字先进入内存待确认区。",
            sensitivity: "由内容决定",
            allow_memory: true,
            allow_ai: true,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Share),
        },
        CaptureConnector {
            source: CaptureSource::Calendar,
            label: "系统日历",
            availability: if android {
                CaptureAvailability::Ready
            } else {
                CaptureAvailability::Unsupported
            },
            status: if android {
                "需只读授权"
            } else {
                "桌面无系统日历桥接"
            },
            description: "读取既有日程生成候选；写回必须在系统日历界面再次确认。",
            sensitivity: "高",
            allow_memory: true,
            allow_ai: false,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Calendar),
        },
        CaptureConnector {
            source: CaptureSource::Health,
            label: "健康摘要",
            availability: if android {
                CaptureAvailability::Ready
            } else {
                CaptureAvailability::Unsupported
            },
            status: if android {
                "需授权"
            } else {
                "桌面无此机制"
            },
            description: "只读取你授权的 Health Connect 指标，并以摘要参与建议。",
            sensitivity: "极高",
            allow_memory: true,
            allow_ai: false,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Health),
        },
        CaptureConnector {
            source: CaptureSource::Email,
            label: "邮箱事件",
            availability: CaptureAvailability::Ready,
            status: "可配置",
            description: "只在你主动操作时连接；凭据与邮件正文不入库、不进同步。",
            sensitivity: "极高",
            allow_memory: true,
            allow_ai: false,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Email),
        },
        CaptureConnector {
            source: CaptureSource::Soulous,
            label: "Soulous",
            availability: CaptureAvailability::Ready,
            status: "可配置",
            description: "只读同步明确允许的事实；是否推送日程另行授权。",
            sensitivity: "高",
            allow_memory: true,
            allow_ai: false,
            allow_backup: true,
            platforms: platform_matrix(CaptureSource::Soulous),
        },
        CaptureConnector {
            source: CaptureSource::Location,
            label: "位置",
            availability: CaptureAvailability::Connector,
            status: "暂缓",
            description: "仅规划按需取点，不做常驻轨迹采集。当前版本不申请位置权限。",
            sensitivity: "极高",
            allow_memory: false,
            allow_ai: false,
            allow_backup: false,
            platforms: platform_matrix(CaptureSource::Location),
        },
    ]
}

pub fn capture_source_label(source: CaptureSource) -> &'static str {
    capture_connectors(true)
        .into_iter()
        .find(|c| c.source == source)
        .map(|c| c.label)
        .unwrap_or("外部输入")
}

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

fn first_match(text: &str, re: &Regex, group: usize) -> Option<String> {
    let caps = re.captures(text)?;
    let value = caps.get(group)?.as_str().trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 第一行像样的内容当"事项"。跳过空行和裸 URL。
///
/// 截断按**字符**而不是字节——正文基本都是中文，按字节切会在多字节序列中间
/// 断开，直接 panic。
fn first_matter_line(text: &str) -> Option<String> {
    // 绑在循环外：`lazy_re!` 本身只编译一次（`OnceLock`），但 clippy 的
    // `regex_creation_in_loops` 看不穿宏，会把循环体里的调用报成每轮重编译。
    let bare_url = lazy_re!(r"(?i)^https?://");
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || bare_url.is_match(line) {
            continue;
        }
        let mut chars = line.chars();
        let head: String = chars.by_ref().take(48).collect();
        return Some(if chars.next().is_some() {
            format!("{head}…")
        } else {
            head
        });
    }
    None
}

/// 抽取给人核对的线索。**不是**正式事件解析（那是 [`crate::extract`] 的活）。
pub fn extract_capture_clues(text: &str) -> CaptureClues {
    let normalized = text.trim();
    if normalized.is_empty() {
        return CaptureClues::default();
    }

    let amount = first_match(
        normalized,
        lazy_re!(
            r"(?i)(?:¥|￥|RMB\s*|CNY\s*|人民币\s*)[0-9]+(?:\.[0-9]{1,2})?|[0-9]+(?:\.[0-9]{1,2})?\s*元"
        ),
        0,
    );

    let explicit_location = first_match(
        normalized,
        lazy_re!(
            r"(?:地点|地址|会场|上课地点|取货点|出发地|目的地)\s*[:：]\s*([^\n，,。；;]{2,40})"
        ),
        1,
    );
    let location = match explicit_location {
        Some(v) => Some(v),
        None => first_match(
            normalized,
            lazy_re!(
                r"在\s*([^\n，,。；;]{2,24}?)(?:开会|见面|集合|签到|上课|考试|举办|领取|取货)"
            ),
            1,
        ),
    };

    let time = first_match(
        normalized,
        lazy_re!(concat!(
            r"(?:今天|明天|后天|大后天|(?:本周|这周|下周)[一二三四五六日天]?|",
            r"周[一二三四五六日天]|星期[一二三四五六日天]|",
            r"[0-9]{1,2}月[0-9]{1,2}日|[0-9]{4}[-/.年][0-9]{1,2}[-/.月][0-9]{1,2}日?)",
            r"(?:\s*(?:上午|中午|下午|晚上|凌晨)?\s*[0-9]{1,2}(?::[0-9]{2}|点(?:半|[0-9]{1,2}分)?)?)?"
        )),
        0,
    );

    CaptureClues {
        matter: first_matter_line(normalized),
        time,
        location,
        amount,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_capture_is_android_only() {
        let find = |list: Vec<CaptureConnector>| {
            list.into_iter()
                .find(|c| c.source == CaptureSource::Notification)
                .expect("notification connector is always listed")
        };
        assert_eq!(
            find(capture_connectors(true)).availability,
            CaptureAvailability::Ready
        );
        // 桌面必须是"不支持"，不能是"待接入"——后者暗示以后会有，是假的。
        assert_eq!(
            find(capture_connectors(false)).availability,
            CaptureAvailability::Unsupported
        );
    }

    /// 高敏感位置在没有实现按需取点以前不能标成可用。
    #[test]
    fn unimplemented_entries_are_not_advertised_as_ready() {
        for c in capture_connectors(true) {
            if c.source == CaptureSource::Location {
                assert_ne!(
                    c.availability,
                    CaptureAvailability::Ready,
                    "{} 在本仓尚未实现，不能标成可用",
                    c.label
                );
            }
        }
    }

    #[test]
    fn source_strings_round_trip() {
        for c in capture_connectors(true) {
            assert_eq!(CaptureSource::parse(c.source.as_str()), Some(c.source));
        }
        assert_eq!(CaptureSource::parse("no_such_source"), None);
    }

    #[test]
    fn duplicate_text_across_sources_merges_without_losing_provenance() {
        let mut inbox = CaptureInbox::new();
        let first = inbox.push(CaptureSource::Share, "", "明天 10 点开会", 1_000);
        let merged = inbox.push(CaptureSource::Calendar, "", "明天10点开会", 2_000);
        assert_eq!(first.id, merged.id);
        assert_eq!(inbox.len(), 1);
        assert_eq!(merged.also_seen_from, vec![CaptureSource::Calendar]);
        assert!(merged.note.as_deref().unwrap().contains("合并"));
    }

    #[test]
    fn external_candidates_expire_but_manual_drafts_do_not() {
        let mut inbox = CaptureInbox::new();
        inbox.push(CaptureSource::Share, "", "外部候选", 1_000);
        inbox.push(CaptureSource::Manual, "", "手动候选", 1_001);
        assert_eq!(inbox.prune_expired(1_000 + 8 * 24 * 60 * 60 * 1000), 1);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox.snapshot()[0].source, CaptureSource::Manual);
    }

    #[test]
    fn clues_pick_up_time_location_and_amount() {
        let clues = extract_capture_clues(
            "会员续费提醒\n下周三 下午3点 在实验楼B301 开会\n地点：实验楼B301\n合计 ¥128.50",
        );
        assert_eq!(clues.matter.as_deref(), Some("会员续费提醒"));
        assert_eq!(clues.time.as_deref(), Some("下周三 下午3点"));
        assert_eq!(clues.location.as_deref(), Some("实验楼B301"));
        assert_eq!(clues.amount.as_deref(), Some("¥128.50"));
        assert!(clues.summary().contains("金额：¥128.50"));
    }

    #[test]
    fn natural_location_is_used_only_without_an_explicit_one() {
        let natural = extract_capture_clues("明天在图书馆三楼自习室集合");
        assert_eq!(natural.location.as_deref(), Some("图书馆三楼自习室"));

        let explicit = extract_capture_clues("地址：东风路 8 号\n在别处集合");
        assert_eq!(explicit.location.as_deref(), Some("东风路 8 号"));
    }

    #[test]
    fn amount_written_in_yuan_is_recognised() {
        assert_eq!(
            extract_capture_clues("订单金额 39.9 元").amount.as_deref(),
            Some("39.9 元")
        );
    }

    #[test]
    fn a_bare_url_is_not_the_matter_line() {
        let clues = extract_capture_clues("https://example.com/order/123\n订单已发货");
        assert_eq!(clues.matter.as_deref(), Some("订单已发货"));
    }

    #[test]
    fn empty_input_yields_nothing_and_says_so() {
        let clues = extract_capture_clues("   \n  ");
        assert!(clues.is_empty());
        assert!(clues.summary().contains("暂未提取到明确线索"));
    }

    /// 按字节切中文会 panic；这条钉住按字符截断。
    #[test]
    fn a_long_chinese_matter_line_truncates_on_char_boundaries() {
        let line = "考".repeat(80);
        let matter = extract_capture_clues(&line).matter.unwrap();
        assert_eq!(matter.chars().count(), 49, "48 个字 + 省略号");
        assert!(matter.ends_with('…'));
    }

    #[test]
    fn inbox_holds_drafts_until_they_are_removed() {
        let mut inbox = CaptureInbox::new();
        assert!(inbox.is_empty());

        let a = inbox.push(
            CaptureSource::Share,
            "",
            "明天 10 点开会",
            1_700_000_000_000,
        );
        let b = inbox.push(
            CaptureSource::Manual,
            "手写标题",
            "随便写点",
            1_700_000_001_000,
        );
        assert_ne!(a.id, b.id, "id 必须唯一");
        assert_eq!(inbox.len(), 2);

        // 标题留空时用线索里的事项兜底。
        assert_eq!(a.title, "明天 10 点开会");
        assert_eq!(b.title, "手写标题");

        // 最新的排前面。
        assert_eq!(inbox.snapshot()[0].id, b.id);

        assert!(inbox.remove(&a.id));
        assert!(!inbox.remove(&a.id), "删第二次应报告「没删到」");
        assert_eq!(inbox.len(), 1);
        assert!(inbox.get(&b.id).is_some());
    }

    #[test]
    fn an_untitled_draft_without_clues_falls_back_to_the_entry_name() {
        let mut inbox = CaptureInbox::new();
        let d = inbox.push(CaptureSource::Share, "", "   ", 1_700_000_000_000);
        assert_eq!(d.title, "系统分享");
    }

    #[test]
    fn oversized_input_is_truncated_rather_than_rejected() {
        let mut inbox = CaptureInbox::new();
        let huge = "字".repeat(MAX_DRAFT_CHARS + 500);
        let d = inbox.push(CaptureSource::Share, "x", &huge, 1_700_000_000_000);
        assert_eq!(d.text.chars().count(), MAX_DRAFT_CHARS + 1);
        assert!(d.text.ends_with('…'));
    }

    #[test]
    fn the_queue_is_capped_and_drops_the_oldest() {
        let mut inbox = CaptureInbox::new();
        let mut first = String::new();
        for i in 0..(MAX_DRAFTS + 5) {
            let d = inbox.push(
                CaptureSource::Share,
                &format!("第 {i} 条"),
                &format!("正文 {i}"),
                1_700_000_000_000 + i as i64,
            );
            if i == 0 {
                first = d.id;
            }
        }
        assert_eq!(inbox.len(), MAX_DRAFTS);
        assert!(inbox.get(&first).is_none(), "最旧的应被挤出");
    }
}

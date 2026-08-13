//! 多入口采集的纯领域层（自 Solum Harmony `core/capture.ets` 回移）。
//!
//! 本模块不碰数据库、不碰网络、不依赖壳层：它只负责三件事——**入口目录**
//! （哪些采集通道存在、各自什么状态）、**待确认草稿队列**，以及给人核对用的
//! **线索抽取**。外部文本先进 [`CaptureInbox`]，用户在界面上确认后才交给既有的
//! intent/extract/store 管道。收到不等于保存，这是这层存在的全部理由。
//!
//! **与鸿蒙版的两处刻意差异**：
//!
//! 1. **入口清单按本仓真实能力给状态，不照抄鸿蒙。** 鸿蒙把「第三方通知」标成
//!    「鸿蒙未开放」、把系统分享与截图 OCR 标成「可用」——这三条对本仓全都反着：
//!    本仓有 Android 通知捕获（F1/F20，桌面没有），却**没有**系统分享目标
//!    （`AndroidManifest.xml` 里没有 `ACTION_SEND` 过滤器）也**没有**端侧 OCR。
//!    照抄会让界面对用户撒谎，所以 [`capture_connectors`] 按平台算出真实状态。
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
    Conversation,
    Notification,
    Email,
    SystemShare,
    Screenshot,
    Calendar,
    BrowserExtension,
    OfficialApi,
}

impl CaptureSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Notification => "notification",
            Self::Email => "email",
            Self::SystemShare => "system_share",
            Self::Screenshot => "screenshot",
            Self::Calendar => "calendar",
            Self::BrowserExtension => "browser_extension",
            Self::OfficialApi => "official_api",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "conversation" => Self::Conversation,
            "notification" => Self::Notification,
            "email" => Self::Email,
            "system_share" => Self::SystemShare,
            "screenshot" => Self::Screenshot,
            "calendar" => Self::Calendar,
            "browser_extension" => Self::BrowserExtension,
            "official_api" => Self::OfficialApi,
            _ => return None,
        })
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureConnector {
    pub source: CaptureSource,
    pub label: &'static str,
    pub availability: CaptureAvailability,
    /// 一句话状态，直接上屏。
    pub status: &'static str,
    pub description: &'static str,
}

/// 这台设备上的采集入口清单。
///
/// `android` 决定通知捕获这条：它是 Android 通知使用权的产物，桌面端没有等价
/// 机制（`solum-notif-access` 的 desktop 实现是空壳），所以桌面必须显示
/// 「不支持」而不是「待接入」——后者会暗示"等下个版本就有了"，那是假的。
pub fn capture_connectors(android: bool) -> Vec<CaptureConnector> {
    vec![
        CaptureConnector {
            source: CaptureSource::Conversation,
            label: "Solum 对话",
            availability: CaptureAvailability::Ready,
            status: "可用",
            description: "直接说「下周三提醒我交材料」。这是不依赖任何系统能力的主入口。",
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
        },
        CaptureConnector {
            source: CaptureSource::Email,
            label: "邮箱接入",
            availability: CaptureAvailability::Ready,
            status: "可用",
            description: "只在你主动操作时连接；凭据与邮件内容不入库、不进同步、不做 AI 语料。",
        },
        CaptureConnector {
            source: CaptureSource::SystemShare,
            label: "系统分享",
            availability: CaptureAvailability::Connector,
            status: "待接入",
            description: "把其他应用的文字分享给息壤。Android 侧尚未注册分享目标。",
        },
        CaptureConnector {
            source: CaptureSource::Screenshot,
            label: "截图识别",
            availability: CaptureAvailability::Connector,
            status: "待接入",
            description: "对截图做端侧文字识别。本仓尚未接入任何 OCR 实现。",
        },
        CaptureConnector {
            source: CaptureSource::Calendar,
            label: "日历接入",
            availability: CaptureAvailability::Connector,
            status: "待接入",
            description: "通过系统日历或你主动配置的 CalDAV 同步既有日程。",
        },
        CaptureConnector {
            source: CaptureSource::BrowserExtension,
            label: "浏览器扩展",
            availability: CaptureAvailability::Companion,
            status: "需配套端",
            description: "网页订单与活动页由扩展一键保存；配套端尚未开发。",
        },
        CaptureConnector {
            source: CaptureSource::OfficialApi,
            label: "官方开放接口",
            availability: CaptureAvailability::Connector,
            status: "白名单接入",
            description: "只连接数据源明确允许访问、且你主动授权的接口。",
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
        let text = truncate_chars(text.trim(), MAX_DRAFT_CHARS);
        let title = {
            let t = title.trim();
            if !t.is_empty() {
                truncate_chars(t, 48)
            } else {
                extract_capture_clues(&text)
                    .matter
                    .unwrap_or_else(|| capture_source_label(source).to_string())
            }
        };
        let draft = CaptureDraft {
            id: self.next_id(now_ms),
            source,
            title,
            text,
            created_at: now_ms,
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

    /// 防止有人把鸿蒙的清单整段贴回来：那份把系统分享/截图标成"可用"，
    /// 本仓两条都没实现。
    #[test]
    fn unimplemented_entries_are_not_advertised_as_ready() {
        for c in capture_connectors(true) {
            if matches!(
                c.source,
                CaptureSource::SystemShare | CaptureSource::Screenshot | CaptureSource::Calendar
            ) {
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
            CaptureSource::SystemShare,
            "",
            "明天 10 点开会",
            1_700_000_000_000,
        );
        let b = inbox.push(
            CaptureSource::Conversation,
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
        let d = inbox.push(CaptureSource::SystemShare, "", "   ", 1_700_000_000_000);
        assert_eq!(d.title, "系统分享");
    }

    #[test]
    fn oversized_input_is_truncated_rather_than_rejected() {
        let mut inbox = CaptureInbox::new();
        let huge = "字".repeat(MAX_DRAFT_CHARS + 500);
        let d = inbox.push(CaptureSource::SystemShare, "x", &huge, 1_700_000_000_000);
        assert_eq!(d.text.chars().count(), MAX_DRAFT_CHARS + 1);
        assert!(d.text.ends_with('…'));
    }

    #[test]
    fn the_queue_is_capped_and_drops_the_oldest() {
        let mut inbox = CaptureInbox::new();
        let mut first = String::new();
        for i in 0..(MAX_DRAFTS + 5) {
            let d = inbox.push(
                CaptureSource::SystemShare,
                &format!("第 {i} 条"),
                "正文",
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

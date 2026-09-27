//! `catalog` -- the connector list the UI renders, and the one platform fact it encodes: Android gets notification capture because that is a product of Android's notification permission, and desktop has no equivalent — so desktop shows “unsupported”, not “coming soon”. The second one would be a lie the UI could not take back.
//!
//! Part of the `capture` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::classify::{platform_matrix, CaptureAvailability, CapturePlatformMatrix};
use super::*;

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

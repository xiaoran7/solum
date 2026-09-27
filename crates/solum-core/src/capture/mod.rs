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
//!
//! Split by concern; every section reached these crate imports through
//! `use super::*;` when this was one file, so they stay here, in the parent a
//! sibling's glob actually resolves to.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
// -- re-exports: the paths callers used before the split, unchanged. A
// `pub use` cannot be wider than the item it re-exports, so a name that was
// `pub` before the split stays `pub` here.
pub use catalog::{capture_connectors, capture_source_label, CaptureConnector};
pub use clues::extract_capture_clues;
pub use draft::{CaptureClues, CaptureDraft};
pub use inbox::CaptureInbox;
pub use source::CaptureSource;

// -- macro, hoisted: `#[macro_use]` on a `mod` reaches DOWNWARD only, and
// -- the section declarations are anchored on the first `use` -- so a macro
// -- below them can never be reached, however many attributes are added.
macro_rules! lazy_re {
    ($pat:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pat).expect("static regex compiles"))
    }};
}

mod catalog;
mod classify;
mod clues;
mod draft;
mod inbox;
mod source;
#[cfg(test)]
mod tests;

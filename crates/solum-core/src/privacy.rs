//! 首启隐私同意与应用内隐私政策全文（自 PA-harmony `privacyConsent*/privacyPolicy.ets` 回移）。
//!
//! **政策正文不是从鸿蒙原样搬过来的。** 鸿蒙 0.2.0 是上架包，它的政策里写着
//! 「本版本不提供多设备同步」「不使用第三方通知监听」——这两句对本仓都是**假的**：
//! 本仓有端到端加密的多设备同步（[`crate::sync`]）、有 Android 通知监听
//! （F1/F20）、还有邮箱连接器（F21）。照搬会让对外声明与代码事实脱节，违反
//! AGENTS.md 那条「对外材料里每个可核对的名词都要能落到仓库事实上」。
//! 所以正文是按本仓 `docs/PRIVACY.md` 与实际代码重写的，只有**结构**沿用鸿蒙。
//!
//! 同意记录落本机文件 `solum-privacy-consent.json`，**刻意不进 SQLite、不进同步**：
//! 「这台设备上有人点过同意」是设备级事实，不该由另一台设备的同意状态代答
//! （判据同鸿蒙 MISC 2026-07-23：「用户想要什么」该同步，「这台设备发生过什么」不该）。
//!
//! 只有 GUI 壳层（solum-app）过这道门。`solum-cli` 不受影响——它是本机自动化入口，
//! 给它加交互式同意门只会卡住脚本，而它并不新增任何数据出境路径。

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 政策正文发生**实质**变化（新增数据类型、新增权限、新增外发场景）时递增。
/// 措辞润色不算——递增一次就等于把弹窗重新推给所有老用户一遍。
pub const PRIVACY_POLICY_VERSION: u32 = 1;

pub const PRIVACY_POLICY_TITLE: &str = "Solum（息壤）隐私政策";
pub const PRIVACY_POLICY_UPDATED_ON: &str = "2026-07-29";

const CONSENT_FILE_NAME: &str = "solum-privacy-consent.json";

/// 只有与当前版本**完全一致**的记录才算数。
///
/// 「比当前更高」不能放行：它只可能来自损坏文件、手工篡改，或装过别的分支构建。
/// 这三种情况下，当前这份政策都没有可验证的同意记录，应当重新展示。
pub fn has_current_consent(version: Option<u32>) -> bool {
    version == Some(PRIVACY_POLICY_VERSION)
}

/// 落盘的同意记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyConsent {
    pub version: u32,
    /// RFC 3339 时间戳，仅供用户自查「我是什么时候同意的」。
    pub accepted_at: String,
}

impl PrivacyConsent {
    /// `SOLUM_PRIVACY_CONSENT` 覆盖（移动端启动时指向 app-data），否则与其余
    /// 本机配置同目录。
    pub fn path() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("SOLUM_PRIVACY_CONSENT") {
            if !p.trim().is_empty() {
                return p.into();
            }
        }
        crate::paths::resolve_with_adoption(CONSENT_FILE_NAME)
    }

    /// 文件缺失/读不动/格式坏 → `None`（＝没同意过），绝不报错：
    /// 坏文件的正确后果是「再问一次」，不是「应用起不来」。
    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        let parsed: Self = serde_json::from_str(&text).ok()?;
        if parsed.accepted_at.trim().is_empty() {
            return None;
        }
        Some(parsed)
    }

    /// 当前是否还需要展示同意页。
    pub fn needs_consent() -> bool {
        !has_current_consent(Self::load().map(|c| c.version))
    }

    /// 记下「本机、本版本、此刻」同意过。
    pub fn accept(now: chrono::DateTime<chrono::Local>) -> Result<Self> {
        let consent = Self {
            version: PRIVACY_POLICY_VERSION,
            accepted_at: now.to_rfc3339(),
        };
        let json = serde_json::to_string_pretty(&consent)?;
        crate::fsatomic::write_atomic(&Self::path(), &json)?;
        Ok(consent)
    }
}

/// 首启弹窗的精简文本。它与 [`PRIVACY_POLICY_FULL_BODY`] 和 `docs/PRIVACY.md`
/// 描述同一套数据实践——改任何一份都要核对另外两份。
pub const PRIVACY_GATE_TITLE: &str = "息壤 · 隐私与数据说明";

pub const PRIVACY_GATE_BODY: &str = r#"## 开始前，请先知道

- **大部分内容留在本机。** 你的对话、日程、提醒、记忆和使用记录，默认保存在这台设备上。
- **只有你主动使用联网功能时，相关内容才会发出。** 例如你主动向 AI 提问、开启多设备同步，或使用邮箱功能。
- **通知功能由你控制。** 你可以逐个选择允许读取通知的应用，也可以随时关闭“通知上云”。通知里可能包含联系人、银行或其他平台发来的内容，开启前请确认你了解这一点。
- **重要操作一定会再次问你。** 删除、付款、发邮件等操作，不会在未经确认时直接执行。
- **息壤不投放广告，也不加入第三方统计或跟踪。**

你可以随时在设置中关闭联网功能、删除记忆或清除本机数据。发给 AI 服务商的内容，将按你所选择服务商的隐私规则处理。

点击“查看完整隐私政策”可以了解更多。"#;

pub const PRIVACY_GATE_DECLINE_MESSAGE: &str = "需要同意隐私政策才能使用息壤，应用即将退出。";

/// 应用内完整政策。与 `docs/PRIVACY.md` 描述同一套实践；后者是决策权威，
/// 本常量是给用户在应用里读的呈现版，两边实质内容必须一致。
pub const PRIVACY_POLICY_FULL_BODY: &str = r#"# 息壤隐私政策

## 1. 息壤如何处理你的数据

息壤是一个由你自己使用和管理的智能助手。默认情况下，你的对话、日程、提醒、记忆、习惯记录和复盘内容都保存在本机。

只有在你主动使用需要联网的功能时，完成这次操作所需的内容才会离开设备。

## 2. 哪些情况会联网

- **向 AI 提问：** 你发送的消息，以及回答这条消息所需的少量近期对话和相关记忆，会发给你选择的 AI 服务商。
- **多设备同步：** 需要同步的内容会先在你的设备上加密，再发送到你自己设置的同步服务。同步服务看不到原文。
- **通知上云：** 开启后，你允许息壤读取的通知可能会发给 AI 服务商，帮助理解和整理内容。
- **邮箱功能：** 只有你主动查看、搜索或发送邮件时，息壤才会连接邮箱服务。

已经发给外部服务的内容无法由息壤追回。相关内容如何保存和处理，也会受到对应服务商隐私规则的约束。

## 3. 哪些内容不会交给 AI

以下内容不会作为 AI 提问内容发送：

- 你导入的聊天记录原文
- 本机保存的完整对话历史
- 你导入的 PDF 或文本文档原件
- 邮箱密码、授权信息、邮件正文和搜索结果
- 完整的人格版本记录
- 安全操作记录
- 穿戴设备的逐条原始数据

为了保持当前对话连贯，发送消息时最多会附带当前会话最近四轮已经完成的对话，不会上传全部历史。

## 4. 关于通知

通知读取功能需要你先在系统中授权，然后逐个选择允许读取通知的应用。

“允许读取通知”和“允许根据通知直接创建日程”是两个不同的开关。创建日程默认关闭，需要你单独允许。

“通知上云”默认开启，你可以随时在“设置 → 隐私”中关闭。通知可能包含联系人、银行、快递或其他平台发来的内容；开启后，这些内容也可能发送给你选择的 AI 服务商。关闭后，新收到的通知不会再发送；之前明确标记为不发送的通知，将来也不会补发。

设备同步和通知上云互不影响。关闭通知上云后，你自己的设备之间仍可继续加密同步。

## 5. 关于邮箱

邮箱密码和授权信息只保存在本机。邮件内容不会进入记忆、设备同步或 AI 对话。

息壤不会在后台自动读取邮箱。发送邮件前会显示完整内容，并要求你再次确认。

## 6. 重要操作

删除、付款、发送邮件等重要或无法撤销的操作，都必须由你再次确认。AI 的回答不能跳过这一步。

## 7. 你可以随时控制和删除

你可以随时：

- 关闭通知上云
- 取消某个应用的通知读取权限
- 查看、修改或删除记忆
- 删除本机对话和导入的资料
- 退出账号或断开邮箱
- 在系统设置中清除息壤的全部本机数据

如果你使用了自己搭建的同步服务，还需要在该服务上单独删除已经同步的加密内容。

## 8. 息壤不会做什么

- 不投放广告
- 不加入第三方统计、跟踪或崩溃上报
- 不出售你的数据
- 不会因为登录账号就上传全部本机数据

## 9. 政策更新

如果数据处理方式发生重要变化，息壤会更新隐私政策，并在下次启动时再次请你确认。"#;

/// 给壳层一次取齐的呈现结构。
#[derive(Debug, Clone, Serialize)]
pub struct PolicyDocument {
    pub version: u32,
    pub title: &'static str,
    pub updated_on: &'static str,
    pub gate_title: &'static str,
    pub gate_body: &'static str,
    pub decline_message: &'static str,
    pub full_body: &'static str,
}

pub fn policy_document() -> PolicyDocument {
    PolicyDocument {
        version: PRIVACY_POLICY_VERSION,
        title: PRIVACY_POLICY_TITLE,
        updated_on: PRIVACY_POLICY_UPDATED_ON,
        gate_title: PRIVACY_GATE_TITLE,
        gate_body: PRIVACY_GATE_BODY,
        decline_message: PRIVACY_GATE_DECLINE_MESSAGE,
        full_body: PRIVACY_POLICY_FULL_BODY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_current_version_counts_as_consent() {
        assert!(has_current_consent(Some(PRIVACY_POLICY_VERSION)));
        assert!(!has_current_consent(None));
        assert!(!has_current_consent(Some(PRIVACY_POLICY_VERSION - 1)));
        // 未来版本同样不放行——来源不可验证。
        assert!(!has_current_consent(Some(PRIVACY_POLICY_VERSION + 1)));
    }

    #[test]
    fn consent_round_trips_through_json() {
        let consent = PrivacyConsent {
            version: PRIVACY_POLICY_VERSION,
            accepted_at: "2026-07-27T10:00:00+08:00".to_string(),
        };
        let raw = serde_json::to_string(&consent).unwrap();
        let back: PrivacyConsent = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, consent);
    }

    #[test]
    fn a_record_without_a_timestamp_is_not_a_consent() {
        // 手工造出来的半截文件不能当成同意过。
        let raw = r#"{"version":1,"accepted_at":"  "}"#;
        let parsed: PrivacyConsent = serde_json::from_str(raw).unwrap();
        assert!(parsed.accepted_at.trim().is_empty());
    }

    /// 三个落盘场景**刻意合并成一个测试**：`SOLUM_PRIVACY_CONSENT` 是进程级环境
    /// 变量，而 cargo 默认多线程并发跑同一进程内的用例——拆成三个 `#[test]` 会
    /// 互相改写对方指向的路径，出现与代码无关的偶发失败（本仓已因同类问题吃过亏）。
    #[test]
    fn consent_file_lifecycle() {
        let dir = std::env::temp_dir().join(format!("solum-privacy-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("solum-privacy-consent.json");
        std::env::set_var("SOLUM_PRIVACY_CONSENT", &path);

        // ① 干净环境：必须拦。
        assert!(PrivacyConsent::needs_consent(), "干净环境必须要求同意");

        // ② 同意后落盘、可读回、不再拦。
        let written = PrivacyConsent::accept(chrono::Local::now()).unwrap();
        assert_eq!(written.version, PRIVACY_POLICY_VERSION);
        assert!(!PrivacyConsent::needs_consent(), "同意后不该再拦");
        assert_eq!(PrivacyConsent::load().unwrap(), written);

        // ③ 旧版本记录：重新征求。
        std::fs::write(
            &path,
            r#"{"version":0,"accepted_at":"2026-01-01T00:00:00+08:00"}"#,
        )
        .unwrap();
        assert!(
            PrivacyConsent::needs_consent(),
            "旧版本同意记录必须重新征求"
        );

        // ④ 坏文件：当作没同意过，而不是让应用起不来。
        std::fs::write(&path, "{ not json").unwrap();
        assert!(PrivacyConsent::load().is_none());
        assert!(PrivacyConsent::needs_consent());

        std::env::remove_var("SOLUM_PRIVACY_CONSENT");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 正文里不能出现只对鸿蒙上架版成立、对本仓为假的断言。这条测试是防止
    /// 以后有人图省事把鸿蒙文案整段贴回来。
    #[test]
    fn the_policy_does_not_repeat_harmony_only_claims() {
        for text in [PRIVACY_GATE_BODY, PRIVACY_POLICY_FULL_BODY] {
            assert!(
                !text.contains("本版本不提供多设备同步"),
                "本仓有同步，不能照抄鸿蒙这句"
            );
            assert!(
                !text.contains("不使用第三方通知监听"),
                "本仓有 Android 通知捕获，不能照抄鸿蒙这句"
            );
        }
        // 反向确认：本仓确实把这两件事讲清楚了。
        assert!(PRIVACY_POLICY_FULL_BODY.contains("先在你的设备上加密"));
        assert!(PRIVACY_POLICY_FULL_BODY.contains("通知上云"));
    }
}

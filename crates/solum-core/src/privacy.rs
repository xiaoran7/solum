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
pub const PRIVACY_POLICY_UPDATED_ON: &str = "2026-07-27";

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

pub const PRIVACY_GATE_BODY: &str = concat!(
    "息壤是一个个人自用、自托管的助手。开始使用前，请先了解它如何处理你的数据：\n\n",
    "• 本地优先：对话、日程、提醒、记忆、行为日志、复盘与审计日志都保存在这台设备的本地数据库里。\n\n",
    "• 云端 AI 由你自己配置：息壤不内置云服务。只有你填入厂商 API Key（或登录你自建的 solum-cloud 代理）\
     并主动发消息时，该消息与生成回复所需的少量上下文才会发往你选定的服务商。发出去的数据受**该服务商**\
     的条款约束，息壤不代为承诺。\n\n",
    "• 多设备同步是端到端加密的：数据块在本机加密后才发往你自己搭的中转服务器，服务器解不开、也不留明文。\n\n",
    "• Android 通知捕获（可选）：授予通知使用权后，息壤才能读取你**逐个应用**授权的通知。\
     「读取该应用通知」与「允许它直接建日程」是两条独立授权，后者默认关闭。\n\n",
    "• 「通知上云」开关默认开启，可随时在「设置 → 隐私」关闭。请注意：通知里往往含有**第三方**\
     （联系人、银行、平台）产生的内容，开启即意味着这些内容也会发往你所选的 AI 服务商。\n\n",
    "• 邮箱连接器（可选）：凭据只存本机文件，邮件内容不写数据库、不进同步、不做 AI 语料；\
     对外发信逐封人工确认。\n\n",
    "• 高危操作（删除、支付、外发）一律强制人工确认，不受主动度设置或模型输出绕过。\n\n",
    "• 不投放广告，不接入任何第三方统计、追踪或崩溃上报 SDK。\n\n",
    "• 你可随时在记忆台账逐条查看和删除，关闭上云开关，或退出账号；删除本地数据目录即清除全部本机数据。\n\n",
    "点击下方「查看完整隐私政策」可阅读全文。继续使用即表示你已阅读并同意以上内容。"
);

pub const PRIVACY_GATE_DECLINE_MESSAGE: &str = "需要同意隐私政策才能使用息壤，应用即将退出。";

/// 应用内完整政策。与 `docs/PRIVACY.md` 描述同一套实践；后者是决策权威，
/// 本常量是给用户在应用里读的呈现版，两边实质内容必须一致。
pub const PRIVACY_POLICY_FULL_BODY: &str = concat!(
    "1. 这是什么\n",
    "Solum（息壤）是一个个人自用、自托管的智能助手，源码开源。它默认把数据留在你自己的设备上，\
     只在完成单次任务时把最小必要上下文交给**你自行配置**的云端 AI 服务商。本政策说明哪些数据会\
     离开设备、去哪里、你如何控制。\n\n",
    "2. 默认只存本机的数据\n",
    "对话、日程、提醒、固定提醒、建议、记忆台账、人格版本、行为日志、周期复盘与 append-only 审计日志，\
     都保存在本机应用数据目录下的 SQLite 数据库。桌面端与命令行 `solum` 共用同一份库。\n\n",
    "3. 永不发往云端 AI 的数据（硬约束，无开关）\n",
    "以下数据在任何设置下都不会发给 AI 服务商：导入的聊天记录原文（含对话另一方内容）；\
     壳层本地会话历史的完整转录；资料工作台导入的 PDF/文本原件；邮箱凭据与邮件正文、搜索结果、\
     附件元信息；数字人格画像的全部版本；append-only 审计日志；穿戴设备逐条原始采样。\n",
    "其中会话历史、资料文件与邮件数据的本地性更强——它们不写入 SQLite，也不进入同步或数据导出。\
     云端对话请求至多临时携带**当前会话最近四轮已完成对话**，而不是完整历史。\n\n",
    "4. 云端 AI（可选，由你配置）\n",
    "息壤不内置云服务。你有两条通路：直接填入厂商 API Key，或登录你自建的 solum-cloud 代理\
     （此时第三方 Key 只存在于你自己服务器的环境变量里，不下发到设备）。两者都只在你主动发送消息时\
     才发起请求；登录本身不上传本机数据库。发往云端的数据由**你所选服务商**的隐私条款约束。\
     更换或关闭服务商即停止相应外流，但已经发出的历史无法追回。\n\n",
    "5. 多设备同步（可选）\n",
    "同步在本机用 XChaCha20-Poly1305 加密后才把数据块发往**你自己部署**的中转服务器；\
     服务器只存密文与投递元数据，没有密钥、解不开内容。主密钥由你的用户名与密码在每台设备本地\
     派生（PBKDF2-HMAC-SHA256 + HKDF），不经过服务器。\n",
    "**同步与「通知上云」是两件事**：关掉上云开关不会、也不需要停掉你自己设备之间的同步。\n\n",
    "6. Android 通知捕获（可选）\n",
    "需要你在系统里授予通知使用权，并**逐个应用**选择允许捕获。「允许读取某应用通知」与\
     「允许该应用的通知直接写入日程」是两条独立授权，后者默认关闭、需单独确认，且设有每应用每日\
     上限以限制单个应用出问题时的损害。撤销读取会连带撤销写入授权。\n",
    "「通知上云」开关默认开启：开启时被捕获的通知文本会作为上下文发往你所选的 AI 服务商。\
     **通知内容里有很大一部分是第三方（联系人、银行、快递、平台）产生的，这些第三方未必知情。**\
     开关可随时关闭；关闭后新捕获的通知不再外发，也不作为记忆检索语料。该判定在捕获时刻逐条写死、\
     此后不可变，所以先前标为不外发的历史不会因为重新开启而被补传。桌面端没有这条管线。\n\n",
    "7. 邮箱连接器（可选）\n",
    "账户凭据（授权码 / OAuth refresh token）只写入本机 gitignore 的配置文件，不进数据库、不进同步。\
     邮件正文与搜索结果只存在于当前进程与界面内存，不落库、不进记忆检索、不进通知上云管线，\
     且不后台拉取——只在你主动操作时连接。对外发信在完整预览后逐封人工确认，审计只留账户 id、\
     收件人数量与结果。\n\n",
    "8. 高危操作\n",
    "删除、支付、外发等不可逆操作强制走人工确认（预览 → 确认 → 一次性令牌），不受主动度设置\
     或模型输出绕过。相关操作留 append-only 审计记录。\n\n",
    "9. 我们不做的事\n",
    "不投放广告；不接入第三方统计、追踪或崩溃上报 SDK；不出售或向数据经纪商提供你的数据；\
     不因登录而上传本机数据库；不把第三方 API Key 写进客户端。\n\n",
    "10. 你的控制权\n",
    "随时在「设置 → 隐私」关闭通知上云；在记忆台账查看、编辑、删除任意一条记忆（批量清理需二次确认）；\
     在对话会话栏删除本机会话；退出账号即清除本机令牌。删除应用数据目录即清除全部本机数据；\
     同步服务器上的密文需要你自行清理并轮换密钥。\n\n",
    "11. 默认值说明\n",
    "上述「默认开启」是面向原作者个人自用的便利默认。若你把息壤部署给自己或他人使用，\
     请按你的场景自行调整默认值（例如改为默认关闭 / opt-in），相关开关在设置与配置中均可修改。\n\n",
    "12. 政策变更\n",
    "发生实质变化时应用会递增隐私政策版本，并在下次启动时重新展示同意页。"
);

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
        assert!(PRIVACY_POLICY_FULL_BODY.contains("XChaCha20-Poly1305"));
        assert!(PRIVACY_POLICY_FULL_BODY.contains("通知上云"));
    }
}

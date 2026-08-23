//! Android/WebView adapter for the platform-owned portion of `solum-export` v3.
//!
//! The WebView owns chat state and IndexedDB document blobs. Rust owns local
//! configuration and the notification spool. This module joins their immutable
//! projections without copying credentials or attachment bytes into the main
//! JSON payload.

use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, TimeZone, Utc};
use serde::Deserialize;

use solum_core::email::{EmailConfig, EmailProvider};
use solum_core::export_v3::{
    DocumentManifest, EmailRestoreIntent, ExternalRestoreIntents, PendingNotificationInput,
    PlatformPortableData, PortableChat, PortableLlmSettings, SoulousRestoreIntent,
};
use solum_core::llm::LlmConfig;
use solum_core::soulous::SoulousConfig;

const MAX_PENDING_FILES: usize = 10_000;
const MAX_PENDING_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ATOMIC_SPOOL_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebViewPortableSnapshot {
    pub profile_id: String,
    pub chat: PortableChat,
    pub documents: Vec<DocumentManifest>,
}

pub(crate) fn validate_profile(expected: &str, actual: &str) -> Result<(), String> {
    if expected == actual && valid_profile_id(expected) {
        Ok(())
    } else {
        Err("平台快照与当前账号 profile 不一致，已取消导出".into())
    }
}

fn valid_profile_id(value: &str) -> bool {
    value == "guest"
        || (value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
            }))
}

pub(crate) fn collect(
    snapshot: WebViewPortableSnapshot,
    profile_dir: &Path,
    health_sources: Vec<String>,
    background_reminders: bool,
) -> Result<PlatformPortableData, String> {
    let llm = load_llm_projection(&super::llm_config_file())?;
    let email_accounts = load_email_intents(&super::email_config_file())?;
    let soulous = load_soulous_intent(&super::soulous_config_file())?;
    let pending_notifications = snapshot_pending_notifications(profile_dir)?;

    Ok(PlatformPortableData {
        chat: snapshot.chat,
        documents: snapshot.documents,
        pending_notifications,
        llm,
        external_restore_intents: ExternalRestoreIntents {
            email_accounts,
            soulous,
            health_sources,
            background_reminders,
        },
    })
}

fn load_llm_projection(path: &Path) -> Result<Option<PortableLlmSettings>, String> {
    let env_active = ["SOLUM_LLM_BASE_URL", "SOLUM_LLM_API_KEY"]
        .iter()
        .all(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()));
    let config = if env_active {
        Some(LlmConfig::load().ok_or_else(|| "LLM 环境配置损坏，已取消导出".to_string())?)
    } else {
        read_optional_config(path, "LLM", LlmConfig::from_json)?
    };
    Ok(config.map(project_llm))
}

fn project_llm(config: LlmConfig) -> PortableLlmSettings {
    PortableLlmSettings {
        base_url: config.base_url,
        model: config.model,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        timeout_secs: config.timeout_secs,
    }
}

fn load_email_intents(path: &Path) -> Result<Vec<EmailRestoreIntent>, String> {
    let config = read_optional_config(path, "邮箱", EmailConfig::from_json)?.unwrap_or_default();
    Ok(config
        .accounts
        .into_iter()
        .map(|account| EmailRestoreIntent {
            id: account.id,
            label: account.label,
            provider: match account.provider {
                EmailProvider::Qq => "qq",
                EmailProvider::Gmail => "gmail",
                EmailProvider::Microsoft => "microsoft",
                EmailProvider::Custom => "custom",
            }
            .into(),
            address: account.address,
        })
        .collect())
}

fn load_soulous_intent(path: &Path) -> Result<Option<SoulousRestoreIntent>, String> {
    Ok(
        read_optional_config(path, "Soulous", SoulousConfig::from_json)?.map(|config| {
            SoulousRestoreIntent {
                server_url: config.server_url,
                push_schedule_events: config.push_schedule_events,
            }
        }),
    )
}

fn read_optional_config<T>(
    path: &Path,
    label: &str,
    parse: impl FnOnce(&str) -> solum_core::Result<T>,
) -> Result<Option<T>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(format!(
                "读取{label}配置失败（{}），已取消导出",
                safe_file_name(path)
            ));
        }
    };
    parse(&raw).map(Some).map_err(|_| {
        format!(
            "{label}配置损坏或版本不兼容（{}），已取消导出",
            safe_file_name(path)
        )
    })
}

#[derive(Debug)]
struct PendingSource {
    path: PathBuf,
    failed: bool,
    legacy: bool,
    sort_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPendingNotification {
    ts: i64,
    pkg: String,
    title: String,
    text: String,
}

fn snapshot_pending_notifications(
    profile_dir: &Path,
) -> Result<Vec<PendingNotificationInput>, String> {
    let spool = profile_dir.join("notif-spool");
    let mut sources = Vec::new();
    collect_json_files(&spool, false, &mut sources)?;
    collect_json_files(&spool.join("failed"), true, &mut sources)?;
    for name in ["notif-inbox.jsonl", "notif-inbox.processing.jsonl"] {
        let path = profile_dir.join(name);
        if path.exists() {
            sources.push(PendingSource {
                sort_key: format!("legacy:{name}"),
                path,
                failed: false,
                legacy: true,
            });
        }
    }
    if sources.len() > MAX_PENDING_FILES {
        return Err("通知待处理文件数量超过快照上限，已取消导出".into());
    }
    sources.sort_by(|left, right| left.sort_key.cmp(&right.sort_key));

    let mut expected_bytes = 0u64;
    for source in &sources {
        let metadata = std::fs::symlink_metadata(&source.path)
            .map_err(|_| pending_error(&source.path, "无法读取文件元数据"))?;
        if !metadata.file_type().is_file() {
            return Err(pending_error(&source.path, "不是普通文件"));
        }
        if !source.legacy && metadata.len() > MAX_ATOMIC_SPOOL_FILE_BYTES {
            return Err(pending_error(&source.path, "单文件超过上限"));
        }
        expected_bytes = expected_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "通知待处理快照字节数溢出，已取消导出".to_string())?;
        if expected_bytes > MAX_PENDING_BYTES {
            return Err("通知待处理快照超过 64 MiB 上限，已取消导出".into());
        }
    }

    let mut rows = Vec::new();
    let mut actual_bytes = 0u64;
    for source in sources {
        let bytes = std::fs::read(&source.path)
            .map_err(|_| pending_error(&source.path, "读取失败或文件已变化"))?;
        actual_bytes = actual_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| "通知待处理快照字节数溢出，已取消导出".to_string())?;
        if actual_bytes > MAX_PENDING_BYTES {
            return Err("通知待处理快照超过 64 MiB 上限，已取消导出".into());
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| pending_error(&source.path, "不是 UTF-8"))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let raw: RawPendingNotification = serde_json::from_str(line)
                .map_err(|_| pending_error(&source.path, "JSON 字段损坏或不兼容"))?;
            let package_name = raw.pkg.trim().to_string();
            let title = raw.title.trim().to_string();
            let body = raw.text.trim().to_string();
            if package_name.is_empty() || (title.is_empty() && body.is_empty()) {
                return Err(pending_error(&source.path, "缺少必要字段"));
            }
            let received_at = Utc
                .timestamp_millis_opt(raw.ts)
                .single()
                .ok_or_else(|| pending_error(&source.path, "时间戳无效"))?
                .to_rfc3339_opts(SecondsFormat::Millis, true);
            rows.push(PendingNotificationInput {
                package_name,
                title,
                body,
                received_at,
                failed: source.failed,
            });
            if rows.len() > MAX_PENDING_FILES {
                return Err("通知待处理记录数量超过快照上限，已取消导出".into());
            }
        }
    }
    Ok(rows)
}

fn collect_json_files(
    dir: &Path,
    failed: bool,
    out: &mut Vec<PendingSource>,
) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(format!(
                "无法枚举通知待处理目录（{}），已取消导出",
                safe_file_name(dir)
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|_| "枚举通知待处理目录失败，已取消导出".to_string())?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|_| pending_error(&path, "无法读取文件类型"))?;
        if !kind.is_file() || kind.is_symlink() {
            return Err(pending_error(&path, "不是普通文件"));
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        out.push(PendingSource {
            path,
            failed,
            legacy: false,
            sort_key: name,
        });
    }
    Ok(())
}

fn pending_error(path: &Path, reason: &str) -> String {
    format!(
        "通知待处理快照失败（{}：{reason}），已取消导出",
        safe_file_name(path)
    )
}

fn safe_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use solum_core::email::{EmailAccount, EmailAuth, EmailEndpoints, SmtpTls};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solum-portable-snapshot-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn profile_binding_accepts_only_the_exact_guest_or_lowercase_uuid() {
        assert!(validate_profile("guest", "guest").is_ok());
        let id = "12345678-1234-4abc-8def-1234567890ab";
        assert!(validate_profile(id, id).is_ok());
        assert!(validate_profile(id, "guest").is_err());
        assert!(validate_profile(&id.to_uppercase(), &id.to_uppercase()).is_err());
    }

    #[test]
    fn active_failed_and_legacy_notifications_form_one_deterministic_snapshot() {
        let dir = temp_dir("spool");
        let spool = dir.join("notif-spool");
        std::fs::create_dir_all(spool.join("failed")).unwrap();
        std::fs::write(
            spool.join("100-a.json"),
            r#"{"ts":1000,"pkg":"com.a","title":"A","text":"one"}
"#,
        )
        .unwrap();
        std::fs::write(
            spool.join("failed").join("200-b.try3.json"),
            r#"{"ts":2000,"pkg":"com.b","title":"B","text":"two"}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("notif-inbox.jsonl"),
            r#"{"ts":3000,"pkg":"com.legacy","title":"C","text":"three"}
"#,
        )
        .unwrap();

        let rows = snapshot_pending_notifications(&dir).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].package_name, "com.a");
        assert!(!rows[0].failed);
        assert_eq!(rows[1].package_name, "com.b");
        assert!(rows[1].failed);
        assert_eq!(rows[2].package_name, "com.legacy");
        assert_eq!(rows[0].received_at, "1970-01-01T00:00:01.000Z");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_spool_fails_without_echoing_personal_content() {
        let dir = temp_dir("malformed");
        let spool = dir.join("notif-spool");
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("private.json"),
            r#"{"ts":1,"pkg":"com.a","title":"sensitive personal title""#,
        )
        .unwrap();
        let error = snapshot_pending_notifications(&dir).unwrap_err();
        assert!(error.contains("private.json"));
        assert!(!error.contains("sensitive personal title"));
        assert!(spool.join("private.json").exists(), "snapshot is read-only");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_atomic_spool_file_is_rejected_before_parsing() {
        let dir = temp_dir("oversized");
        let spool = dir.join("notif-spool");
        std::fs::create_dir_all(&spool).unwrap();
        std::fs::write(
            spool.join("large.json"),
            vec![b'x'; MAX_ATOMIC_SPOOL_FILE_BYTES as usize + 1],
        )
        .unwrap();
        let error = snapshot_pending_notifications(&dir).unwrap_err();
        assert!(error.contains("单文件超过上限"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_projections_cannot_serialize_credentials() {
        let llm = project_llm(LlmConfig {
            base_url: "https://example.invalid/v1".into(),
            api_key: "llm-secret-must-not-leak".into(),
            model: "fixture-model".into(),
            temperature: Some(0.2),
            max_tokens: Some(512),
            timeout_secs: 30,
        });
        let email = EmailConfig {
            accounts: vec![EmailAccount {
                id: "fixture-mail".into(),
                label: "Fixture mail".into(),
                provider: EmailProvider::Custom,
                address: "fixture@example.invalid".into(),
                endpoints: EmailEndpoints {
                    imap_host: "imap.example.invalid".into(),
                    imap_port: 993,
                    smtp_host: "smtp.example.invalid".into(),
                    smtp_port: 465,
                    smtp_tls: SmtpTls::Wrapper,
                },
                auth: EmailAuth::AppPassword {
                    secret: "mail-secret-must-not-leak".into(),
                },
            }],
        };
        let intents: Vec<_> = email
            .accounts
            .into_iter()
            .map(|account| EmailRestoreIntent {
                id: account.id,
                label: account.label,
                provider: "custom".into(),
                address: account.address,
            })
            .collect();
        let json = serde_json::to_string(&(llm, intents)).unwrap();
        assert!(!json.contains("llm-secret-must-not-leak"));
        assert!(!json.contains("mail-secret-must-not-leak"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn an_existing_broken_config_is_not_misreported_as_absent() {
        let dir = temp_dir("config");
        let path = dir.join("solum-llm.json");
        std::fs::write(&path, "{broken-secret-json").unwrap();
        let error = read_optional_config(&path, "LLM", LlmConfig::from_json).unwrap_err();
        assert!(error.contains("solum-llm.json"));
        assert!(!error.contains("broken-secret-json"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

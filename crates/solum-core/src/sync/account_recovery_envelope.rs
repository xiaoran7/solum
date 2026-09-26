//! undefined
//!
//! Part of the `sync` module; see `mod.rs` for the module contract.

use super::*;

// ---- account recovery envelope -------------------------------------------------

/// Stable wire constants shared by every Solum client. The recipient is fixed
/// server-side: callers cannot use the recovery route as a general envelope
/// directory or ask for another device's entry.
pub const RECOVERY_KEY_VERSION: u32 = 1;
pub const RECOVERY_ALGORITHM: &str = "recovery-xchacha20poly1305";
const RECOVERY_DOMAIN: &str = "solum-sync-recovery-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryProvision {
    /// This account had no envelope; this device generated the random master
    /// key and won the create-if-absent race.
    Created,
    /// The server envelope supplied the key for a profile that had no key.
    Recovered,
    /// The local key and server envelope already represented the same key.
    Verified,
    /// An existing local key was attached to an account that had no envelope.
    Attached,
}

/// Small transport seam keeps the create/recover/conflict state machine
/// testable without a socket. `put_if_absent` returns the effective envelope,
/// which may belong to another first device that won the race.
pub trait RecoveryEnvelopeTransport {
    fn get(&self, session: &crate::account::AccountSession) -> Result<Option<Vec<u8>>>;
    fn put_if_absent(
        &self,
        session: &crate::account::AccountSession,
        envelope: &[u8],
    ) -> Result<Vec<u8>>;
}

pub struct HttpRecoveryEnvelopeTransport {
    agent: ureq::Agent,
}

impl HttpRecoveryEnvelopeTransport {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
        }
    }

    fn endpoint(session: &crate::account::AccountSession) -> String {
        format!("{}/v1/keys/recovery", session.server_url)
    }

    fn decode_response(value: &serde_json::Value) -> Result<Option<Vec<u8>>> {
        let Some(encoded) = value.get("envelope").and_then(|item| item.as_str()) else {
            return Ok(None);
        };
        let envelope = B64
            .decode(encoded)
            .map_err(|_| CoreError::Invalid("服务器返回了损坏的恢复密钥信封".into()))?;
        if envelope.len() < 24 + 16 || envelope.len() > 65_536 {
            return Err(CoreError::Invalid(
                "服务器返回的恢复密钥信封长度不合法".into(),
            ));
        }
        Ok(Some(envelope))
    }

    fn map_error(error: ureq::Error) -> CoreError {
        match error {
            ureq::Error::Status(401, _) => CoreError::Invalid("登录已过期，请重新登录".into()),
            ureq::Error::Status(404, _) => CoreError::Invalid(
                "账号服务器版本过旧：缺少同步密钥恢复接口，请先升级 solum-cloud".into(),
            ),
            ureq::Error::Status(code, _) => {
                CoreError::Invalid(format!("同步密钥恢复服务暂时不可用（{code}）"))
            }
            other => CoreError::Invalid(format!(
                "无法连接同步密钥恢复服务：{}",
                humanize_transport_error(&other)
            )),
        }
    }
}

impl Default for HttpRecoveryEnvelopeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryEnvelopeTransport for HttpRecoveryEnvelopeTransport {
    fn get(&self, session: &crate::account::AccountSession) -> Result<Option<Vec<u8>>> {
        let response = self
            .agent
            .get(&Self::endpoint(session))
            .set("Authorization", &format!("Bearer {}", session.access_token))
            .call()
            .map_err(Self::map_error)?;
        let value: serde_json::Value = response
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("恢复接口响应不是 JSON：{e}")))?;
        Self::decode_response(&value)
    }

    fn put_if_absent(
        &self,
        session: &crate::account::AccountSession,
        envelope: &[u8],
    ) -> Result<Vec<u8>> {
        let response = self
            .agent
            .put(&Self::endpoint(session))
            .set("Authorization", &format!("Bearer {}", session.access_token))
            .send_json(serde_json::json!({
                "key_version": RECOVERY_KEY_VERSION,
                "algorithm": RECOVERY_ALGORITHM,
                "envelope": B64.encode(envelope),
            }))
            .map_err(Self::map_error)?;
        let value: serde_json::Value = response
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("恢复接口响应不是 JSON：{e}")))?;
        Self::decode_response(&value)?
            .ok_or_else(|| CoreError::Invalid("恢复接口没有返回有效的密钥信封".into()))
    }
}

fn recovery_aad(user_id: &str) -> Vec<u8> {
    format!("{RECOVERY_DOMAIN}\0{user_id}\0{RECOVERY_KEY_VERSION}").into_bytes()
}

fn derive_recovery_key(user_id: &str, password: &str) -> Result<[u8; 32]> {
    use sha2::{Digest, Sha256};

    if !crate::account::is_valid_user_id(user_id) || password.is_empty() {
        return Err(CoreError::Invalid(
            "恢复同步密钥需要有效账号和非空登录密码".into(),
        ));
    }
    let salt = Sha256::digest(format!("{RECOVERY_DOMAIN}:salt:{user_id}").as_bytes());
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ROUNDS, &mut key);
    Ok(key)
}

pub(super) fn wrap_recovery_key(
    user_id: &str,
    password: &str,
    master_key: &[u8; 32],
) -> Result<Vec<u8>> {
    let key = derive_recovery_key(user_id, password)?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let mut nonce = [0u8; 24];
    getrandom::getrandom(&mut nonce).map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: master_key,
                aad: &recovery_aad(user_id),
            },
        )
        .map_err(|_| CoreError::Invalid("同步主密钥包装失败".into()))?;
    let mut envelope = nonce.to_vec();
    envelope.extend(ciphertext);
    Ok(envelope)
}

pub(super) fn unwrap_recovery_key(
    user_id: &str,
    password: &str,
    envelope: &[u8],
) -> Result<[u8; 32]> {
    if envelope.len() < 24 + 16 {
        return Err(CoreError::Invalid("恢复密钥信封太短".into()));
    }
    let key = derive_recovery_key(user_id, password)?;
    let (nonce, ciphertext) = envelope.split_at(24);
    let plaintext = XChaCha20Poly1305::new((&key).into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &recovery_aad(user_id),
            },
        )
        .map_err(|_| {
            CoreError::Invalid("无法解开同步密钥：账号密码不匹配，或服务器信封已损坏".into())
        })?;
    plaintext
        .try_into()
        .map_err(|_| CoreError::Invalid("恢复信封中的同步主密钥长度不正确".into()))
}

pub(super) fn read_local_account_key(path: &std::path::Path) -> Result<Option<[u8; 32]>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CoreError::Storage(error.to_string())),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    if value.get("token").is_some() || value.get("username").is_some() {
        return Err(CoreError::Invalid(
            "账号 profile 中存在旧版同步配置，请先显式迁移，系统不会自动覆盖".into(),
        ));
    }
    let key = value
        .get("key")
        .and_then(|item| item.as_str())
        .ok_or_else(|| CoreError::Invalid("账号 profile 的同步配置缺少 key".into()))?;
    SyncConfig {
        url: String::new(),
        token: String::new(),
        key: key.to_string(),
        account: None,
    }
    .key_bytes()
    .map(Some)
}

fn save_account_sync_config(
    path: &std::path::Path,
    server_url: &str,
    key: &[u8; 32],
) -> Result<()> {
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "url": server_url,
        "key": to_hex(key),
    }))?;
    crate::fsatomic::write_atomic(path, &json)
}

/// Ensure one authenticated account profile has the same random E2EE master
/// key as its server-held recovery envelope. Server writes are create-only;
/// an existing local/server mismatch is never repaired by overwriting either
/// side because that would make old ciphertext silently unreadable.
pub fn provision_account_sync_with<T: RecoveryEnvelopeTransport>(
    transport: &T,
    session: &crate::account::AccountSession,
    password: &str,
    config_path: &std::path::Path,
) -> Result<RecoveryProvision> {
    let user_id = session
        .stable_user_id()
        .ok_or_else(|| CoreError::Invalid("账号缺少不可变 user_id".into()))?;
    let local = read_local_account_key(config_path)?;
    if let Some(remote) = transport.get(session)? {
        let recovered = unwrap_recovery_key(user_id, password, &remote)?;
        if let Some(existing) = local {
            if existing != recovered {
                return Err(CoreError::Invalid(
                    "本机同步主密钥与账号恢复信封冲突；为避免数据损坏，已停止登录切换".into(),
                ));
            }
            save_account_sync_config(config_path, &session.server_url, &existing)?;
            return Ok(RecoveryProvision::Verified);
        }
        save_account_sync_config(config_path, &session.server_url, &recovered)?;
        return Ok(RecoveryProvision::Recovered);
    }

    let (candidate, had_local) = match local {
        Some(key) => (key, true),
        None => {
            let mut key = [0u8; 32];
            getrandom::getrandom(&mut key)
                .map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
            (key, false)
        }
    };
    let proposed = wrap_recovery_key(user_id, password, &candidate)?;
    let effective = transport.put_if_absent(session, &proposed)?;
    let recovered = unwrap_recovery_key(user_id, password, &effective)?;
    if had_local && candidate != recovered {
        return Err(CoreError::Invalid(
            "另一台设备已为该账号建立不同的同步主密钥；本机旧密钥未被覆盖".into(),
        ));
    }
    save_account_sync_config(config_path, &session.server_url, &recovered)?;
    Ok(if had_local {
        RecoveryProvision::Attached
    } else if effective == proposed {
        RecoveryProvision::Created
    } else {
        RecoveryProvision::Recovered
    })
}

pub fn provision_account_sync(
    session: &crate::account::AccountSession,
    password: &str,
    config_path: &std::path::Path,
) -> Result<RecoveryProvision> {
    provision_account_sync_with(
        &HttpRecoveryEnvelopeTransport::new(),
        session,
        password,
        config_path,
    )
}

/// Read the account sync config from an explicit path (no env overrides).
/// `None` when the file does not exist yet.
pub fn load_account_sync_config(config_path: &std::path::Path) -> Option<SyncConfig> {
    let raw = std::fs::read_to_string(config_path).ok()?;
    match serde_json::from_str::<SyncConfigFile>(&raw).ok()? {
        SyncConfigFile::Direct { url, token, key } => Some(SyncConfig {
            url,
            token,
            key,
            account: None,
        }),
        SyncConfigFile::Credentials {
            url,
            username,
            password,
        } => {
            let (token, key) = derive_credentials(&username, &password).ok()?;
            Some(SyncConfig {
                url,
                token,
                key,
                account: None,
            })
        }
        SyncConfigFile::Account { url, key } => Some(SyncConfig {
            url,
            token: String::new(),
            key,
            account: None,
        }),
    }
}

/// Persist the account sync config to an explicit path. Public for the
/// recovery-code login flow, which writes a key recovered from the server
/// envelope before the profile goes live.
pub fn save_account_sync_config_public(
    path: &std::path::Path,
    server_url: &str,
    key: &[u8; 32],
) -> Result<()> {
    save_account_sync_config(path, server_url, key)
}

/// After a recovery-code login wrote the unwrapped master key to disk, verify
/// the persisted file really holds that key before the profile goes live.
/// The code envelope was already authenticated by AEAD; this is the remaining
/// local-vs-disk integrity check and never overwrites either side.
pub fn verify_account_sync_provision(
    session: &crate::account::AccountSession,
    expected_key: &[u8; 32],
    config_path: &std::path::Path,
) -> Result<()> {
    if session.stable_user_id().is_none() {
        return Err(CoreError::Invalid("账号缺少不可变 user_id".into()));
    }
    let local = read_local_account_key(config_path)?;
    match local {
        Some(existing) => {
            if existing != *expected_key {
                return Err(CoreError::Invalid(
                    "本机同步主密钥与恢复码解出的密钥不一致；已停止登录切换".into(),
                ));
            }
            Ok(())
        }
        None => Err(CoreError::Invalid("同步密钥配置写入失败，请重试".into())),
    }
}

/// Change the account password end-to-end: derive the new password's wrap
/// key, rewrap the master key into a fresh envelope client-side, then let the
/// server swap credential hash + envelope atomically. The old password stays
/// the only live credential until the server commit succeeds.
pub fn change_account_password(
    session: &crate::account::AccountSession,
    current_password: &str,
    new_password: &str,
    master_key: &[u8; 32],
) -> Result<String> {
    use base64::Engine;

    let user_id = session
        .stable_user_id()
        .ok_or_else(|| CoreError::Invalid("账号缺少不可变 user_id".into()))?;
    if new_password.chars().count() < 8 {
        return Err(CoreError::Invalid("新密码至少需要 8 个字符".into()));
    }
    // Rewrap first: nothing changes remotely until the new envelope exists.
    let proposed = wrap_recovery_key(user_id, new_password, master_key)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let response = agent
        .post(&format!("{}/v1/account/password", session.server_url))
        .set("Authorization", &format!("Bearer {}", session.access_token))
        .send_json(serde_json::json!({
            "current_password": current_password,
            "new_password": new_password,
            "password_envelope": B64.encode(proposed),
        }))
        .map_err(|error| match error {
            ureq::Error::Status(401, _) => CoreError::Invalid("登录已过期，请重新登录".into()),
            ureq::Error::Status(403, _) => CoreError::Invalid("当前密码不正确".into()),
            ureq::Error::Status(code, _) => {
                CoreError::Invalid(format!("密码修改暂时不可用（{code}）"))
            }
            other => CoreError::Invalid(format!(
                "无法连接密码修改服务：{}",
                humanize_transport_error(&other)
            )),
        })?;
    if response.status() != 200 {
        return Err(CoreError::Invalid(format!(
            "密码修改暂时不可用（{}）",
            response.status()
        )));
    }
    Ok("密码已更新。其他设备需要用新密码重新登录".to_string())
}

pub(super) fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

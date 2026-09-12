//! Recovery code: a user-chosen second key independent of the login password.
//!
//! The code is generated on-device, shown once to the user, and never leaves
//! the device. What the server stores is an envelope wrapping the same E2EE
//! master key as the password envelope (independent key domain), plus a
//! client-computed SHA-256 of the derived wrap key used only for online
//! verification. Losing both the password and the code is unrecoverable by
//! design; the server cannot decrypt backups either way.

use crate::error::{CoreError, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const RECOVERY_CODE_ALGORITHM: &str = "recovery-code-xchacha20poly1305";
pub const RECOVERY_CODE_KEY_VERSION: u32 = 1;
const RECOVERY_CODE_DOMAIN: &str = "solum-sync-recovery-code-v1";
/// Crockford Base32 alphabet: no I, L, O, U to prevent transcription errors.
const CODE_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 20 characters over 32 symbols = 100 bits of entropy.
const CODE_LEN: usize = 20;
pub const PBKDF2_ROUNDS: u32 = 600_000;

/// A generated recovery code plus everything the server needs to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCodeIssuance {
    /// Canonical display form `XXXXX-XXXXX-XXXXX-XXXXX`. Show once; the user
    /// is responsible for saving it. Never sent to the server.
    pub display_code: String,
    /// Opaque envelope bytes for `PUT /v1/account/recovery-code`.
    pub envelope: Vec<u8>,
    /// Hex SHA-256 of the derived wrap key for online verification.
    pub verify_hash: String,
}

/// Normalize a recovery code as typed into the compact 20-character derivation
/// form: strip separators and whitespace, uppercase, and map common
/// confusables (`I/L`→`1`, `O`→`0`). No checksum exists — validity can only be
/// decided by unwrapping or server verification.
pub fn normalize_code(input: &str) -> String {
    let mut out = String::with_capacity(CODE_LEN);
    for ch in input.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' => {
                let upper = ch.to_ascii_uppercase();
                let mapped = match upper {
                    'I' | 'L' => '1',
                    'O' => '0',
                    other => other,
                };
                out.push(mapped);
            }
            '0'..='9' => out.push(ch),
            '-' | ' ' | '\t' | '\'' | '’' => {}
            _ => {}
        }
    }
    out
}

/// Generate a new recovery code and wrap the account master key with it.
///
/// The salt is stored inside the envelope (`salt ‖ nonce ‖ ciphertext`) so the
/// challenge endpoint can hand it back without any extra round trip. The AAD
/// binds the independent domain, immutable user id, and protocol version.
pub fn issue_recovery_code(user_id: &str, master_key: &[u8; 32]) -> Result<RecoveryCodeIssuance> {
    if !crate::account::is_valid_user_id(user_id) {
        return Err(CoreError::Invalid("恢复码需要有效的账号标识".into()));
    }
    let mut raw = [0u8; CODE_LEN];
    getrandom::getrandom(&mut raw).map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
    let compact: String = raw
        .iter()
        .map(|&b| CODE_ALPHABET[(b as usize) % CODE_ALPHABET.len()] as char)
        .collect();
    let display_code = format!(
        "{}-{}-{}-{}",
        &compact[0..5],
        &compact[5..10],
        &compact[10..15],
        &compact[15..20]
    );

    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
    let wrap_key = derive_wrap_key(&salt, &compact)?;
    let cipher = XChaCha20Poly1305::new((&wrap_key).into());
    let mut nonce = [0u8; 24];
    getrandom::getrandom(&mut nonce).map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: master_key,
                aad: &recovery_code_aad(user_id),
            },
        )
        .map_err(|_| CoreError::Invalid("恢复码信封生成失败".into()))?;
    let mut envelope = salt.to_vec();
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);

    Ok(RecoveryCodeIssuance {
        display_code,
        envelope,
        verify_hash: wrap_key_digest_hex(&wrap_key),
    })
}

fn wrap_key_digest_hex(wrap_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(wrap_key);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn recovery_code_aad(user_id: &str) -> Vec<u8> {
    format!("{RECOVERY_CODE_DOMAIN}\0{user_id}\0{RECOVERY_CODE_KEY_VERSION}").into_bytes()
}

/// PBKDF2-HMAC-SHA256 from (salt, normalized code).
///
/// Deliberately does NOT mix in the user id: a recovering device must be able
/// to compute the verification digest before it knows which account it is
/// talking about (the challenge returns only the salt). Cross-account domain
/// separation comes from the per-issuance random 128-bit salt; the envelope's
/// AEAD AAD still binds the immutable user id so ciphertext cannot be moved
/// between accounts.
fn derive_wrap_key(salt: &[u8; 16], normalized_code: &str) -> Result<[u8; 32]> {
    if normalized_code.chars().count() != CODE_LEN {
        return Err(CoreError::Invalid("恢复码格式不正确".into()));
    }
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(normalized_code.as_bytes(), salt, PBKDF2_ROUNDS, &mut key);
    Ok(key)
}

/// Recover the master key from an envelope fetched from
/// `GET /v1/keys/recovery-code` using the code the user typed.
pub fn unwrap_master_key(
    user_id: &str,
    normalized_code: &str,
    envelope: &[u8],
) -> Result<[u8; 32]> {
    if !crate::account::is_valid_user_id(user_id) || normalized_code.is_empty() {
        return Err(CoreError::Invalid("解开恢复码需要有效账号和恢复码".into()));
    }
    if envelope.len() < 16 + 24 + 16 {
        return Err(CoreError::Invalid("恢复码信封长度不合法".into()));
    }
    let (salt, rest) = envelope.split_at(16);
    let (nonce, ciphertext) = rest.split_at(24);
    let mut salt_arr = [0u8; 16];
    salt_arr.copy_from_slice(salt);
    let wrap_key = derive_wrap_key(&salt_arr, normalized_code)?;
    let plaintext = XChaCha20Poly1305::new((&wrap_key).into())
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &recovery_code_aad(user_id),
            },
        )
        .map_err(|_| CoreError::Invalid("恢复码不正确，或服务器信封已损坏".into()))?;
    plaintext
        .try_into()
        .map_err(|_| CoreError::Invalid("恢复码信封中的同步主密钥长度不正确".into()))
}

/// Compute the verification digest a client submits to `/v1/auth/recover`:
/// SHA-256 of the wrap key derived from the challenge salt and the typed code.
pub fn compute_verify_hash(salt_hex: &str, normalized_code: &str) -> Result<String> {
    let decoded = decode_even_hex(salt_hex)
        .ok_or_else(|| CoreError::Invalid("服务器返回的恢复码盐值不合法".into()))?;
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&decoded);
    let wrap_key = derive_wrap_key(&salt, normalized_code)?;
    Ok(wrap_key_digest_hex(&wrap_key))
}

fn decode_even_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..16)
        .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// Park a freshly issued display code locally until the UI has shown it.
///
/// Account login restarts the whole app before the webview can render
/// anything, so the code issued at first login cannot be handed to the
/// current page. It is written to this local-only file instead, and the next
/// startup shows it until the user acknowledges saving it. The file shares
/// the trust domain of the local sync-key file (plaintext on disk) and is
/// deleted on acknowledgement or when a new code is issued.
pub fn save_pending_display(path: &std::path::Path, display_code: &str) -> Result<()> {
    let normalized = normalize_code(display_code);
    if normalized.chars().count() != CODE_LEN {
        return Err(CoreError::Invalid("待展示恢复码格式不正确".into()));
    }
    crate::fsatomic::write_atomic(path, &normalized)
}

/// Read the parked display code, if any. Content that no longer normalizes
/// to a 20-character code is treated as absent: a corrupt code cannot be
/// typed correctly anyway, and acknowledgement will clean the file up.
pub fn load_pending_display(path: &std::path::Path) -> Result<Option<String>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CoreError::Storage(error.to_string())),
    };
    let normalized = normalize_code(&raw);
    if normalized.chars().count() != CODE_LEN {
        return Ok(None);
    }
    Ok(Some(normalized))
}

/// Delete the parked display code after the user acknowledged saving it (or
/// after a re-issue made it stale). Missing file counts as success.
pub fn clear_pending_display(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::Storage(error.to_string())),
    }
}

/// Transport seam mirroring `RecoveryEnvelopeTransport`: keeps the whole
/// recovery-code state machine testable without a socket.
pub trait RecoveryCodeTransport {
    fn challenge(&self, server_url: &str, username: &str) -> Result<String>;
    fn recover_session(
        &self,
        server_url: &str,
        username: &str,
        verify_hash: &str,
    ) -> Result<crate::account::AccountSession>;
    fn put_envelope(
        &self,
        session: &crate::account::AccountSession,
        envelope: &[u8],
        verify_hash: &str,
    ) -> Result<()>;
    fn get_envelope(&self, session: &crate::account::AccountSession) -> Result<Option<Vec<u8>>>;
}

pub struct HttpRecoveryCodeTransport {
    agent: ureq::Agent,
}

impl Default for HttpRecoveryCodeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpRecoveryCodeTransport {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
        }
    }

    fn map_error(error: ureq::Error) -> CoreError {
        match error {
            ureq::Error::Status(401, _) => CoreError::Invalid("登录已过期，请重新登录".into()),
            ureq::Error::Status(403, _) => CoreError::Invalid("当前凭据不正确".into()),
            ureq::Error::Status(404, _) => CoreError::Invalid(
                "账号服务器版本过旧：缺少恢复码接口，请先升级 solum-cloud".into(),
            ),
            ureq::Error::Status(code, _) => {
                CoreError::Invalid(format!("恢复码服务暂时不可用（{code}）"))
            }
            other => CoreError::Invalid(format!(
                "无法连接恢复码服务：{}",
                crate::sync::humanize_transport_error(&other)
            )),
        }
    }
}

impl RecoveryCodeTransport for HttpRecoveryCodeTransport {
    fn challenge(&self, server_url: &str, username: &str) -> Result<String> {
        let response = self
            .agent
            .post(&format!("{server_url}/v1/auth/recover/challenge"))
            .send_json(json!({ "username": username }))
            .map_err(Self::map_error)?;
        let value: serde_json::Value = response
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("恢复码挑战响应不是 JSON：{e}")))?;
        let salt = value
            .get("pbkdf2_salt")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CoreError::Invalid("服务器没有返回恢复码盐值".into()))?;
        if decode_even_hex(salt).is_none() {
            return Err(CoreError::Invalid("服务器返回的恢复码盐值不合法".into()));
        }
        Ok(salt.to_string())
    }

    fn recover_session(
        &self,
        server_url: &str,
        username: &str,
        verify_hash: &str,
    ) -> Result<crate::account::AccountSession> {
        let response = self
            .agent
            .post(&format!("{server_url}/v1/auth/recover"))
            .send_json(json!({
                "username": username,
                "verify_hash": verify_hash,
            }))
            .map_err(Self::map_error)?;
        let value: serde_json::Value = response
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("恢复响应不是 JSON：{e}")))?;
        let session_value = value;
        let field = |k: &str| {
            session_value
                .get(k)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        };
        let user_id = session_value
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if user_id.is_empty() || field("access_token").is_empty() {
            return Err(CoreError::Invalid("服务器返回了无法识别的恢复结果".into()));
        }
        let mut session = crate::account::AccountSession {
            server_url: server_url.to_string(),
            user_id: user_id.to_string(),
            username: field("username").to_string(),
            access_token: field("access_token").to_string(),
            refresh_token: field("refresh_token").to_string(),
            model: String::new(),
        };
        session
            .normalize()
            .map_err(|_| CoreError::Invalid("服务器返回了无法识别的恢复结果".into()))?;
        Ok(session)
    }

    fn put_envelope(
        &self,
        session: &crate::account::AccountSession,
        envelope: &[u8],
        verify_hash: &str,
    ) -> Result<()> {
        self.agent
            .put(&format!("{}/v1/account/recovery-code", session.server_url))
            .set("Authorization", &format!("Bearer {}", session.access_token))
            .send_json(json!({
                "key_version": RECOVERY_CODE_KEY_VERSION,
                "algorithm": RECOVERY_CODE_ALGORITHM,
                "envelope": B64.encode(envelope),
                "verify_hash": verify_hash,
            }))
            .map_err(Self::map_error)?;
        Ok(())
    }

    fn get_envelope(&self, session: &crate::account::AccountSession) -> Result<Option<Vec<u8>>> {
        let response = self
            .agent
            .get(&format!("{}/v1/keys/recovery-code", session.server_url))
            .set("Authorization", &format!("Bearer {}", session.access_token))
            .call()
            .map_err(Self::map_error)?;
        let value: serde_json::Value = response
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("恢复码查询响应不是 JSON：{e}")))?;
        let Some(encoded) = value.get("envelope").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        let envelope = B64
            .decode(encoded)
            .map_err(|_| CoreError::Invalid("服务器返回了损坏的恢复码信封".into()))?;
        if envelope.len() < 16 + 24 + 16 {
            return Err(CoreError::Invalid(
                "服务器返回的恢复码信封长度不合法".into(),
            ));
        }
        Ok(Some(envelope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_ID: &str = "11111111-2222-4333-8444-555555555555";

    fn test_master_key() -> [u8; 32] {
        let mut key = [7u8; 32];
        key[0] = 42;
        key
    }

    #[test]
    fn normalizes_separators_case_and_confusables() {
        assert_eq!(
            normalize_code("abcde-fghjk-mnpqr-stvwx"),
            "ABCDEFGHJKMNPQRSTVWX"
        );
        assert_eq!(
            normalize_code("ABCDE FGHJK MNPQR STVWX"),
            "ABCDEFGHJKMNPQRSTVWX"
        );
        assert_eq!(normalize_code("i l o"), "110");
        assert_eq!(normalize_code("IL Oi"), "1101");
        assert_eq!(normalize_code(""), "");
        // Non-alphanumeric noise is dropped, not treated as an error: the
        // unwrap itself decides whether the result is valid.
        assert_eq!(normalize_code("'ab-cd'"), "ABCD");
        // The display form normalizes back to its own compact derivation form.
        let issuance = issue_recovery_code(USER_ID, &test_master_key()).unwrap();
        assert_eq!(
            normalize_code(&issuance.display_code),
            normalize_code(&issuance.display_code.replace('-', "").to_lowercase())
        );
        assert_eq!(normalize_code(&issuance.display_code).len(), CODE_LEN);
    }

    #[test]
    fn issuance_roundtrips_through_unwrap() {
        let master = test_master_key();
        let issuance = issue_recovery_code(USER_ID, &master).unwrap();
        let typed = normalize_code(&issuance.display_code.replace('-', "").to_lowercase());
        let normalized = normalize_code(&issuance.display_code);
        assert_eq!(typed.len(), CODE_LEN);
        let recovered = unwrap_master_key(USER_ID, &normalized, &issuance.envelope).unwrap();
        assert_eq!(recovered, master);
    }

    #[test]
    fn wrong_code_or_user_fails_closed() {
        let master = test_master_key();
        let issuance = issue_recovery_code(USER_ID, &master).unwrap();
        let wrong_user = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        assert!(unwrap_master_key(wrong_user, "ABCDEFGHIJKLMNOPQRST", &issuance.envelope).is_err());
        let mut other = normalize_code(&issuance.display_code).into_bytes();
        other[0] = if other[0] == b'A' { b'B' } else { b'A' };
        let other_code = String::from_utf8(other).unwrap();
        assert!(unwrap_master_key(USER_ID, &other_code, &issuance.envelope).is_err());
    }

    #[test]
    fn verify_hash_matches_derivation_and_rejects_bad_salt() {
        let master = test_master_key();
        let issuance = issue_recovery_code(USER_ID, &master).unwrap();
        let normalized = normalize_code(&issuance.display_code);
        // The stored verify hash must equal what a client would submit after
        // deriving the same wrap key from the challenge salt.
        let salt_hex = hex_encode(&issuance.envelope[..16]);
        let submitted = compute_verify_hash(&salt_hex, &normalized).unwrap();
        assert_eq!(submitted, issuance.verify_hash);
        assert!(compute_verify_hash("zz", &normalized).is_err());
        assert!(compute_verify_hash("", &normalized).is_err());
    }

    #[test]
    fn codes_are_unique_and_display_grouped() {
        let master = test_master_key();
        let first = issue_recovery_code(USER_ID, &master).unwrap();
        let second = issue_recovery_code(USER_ID, &master).unwrap();
        assert_ne!(first.display_code, second.display_code);
        assert_ne!(first.envelope, second.envelope);
        let parts: Vec<&str> = first.display_code.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert!(parts.iter().all(|p| p.len() == 5));
        for ch in first.display_code.chars() {
            if ch != '-' {
                assert!(CODE_ALPHABET.contains(&(ch as u8)));
            }
        }
    }

    #[test]
    fn rejects_invalid_inputs() {
        let master = test_master_key();
        assert!(issue_recovery_code("not-a-uuid", &master).is_err());
        let issuance = issue_recovery_code(USER_ID, &master).unwrap();
        assert!(unwrap_master_key(USER_ID, "SHORT", &issuance.envelope).is_err());
        assert!(unwrap_master_key(USER_ID, "ABCDEFGHIJKLMNOPQRST", &[]).is_err());
    }

    fn temp_pending_path(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solum-recovery-pending-{}-{}",
            label,
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("pending-recovery-code.txt")
    }

    #[test]
    fn pending_display_roundtrips_and_normalizes() {
        let path = temp_pending_path("roundtrip");
        let issuance = issue_recovery_code(USER_ID, &test_master_key()).unwrap();
        // Display form (with dashes) is parked in canonical compact form.
        save_pending_display(&path, &issuance.display_code).unwrap();
        assert_eq!(
            load_pending_display(&path).unwrap().as_deref(),
            Some(normalize_code(&issuance.display_code).as_str())
        );
        clear_pending_display(&path).unwrap();
        assert_eq!(load_pending_display(&path).unwrap(), None);
        // Clearing an already-absent file stays a success.
        clear_pending_display(&path).unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pending_display_rejects_bad_code_and_corrupt_content() {
        let path = temp_pending_path("bad");
        assert!(save_pending_display(&path, "SHORT").is_err());
        // Nothing was written by the failed save.
        assert_eq!(load_pending_display(&path).unwrap(), None);
        std::fs::write(&path, "乱写的垃圾内容").unwrap();
        assert_eq!(load_pending_display(&path).unwrap(), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

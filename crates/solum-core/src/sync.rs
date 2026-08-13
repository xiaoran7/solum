//! Multi-device sync client (F17, ARCHITECTURE.md §3.8) — local-first.
//!
//! Each device's SQLite store is authoritative. Mutations are captured into
//! `sync_oplog` by triggers (see `store::migrate_sync`); this module pushes
//! locally-originated ops to the relay as an **end-to-end encrypted** blob and
//! pulls + merges other devices' blobs (last-write-wins per row on a
//! `(hlc, origin)` stamp). The relay (`solum-sync-server`) only ever sees
//! ciphertext. The logged-in Solum account selects the relay tenant, while a
//! separate symmetric key in `solum-sync.json` remains device-only and opens
//! the end-to-end encrypted blobs. Legacy direct relay tokens remain readable.
//! The file's path can be overridden with `SOLUM_SYNC_CONFIG`, same as
//! `SOLUM_LLM_CONFIG`; without an explicit override it follows the active
//! account profile and stays in the historical app-data location for guest.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::store::Store;

/// One captured mutation, as it travels between devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOp {
    pub tbl: String,
    pub guid: String,
    /// `"upsert"` or `"delete"`.
    pub op: String,
    /// Row JSON (FKs already translated to guids); `None` for deletes.
    pub payload: Option<String>,
    /// Millisecond UTC wall-clock stamp — the LWW ordering key.
    pub hlc: String,
    /// Device id that originated the change (LWW tie-breaker).
    pub origin: String,
}

/// The plaintext of one pushed blob.
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncBatch {
    pub device: String,
    pub ops: Vec<SyncOp>,
}

/// A blob as returned by the relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulledBlob {
    pub seq: i64,
    pub device: String,
    #[serde(rename = "blob")]
    pub blob_b64: String,
}

/// What one `sync_once` did (for CLI / UI display).
#[derive(Debug, Default, Serialize)]
pub struct SyncOutcome {
    /// Local ops uploaded this round.
    pub pushed: usize,
    /// Remote blobs downloaded this round.
    pub pulled_blobs: usize,
    /// Remote ops merged into the local store.
    pub applied: usize,
    /// Remote ops skipped (older than local state, duplicates, orphans).
    pub skipped: usize,
    /// Remote ops this build could not interpret and parked for a later one
    /// (unknown table, unusable payload). Non-zero means a peer is ahead of
    /// this device — the data is held, not lost.
    pub quarantined: usize,
    /// Blobs that could not be opened at all (bad ciphertext, wrong key) and
    /// were parked so the cursor could advance. Non-zero is a real problem
    /// worth surfacing — usually the devices' sync keys disagree.
    pub bad_blobs: usize,
    /// This device's cursor pointed below the relay's retention floor, so an
    /// unknown number of remote ops are gone for good. Set means **this sync
    /// was not complete**; the device needs a fresh full copy from a peer
    /// (export → import) rather than more incremental rounds.
    pub history_gap: bool,
}

// ---- config -------------------------------------------------------------------

/// Client-side sync settings. Loaded from env (`SOLUM_SYNC_URL` / `SOLUM_SYNC_KEY`,
/// plus optional legacy `SOLUM_SYNC_TOKEN`) first, then a `solum-sync.json` file — path from
/// `SOLUM_SYNC_CONFIG` if set, else the active account profile (guest retains
/// the historical app-data path). The recommended shape is `{url,key}` and takes its
/// access token from `solum-account.json`. The legacy `{url,token,key}` and
/// `{url,username,password}` shapes remain readable for migration. Absent
/// config or an account for the recommended shape means sync is off.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncConfig {
    /// Relay base URL, e.g. `http://127.0.0.1:8787`.
    pub url: String,
    /// Legacy relay token. Empty when account authentication is active.
    pub token: String,
    /// 64 hex chars — the 32-byte pre-shared master key (§3.8).
    pub key: String,
    /// Logged-in identity used to select the relay tenant. Never serialized
    /// into `solum-sync.json`; its source of truth is `solum-account.json`.
    #[serde(skip)]
    pub account: Option<crate::account::AccountSession>,
}

/// The two shapes `solum-sync.json` may take. Untagged so existing raw
/// `{url,token,key}` files (already deployed, gitignored on every device)
/// keep parsing unchanged.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SyncConfigFile {
    Credentials {
        url: String,
        username: String,
        password: String,
    },
    Direct {
        url: String,
        token: String,
        key: String,
    },
    Account {
        url: String,
        key: String,
    },
}

impl SyncConfig {
    pub fn load() -> Option<SyncConfig> {
        if let (Ok(url), Ok(key)) = (
            std::env::var("SOLUM_SYNC_URL"),
            std::env::var("SOLUM_SYNC_KEY"),
        ) {
            if let Ok(token) = std::env::var("SOLUM_SYNC_TOKEN") {
                return Some(SyncConfig {
                    url,
                    token,
                    key,
                    account: None,
                });
            }
            return crate::account::AccountSession::load()
                .filter(|account| account.stable_user_id().is_some())
                .map(|account| SyncConfig {
                    url,
                    token: String::new(),
                    key,
                    account: Some(account),
                });
        }
        let path: std::path::PathBuf = match std::env::var("SOLUM_SYNC_CONFIG") {
            Ok(p) => p.into(),
            Err(_) => crate::paths::resolve_profile_with_adoption("solum-sync.json"),
        };
        let raw = std::fs::read_to_string(path).ok()?;
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
            SyncConfigFile::Account { url, key } => crate::account::AccountSession::load()
                .filter(|account| account.stable_user_id().is_some())
                .map(|account| SyncConfig {
                    url,
                    token: String::new(),
                    key,
                    account: Some(account),
                }),
        }
    }

    /// Redacted one-liner for status displays.
    pub fn masked_summary(&self) -> String {
        let auth = self
            .account
            .as_ref()
            .map(|session| format!("账号 {}", session.username))
            .unwrap_or_else(|| "兼容令牌 legacy".to_string());
        format!(
            "{} · {} · key ****{}",
            self.url,
            auth,
            &self.key[self.key.len().saturating_sub(4)..]
        )
    }

    fn key_bytes(&self) -> Result<[u8; 32]> {
        let hex = self.key.trim();
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CoreError::Invalid(
                "SOLUM_SYNC_KEY 必须是 64 位十六进制（32 字节主密钥）".into(),
            ));
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| CoreError::Invalid("SOLUM_SYNC_KEY 不是合法十六进制".into()))?;
        }
        Ok(out)
    }
}

/// Derives `(relay_token_hex, e2e_key_hex)` from a username+password pair
/// (2026-07-22, user-requested — copying a raw 64-hex key to every new device
/// was the friction point; a memorable credential replaces it everywhere the
/// old `{url,token,key}` file still works, see `SyncConfigFile`).
///
/// The relay server is completely unaware of this: it still only ever
/// compares a static Bearer token (`SOLUM_SYNC_SERVER_TOKEN`), and this
/// function's `token` output is nothing more than *the value a human picked
/// as that token, computed the same way on every device* instead of typed in
/// by hand. Same story for the encryption key (§3.8) — the relay never had
/// access to it before this change and still doesn't.
///
/// Two-stage KDF: PBKDF2-HMAC-SHA256 (deliberately slow, password-hardened —
/// resists offline brute force if the relay token ever leaks, which matters
/// more the weaker the chosen password is) stretches the password into one
/// 32-byte intermediate key, salted with a hash of the username so two
/// different usernames sharing a password still diverge; the username is not
/// treated as secret. HKDF-SHA256 (fast) then expands that single
/// intermediate key into two independent 32-byte outputs — domain-separated
/// by the `info` label — so recovering the token reveals nothing about the
/// encryption key or vice versa, even though both trace back to one
/// password.
///
/// **PBKDF2, not Argon2id (2026-07-22 revision)**: this exact algorithm also
/// needs to run in the ops dashboard's vanilla-JS frontend so a browser can
/// derive the same token from a typed username+password (matching the app's
/// login UX everywhere) — and the dashboard's own "no vendored dependencies"
/// rule rules out shipping a JS Argon2 implementation, while hand-rolling
/// Argon2id in JS is not something to risk for security-critical code.
/// PBKDF2 and HKDF are both natively available via every browser's
/// `SubtleCrypto`, so the browser and this function derive bit-identical
/// output with zero added JS dependencies. Trade-off accepted: PBKDF2 is not
/// memory-hard, so it resists GPU/ASIC-parallel brute force less than
/// Argon2id would for the same wall-clock cost — acceptable here because
/// this secures a single-user self-hosted relay, not a high-value target,
/// and the KDF was never going to save a weak password from being weak.
const PBKDF2_ROUNDS: u32 = 600_000;
/// Fixed, explicit HKDF-Extract salt (not the `None` default) so this exactly
/// matches what the browser passes to `SubtleCrypto.deriveBits({name:"HKDF"})`
/// — that API has no "omit the salt" option, so Rust must not rely on
/// `hkdf`'s implicit zero-salt convention either; both sides state it.
const HKDF_SALT: [u8; 32] = [0u8; 32];

pub fn derive_credentials(username: &str, password: &str) -> Result<(String, String)> {
    use sha2::{Digest, Sha256};

    let salt = Sha256::digest(username.as_bytes());
    let mut ikm = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ROUNDS, &mut ikm);

    let hk = hkdf::Hkdf::<Sha256>::new(Some(&HKDF_SALT), &ikm);
    let mut token_bytes = [0u8; 32];
    hk.expand(b"solum-sync-v1:token", &mut token_bytes)
        .map_err(|e| CoreError::Invalid(format!("token 派生失败：{e}")))?;
    let mut key_bytes = [0u8; 32];
    hk.expand(b"solum-sync-v1:key", &mut key_bytes)
        .map_err(|e| CoreError::Invalid(format!("key 派生失败：{e}")))?;

    Ok((to_hex(&token_bytes), to_hex(&key_bytes)))
}

/// Derive only the device-side encryption key for account-authenticated sync.
/// The relay access token comes from `solum-account.json`; this password and
/// its derived key never leave the device.
pub fn derive_sync_key(username: &str, password: &str) -> Result<String> {
    derive_credentials(username, password).map(|(_, key)| key)
}

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
            other => CoreError::Invalid(format!("无法连接同步密钥恢复服务：{other}")),
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

fn wrap_recovery_key(user_id: &str, password: &str, master_key: &[u8; 32]) -> Result<Vec<u8>> {
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

fn unwrap_recovery_key(user_id: &str, password: &str, envelope: &[u8]) -> Result<[u8; 32]> {
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

fn read_local_account_key(path: &std::path::Path) -> Result<Option<[u8; 32]>> {
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

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- crypto (end-to-end; the relay never sees plaintext) ------------------------

/// XChaCha20-Poly1305 with a random 24-byte nonce prepended to the ciphertext.
fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; 24];
    getrandom::getrandom(&mut nonce).map_err(|e| CoreError::Invalid(format!("os rng: {e}")))?;
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| CoreError::Invalid("加密失败".into()))?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(out)
}

fn decrypt(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 24 {
        return Err(CoreError::Invalid("同步数据太短，不是合法密文".into()));
    }
    let (nonce, ct) = blob.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce), ct)
        .map_err(|_| CoreError::Invalid("解密失败——各设备的 SOLUM_SYNC_KEY 是否一致？".into()))
}

// ---- transport ------------------------------------------------------------------

/// How encrypted blobs reach the relay. Abstracted so tests can run the full
/// merge pipeline without a network.
pub trait SyncTransport {
    /// Upload one blob; returns the relay-assigned sequence number.
    fn push(&self, device: &str, blob: &[u8]) -> Result<i64>;
    /// Blobs with `seq > since` from devices other than `device`.
    fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>>;
    /// Lowest sequence the relay still holds, or `None` if it does not say.
    ///
    /// Relays enforce retention, so history below this is gone. A client whose
    /// cursor sits below it has missed ops it can never fetch — and would
    /// otherwise pull "everything after my cursor", get a later unrelated set,
    /// and call that a successful sync. Must be read from the *same* response
    /// as `pull` to be meaningful.
    fn oldest_seq(&self) -> Option<i64> {
        None
    }
}

/// Connect timeout for relay calls. A relay that cannot be reached should fail
/// in seconds, not hang: sync runs on the ticker, and a hung request used to
/// hold the orchestrator lock (and therefore reminders) with it.
const CONNECT_TIMEOUT_SECS: u64 = 10;
/// Whole-request ceiling, connect + TLS + transfer. Bounded on purpose — a
/// half-open connection to a dead relay otherwise blocks until the OS gives up,
/// which on some platforms is minutes.
const REQUEST_TIMEOUT_SECS: u64 = 60;
/// Largest pull response we will buffer. The relay pages at 500 blobs × up to
/// 8 MiB, which is multiple GiB of JSON if a peer (or the relay itself) is
/// hostile or broken. We are not obliged to hold that in memory.
const MAX_PULL_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// The real relay client (`solum-sync-server` wire protocol).
pub struct HttpTransport {
    url: String,
    auth: RelayAuth,
    agent: ureq::Agent,
    /// Captured from the last `pull` response. Interior mutability because the
    /// trait takes `&self` — and it has to come from the *same* response as the
    /// blobs, or the two could describe different relay states.
    oldest_seq: std::cell::Cell<Option<i64>>,
}

enum RelayAuth {
    Legacy(String),
    Account(std::sync::Mutex<crate::account::AccountSession>),
}

enum RelayRequestError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for RelayRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("认证失败"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl HttpTransport {
    /// Fails if the relay URL is not an acceptable endpoint (see
    /// [`crate::net::validate_endpoint`]) — the sync token and every op in the
    /// oplog would otherwise travel in cleartext.
    pub fn new(cfg: &SyncConfig) -> Result<HttpTransport> {
        let url = crate::net::validate_endpoint(&cfg.url, "同步服务器 url")?;
        if let Some(session) = &cfg.account {
            if url != session.server_url {
                return Err(CoreError::Invalid(
                    "账号同步必须使用登录服务器同源地址，已拒绝向其他站点发送账号令牌".into(),
                ));
            }
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build();
        Ok(HttpTransport {
            url,
            auth: match &cfg.account {
                Some(session) => RelayAuth::Account(std::sync::Mutex::new(session.clone())),
                None => RelayAuth::Legacy(cfg.token.clone()),
            },
            agent,
            oldest_seq: std::cell::Cell::new(None),
        })
    }

    fn with_auth<T>(
        &self,
        send: impl Fn(&str) -> std::result::Result<T, RelayRequestError>,
    ) -> Result<T> {
        match &self.auth {
            RelayAuth::Legacy(token) => send(token).map_err(|e| match e {
                RelayRequestError::Unauthorized => CoreError::Invalid("同步服务器认证失败".into()),
                RelayRequestError::Other(message) => {
                    CoreError::Invalid(format!("同步服务器请求失败: {message}"))
                }
            }),
            RelayAuth::Account(session) => {
                let mut session = session
                    .lock()
                    .map_err(|_| CoreError::Invalid("同步账号状态锁中毒".into()))?;
                match send(&session.access_token) {
                    Ok(value) => Ok(value),
                    Err(RelayRequestError::Unauthorized) => {
                        let rotated = crate::account::refresh_and_save(&session)?;
                        *session = rotated;
                        send(&session.access_token).map_err(|e| {
                            CoreError::Invalid(format!("刷新账号后重试同步请求失败: {e}"))
                        })
                    }
                    Err(RelayRequestError::Other(message)) => {
                        Err(CoreError::Invalid(format!("同步服务器请求失败: {message}")))
                    }
                }
            }
        }
    }
}

impl SyncTransport for HttpTransport {
    fn push(&self, device: &str, blob: &[u8]) -> Result<i64> {
        let resp: serde_json::Value = self
            .with_auth(|token| {
                self.agent
                    .post(&format!("{}/v1/push", self.url))
                    .set("Authorization", &format!("Bearer {token}"))
                    .set("X-Device", device)
                    .set("Content-Type", "application/octet-stream")
                    .send_bytes(blob)
                    .map_err(|error| match error {
                        ureq::Error::Status(401, _) => RelayRequestError::Unauthorized,
                        other => RelayRequestError::Other(other.to_string()),
                    })
            })?
            .into_json()
            .map_err(|e| CoreError::Invalid(format!("同步服务器响应不可解析: {e}")))?;
        resp.get("seq")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CoreError::Invalid("push 响应缺少 seq".into()))
    }

    fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>> {
        let reader = self
            .with_auth(|token| {
                self.agent
                    .get(&format!(
                        "{}/v1/pull?since={since}&device={device}",
                        self.url
                    ))
                    .set("Authorization", &format!("Bearer {token}"))
                    .call()
                    .map_err(|error| match error {
                        ureq::Error::Status(401, _) => RelayRequestError::Unauthorized,
                        other => RelayRequestError::Other(other.to_string()),
                    })
            })?
            .into_reader();
        // Read through a cap rather than `into_json()` straight off the socket:
        // `into_json` will happily allocate whatever the peer sends.
        use std::io::Read as _;
        let mut buf = Vec::new();
        reader
            .take(MAX_PULL_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| CoreError::Invalid(format!("读取同步响应失败: {e}")))?;
        if buf.len() > MAX_PULL_RESPONSE_BYTES {
            return Err(CoreError::Invalid(format!(
                "同步响应超过 {} MiB 上限，已拒绝",
                MAX_PULL_RESPONSE_BYTES / 1024 / 1024
            )));
        }
        let body: serde_json::Value = serde_json::from_slice(&buf)
            .map_err(|e| CoreError::Invalid(format!("同步服务器响应不可解析: {e}")))?;
        // Older relays do not send this; `None` then means "cannot tell",
        // which the caller treats as "no gap detected" rather than inventing one.
        self.oldest_seq
            .set(body.get("oldest_seq").and_then(|v| v.as_i64()));
        let blobs = body
            .get("blobs")
            .cloned()
            .ok_or_else(|| CoreError::Invalid("pull 响应缺少 blobs".into()))?;
        Ok(serde_json::from_value(blobs)?)
    }

    fn oldest_seq(&self) -> Option<i64> {
        self.oldest_seq.get()
    }
}

// ---- driver ---------------------------------------------------------------------

const PUSHED_KEY: &str = "sync_pushed_oplog_id";
const PULLED_KEY: &str = "sync_pulled_seq";
/// Records that this device's cursor fell below the relay's retention floor.
/// Durable on purpose: the user has to act on it (re-seed from a peer), and a
/// warning that only exists in one round's return value is a warning nobody
/// sees.
pub const HISTORY_GAP_KEY: &str = "sync_history_gap";
/// Keep enough room below the relay's 8 MiB hard limit for transport framing
/// and future protocol fields. A single oversized local row fails loudly
/// instead of blocking every later operation behind it.
const MAX_PUSH_BLOB_BYTES: usize = 7 * 1024 * 1024;
const MAX_PUSH_OPS: i64 = 256;

fn cursor(store: &Store, key: &str) -> Result<i64> {
    Ok(store
        .sync_state(key)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// One push + pull + merge round. Safe to call repeatedly; every step is
/// idempotent (re-delivered ops lose the LWW comparison and are skipped).
pub fn sync_once(
    store: &Store,
    transport: &dyn SyncTransport,
    cfg: &SyncConfig,
) -> Result<SyncOutcome> {
    let key = cfg.key_bytes()?;
    let device = store.device_id()?;
    let mut outcome = SyncOutcome::default();

    // Push local ops in bounded encrypted blobs. A backlog can grow far past
    // the relay's request cap (especially with health samples), so advance the
    // cursor only after each individual blob is accepted.
    let mut pushed_cursor = cursor(store, PUSHED_KEY)?;
    loop {
        let ops = store.local_ops_after_limited(pushed_cursor, MAX_PUSH_OPS)?;
        if ops.is_empty() {
            break;
        }

        let mut pos = 0usize;
        while pos < ops.len() {
            let mut batch_ops = Vec::new();
            let mut last_id = pushed_cursor;
            while pos < ops.len() {
                let (id, op) = &ops[pos];
                batch_ops.push(op.clone());
                let candidate = SyncBatch {
                    device: device.clone(),
                    ops: batch_ops.clone(),
                };
                let blob = encrypt(&key, serde_json::to_string(&candidate)?.as_bytes())?;
                if blob.len() > MAX_PUSH_BLOB_BYTES {
                    batch_ops.pop();
                    if batch_ops.is_empty() {
                        return Err(CoreError::Invalid(format!(
                            "单条同步变更超过 {} MiB，无法安全上传",
                            MAX_PUSH_BLOB_BYTES / 1024 / 1024
                        )));
                    }
                    break;
                }
                last_id = *id;
                pos += 1;
            }

            let count = batch_ops.len();
            let batch = SyncBatch {
                device: device.clone(),
                ops: batch_ops,
            };
            let blob = encrypt(&key, serde_json::to_string(&batch)?.as_bytes())?;
            transport.push(&device, &blob)?;
            store.set_sync_state(PUSHED_KEY, &last_id.to_string())?;
            pushed_cursor = last_id;
            outcome.pushed += count;
        }

        if ops.len() < MAX_PUSH_OPS as usize {
            break;
        }
    }

    // Pull + merge everyone else's.
    let since = cursor(store, PULLED_KEY)?;
    let blobs = transport.pull(since, &device)?;
    // Retention gap check, before merging anything.
    //
    // `since > 0` excludes a first-ever sync, which legitimately starts below
    // the floor. Beyond that, a cursor under the oldest surviving sequence
    // means the relay swept ops this device never saw. We cannot recover them
    // here, so the one thing that must not happen is reporting success: the
    // flag is recorded durably and surfaced, and the merge still proceeds so
    // the device at least catches up on what *does* remain.
    if let Some(oldest) = transport.oldest_seq() {
        if since > 0 && oldest > since + 1 {
            outcome.history_gap = true;
            store.set_sync_state(HISTORY_GAP_KEY, &format!("cursor={since};oldest={oldest}"))?;
        }
    }
    outcome.pulled_blobs = blobs.len();
    for b in &blobs {
        // Opening a blob is all-or-nothing and entirely outside our control:
        // corrupt base64, a peer with a different SOLUM_SYNC_KEY, a truncated
        // upload. Returning Err here (as this used to) leaves the cursor
        // unadvanced, so the *same* blob is re-fetched and re-fails on every
        // round forever, and every later blob behind it never arrives — one
        // bad byte permanently deafens the device. Park it and move on; the
        // ciphertext is kept so it can still be recovered later.
        let opened = B64
            .decode(&b.blob_b64)
            .map_err(|e| format!("base64 解码失败: {e}"))
            .and_then(|raw| decrypt(&key, &raw).map_err(|e| e.to_string()))
            .and_then(|plain| {
                serde_json::from_slice::<SyncBatch>(&plain)
                    .map_err(|e| format!("批次不可解析: {e}"))
            });
        let batch = match opened {
            Ok(batch) => batch,
            Err(reason) => {
                store.hold_bad_blob(b.seq, &b.device, &b.blob_b64, &reason)?;
                outcome.bad_blobs += 1;
                store.set_sync_state(PULLED_KEY, &b.seq.to_string())?;
                continue;
            }
        };
        let counts = store.apply_remote_ops(&batch.ops)?;
        outcome.applied += counts.applied;
        outcome.skipped += counts.skipped;
        outcome.quarantined += counts.quarantined;
        store.set_sync_state(PULLED_KEY, &b.seq.to_string())?;
    }
    Ok(outcome)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{Channel, Event, EventKind, Notification, NotificationStatus};
    use chrono::NaiveDate;
    use std::sync::Mutex;

    /// In-memory relay with the exact semantics of solum-sync-server.
    #[derive(Default)]
    pub struct MemTransport {
        rows: Mutex<Vec<(i64, String, Vec<u8>)>>,
    }

    impl SyncTransport for MemTransport {
        fn push(&self, device: &str, blob: &[u8]) -> Result<i64> {
            let mut rows = self.rows.lock().unwrap();
            let seq = rows.len() as i64 + 1;
            rows.push((seq, device.to_string(), blob.to_vec()));
            Ok(seq)
        }
        fn pull(&self, since: i64, device: &str) -> Result<Vec<PulledBlob>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|(seq, dev, _)| *seq > since && dev != device)
                .map(|(seq, dev, blob)| PulledBlob {
                    seq: *seq,
                    device: dev.clone(),
                    blob_b64: B64.encode(blob),
                })
                .collect())
        }
    }

    pub fn test_cfg() -> SyncConfig {
        SyncConfig {
            url: "mem://".into(),
            token: "t".into(),
            key: "11".repeat(32),
            account: None,
        }
    }

    #[test]
    fn derive_credentials_is_deterministic() {
        let a = derive_credentials("solum", "514000").unwrap();
        let b = derive_credentials("solum", "514000").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn derive_credentials_diverges_on_username_or_password() {
        let base = derive_credentials("solum", "514000").unwrap();
        let other_user = derive_credentials("xiaoran", "514000").unwrap();
        let other_pass = derive_credentials("solum", "999999").unwrap();
        assert_ne!(base, other_user);
        assert_ne!(base, other_pass);
    }

    #[test]
    fn derive_credentials_token_and_key_are_independent_looking() {
        let (token, key) = derive_credentials("solum", "514000").unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(key.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, key);
    }

    #[test]
    fn recovery_envelope_binds_password_and_account_uuid() {
        let user = "9d4df1be-9f7b-4a3a-b986-ec920d2df60e";
        let other = "6ea64c69-0531-45cb-b585-500c5f479fd8";
        let master = [0x5au8; 32];
        let envelope = wrap_recovery_key(user, "correct horse", &master).unwrap();
        assert_eq!(
            unwrap_recovery_key(user, "correct horse", &envelope).unwrap(),
            master
        );
        assert!(unwrap_recovery_key(user, "wrong", &envelope).is_err());
        assert!(unwrap_recovery_key(other, "correct horse", &envelope).is_err());
        assert!(!envelope
            .windows(master.len())
            .any(|window| window == master));
    }

    #[derive(Default)]
    struct MemRecoveryTransport {
        envelope: Mutex<Option<Vec<u8>>>,
    }

    impl RecoveryEnvelopeTransport for MemRecoveryTransport {
        fn get(&self, _session: &crate::account::AccountSession) -> Result<Option<Vec<u8>>> {
            Ok(self.envelope.lock().unwrap().clone())
        }

        fn put_if_absent(
            &self,
            _session: &crate::account::AccountSession,
            proposed: &[u8],
        ) -> Result<Vec<u8>> {
            let mut envelope = self.envelope.lock().unwrap();
            Ok(envelope.get_or_insert_with(|| proposed.to_vec()).clone())
        }
    }

    fn recovery_session() -> crate::account::AccountSession {
        crate::account::AccountSession {
            server_url: "https://cloud.example".into(),
            user_id: "9d4df1be-9f7b-4a3a-b986-ec920d2df60e".into(),
            username: "alice".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            model: crate::account::DEFAULT_CLOUD_MODEL.into(),
        }
    }

    #[test]
    fn account_sync_refuses_sending_access_token_to_another_origin() {
        let session = recovery_session();
        let wrong = SyncConfig {
            url: "https://relay.attacker.example".into(),
            token: String::new(),
            key: "11".repeat(32),
            account: Some(session.clone()),
        };
        assert!(HttpTransport::new(&wrong).is_err());
        let same = SyncConfig {
            url: session.server_url.clone(),
            token: String::new(),
            key: "11".repeat(32),
            account: Some(session),
        };
        assert!(HttpTransport::new(&same).is_ok());
    }

    #[test]
    fn first_device_creates_and_second_device_recovers_same_random_key() {
        let transport = MemRecoveryTransport::default();
        let unique = format!(
            "solum-recovery-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let dir = std::env::temp_dir().join(unique);
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        let session = recovery_session();

        assert_eq!(
            provision_account_sync_with(&transport, &session, "pw", &first).unwrap(),
            RecoveryProvision::Created
        );
        assert_eq!(
            provision_account_sync_with(&transport, &session, "pw", &second).unwrap(),
            RecoveryProvision::Recovered
        );
        assert_eq!(
            read_local_account_key(&first).unwrap(),
            read_local_account_key(&second).unwrap()
        );
        assert!(provision_account_sync_with(&transport, &session, "wrong", &second).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sync_config_file_parses_all_supported_shapes() {
        let creds: SyncConfigFile = serde_json::from_str(
            r#"{"url":"https://relay.example","username":"solum","password":"514000"}"#,
        )
        .unwrap();
        match creds {
            SyncConfigFile::Credentials {
                url,
                username,
                password,
            } => {
                assert_eq!(url, "https://relay.example");
                assert_eq!(username, "solum");
                assert_eq!(password, "514000");
            }
            SyncConfigFile::Direct { .. } | SyncConfigFile::Account { .. } => {
                panic!("expected Credentials shape")
            }
        }

        let direct: SyncConfigFile = serde_json::from_str(
            &serde_json::json!({"url": "https://relay.example", "token": "t", "key": "11".repeat(32)})
                .to_string(),
        )
        .unwrap();
        match direct {
            SyncConfigFile::Direct { url, token, key } => {
                assert_eq!(url, "https://relay.example");
                assert_eq!(token, "t");
                assert_eq!(key, "11".repeat(32));
            }
            SyncConfigFile::Credentials { .. } | SyncConfigFile::Account { .. } => {
                panic!("expected Direct shape")
            }
        }

        let account: SyncConfigFile = serde_json::from_str(
            &serde_json::json!({"url": "https://relay.example", "key": "22".repeat(32)})
                .to_string(),
        )
        .unwrap();
        match account {
            SyncConfigFile::Account { url, key } => {
                assert_eq!(url, "https://relay.example");
                assert_eq!(key, "22".repeat(32));
            }
            _ => panic!("expected Account shape"),
        }
    }

    fn dt(d: u32, h: u32, mi: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 7, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn sample_event(title: &str) -> Event {
        Event::new(title, EventKind::Meeting, dt(9, 15, 0), title, dt(7, 10, 0))
    }

    fn add_event_with_reminder(s: &Store, title: &str) -> (i64, i64) {
        let ev = s.insert_event(&sample_event(title), None).unwrap();
        let nid = s
            .insert_notification(&Notification {
                id: None,
                event_id: ev,
                fire_at: dt(9, 14, 30),
                lead_label: "30m".into(),
                channels: vec![Channel::Push],
                status: NotificationStatus::Pending,
                created_at: dt(7, 10, 0),
                fired_at: None,
            })
            .unwrap();
        (ev, nid)
    }

    /// A relay that hands back one unopenable blob wedged between two good
    /// ones. Reproduces the poison-pill: before the fix, the bad blob failed
    /// the whole round, the cursor never moved, and blob 3 never arrived — on
    /// this round or any future one.
    #[derive(Default)]
    struct PoisonedTransport {
        rows: Mutex<Vec<(i64, String, String)>>,
    }
    impl SyncTransport for PoisonedTransport {
        fn push(&self, _device: &str, _blob: &[u8]) -> Result<i64> {
            Ok(0)
        }
        fn pull(&self, since: i64, _device: &str) -> Result<Vec<PulledBlob>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|(seq, _, _)| *seq > since)
                .map(|(seq, dev, b64)| PulledBlob {
                    seq: *seq,
                    device: dev.clone(),
                    blob_b64: b64.clone(),
                })
                .collect())
        }
    }

    #[test]
    fn one_unopenable_blob_does_not_wedge_the_pull_cursor() {
        let cfg = test_cfg();
        let key = cfg.key_bytes().unwrap();

        // Build the good blobs from a *real* peer store rather than hand-rolled
        // JSON, so the payload shape is whatever the current schema actually
        // emits — otherwise this test could pass by quarantining everything.
        let peer = Store::open_in_memory().unwrap();
        let mut sealed_cursor = 0i64;
        let mut seal = |title: &str| {
            peer.insert_event(&sample_event(title), None).unwrap();
            let ops = peer.local_ops_after_limited(sealed_cursor, 256).unwrap();
            sealed_cursor = ops.last().map(|(id, _)| *id).unwrap_or(sealed_cursor);
            let batch = SyncBatch {
                device: "peer".into(),
                ops: ops.into_iter().map(|(_, op)| op).collect(),
            };
            B64.encode(encrypt(&key, serde_json::to_string(&batch).unwrap().as_bytes()).unwrap())
        };

        let relay = PoisonedTransport::default();
        relay.rows.lock().unwrap().extend([
            (1, "peer".to_string(), seal("第一条")),
            // Valid base64, but not our ciphertext — wrong key / corruption.
            (2, "peer".to_string(), B64.encode(vec![0u8; 64])),
            (3, "peer".to_string(), seal("第三条")),
        ]);

        let store = Store::open_in_memory().unwrap();
        let out = sync_once(&store, &relay, &cfg).unwrap();

        assert_eq!(out.bad_blobs, 1, "the unopenable blob should be parked");
        assert_eq!(store.bad_blob_count().unwrap(), 1);
        // The blob *behind* the bad one is the whole point: it must arrive.
        let titles: Vec<String> = store
            .list_events()
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert!(titles.iter().any(|t| t == "第一条"), "got {titles:?}");
        assert!(titles.iter().any(|t| t == "第三条"), "got {titles:?}");

        // And the cursor really moved: a second round pulls nothing new.
        let again = sync_once(&store, &relay, &cfg).unwrap();
        assert_eq!(again.pulled_blobs, 0);
        assert_eq!(again.bad_blobs, 0);
    }

    /// A relay that has swept history below this device's cursor. Reproduces
    /// the regression the retention sweep introduced: without a gap signal the
    /// device pulls "everything after my cursor", gets a later unrelated set,
    /// and reports a clean sync while having permanently missed ops.
    struct SweptTransport {
        oldest: i64,
    }
    impl SyncTransport for SweptTransport {
        fn push(&self, _device: &str, _blob: &[u8]) -> Result<i64> {
            Ok(0)
        }
        fn pull(&self, _since: i64, _device: &str) -> Result<Vec<PulledBlob>> {
            Ok(Vec::new())
        }
        fn oldest_seq(&self) -> Option<i64> {
            Some(self.oldest)
        }
    }

    #[test]
    fn a_cursor_below_the_relays_retention_floor_is_reported_not_ignored() {
        let cfg = test_cfg();
        let store = Store::open_in_memory().unwrap();
        // This device last synced at seq 5; the relay has since swept
        // everything below 900.
        store.set_sync_state(PULLED_KEY, "5").unwrap();

        let out = sync_once(&store, &SweptTransport { oldest: 900 }, &cfg).unwrap();
        assert!(
            out.history_gap,
            "a swept-away history must not be reported as a clean sync"
        );
        assert!(
            store.sync_state(HISTORY_GAP_KEY).unwrap().is_some(),
            "the gap must be recorded durably, not just returned once"
        );
    }

    #[test]
    fn a_contiguous_cursor_is_not_mistaken_for_a_gap() {
        let cfg = test_cfg();
        let store = Store::open_in_memory().unwrap();
        store.set_sync_state(PULLED_KEY, "899").unwrap();
        // oldest == since + 1 means nothing was missed.
        let out = sync_once(&store, &SweptTransport { oldest: 900 }, &cfg).unwrap();
        assert!(!out.history_gap);

        // …and a first-ever sync (cursor 0) is never a gap.
        let fresh = Store::open_in_memory().unwrap();
        let out = sync_once(&fresh, &SweptTransport { oldest: 900 }, &cfg).unwrap();
        assert!(!out.history_gap);
    }

    #[test]
    fn relay_url_must_not_be_cleartext_http() {
        let bad = SyncConfig {
            url: "http://relay.example.com".into(),
            ..test_cfg()
        };
        assert!(HttpTransport::new(&bad).is_err());
        let ok = SyncConfig {
            url: "https://relay.example.com".into(),
            ..test_cfg()
        };
        assert!(HttpTransport::new(&ok).is_ok());
    }

    #[test]
    fn crypto_round_trip_and_key_validation() {
        let cfg = test_cfg();
        let key = cfg.key_bytes().unwrap();
        let blob = encrypt(&key, "机密内容".as_bytes()).unwrap();
        assert_ne!(&blob[24..], "机密内容".as_bytes());
        assert_eq!(decrypt(&key, &blob).unwrap(), "机密内容".as_bytes());
        // Wrong key → error, not garbage.
        let other = SyncConfig {
            key: "22".repeat(32),
            ..test_cfg()
        }
        .key_bytes()
        .unwrap();
        assert!(decrypt(&other, &blob).is_err());
        assert!(SyncConfig {
            key: "abc".into(),
            ..test_cfg()
        }
        .key_bytes()
        .is_err());
    }

    #[test]
    fn a_large_backlog_is_uploaded_in_multiple_cursor_safe_blobs() {
        let source = Store::open_in_memory().unwrap();
        for i in 0..300 {
            add_event_with_reminder(&source, &format!("批量同步-{i}"));
        }
        let relay = MemTransport::default();
        let cfg = test_cfg();

        let outcome = sync_once(&source, &relay, &cfg).unwrap();
        assert_eq!(outcome.pushed, 600);
        assert!(relay.rows.lock().unwrap().len() > 1);
        assert_eq!(source.sync_state(PUSHED_KEY).unwrap(), Some("600".into()));

        let destination = Store::open_in_memory().unwrap();
        sync_once(&destination, &relay, &cfg).unwrap();
        assert_eq!(destination.list_events().unwrap().len(), 300);
    }

    #[test]
    fn two_devices_converge() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        // A creates an event + reminder; B creates its own.
        add_event_with_reminder(&a, "A的会议");
        add_event_with_reminder(&b, "B的考试");

        // A pushes; B pulls A's data and pushes its own; A pulls B's.
        let oa = sync_once(&a, &relay, &cfg).unwrap();
        assert!(oa.pushed > 0);
        let ob = sync_once(&b, &relay, &cfg).unwrap();
        assert!(ob.applied > 0);
        let oa2 = sync_once(&a, &relay, &cfg).unwrap();
        assert!(oa2.applied > 0);

        let titles = |s: &Store| {
            let mut t: Vec<String> = s
                .list_events()
                .unwrap()
                .iter()
                .map(|e| e.title.clone())
                .collect();
            t.sort();
            t
        };
        assert_eq!(
            titles(&a),
            vec!["A的会议".to_string(), "B的考试".to_string()]
        );
        assert_eq!(titles(&a), titles(&b));
        assert_eq!(a.list_notifications().unwrap().len(), 2);
        assert_eq!(b.list_notifications().unwrap().len(), 2);

        // Idempotency: another round applies nothing new.
        let oa3 = sync_once(&a, &relay, &cfg).unwrap();
        assert_eq!((oa3.pushed, oa3.applied), (0, 0));
    }

    /// A blob as a *newer* build would emit it: one op this build understands,
    /// one naming a table it has never heard of.
    fn blob_from_a_newer_peer(cfg: &SyncConfig) -> Vec<u8> {
        let batch = SyncBatch {
            device: "future".into(),
            ops: vec![
                SyncOp {
                    tbl: "events".into(),
                    guid: "e".repeat(32),
                    op: "upsert".into(),
                    payload: Some(
                        serde_json::json!({
                            "title": "新版设备建的会议",
                            "kind": "meeting",
                            "start": crate::model::fmt_ts(&dt(9, 15, 0)),
                            "people_json": "[]",
                            "created_at": crate::model::fmt_ts(&dt(7, 10, 0)),
                        })
                        .to_string(),
                    ),
                    hlc: "2026-07-07T10:00:00.000".into(),
                    origin: "future".into(),
                },
                SyncOp {
                    tbl: "widget_defs".into(),
                    guid: "w".repeat(32),
                    op: "upsert".into(),
                    payload: Some(r#"{"name":"日程表"}"#.into()),
                    hlc: "2026-07-07T10:00:01.000".into(),
                    origin: "future".into(),
                },
            ],
        };
        encrypt(
            &cfg.key_bytes().unwrap(),
            serde_json::to_string(&batch).unwrap().as_bytes(),
        )
        .unwrap()
    }

    /// Regression (2026-07-19): an op from a table this build does not know
    /// must not wedge the pull cursor. The catch-all arm of `apply_one` used
    /// to return `Err`, which rolled the whole merge transaction back — losing
    /// the legitimate ops sharing the blob — *and* left `sync_pulled_seq`
    /// unadvanced, so every later round re-fetched the same blob and failed
    /// identically. Push runs before pull, so the device kept uploading while
    /// silently never receiving again.
    #[test]
    fn an_op_from_a_newer_peer_does_not_wedge_the_pull_cursor() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let old = Store::open_in_memory().unwrap();
        relay.push("future", &blob_from_a_newer_peer(&cfg)).unwrap();

        let outcome = sync_once(&old, &relay, &cfg).unwrap();

        // The known op still lands — one unknown sibling must not void it.
        let titles: Vec<String> = old
            .list_events()
            .unwrap()
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["新版设备建的会议".to_string()]);
        // The unknown one is held for a later build, not dropped, not fatal.
        assert_eq!(outcome.quarantined, 1);
        // And the cursor moved, so the next round is not a replay of this one.
        assert_eq!(cursor(&old, PULLED_KEY).unwrap(), 1);
        let again = sync_once(&old, &relay, &cfg).unwrap();
        assert_eq!((again.pulled_blobs, again.applied), (0, 0));
    }

    #[test]
    fn health_samples_sync_and_dedup_survive_round_trip() {
        use crate::wearable::{HealthMetric, HealthSample};
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let hr = HealthSample::new(
            HealthMetric::HeartRate,
            dt(12, 9, 0),
            dt(12, 9, 0),
            72.0,
            "health_connect",
        );
        a.insert_health_sample_if_new(&hr).unwrap();

        sync_once(&a, &relay, &cfg).unwrap();
        let ob = sync_once(&b, &relay, &cfg).unwrap();
        assert!(ob.applied > 0);

        let samples = b.list_health_samples().unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].kind, HealthMetric::HeartRate);
        assert_eq!(samples[0].value, 72.0);

        // B re-polling the same instant locally must stay deduped even after
        // the row arrived via sync (dedup_key survives the round trip).
        assert_eq!(b.insert_health_sample_if_new(&hr).unwrap(), None);
    }

    #[test]
    fn soulous_facts_sync_with_stable_guids_lww_and_deletes() {
        use crate::soulous::{SoulousFact, SoulousKind, SOURCE};

        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();
        let fact = |title: &str, imported_at| SoulousFact {
            id: None,
            external_id: "exam-42".into(),
            kind: SoulousKind::Exam,
            title: title.into(),
            occurs_at: Some(dt(20, 9, 0)),
            ends_at: None,
            payload_json: r#"{"id":42,"courseName":"算法"}"#.into(),
            source: SOURCE.into(),
            imported_at,
        };

        a.replace_soulous_snapshot(&[fact("算法", dt(18, 10, 0))])
            .unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert_eq!(b.list_soulous_facts().unwrap()[0].title, "算法");

        // A later snapshot updates the same stable source guid, so B's row
        // converges by the normal trigger/LWW path rather than duplicating it.
        std::thread::sleep(std::time::Duration::from_millis(5));
        a.replace_soulous_snapshot(&[fact("算法（调考）", dt(18, 11, 0))])
            .unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        let after = b.list_soulous_facts().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].title, "算法（调考）");

        std::thread::sleep(std::time::Duration::from_millis(5));
        a.replace_soulous_snapshot(&[]).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert!(b.list_soulous_facts().unwrap().is_empty());
    }

    #[test]
    fn delete_propagates_and_wins() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let (ev, _) = add_event_with_reminder(&a, "要删的会");
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert_eq!(b.list_events().unwrap().len(), 1);

        // A deletes (cascade removes the reminder too) → B converges.
        a.delete_event(ev).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert!(b.list_events().unwrap().is_empty());
        assert!(b.list_notifications().unwrap().is_empty());
        assert!(a.list_events().unwrap().is_empty());
    }

    #[test]
    fn persona_versions_merge_with_renumber_and_active_pointer() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        // Both devices independently create their own v1 (version collision).
        let da = crate::persona::PersonaDraft {
            tone: "干练".into(),
            ..Default::default()
        };
        let db = crate::persona::PersonaDraft {
            tone: "活泼".into(),
            ..Default::default()
        };
        a.insert_persona_version(&da, "manual", None, dt(7, 10, 0))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5)); // hlc 分离
        b.insert_persona_version(&db, "manual", None, dt(7, 11, 0))
            .unwrap();

        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();

        // Both stores hold both versions (renumbered locally, history intact).
        assert_eq!(a.list_persona_versions().unwrap().len(), 2);
        assert_eq!(b.list_persona_versions().unwrap().len(), 2);
        // The active pointer converges on the later write (B's 活泼).
        assert_eq!(a.active_persona().unwrap().unwrap().draft.tone, "活泼");
        assert_eq!(b.active_persona().unwrap().unwrap().draft.tone, "活泼");
    }

    #[test]
    fn lww_update_wins_over_older_state() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let (_, nid) = add_event_with_reminder(&a, "状态之争");
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();

        // B dismisses the reminder later than A created it → dismissal wins on A.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b_nid = b.list_notifications().unwrap()[0].id.unwrap();
        b.dismiss_notification(b_nid).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        let n = a
            .list_notifications()
            .unwrap()
            .into_iter()
            .find(|n| n.id == Some(nid))
            .unwrap();
        assert_eq!(n.status, NotificationStatus::Dismissed);
    }

    #[test]
    fn rule_table_and_proactivity_sync_as_lww_docs() {
        let cfg = test_cfg();
        let relay = MemTransport::default();
        let a = Store::open_in_memory().unwrap();
        let b = Store::open_in_memory().unwrap();

        let mut c = crate::proactivity::ProactivityConfig::defaults();
        c.set(
            crate::proactivity::ProactivityDimension::WeeklyReview,
            crate::proactivity::ProactivityLevel::Butler,
        );
        a.save_proactivity(&c).unwrap();
        sync_once(&a, &relay, &cfg).unwrap();
        sync_once(&b, &relay, &cfg).unwrap();
        assert_eq!(b.load_proactivity().unwrap(), c);
    }
}

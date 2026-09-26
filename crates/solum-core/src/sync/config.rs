//! Sync configuration: where the relay URL, the device key and the tenant live.
//!
//! Part of the `sync` module; see `mod.rs` for the module contract.

use super::*;

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
pub(super) enum SyncConfigFile {
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
                    // 配置文件里的中心地址可能是域名合并前的旧值，加载时对齐当前身份。
                    url: crate::account::migrate_legacy_center_url(&url),
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

    pub fn key_bytes(&self) -> Result<[u8; 32]> {
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

    /// Return the existing end-to-end master key for local backup codecs.
    ///
    /// The caller receives bytes only; this method never logs or persists an
    /// additional copy. Password derivation and recovery envelopes remain
    /// outside the backup uploader.
    pub fn e2ee_master_key(&self) -> Result<[u8; 32]> {
        self.key_bytes()
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
pub(super) const PBKDF2_ROUNDS: u32 = 600_000;
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

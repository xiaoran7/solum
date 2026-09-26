//! End-to-end crypto. The relay only ever sees these bytes.
//!
//! Part of the `sync` module; see `mod.rs` for the module contract.

use super::*;

// ---- crypto (end-to-end; the relay never sees plaintext) ------------------------

/// XChaCha20-Poly1305 with a random 24-byte nonce prepended to the ciphertext.
pub(crate) fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
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

pub(crate) fn decrypt(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 24 {
        return Err(CoreError::Invalid("同步数据太短，不是合法密文".into()));
    }
    let (nonce, ct) = blob.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce), ct)
        .map_err(|_| CoreError::Invalid("解密失败——各设备的 SOLUM_SYNC_KEY 是否一致？".into()))
}

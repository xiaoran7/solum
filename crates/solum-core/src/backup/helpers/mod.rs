//! `helpers` -- shared by both directions: canonical JSON, the associated-data derivation, integer widening, hex encode/decode and the nonce parser. Split out so neither `seal` nor `open` holds the other's primitives.
//!
//! Part of the `backup` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::format::{BackupResult, AAD_DOMAIN, NONCE_LEN};
use super::*;

pub(super) fn normalized_json(value: &Value) -> BackupResult<Vec<u8>> {
    fn write_value(value: &Value, out: &mut Vec<u8>) -> BackupResult<()> {
        match value {
            Value::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    write_value(item, out)?;
                }
                out.push(b']');
            }
            Value::Object(object) => {
                out.push(b'{');
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)
                        .map_err(|_| BackupError::InvalidPayload)?;
                    out.push(b':');
                    write_value(&object[key], out)?;
                }
                out.push(b'}');
            }
            scalar => {
                serde_json::to_writer(out, scalar).map_err(|_| BackupError::InvalidPayload)?
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    write_value(value, &mut out)?;
    Ok(out)
}

pub(super) fn aad(raw_header: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(AAD_DOMAIN.len() + raw_header.len());
    out.extend_from_slice(AAD_DOMAIN);
    out.extend_from_slice(raw_header);
    out
}

pub(super) fn to_u64(value: usize) -> BackupResult<u64> {
    u64::try_from(value).map_err(|_| BackupError::SizeLimit)
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(super) fn decode_nonce(encoded: &str) -> BackupResult<[u8; NONCE_LEN]> {
    if encoded.len() != NONCE_LEN * 2
        || !encoded
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(BackupError::InvalidContainer);
    }
    let mut nonce = [0u8; NONCE_LEN];
    for (index, pair) in encoded.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        nonce[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(nonce)
}

fn hex_nibble(byte: u8) -> BackupResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(BackupError::InvalidContainer),
    }
}

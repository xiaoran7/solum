//! `helpers` -- the shared primitives the builders and validators both use: guid restoration, stable ids, canonical JSON and the small scalar checks.
//!
//! Part of the `export_v3` module; see `mod.rs` for the contract.
//! Moved verbatim from the single-file source file.

use super::*;

pub(super) fn restore_guids(rows: Option<&Value>) -> Result<BTreeSet<String>> {
    rows.and_then(Value::as_array)
        .ok_or_else(|| invalid("missing restore table"))?
        .iter()
        .map(|row| {
            row.get("guid")
                .and_then(Value::as_str)
                .filter(|guid| is_lower_hex(guid, 32))
                .map(ToOwned::to_owned)
                .ok_or_else(|| invalid("invalid restore guid"))
        })
        .collect()
}

pub(super) fn next_stable_id(
    domain: &str,
    identity: &Value,
    ordinals: &mut BTreeMap<Vec<u8>, u64>,
) -> Result<String> {
    let canonical = canonical_json(identity)?;
    let ordinal = ordinals.entry(canonical.clone()).or_default();
    let mut hasher = Sha256::new();
    hasher.update(b"solum-export:v3:");
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(&canonical);
    hasher.update([0]);
    hasher.update(ordinal.to_string().as_bytes());
    let digest = hasher.finalize();
    *ordinal += 1;
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    fn write(value: &Value, out: &mut Vec<u8>) -> Result<()> {
        match value {
            Value::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    write(item, out)?;
                }
                out.push(b']');
            }
            Value::Object(object) => {
                out.push(b'{');
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)?;
                    out.push(b':');
                    write(&object[key], out)?;
                }
                out.push(b'}');
            }
            scalar => serde_json::to_writer(out, scalar)?,
        }
        Ok(())
    }
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

pub(super) fn validate_local_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._:".contains(&byte))
    {
        return Err(invalid("invalid portable local id"));
    }
    Ok(())
}

pub(super) fn validate_rfc3339(value: &str) -> Result<()> {
    DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| invalid("invalid RFC3339 timestamp"))
}

pub(super) fn validate_string_set(
    values: &[String],
    max_count: usize,
    max_chars: usize,
) -> Result<()> {
    if values.len() > max_count {
        return Err(invalid("string set exceeds count limit"));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        if value.is_empty() || value.chars().count() > max_chars || !unique.insert(value.as_str()) {
            return Err(invalid("invalid string set"));
        }
    }
    Ok(())
}

pub(super) fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn invalid(message: &str) -> CoreError {
    CoreError::Invalid(format!("solum-export v3: {message}"))
}

use anyhow::{Context, Result, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn exact(v: &Value, fields: &[&str]) -> Result<()> {
    let o = v.as_object().context("object required")?;
    ensure!(
        o.len() == fields.len() && fields.iter().all(|k| o.contains_key(*k)),
        "unexpected fields"
    );
    Ok(())
}
pub fn text(v: &Value) -> Result<&str> {
    v.as_str().context("string required")
}
pub fn integer(v: &Value, min: u64, max: u64) -> Result<u64> {
    let n = v.as_u64().context("integer required")?;
    ensure!((min..=max).contains(&n), "integer out of bounds");
    Ok(n)
}
pub fn number(v: &Value) -> Result<u64> {
    integer(v, 0, (1 << 53) - 1)
}
pub fn hex_bytes(v: &Value, size: usize) -> Result<Vec<u8>> {
    let s = text(v)?;
    ensure!(
        s.len() == size * 2
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid hex"
    );
    Ok(hex::decode(s)?)
}
pub fn b64(v: &Value, limit: usize) -> Result<Vec<u8>> {
    let s = text(v)?;
    ensure!(s.len() <= limit.div_ceil(3) * 4, "base64 limit");
    let b = STANDARD.decode(s)?;
    ensure!(
        b.len() <= limit && STANDARD.encode(&b) == s,
        "noncanonical base64"
    );
    Ok(b)
}
pub fn url64(s: &str, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        !s.is_empty() && s.len() <= limit.div_ceil(3) * 4,
        "base64url limit"
    );
    let b = URL_SAFE_NO_PAD.decode(s)?;
    ensure!(
        b.len() <= limit && URL_SAFE_NO_PAD.encode(&b) == s,
        "noncanonical base64url"
    );
    Ok(b)
}
pub fn sha256(raw: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(raw.as_ref()))
}
pub fn origin(v: &Value) -> Result<()> {
    let s = text(v)?;
    ensure!(s.len() <= 2048, "origin limit");
    let u = url::Url::parse(s)?;
    ensure!(
        s.starts_with("https://")
            && u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none()
            && !s[8..].contains(['/', '?', '#', '@']),
        "HTTPS origin required"
    );
    Ok(())
}
pub fn worker(v: &Value) -> Result<()> {
    exact(
        v,
        &[
            "worker_id",
            "boot_id",
            "encryption_key",
            "identity_key",
            "model_manifest",
            "logical_origin",
            "evidence_profile",
            "deployment_digest",
        ],
    )?;
    for k in [
        "boot_id",
        "encryption_key",
        "identity_key",
        "model_manifest",
        "deployment_digest",
    ] {
        hex_bytes(&v[k], 32)?;
    }
    hex_bytes(&v["worker_id"], 16)?;
    origin(&v["logical_origin"])?;
    ensure!(
        v["evidence_profile"] == crate::PROFILE,
        "wrong evidence profile"
    );
    Ok(())
}
pub fn signed(v: &Value, key: &Value, domain: &str) -> Result<(Value, Vec<u8>)> {
    exact(v, &["payload", "signature"])?;
    let raw = url64(text(&v["payload"])?, 128 * 1024)?;
    let key: [u8; 32] = hex_bytes(key, 32)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("key length"))?;
    let key = VerifyingKey::from_bytes(&key)?;
    let mut message = Vec::with_capacity(domain.len() + 1 + raw.len());
    message.extend_from_slice(domain.as_bytes());
    message.push(0);
    message.extend_from_slice(&raw);
    key.verify_strict(
        &message,
        &Signature::from_slice(&hex_bytes(&v["signature"], 64)?)?,
    )?;
    Ok((crate::json::decode(&raw)?, raw))
}
/// Preserve Python json.dumps(sort_keys=True, separators=(',', ':'), ensure_ascii=True)
/// in report-data bindings. These objects contain only strings/arrays/integers.
pub fn canonical(v: &Value) -> Result<Vec<u8>> {
    fn write(v: &Value, out: &mut String) -> Result<()> {
        match v {
            Value::String(s) => {
                let quoted = serde_json::to_string(s)?;
                for ch in quoted.chars() {
                    if ch <= '\u{7e}' {
                        out.push(ch);
                    } else {
                        for unit in ch.encode_utf16(&mut [0; 2]) {
                            out.push_str(&format!("\\u{unit:04x}"));
                        }
                    }
                }
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(x, out)?;
                }
                out.push(']');
            }
            Value::Object(o) => {
                out.push('{');
                let mut keys = o.keys().collect::<Vec<_>>();
                keys.sort();
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(&Value::String((*k).clone()), out)?;
                    out.push(':');
                    write(&o[*k], out)?;
                }
                out.push('}');
            }
            Value::Number(n) => {
                ensure!(n.is_u64() || n.is_i64(), "canonical integers required");
                out.push_str(&n.to_string());
            }
            _ => out.push_str(&v.to_string()),
        };
        Ok(())
    }
    let mut out = String::new();
    write(v, &mut out)?;
    Ok(out.into_bytes())
}

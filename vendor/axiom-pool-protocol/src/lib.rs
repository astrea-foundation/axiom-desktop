//! Strict application bindings around the pinned upstream EHBP transport.
#![forbid(unsafe_code)]

pub mod authorization;
pub mod crypto;
pub mod frames;
pub mod strict;
#[cfg(any(test, feature = "test-fixture"))]
pub mod test_support;
pub mod types;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub const PROTOCOL: &str = "axiom-pool-e2ee-v1";
pub const OUTER_PROTOCOL: &str = "axiom-gateway-pool-v1";
pub const AUDIENCE: &str = "axiom-pool-worker-v1";
pub const GRANT_DOMAIN: &str = "axiom-pool-grant-v1";
pub const TICKET_DOMAIN: &str = "axiom-pool-ticket-v1";
pub const RECEIPT_DOMAIN: &str = "axiom-pool-receipt-v1";
pub const IDENTITY_DOMAIN: &str = "axiom-pool-identity-v1";
pub const CONTROL_DOMAIN: &str = "axiom-pool-control-v1";
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_REQUEST: usize = 16 * 1024 * 1024;
pub const MAX_RESPONSE: usize = 96 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedDocument {
    pub payload: String,
    pub signature: String,
}

pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

pub fn random_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}
pub fn random_challenge() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

pub fn hex_bytes(value: &str, length: usize) -> Result<Vec<u8>> {
    ensure!(
        value.len() == length * 2
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid hexadecimal identifier"
    );
    Ok(hex::decode(value)?)
}

pub fn origin(value: &str, fixture: bool) -> Result<()> {
    let parsed = url::Url::parse(value)?;
    let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    ensure!(
        parsed.scheme() == "https" || (fixture && loopback && parsed.scheme() == "http"),
        "HTTPS origin required"
    );
    ensure!(
        parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.origin().ascii_serialization() == value,
        "invalid origin"
    );
    Ok(())
}

pub fn sign<T: Serialize>(
    value: &T,
    key: &ed25519_dalek::SigningKey,
    domain: &str,
) -> Result<SignedDocument> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::Signer;
    let raw = serde_json::to_vec(value)?;
    ensure!(raw.len() <= 128 * 1024, "signed payload exceeds limit");
    Ok(SignedDocument {
        payload: URL_SAFE_NO_PAD.encode(&raw),
        signature: hex::encode(
            key.sign(&[domain.as_bytes(), &[0], &raw].concat())
                .to_bytes(),
        ),
    })
}

pub fn verify<T: DeserializeOwned>(
    doc: &SignedDocument,
    key: &ed25519_dalek::VerifyingKey,
    domain: &str,
) -> Result<T> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    ensure!(
        doc.payload.len() <= 180 * 1024,
        "signed payload exceeds limit"
    );
    let raw = URL_SAFE_NO_PAD.decode(&doc.payload)?;
    ensure!(
        URL_SAFE_NO_PAD.encode(&raw) == doc.payload,
        "noncanonical base64"
    );
    let signature = ed25519_dalek::Signature::from_slice(&hex_bytes(&doc.signature, 64)?)?;
    key.verify_strict(&[domain.as_bytes(), &[0], &raw].concat(), &signature)?;
    strict::decode(&raw)
}

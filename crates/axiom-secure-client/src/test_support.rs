//! Test-only support for an encrypted provider fixture.

use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest as _, Sha256};

use crate::{Result, providers::near::crypto};

const RESPONSE_SIGNING_SEED: [u8; 32] = [0x5A; 32];

/// A model-side identity used by the deterministic encrypted fixture.
pub struct FixtureModelIdentity(crypto::ClientIdentity);

impl FixtureModelIdentity {
    #[must_use]
    pub fn generate() -> Self {
        Self(crypto::ClientIdentity::generate())
    }

    #[must_use]
    pub fn public_key_hex(&self) -> String {
        self.0.public_key_hex()
    }

    pub fn decrypt_hex(&self, ciphertext_hex: &str) -> Result<Vec<u8>> {
        self.0.decrypt_hex(ciphertext_hex)
    }
}

impl std::fmt::Debug for FixtureModelIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FixtureModelIdentity([REDACTED])")
    }
}

pub fn encrypt_for_client(client_public_key_hex: &str, plaintext: &[u8]) -> Result<String> {
    let key = crypto::Ed25519PublicKey::from_hex(client_public_key_hex)?;
    crypto::encrypt_hex(key, plaintext)
}

/// Canonical address of the deterministic test-only response signer.
#[must_use]
pub fn fixture_response_signing_address() -> String {
    let key = SigningKey::from_bytes(&RESPONSE_SIGNING_SEED);
    hex::encode(key.verifying_key().to_bytes())
}

/// Build the exact closed NEAR receipt shape around deterministic fixture response bytes.
#[must_use]
pub fn sign_response_receipt(
    response_body: &[u8],
    request_hash: &str,
    chat_id: &str,
    model: &str,
) -> serde_json::Value {
    let key = SigningKey::from_bytes(&RESPONSE_SIGNING_SEED);
    let response_hash = hex::encode(Sha256::digest(response_body));
    let signed_text = format!("{model}:{request_hash}:{response_hash}");
    let signature_bytes = key.sign(signed_text.as_bytes()).to_bytes();
    serde_json::json!({
        "provider": "near",
        "protocol": "near-v3",
        "chat_id": chat_id,
        "model": model,
        "request_hash": request_hash,
        "response_hash": response_hash,
        "response_body_base64": general_purpose::STANDARD.encode(response_body),
        "signed_text": signed_text,
        "signature": hex::encode(signature_bytes),
        "signing_address": fixture_response_signing_address(),
    })
}

#[must_use]
pub fn sign_attestation(
    model: &str,
    nonce_hex: &str,
    base_url: &str,
    model_public_key_hex: &str,
) -> String {
    crate::providers::fixture::sign_evidence_for_test(
        model,
        nonce_hex,
        base_url,
        model_public_key_hex,
    )
}

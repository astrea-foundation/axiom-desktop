//! Test-only support for an encrypted provider fixture.

use crate::{Result, providers::near::crypto};

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

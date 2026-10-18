//! Strict interfaces retained by the vendored Phala ACI verifier.
//!
//! Every response path is receipt-gated. The former upstream NEAR cipher used
//! permissive default methods here; those defaults are intentionally absent so
//! a provider cannot silently bypass Phala's request context or terminal proof.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::relay::{RelayApi, RelayCompletion};

/// One named attestation check for display purposes. A model is only usable
/// when the attestor returned `Ok`; checks describe the proof that passed.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct VerificationCheck {
    pub id: String,
    pub label: String,
    pub status: String,
    pub ok: bool,
}

impl VerificationCheck {
    pub fn new(id: &str, label: &str, status: impl Into<String>, ok: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status: status.into(),
            ok,
        }
    }
}

/// Result of a full, successful Phala verification.
#[derive(Debug, Clone)]
pub struct VerifiedModel {
    pub model_public_key_hex: String,
    pub tls_fingerprint: Option<String>,
    pub checks: Vec<VerificationCheck>,
    /// Locally verified public state: nonce-bound keyset, receipt keys, and
    /// content-addressed worker sessions. Never prompt or secret key material.
    pub provider_state: Option<serde_json::Value>,
    /// Hard expiry imposed by the verified evidence.
    pub expires_at_unix: Option<u64>,
}

/// Inputs required to challenge Phala evidence through the authenticated
/// Axiom relay. The relay keeps the provider API credential off this client.
pub struct AttestationRequest<'a> {
    pub model_id: &'a str,
    pub expected_model: &'a str,
    pub relay: &'a dyn RelayApi,
    /// Accepted NVIDIA intermediate fingerprints from the locally verified signed policy.
    pub nvidia_anchor_sha256: &'a [String],
}

/// Verifies one model's Phala evidence locally. An error means the model must
/// not receive ciphertext.
#[async_trait]
pub trait Attestor: Send + Sync {
    async fn verify_model(&self, request: AttestationRequest<'_>) -> Result<VerifiedModel>;
}

/// One message exactly as it appears in the encrypted provider request.
#[derive(Debug, Clone)]
pub struct ProviderWireMessage {
    pub role: String,
    pub content: Option<String>,
    pub assistant_null_content: bool,
    pub has_extended_fields: bool,
}

/// Provider-facing request shape used to bind the exact encrypted and restored
/// plaintext bodies before the relay sends them.
#[derive(Debug, Clone)]
pub struct ProviderWireRequest {
    pub model: String,
    pub messages: Vec<ProviderWireMessage>,
    pub max_tokens: u32,
    pub stream: bool,
    pub sampling: Option<serde_json::Value>,
    pub response_format: Option<serde_json::Value>,
    pub reasoning_effort: Option<String>,
    pub has_tools: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DecryptedProviderCompletion {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub refusal: Option<String>,
}

/// One receipt-gated Phala request's client-side cryptographic state.
///
/// There are deliberately no ordinary encrypt/decrypt fallbacks: every field
/// uses its protocol AAD, every request supplies a bound context, and every
/// response requires terminal proof verification.
pub trait CipherSession: Send {
    fn client_public_key_hex(&self) -> &str;
    fn is_ready(&self) -> bool;
    fn encrypt_field(&mut self, plaintext: &[u8], field: &str) -> Result<String>;
    fn decrypt_stream_field(
        &mut self,
        wire: &str,
        response_id: Option<&str>,
        field: Option<&str>,
    ) -> Result<Vec<u8>>;
    fn prepare_request_context(
        &mut self,
        request: &ProviderWireRequest,
    ) -> Result<serde_json::Value>;
    fn decrypt_verified_completion(
        &mut self,
        completion: &RelayCompletion,
    ) -> Result<DecryptedProviderCompletion>;
    fn verify_stream_completion(&mut self, proof: &serde_json::Value) -> Result<()>;
}

/// Factory for per-request Phala cipher sessions.
pub trait ProviderCipher: Send + Sync {
    fn new_session(&self, verified: &VerifiedModel) -> Result<Box<dyn CipherSession>>;
}

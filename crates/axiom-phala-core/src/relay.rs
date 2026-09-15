//! Minimal relay boundary required by the local Phala verifier.

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::Result;

/// Terminal provider proof passed to the receipt-gated Phala cipher.
#[derive(Debug, Clone)]
pub struct RelayCompletion {
    pub proof: serde_json::Value,
}

/// Phala evidence returned by the authenticated Axiom relay.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderAttestationEvidence {
    pub provider: String,
    pub attestation_protocol: String,
    pub evidence: serde_json::Value,
}

/// Evidence-only relay abstraction used by the Phala attestor.
#[async_trait]
pub trait RelayApi: Send + Sync {
    async fn provider_attestation_report(
        &self,
        model_id: &str,
        nonce: &str,
    ) -> Result<ProviderAttestationEvidence>;
}

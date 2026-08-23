use async_trait::async_trait;
use axiom_inference::{AssistantTurn, InferenceRequest, ModelInfo, ProviderEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Result, SecureClientConfig, SecureClientError, SecureProvider, SecurityEvidence, TrustPolicy,
    VerifiedSession, provider::sealed,
};

pub(crate) mod attestation;
#[allow(dead_code)]
pub(crate) mod crypto;

pub(crate) const PROVIDER_ID: &str = "near";
pub(crate) const E2EE_PROTOCOL: &str = "near-v2";
pub(crate) const ENCRYPTION_VERSION: u16 = 2;
pub(crate) const ATTESTATION_PROTOCOL: &str = "near-tdx-nvidia-v1";

pub(crate) struct NearProvider {
    config: SecureClientConfig,
}

impl NearProvider {
    pub(crate) const fn new(config: SecureClientConfig) -> Self {
        Self { config }
    }
}

impl sealed::Provider for NearProvider {}

#[async_trait]
impl SecureProvider for NearProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn supports(&self, model: &ModelInfo) -> Result<()> {
        if model.provider_id != PROVIDER_ID
            || model.e2ee_protocol != E2EE_PROTOCOL
            || model.e2ee_encryption_version != ENCRYPTION_VERSION
            || model.attestation_protocol != ATTESTATION_PROTOCOL
        {
            return Err(SecureClientError::capability(
                "model does not match the compiled NEAR provider contract",
            ));
        }
        if model.upstream_model.trim().is_empty() || model.provider_base_url.trim().is_empty() {
            return Err(SecureClientError::capability(
                "model omits required provider routing metadata",
            ));
        }
        Ok(())
    }

    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        let verified = attestation::verify_model(
            &self.config,
            &model.id,
            &model.upstream_model,
            &model.provider_base_url,
            policy,
            &cancellation,
        )
        .await?;
        Ok(Box::new(NearVerifiedSession {
            evidence: verified.evidence,
            model_key: verified.model_key,
        }))
    }
}

struct NearVerifiedSession {
    evidence: SecurityEvidence,
    #[allow(dead_code)]
    model_key: crypto::Ed25519PublicKey,
}

impl sealed::Session for NearVerifiedSession {}

#[async_trait]
impl VerifiedSession for NearVerifiedSession {
    fn evidence(&self) -> &SecurityEvidence {
        &self.evidence
    }

    async fn complete(
        &mut self,
        _request: InferenceRequest,
        _cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        Err(SecureClientError::session(
            "encrypted relay transport is not connected yet",
        ))
    }

    async fn stream(
        &mut self,
        _request: InferenceRequest,
        _events: mpsc::Sender<ProviderEvent>,
        _cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        Err(SecureClientError::session(
            "encrypted relay transport is not connected yet",
        ))
    }
}

#[cfg(test)]
mod tests {
    use axiom_inference::ModelInfo;

    use super::*;

    fn config() -> SecureClientConfig {
        SecureClientConfig::new("https://api.axiom.stream").unwrap()
    }

    #[test]
    fn provider_rejects_every_contract_mismatch() {
        let provider = NearProvider::new(config());
        let valid = ModelInfo {
            id: "model-id".into(),
            provider_id: PROVIDER_ID.into(),
            upstream_model: "model/upstream".into(),
            provider_base_url: "https://model.example/v1".into(),
            e2ee_protocol: E2EE_PROTOCOL.into(),
            e2ee_encryption_version: ENCRYPTION_VERSION,
            attestation_protocol: ATTESTATION_PROTOCOL.into(),
            ..ModelInfo::default()
        };
        assert!(provider.supports(&valid).is_ok());

        for mutated in [
            ModelInfo {
                provider_id: "other".into(),
                ..valid.clone()
            },
            ModelInfo {
                e2ee_protocol: "near-v3".into(),
                ..valid.clone()
            },
            ModelInfo {
                e2ee_encryption_version: 3,
                ..valid.clone()
            },
            ModelInfo {
                attestation_protocol: "other".into(),
                ..valid.clone()
            },
        ] {
            assert!(provider.supports(&mutated).is_err());
        }
    }
}

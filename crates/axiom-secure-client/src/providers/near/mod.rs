use std::sync::Arc;

use async_trait::async_trait;
use axiom_inference::{InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, Result, SecureClientConfig, SecureClientError, SecureProvider, SecurityEvidence,
    TrustPolicy, VerifiedSession,
    lease::{AttestationBinding, VerifiedMaterialCache, key_fingerprint},
    provider::sealed,
    relay::{client::RelayClient, dto::ProviderKeyLease},
};

pub(crate) mod attestation;
#[allow(dead_code)]
pub(crate) mod crypto;
pub(crate) mod inference;

pub(crate) const PROVIDER_ID: &str = "near";
pub(crate) const E2EE_PROTOCOL: &str = "near-v2";
pub(crate) const ENCRYPTION_VERSION: u16 = 2;
pub(crate) const ATTESTATION_PROTOCOL: &str = "near-tdx-nvidia-v1";

pub(crate) struct NearProvider {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    cache: VerifiedMaterialCache<NearVerifiedMaterial>,
}

impl NearProvider {
    pub(crate) fn new(config: SecureClientConfig, credential: Arc<ApiCredential>) -> Self {
        Self {
            config,
            credential,
            cache: VerifiedMaterialCache::default(),
        }
    }
}

#[derive(Clone)]
struct NearVerifiedMaterial {
    verified: attestation::VerifiedNearAttestation,
    binding: AttestationBinding,
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

    fn populate_capabilities(&self, model: &mut ModelInfo) {
        model.supports_tools = true;
        model.supports_parallel_tools = true;
        model.supports_streaming = true;
    }

    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        if let Some(material) = self.cache.get(model, policy).await {
            return Ok(session_from_material(
                material,
                &self.config,
                &self.credential,
                model,
            ));
        }
        let _singleflight = self.cache.model_lock(&model.id).await;
        if let Some(material) = self.cache.get(model, policy).await {
            return Ok(session_from_material(
                material,
                &self.config,
                &self.credential,
                model,
            ));
        }
        let mut verified = Box::pin(attestation::verify_model(
            &self.config,
            &model.id,
            &model.upstream_model,
            &model.provider_base_url,
            policy,
            &cancellation,
        ))
        .await?;
        let observed_key_fingerprint = key_fingerprint(&verified.model_key.to_hex())?;
        let wire: ProviderKeyLease = RelayClient::new(&self.config, &self.credential)
            .provider_e2ee_key(
                &model.id,
                Some(&observed_key_fingerprint),
                None,
                &cancellation,
            )
            .await?;
        verify_response_signing_binding(&wire, &verified.response_signing_address)?;
        let binding = AttestationBinding::verify(wire, model, &verified.model_key.to_hex())?;
        verified.evidence.attestation_generation = Some(binding.generation);
        verified.evidence.hard_expires_at_unix_seconds = Some(binding.hard_expires_at_unix_seconds);
        let material = NearVerifiedMaterial { verified, binding };
        self.cache
            .insert(
                model.clone(),
                policy.clone(),
                material.clone(),
                material
                    .binding
                    .hard_expires_at_unix_seconds
                    .min(policy.expires_at_unix_seconds),
                self.config.verified_session_ttl,
            )
            .await;
        Ok(session_from_material(
            material,
            &self.config,
            &self.credential,
            model,
        ))
    }

    async fn invalidate(&self, model_id: &str) {
        self.cache.invalidate(model_id).await;
    }
}

fn session_from_material(
    material: NearVerifiedMaterial,
    config: &SecureClientConfig,
    credential: &Arc<ApiCredential>,
    model: &ModelInfo,
) -> Box<dyn VerifiedSession> {
    Box::new(NearVerifiedSession {
        evidence: material.verified.evidence,
        model_key: material.verified.model_key,
        response_signing_address: material.verified.response_signing_address,
        config: config.clone(),
        credential: Arc::clone(credential),
        model: model.clone(),
        binding: material.binding,
    })
}

struct NearVerifiedSession {
    evidence: SecurityEvidence,
    model_key: crypto::Ed25519PublicKey,
    response_signing_address: String,
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    model: ModelInfo,
    binding: AttestationBinding,
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
    ) -> Result<InferenceResponse> {
        inference::complete(
            inference::NearInferenceContext {
                config: &self.config,
                credential: &self.credential,
                model: &self.model,
                model_key: self.model_key,
                response_signing_address: &self.response_signing_address,
                binding: &self.binding,
            },
            _request,
            &_cancellation,
        )
        .await
    }

    async fn stream(
        &mut self,
        _request: InferenceRequest,
        _events: mpsc::Sender<ProviderEvent>,
        _cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        inference::stream(
            inference::NearInferenceContext {
                config: &self.config,
                credential: &self.credential,
                model: &self.model,
                model_key: self.model_key,
                response_signing_address: &self.response_signing_address,
                binding: &self.binding,
            },
            _request,
            _events,
            &_cancellation,
        )
        .await
    }
}

fn verify_response_signing_binding(wire: &ProviderKeyLease, locally_attested: &str) -> Result<()> {
    let wire_address = wire.response_signing_address.as_deref().ok_or_else(|| {
        SecureClientError::attestation("backend lease omitted the NEAR response-signing address")
    })?;
    let wire_fingerprint = wire
        .response_signing_key_fingerprint
        .as_deref()
        .ok_or_else(|| {
            SecureClientError::attestation(
                "backend lease omitted the NEAR response-signing fingerprint",
            )
        })?;
    if !canonical_response_signing_address(wire_address)
        || !canonical_response_signing_address(locally_attested)
        || wire_address != locally_attested
    {
        return Err(SecureClientError::attestation(
            "backend lease response signer does not match local provider evidence",
        ));
    }
    let address_bytes = hex::decode(&wire_address[2..])
        .map_err(|_| SecureClientError::attestation("backend lease response signer is invalid"))?;
    let expected_fingerprint = hex::encode(Sha256::digest(address_bytes));
    if wire_fingerprint != expected_fingerprint {
        return Err(SecureClientError::attestation(
            "backend lease response-signer fingerprint is invalid",
        ));
    }
    Ok(())
}

fn canonical_response_signing_address(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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
        let provider = NearProvider::new(config(), Arc::new(ApiCredential::new("test")));
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

    #[test]
    fn response_signer_lease_must_match_local_attestation_and_fingerprint() {
        let address = format!("0x{}", "12".repeat(20));
        let fingerprint = hex::encode(Sha256::digest(hex::decode(&address[2..]).unwrap()));
        let mut wire = ProviderKeyLease {
            model_id: "model-id".into(),
            model: "model/upstream".into(),
            base_url: "https://model.example/v1".into(),
            provider: PROVIDER_ID.into(),
            attestation_protocol: ATTESTATION_PROTOCOL.into(),
            encryption_version: ENCRYPTION_VERSION,
            e2ee_protocol: E2EE_PROTOCOL.into(),
            model_public_key: "34".repeat(32),
            verified: true,
            attestation_generation: 1,
            model_key_fingerprint: "56".repeat(32),
            keyset_digest: None,
            response_signing_address: Some(address.clone()),
            response_signing_key_fingerprint: Some(fingerprint),
            hard_expires_at_unix_seconds: u64::MAX,
        };
        assert!(verify_response_signing_binding(&wire, &address).is_ok());

        wire.response_signing_key_fingerprint = Some("00".repeat(32));
        assert!(verify_response_signing_binding(&wire, &address).is_err());
        wire.response_signing_key_fingerprint = None;
        assert!(verify_response_signing_binding(&wire, &address).is_err());
        wire.response_signing_address = Some(format!("0x{}", "13".repeat(20)));
        assert!(verify_response_signing_binding(&wire, &address).is_err());
    }
}

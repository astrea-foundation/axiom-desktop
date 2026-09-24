use std::sync::Arc;

use async_trait::async_trait;
use axiom_inference::{InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, Result, SecureClientConfig, SecureClientError, SecureProvider, SecurityEvidence,
    TrustPolicy, VerifiedSession,
    lease::{AttestationBinding, VerifiedMaterialCache},
    provider::sealed,
    relay::dto::ProviderKeyLease,
};

pub(crate) mod attestation;
pub(crate) mod crypto;
pub(crate) mod inference;

pub(crate) const PROVIDER_ID: &str = "near";
pub(crate) const E2EE_PROTOCOL: &str = "near-v3";
pub(crate) const ENCRYPTION_VERSION: u16 = 2;
pub(crate) const ATTESTATION_PROTOCOL: &str = "near-tdx-nvidia-v2";

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
    worker_session_id: String,
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

    fn supports_contract(
        &self,
        e2ee_protocol: &str,
        e2ee_encryption_version: u16,
        attestation_protocol: &str,
    ) -> bool {
        e2ee_protocol == E2EE_PROTOCOL
            && e2ee_encryption_version == ENCRYPTION_VERSION
            && attestation_protocol == ATTESTATION_PROTOCOL
    }

    fn populate_capabilities(&self, model: &mut ModelInfo) {
        model.supports_streaming = true;
        // NEAR advertises native image parts, but no general document format.
        model.file_mime_types.clear();
        model.reasoning_parameters.clear();
        model.thinking_parameters.clear();
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
        let _singleflight = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            lock = self.cache.model_lock(&model.id) => lock,
        };
        if let Some(material) = self.cache.get(model, policy).await {
            return Ok(session_from_material(
                material,
                &self.config,
                &self.credential,
                model,
            ));
        }
        let (mut verified, wire, worker_session_id) = Box::pin(attestation::verify_model(
            &self.config,
            &self.credential,
            model,
            policy,
            &cancellation,
        ))
        .await?;
        verify_response_signing_binding(&wire, &verified.response_signing_address)?;
        let binding = AttestationBinding::verify(wire, model, &verified.model_key.to_hex())?;
        verified.evidence.attestation_generation = Some(binding.generation);
        // The UI must stop reusing a report when this process stops reusing its
        // attested keys, even if the relay's lease lasts longer.
        let expires = binding
            .hard_expires_at_unix_seconds
            .min(policy.expires_at_unix_seconds)
            .min(
                verified
                    .evidence
                    .verified_at_unix_seconds
                    .saturating_add(self.config.verified_session_ttl.as_secs()),
            );
        verified.evidence.hard_expires_at_unix_seconds = Some(expires);
        let material = NearVerifiedMaterial {
            verified,
            binding,
            worker_session_id,
        };
        self.cache
            .insert(
                model.clone(),
                policy.clone(),
                material.clone(),
                expires,
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
        worker_session_id: material.worker_session_id,
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
    worker_session_id: String,
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
                worker_session_id: &self.worker_session_id,
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
                worker_session_id: &self.worker_session_id,
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
    let address_bytes = hex::decode(wire_address)
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
    value.len() == 64
        && value
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

    #[tokio::test]
    async fn cancelled_inference_does_not_wait_for_an_inflight_warmup() {
        let provider = NearProvider::new(config(), Arc::new(ApiCredential::new("test")));
        let model = ModelInfo {
            id: "model-id".into(),
            provider_id: PROVIDER_ID.into(),
            upstream_model: "model/upstream".into(),
            provider_base_url: "https://model.example/v1".into(),
            e2ee_protocol: E2EE_PROTOCOL.into(),
            e2ee_encryption_version: ENCRYPTION_VERSION,
            attestation_protocol: ATTESTATION_PROTOCOL.into(),
            ..ModelInfo::default()
        };
        let _warmup = provider.cache.model_lock(&model.id).await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            provider.establish(&model, &TrustPolicy::default(), cancellation),
        )
        .await
        .expect("cancellation must interrupt the singleflight wait");
        assert!(result.is_err());
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
                e2ee_protocol: "near-v4".into(),
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
        let address = "12".repeat(32);
        let fingerprint = hex::encode(Sha256::digest(hex::decode(&address).unwrap()));
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
        wire.response_signing_address = Some("13".repeat(32));
        assert!(verify_response_signing_binding(&wire, &address).is_err());
    }
}

//! Deterministic encrypted provider used only by process-level tests.
//!
//! This provider proves local composition and fault handling. Its evidence is
//! signed by a public test key embedded in the feature, so it is not production
//! attestation and can never satisfy a normal production catalog entry.

use std::{sync::Arc, time::SystemTime};

use async_trait::async_trait;
use axiom_inference::{InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecureProvider, SecurityEvidence, SecurityState, TrustPolicy, VerifiedSession,
    http::{bounded_body, pinned_client},
    lease::AttestationBinding,
    provider::sealed,
    providers::near::{crypto, inference},
    test_support::fixture_response_signing_address,
};

pub(crate) const PROVIDER_ID: &str = "axiom-test-fixture";
const E2EE_PROTOCOL: &str = "near-v3";
const ENCRYPTION_VERSION: u16 = 2;
const ATTESTATION_PROTOCOL: &str = "signed-test-fixture-v1";
const SIGNING_SEED: [u8; 32] = [0xA5; 32];
const SIGNATURE_DOMAIN: &[u8] = b"axiom-test-fixture-attestation-v1\0";

pub(crate) struct FixtureProvider {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
}

impl FixtureProvider {
    pub(crate) const fn new(config: SecureClientConfig, credential: Arc<ApiCredential>) -> Self {
        Self { config, credential }
    }
}

impl sealed::Provider for FixtureProvider {}

#[async_trait]
impl SecureProvider for FixtureProvider {
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
                "model does not match the deterministic fixture contract",
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
        let (model_key, evidence) =
            verify_fixture_evidence(&self.config, model, policy, &cancellation).await?;
        let binding = AttestationBinding {
            generation: 1,
            model_key_fingerprint: evidence.model_key_fingerprint.clone(),
            keyset_digest: None,
            hard_expires_at_unix_seconds: evidence
                .hard_expires_at_unix_seconds
                .ok_or_else(|| rejected("fixture evidence omitted its hard expiry"))?,
        };
        Ok(Box::new(FixtureSession {
            evidence,
            model_key,
            response_signing_address: fixture_response_signing_address(),
            binding,
            config: self.config.clone(),
            credential: Arc::clone(&self.credential),
            model: model.clone(),
        }))
    }
}

struct FixtureSession {
    evidence: SecurityEvidence,
    model_key: crypto::Ed25519PublicKey,
    response_signing_address: String,
    binding: AttestationBinding,
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    model: ModelInfo,
}

impl sealed::Session for FixtureSession {}

#[async_trait]
impl VerifiedSession for FixtureSession {
    fn evidence(&self) -> &SecurityEvidence {
        &self.evidence
    }

    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        inference::complete(
            inference::NearInferenceContext {
                config: &self.config,
                credential: &self.credential,
                model: &self.model,
                model_key: self.model_key,
                response_signing_address: &self.response_signing_address,
                binding: &self.binding,
                worker_session_id: "00000000000000000000000000000001",
            },
            request,
            &cancellation,
        )
        .await
    }

    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        inference::stream(
            inference::NearInferenceContext {
                config: &self.config,
                credential: &self.credential,
                model: &self.model,
                model_key: self.model_key,
                response_signing_address: &self.response_signing_address,
                binding: &self.binding,
                worker_session_id: "00000000000000000000000000000001",
            },
            request,
            events,
            &cancellation,
        )
        .await
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureEvidence {
    model: String,
    nonce_hex: String,
    model_public_key_hex: String,
    signature_hex: String,
}

async fn verify_fixture_evidence(
    config: &SecureClientConfig,
    model: &ModelInfo,
    policy: &TrustPolicy,
    cancellation: &CancellationToken,
) -> Result<(crypto::Ed25519PublicKey, SecurityEvidence)> {
    let mut nonce = [0_u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let nonce_hex = hex::encode(nonce);
    let endpoint = format!(
        "{}/attestation/report",
        model.provider_base_url.trim_end_matches('/')
    );
    let endpoint = config.endpoint_policy.validate_provider_url(&endpoint)?;
    let client = pinned_client(
        &endpoint,
        &config.endpoint_policy,
        config.attestation_timeout,
        false,
    )
    .await?;
    let response = tokio::select! {
        () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
        response = client.get(endpoint).query(&[("model", &model.upstream_model), ("nonce", &nonce_hex)]).send() =>
            response.map_err(|_| SecureClientError::new(
                axiom_inference::ProviderFailureKind::AttestationUnavailable,
                "fixture evidence request failed",
            ))?,
    };
    if response.status() != StatusCode::OK {
        return Err(SecureClientError::new(
            axiom_inference::ProviderFailureKind::AttestationUnavailable,
            "fixture evidence endpoint rejected the request",
        ));
    }
    let body = bounded_body(response, config.limits.attestation_report_bytes).await?;
    let wire: FixtureEvidence = serde_json::from_slice(&body).map_err(|_| {
        SecureClientError::new(
            axiom_inference::ProviderFailureKind::AttestationRejected,
            "fixture evidence schema is invalid",
        )
    })?;
    if wire.model != model.upstream_model || wire.nonce_hex != nonce_hex {
        return Err(rejected("fixture evidence binding is invalid"));
    }
    let model_key = crypto::Ed25519PublicKey::from_hex(&wire.model_public_key_hex)
        .map_err(|_| rejected("fixture model key is invalid"))?;
    let signature_bytes = hex::decode(&wire.signature_hex)
        .map_err(|_| rejected("fixture evidence signature is invalid"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| rejected("fixture evidence signature is invalid"))?;
    let signing = SigningKey::from_bytes(&SIGNING_SEED);
    let verifying = VerifyingKey::from_bytes(&signing.verifying_key().to_bytes())
        .map_err(|_| rejected("fixture verifier is invalid"))?;
    let payload = signature_payload(
        &wire.model,
        &wire.nonce_hex,
        &model.provider_base_url,
        &wire.model_public_key_hex,
    );
    verifying
        .verify_strict(&payload, &signature)
        .map_err(|_| rejected("fixture evidence signature was rejected"))?;

    let fingerprint = hex::encode(Sha256::digest(model_key.as_bytes()));
    let verified_at_unix_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    Ok((
        model_key,
        SecurityEvidence {
            state: SecurityState::Verified,
            provider_id: PROVIDER_ID.to_owned(),
            model_id: model.id.clone(),
            attestation_protocol: ATTESTATION_PROTOCOL.to_owned(),
            e2ee_protocol: E2EE_PROTOCOL.to_owned(),
            e2ee_encryption_version: ENCRYPTION_VERSION,
            trust_policy_version: policy.version.clone(),
            verified_at_unix_seconds,
            attestation_generation: Some(1),
            hard_expires_at_unix_seconds: Some(verified_at_unix_seconds.saturating_add(600)),
            model_key_fingerprint: fingerprint,
            tls_spki_fingerprint: None,
            checks: vec![
                EvidenceCheck {
                    id: "fresh_nonce".to_owned(),
                    label: "Fresh challenge".to_owned(),
                    status: "verified test fixture".to_owned(),
                    passed: true,
                },
                EvidenceCheck {
                    id: "fixture_signature".to_owned(),
                    label: "Deterministic fixture signature".to_owned(),
                    status: "verified test fixture".to_owned(),
                    passed: true,
                },
            ],
            provider_claims: vec![
                EvidenceClaim {
                    name: "scope".to_owned(),
                    value: "process integration testing only; not TEE attestation".to_owned(),
                },
                EvidenceClaim {
                    name: "response_signing_address".to_owned(),
                    value: fixture_response_signing_address(),
                },
            ],
            workload_manifest: None,
        },
    ))
}

pub(crate) fn sign_evidence_for_test(
    model: &str,
    nonce_hex: &str,
    base_url: &str,
    model_public_key_hex: &str,
) -> String {
    let signing = SigningKey::from_bytes(&SIGNING_SEED);
    let payload = signature_payload(model, nonce_hex, base_url, model_public_key_hex);
    hex::encode(signing.sign(&payload).to_bytes())
}

fn signature_payload(
    model: &str,
    nonce_hex: &str,
    base_url: &str,
    model_public_key_hex: &str,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(
        SIGNATURE_DOMAIN.len()
            + model.len()
            + nonce_hex.len()
            + base_url.len()
            + model_public_key_hex.len()
            + 4,
    );
    payload.extend_from_slice(SIGNATURE_DOMAIN);
    for value in [model, nonce_hex, base_url, model_public_key_hex] {
        payload.extend_from_slice(value.as_bytes());
        payload.push(0);
    }
    payload
}

fn rejected(detail: &'static str) -> SecureClientError {
    SecureClientError::new(
        axiom_inference::ProviderFailureKind::AttestationRejected,
        detail,
    )
}

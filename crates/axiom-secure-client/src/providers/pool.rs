//! Direct worker E2EE; the platform/gateway transports only sealed worker bytes.
use crate::{
    ApiCredential, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecurityEvidence, SecurityState, TrustPolicy,
    provider::{SecureProvider, VerifiedSession, sealed},
};
use async_trait::async_trait;
use axiom_inference::{
    AssistantTurn, ChatRole, FinishReason, InferenceRequest, InferenceResponse, ModelInfo,
    ProviderEvent, ReasoningEffort, ResponseFormat, SamplingParameters, ThinkingMode, Usage,
};
use axiom_pool_client::{
    PoolClient,
    attestation::{VerifiedWorker, VerifierConfig},
};
use axiom_pool_protocol::types::{Input, Limits, Message};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const ID: &str = "axiom-pool";
pub const ATTESTATION: &str = "tinfoil-gpu-v3";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolOffering {
    pub model_manifest: String,
    pub label: String,
    pub context_tokens: u32,
    pub output_tokens: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolClientConfig {
    pub issuer_key: String,
    pub verifier: VerifierConfig,
    pub models: Vec<PoolOffering>,
}
pub(crate) fn installed_config() -> Result<Option<PoolClientConfig>> {
    let path = match std::env::var_os("AXIOM_POOL_CLIENT_CONFIG_FILE") {
        Some(path) => path,
        None => return Ok(None),
    };
    let raw = std::fs::read(path)
        .map_err(|_| SecureClientError::configuration("pool client configuration unavailable"))?;
    if raw.len() > 128 * 1024 {
        return Err(SecureClientError::configuration(
            "pool configuration exceeds limit",
        ));
    }
    let cfg: PoolClientConfig = axiom_pool_protocol::strict::decode(&raw)
        .map_err(|_| SecureClientError::configuration("invalid pool client configuration"))?;
    if axiom_pool_protocol::hex_bytes(&cfg.issuer_key, 32).is_err()
        || cfg.models.is_empty()
        || cfg.models.len() > 128
    {
        return Err(SecureClientError::configuration(
            "invalid pool issuer/models",
        ));
    }
    for model in &cfg.models {
        if axiom_pool_protocol::hex_bytes(&model.model_manifest, 32).is_err()
            || model.label.is_empty()
            || model.label.len() > 128
            || model.context_tokens == 0
            || model.context_tokens > 131072
            || model.output_tokens == 0
            || model.output_tokens > 32768
            || model.output_tokens > model.context_tokens
        {
            return Err(SecureClientError::configuration("invalid pool offering"));
        }
    }
    Ok(Some(cfg))
}
pub(crate) fn models(config: &SecureClientConfig) -> Vec<ModelInfo> {
    config
        .pool
        .as_ref()
        .map(|cfg| {
            cfg.models
                .iter()
                .map(|offering| {
                    let mut model = ModelInfo {
                        id: format!("pool:{}", offering.model_manifest),
                        label: offering.label.clone(),
                        short_label: offering.label.clone(),
                        provider_id: ID.into(),
                        provider_label: "Axiom confidential pool".into(),
                        upstream_model: offering.model_manifest.clone(),
                        provider_base_url: config
                            .relay_base_url
                            .as_str()
                            .trim_end_matches('/')
                            .into(),
                        e2ee_protocol: axiom_pool_protocol::PROTOCOL.into(),
                        e2ee_encryption_version: 1,
                        attestation_protocol: ATTESTATION.into(),
                        context_window_tokens: offering.context_tokens,
                        max_output_tokens: offering.output_tokens,
                        input_price_microusd_per_million_tokens: Some(0),
                        output_price_microusd_per_million_tokens: Some(0),
                        ..ModelInfo::default()
                    };
                    capabilities(&mut model);
                    model
                })
                .collect()
        })
        .unwrap_or_default()
}
fn capabilities(model: &mut ModelInfo) {
    model.supports_streaming = true;
    model.supports_tools = false;
    model.supports_parallel_tools = false;
    model.supports_images = false;
    model.file_mime_types.clear();
    model.reasoning_replay = false;
    model.supported_reasoning_efforts = vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];
    model.supported_thinking_modes = vec![ThinkingMode::ProviderDefault, ThinkingMode::Enabled];
}
pub(crate) struct PoolProvider {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
}
impl PoolProvider {
    pub fn new(config: SecureClientConfig, credential: Arc<ApiCredential>) -> Self {
        Self { config, credential }
    }
}
impl sealed::Provider for PoolProvider {}
#[async_trait]
impl SecureProvider for PoolProvider {
    fn id(&self) -> &'static str {
        ID
    }
    fn supports(&self, model: &ModelInfo) -> Result<()> {
        let origin = self.config.relay_base_url.as_str().trim_end_matches('/');
        if model.provider_id != ID
            || !self.supports_contract(
                &model.e2ee_protocol,
                model.e2ee_encryption_version,
                &model.attestation_protocol,
            )
            || model.provider_base_url != origin
            || !models(&self.config).iter().any(|m| {
                m.id == model.id
                    && m.upstream_model == model.upstream_model
                    && m.context_window_tokens == model.context_window_tokens
                    && m.max_output_tokens == model.max_output_tokens
            })
        {
            return Err(SecureClientError::capability(
                "unconfigured pool offering/contract",
            ));
        }
        Ok(())
    }
    fn supports_contract(&self, protocol: &str, version: u16, profile: &str) -> bool {
        protocol == axiom_pool_protocol::PROTOCOL && version == 1 && profile == ATTESTATION
    }
    fn populate_capabilities(&self, model: &mut ModelInfo) {
        capabilities(model)
    }
    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancel: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        if !policy.is_valid_at(now())
            || policy.accepted_intel_statuses != ["UpToDate"]
            || !policy.require_nvidia_verified
            || !policy.require_model_binding
        {
            return Err(SecureClientError::attestation(
                "strict pool policy required",
            ));
        }
        let installed = self
            .config
            .pool
            .as_ref()
            .ok_or_else(|| SecureClientError::configuration("pool not configured"))?;
        let client = Arc::new(
            PoolClient::new(
                self.config
                    .relay_base_url
                    .as_str()
                    .trim_end_matches('/')
                    .into(),
                &installed.issuer_key,
                secrecy::SecretString::new(self.credential.expose().to_owned().into_boxed_str()),
                installed.verifier.clone(),
            )
            .map_err(|_| SecureClientError::configuration("invalid pool client configuration"))?,
        );
        let worker = client
            .probe(&model.upstream_model, &cancel)
            .await
            .map_err(|_| SecureClientError::attestation("pool worker attestation failed"))?;
        let evidence = evidence(model, policy, &worker);
        Ok(Box::new(Session {
            client,
            model: model.clone(),
            policy: policy.clone(),
            worker,
            evidence,
        }))
    }
}
struct Session {
    client: Arc<PoolClient>,
    model: ModelInfo,
    policy: TrustPolicy,
    worker: VerifiedWorker,
    evidence: SecurityEvidence,
}
impl sealed::Session for Session {}
#[async_trait]
impl VerifiedSession for Session {
    fn evidence(&self) -> &SecurityEvidence {
        &self.evidence
    }
    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancel: CancellationToken,
    ) -> Result<InferenceResponse> {
        self.run(request, None, cancel).await
    }
    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<InferenceResponse> {
        self.run(request, Some(events), cancel).await
    }
}
impl Session {
    async fn run(
        &mut self,
        request: InferenceRequest,
        events: Option<mpsc::Sender<ProviderEvent>>,
        cancel: CancellationToken,
    ) -> Result<InferenceResponse> {
        request
            .validate()
            .map_err(|_| SecureClientError::capability("invalid pool request"))?;
        if request.model != self.model.id
            || !self.policy.is_valid_at(now())
            || !request.tools.is_empty()
            || request.thinking_mode == ThinkingMode::Disabled
            || request.sampling != SamplingParameters::default()
            || request.response_format != ResponseFormat::Text
        {
            return Err(SecureClientError::capability(
                "pool request controls unsupported",
            ));
        }
        let mut messages = Vec::new();
        for m in request.messages {
            if !m.images.is_empty()
                || !m.files.is_empty()
                || !m.tool_calls.is_empty()
                || m.tool_call_id.is_some()
                || m.name.is_some()
                || m.refusal.is_some()
                || m.reasoning_content.is_some()
                || m.role == ChatRole::Tool
            {
                return Err(SecureClientError::capability(
                    "pool currently accepts text conversation messages",
                ));
            }
            let role = serde_json::to_value(&m.role)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| SecureClientError::capability("message role"))?;
            messages.push(Message {
                role,
                content: m.content,
            });
        }
        let effort = match request.reasoning_effort {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            _ => {
                return Err(SecureClientError::capability(
                    "unsupported reasoning effort",
                ));
            }
        };
        let max_tokens = u64::from(
            request
                .max_output_tokens
                .unwrap_or(self.model.max_output_tokens.min(4096)),
        );
        let input = Input::Chat {
            messages,
            max_tokens,
            reasoning_effort: effort.into(),
            tools: vec![],
        };
        let limits = Limits {
            context_tokens: u64::from(self.model.context_window_tokens),
            output_tokens: max_tokens,
            input_bytes: 3 * 1024 * 1024,
            output_bytes: 16 * 1024 * 1024,
            image_pixels: 0,
            image_steps: 0,
            deadline: now() + 600,
        };
        let request_id = request
            .request_id
            .map(|id| wire_request_id(&id))
            .transpose()?;
        let prepared = self
            .client
            .prepare_pinned(
                &self.model.upstream_model,
                input,
                limits,
                request_id,
                Some(&self.worker),
                &cancel,
            )
            .await
            .map_err(|_| {
                SecureClientError::attestation("pool reservation or worker verification failed")
            })?;
        self.evidence = evidence(&self.model, &self.policy, prepared.worker());
        let subscribers = events.clone();
        let output = self
            .client
            .execute_with(
                prepared,
                move |frame| {
                    let events = subscribers.clone();
                    async move {
                        if let Some(events) = events {
                            let event = match frame.kind.as_str() {
                                "text_delta" => Some(ProviderEvent::TextDelta(
                                    frame.data["text"].as_str().unwrap_or_default().into(),
                                )),
                                "reasoning_delta" => Some(ProviderEvent::ReasoningDelta(
                                    frame.data["text"].as_str().unwrap_or_default().into(),
                                )),
                                _ => None,
                            };
                            if let Some(event) = event {
                                events
                                    .send(event)
                                    .await
                                    .map_err(|_| anyhow::anyhow!("subscriber closed"))?;
                            }
                        }
                        Ok(())
                    }
                },
                cancel.clone(),
            )
            .await
            .map_err(|_| {
                if cancel.is_cancelled() {
                    SecureClientError::cancelled()
                } else {
                    SecureClientError::attestation(
                        "pool response incomplete or unauthenticated; inference was not replayed",
                    )
                }
            })?;
        if !self.policy.is_valid_at(now()) {
            return Err(SecureClientError::attestation(
                "pool trust policy expired before completion",
            ));
        }
        let usage = output.terminal.usage;
        let finish = if output.terminal.finish_reason == "length" {
            FinishReason::Length
        } else {
            FinishReason::Stop
        };
        if let Some(events) = events {
            events
                .send(ProviderEvent::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                })
                .await
                .map_err(|_| SecureClientError::cancelled())?;
            events
                .send(ProviderEvent::ResponseVerified)
                .await
                .map_err(|_| SecureClientError::cancelled())?;
        }
        let reasoning = if output.reasoning.is_empty() {
            None
        } else {
            Some(output.reasoning)
        };
        Ok(InferenceResponse {
            assistant: AssistantTurn {
                text: output.text,
                reasoning: reasoning.clone(),
                tool_calls: vec![],
            },
            reasoning,
            refusal: None,
            usage: Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage.input_tokens + usage.output_tokens,
            },
            finish_reason: Some(finish),
        })
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
fn wire_request_id(id: &str) -> Result<String> {
    if axiom_pool_protocol::hex_bytes(id, 16).is_ok() {
        return Ok(id.into());
    }
    let parts = id.split('-').collect::<Vec<_>>();
    if parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12] {
        let normalized = parts.concat().to_ascii_lowercase();
        if axiom_pool_protocol::hex_bytes(&normalized, 16).is_ok() {
            return Ok(normalized);
        }
    }
    Err(SecureClientError::capability(
        "pool request ID must be a 128-bit identifier",
    ))
}
fn evidence(model: &ModelInfo, policy: &TrustPolicy, worker: &VerifiedWorker) -> SecurityEvidence {
    SecurityEvidence {
        state: SecurityState::Verified,
        provider_id: ID.into(),
        model_id: model.id.clone(),
        attestation_protocol: ATTESTATION.into(),
        e2ee_protocol: axiom_pool_protocol::PROTOCOL.into(),
        e2ee_encryption_version: 1,
        trust_policy_version: policy.version.clone(),
        verified_at_unix_seconds: now(),
        attestation_generation: None,
        hard_expires_at_unix_seconds: Some(worker.expires_at().min(policy.expires_at_unix_seconds)),
        model_key_fingerprint: axiom_pool_protocol::sha256(
            hex::decode(&worker.identity().encryption_key).unwrap_or_default(),
        ),
        tls_spki_fingerprint: None,
        checks: [
            ("cpu_tee", "CPU TEE"),
            ("nvidia_gpu", "NVIDIA GPU"),
            ("worker_binding", "Direct worker key"),
            ("model_binding", "Measured model manifest"),
        ]
        .into_iter()
        .map(|(id, label)| EvidenceCheck {
            id: id.into(),
            label: label.into(),
            status: "verified".into(),
            passed: true,
        })
        .collect(),
        provider_claims: vec![
            EvidenceClaim {
                name: "worker_id".into(),
                value: worker.identity().worker_id.clone(),
            },
            EvidenceClaim {
                name: "worker_boot_id".into(),
                value: worker.identity().boot_id.clone(),
            },
            EvidenceClaim {
                name: "model_manifest_sha256".into(),
                value: worker.identity().model_manifest.clone(),
            },
        ],
        workload_manifest: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_ids_preserve_all_bits_and_reject_lossy_normalization() {
        assert_eq!(
            wire_request_id("00112233-4455-6677-8899-AABBCCDDEEFF").unwrap(),
            "00112233445566778899aabbccddeeff"
        );
        for invalid in [
            "bad",
            "00112233_4455_6677_8899_aabbccddeeff",
            "00112233445566778899aabbccddeeff00",
            "GG112233-4455-6677-8899-AABBCCDDEEFF",
        ] {
            assert!(wire_request_id(invalid).is_err());
        }
    }
    #[test]
    fn installed_offerings_do_not_accept_catalog_origin_or_protocol_substitution() {
        let mut config = SecureClientConfig::new("https://api.example").unwrap();
        config.pool=Some(serde_json::from_value(serde_json::json!({"issuer_key":"07".repeat(32),"verifier":{"binary":"/installed/axiom-pool-verify","binary_sha256":"08".repeat(32),"publisher_key":"09".repeat(32),"minimum_policy_sequence":1},"models":[{"model_manifest":"aa".repeat(32),"label":"Synthetic offering","context_tokens":32768,"output_tokens":4096}]})).unwrap());
        let provider = PoolProvider::new(config.clone(), Arc::new(ApiCredential::new("test-only")));
        let model = models(&config).remove(0);
        assert!(provider.supports(&model).is_ok());
        assert!(model.supports_streaming && !model.supports_tools && !model.supports_images);
        let mut swapped = model.clone();
        swapped.provider_base_url = "https://attacker.example".into();
        assert!(provider.supports(&swapped).is_err());
        let mut downgraded = model.clone();
        downgraded.attestation_protocol = "test-fixture-only".into();
        assert!(provider.supports(&downgraded).is_err());
        let mut oversized = model;
        oversized.context_window_tokens *= 2;
        assert!(provider.supports(&oversized).is_err());
    }
}

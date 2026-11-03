//! Explicit local operator connection to a measured SNP/NVIDIA worker.
use crate::{
    EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError, SecurityEvidence,
    SecurityState, TrustPolicy,
    provider::{SecureProvider, VerifiedSession, sealed},
};
use async_trait::async_trait;
use axiom_inference::{
    AssistantTurn, ChatRole, FinishReason, InferenceRequest, InferenceResponse, ModelInfo,
    ProviderEvent, ReasoningEffort, ResponseFormat, SamplingParameters, ThinkingMode, Usage,
};
use axiom_pool_client::{
    attestation::VerifiedWorker,
    direct::{DirectClient, DirectConfig},
};
use axiom_pool_protocol::types::{Input, Limits, Message};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const ID: &str = "axiom-direct";
pub const MODEL: &str = "direct:qwen3.8-27b";
const PROFILE: &str = "axiom-snp-nvidia-v1";

pub(crate) fn installed_config(relay: &url::Url) -> Result<Option<DirectConfig>> {
    let Some(path) = std::env::var_os("AXIOM_DIRECT_CLIENT_CONFIG_FILE") else {
        return Ok(None);
    };
    if !matches!(relay.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err(SecureClientError::configuration(
            "direct operator configuration requires a local account API",
        ));
    }
    let path = PathBuf::from(path);
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| {
        SecureClientError::configuration("direct operator configuration unavailable")
    })?;
    if !path.is_absolute() || !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(SecureClientError::configuration(
            "ordinary absolute direct configuration required",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(SecureClientError::configuration(
                "private direct configuration required",
            ));
        }
    }
    let raw = std::fs::read(path).map_err(|_| {
        SecureClientError::configuration("direct operator configuration unavailable")
    })?;
    let cfg: DirectConfig = axiom_pool_protocol::strict::decode(&raw)
        .map_err(|_| SecureClientError::configuration("invalid direct operator configuration"))?;
    DirectClient::new(cfg.clone())
        .map_err(|_| SecureClientError::configuration("invalid direct worker pins"))?;
    Ok(Some(cfg))
}

fn capabilities(model: &mut ModelInfo, config: &SecureClientConfig) {
    model.supports_streaming = true;
    model.supports_tools = false;
    model.supports_parallel_tools = false;
    model.supports_images = config.direct.as_ref().is_some_and(|direct| direct.vision);
    model.file_mime_types.clear();
    model.reasoning_replay = false;
    // This measured worker tokenizes with its installed medium template.
    // Advertising another effort would let background titles select a
    // different prompt template and invalidate the exact token accounting.
    model.supported_reasoning_efforts = vec![ReasoningEffort::Medium];
    model.supported_thinking_modes = vec![ThinkingMode::ProviderDefault, ThinkingMode::Enabled];
}
pub(crate) fn models(config: &SecureClientConfig) -> Vec<ModelInfo> {
    let Some(direct) = &config.direct else {
        return vec![];
    };
    let mut model = ModelInfo {
        id: MODEL.into(),
        label: "Qwen3.8 Uncensored · Verda".into(),
        short_label: "Qwen3.8 Uncensored".into(),
        provider_id: ID.into(),
        provider_label: "Axiom Verda".into(),
        upstream_model: direct.model_manifest.clone(),
        provider_base_url: direct.origin.clone(),
        e2ee_protocol: axiom_pool_protocol::PROTOCOL.into(),
        e2ee_encryption_version: 1,
        attestation_protocol: PROFILE.into(),
        context_window_tokens: 32768,
        max_output_tokens: 4096,
        // The operator pays for the rented machine; there is no token tariff.
        input_price_microusd_per_million_tokens: None,
        output_price_microusd_per_million_tokens: None,
        ..ModelInfo::default()
    };
    capabilities(&mut model, config);
    vec![model]
}

pub(crate) fn provider(config: &SecureClientConfig) -> Result<Option<Arc<dyn SecureProvider>>> {
    if config.direct.is_none() {
        return Ok(None);
    }
    Ok(Some(Arc::new(DirectProvider {
        config: config.clone(),
    })))
}
struct DirectProvider {
    config: SecureClientConfig,
}
impl sealed::Provider for DirectProvider {}
#[async_trait]
impl SecureProvider for DirectProvider {
    fn id(&self) -> &'static str {
        ID
    }
    fn supports(&self, model: &ModelInfo) -> Result<()> {
        if !models(&self.config).iter().any(|m| {
            m.id == model.id
                && m.provider_id == model.provider_id
                && m.upstream_model == model.upstream_model
                && m.provider_base_url == model.provider_base_url
                && m.attestation_protocol == model.attestation_protocol
                && m.e2ee_protocol == model.e2ee_protocol
                && m.e2ee_encryption_version == model.e2ee_encryption_version
                && m.context_window_tokens == model.context_window_tokens
                && m.max_output_tokens == model.max_output_tokens
                && m.supports_images == model.supports_images
                && m.supports_tools == model.supports_tools
        }) {
            return Err(SecureClientError::capability(
                "unconfigured direct worker contract",
            ));
        }
        Ok(())
    }
    fn supports_contract(&self, protocol: &str, version: u16, profile: &str) -> bool {
        protocol == axiom_pool_protocol::PROTOCOL && version == 1 && profile == PROFILE
    }
    fn populate_capabilities(&self, model: &mut ModelInfo) {
        capabilities(model, &self.config);
    }
    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancel: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        if !policy.is_valid_at(now())
            || !policy.require_nvidia_verified
            || !policy.require_model_binding
            || !policy.require_live_tls_binding
        {
            return Err(SecureClientError::attestation(
                "strict direct attestation policy required",
            ));
        }
        let cfg =
            self.config.direct.clone().ok_or_else(|| {
                SecureClientError::configuration("direct operator not configured")
            })?;
        let client = DirectClient::new(cfg)
            .map_err(|_| SecureClientError::configuration("invalid direct worker pins"))?;
        let worker = client.probe(&cancel).await.map_err(|error| {
            tracing::warn!(detail = %error, "direct attestation preflight failed");
            if cancel.is_cancelled() {
                SecureClientError::cancelled()
            } else {
                SecureClientError::attestation(
                    "Verda workload attestation failed; no message was sent",
                )
            }
        })?;
        let evidence = evidence(model, policy, &worker, client.public_evidence().await);
        Ok(Box::new(Session {
            client,
            worker,
            evidence,
            model: model.clone(),
            policy: policy.clone(),
        }))
    }
}
struct Session {
    client: DirectClient,
    worker: VerifiedWorker,
    evidence: SecurityEvidence,
    model: ModelInfo,
    policy: TrustPolicy,
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
            .map_err(|_| SecureClientError::capability("invalid direct request"))?;
        if request.model != self.model.id
            || !self.policy.is_valid_at(now())
            || !request.tools.is_empty()
            || request.thinking_mode == ThinkingMode::Disabled
            || request.sampling != SamplingParameters::default()
            || request.response_format != ResponseFormat::Text
            || request.reasoning_effort != ReasoningEffort::Medium
        {
            return Err(SecureClientError::capability(
                "direct worker request controls unsupported",
            ));
        }
        let mut messages = Vec::new();
        let mut input_image_pixels = 0u64;
        let mut input_image_count = 0usize;
        for m in request.messages {
            if (!m.images.is_empty() && !self.model.supports_images)
                || !m.files.is_empty()
                || !m.tool_calls.is_empty()
                || m.tool_call_id.is_some()
                || m.name.is_some()
                || m.refusal.is_some()
                || m.reasoning_content.is_some()
                || m.role == ChatRole::Tool
            {
                return Err(SecureClientError::capability(
                    "direct worker request contains unsupported attachments or message fields",
                ));
            }
            let role = serde_json::to_value(m.role)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| SecureClientError::capability("invalid direct message role"))?;
            let images = m
                .images
                .into_iter()
                .map(|image| {
                    axiom_pool_protocol::vision::InlineImage::from_upload(
                        &image.mime_type,
                        &image.data,
                    )
                    .map_err(|_| {
                        SecureClientError::capability(
                            "Image is invalid or exceeds the installed vision limits",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            input_image_count += images.len();
            if input_image_count > axiom_pool_protocol::vision::MAX_IMAGES {
                return Err(SecureClientError::capability(
                    "This connection supports at most eight images in the current context",
                ));
            }
            for image in &images {
                input_image_pixels = input_image_pixels
                    .checked_add(image.pixels())
                    .ok_or_else(|| SecureClientError::capability("Image pixel budget exceeded"))?;
            }
            messages.push(Message {
                role,
                content: m.content,
                images,
            });
        }
        let effort = match request.reasoning_effort {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            _ => {
                return Err(SecureClientError::capability(
                    "unsupported direct reasoning effort",
                ));
            }
        };
        let max_tokens = u64::from(request.max_output_tokens.unwrap_or(4096));
        if max_tokens > 4096 {
            return Err(SecureClientError::capability(
                "direct worker output limit is 4096 tokens",
            ));
        }
        let input = Input::Chat {
            messages,
            max_tokens,
            reasoning_effort: effort.into(),
            tools: vec![],
        };
        let limits = Limits {
            context_tokens: 32768,
            output_tokens: max_tokens,
            input_bytes: axiom_pool_protocol::MAX_REQUEST as u64,
            output_bytes: 32 * 1024 * 1024,
            image_pixels: 0,
            image_steps: 0,
            input_image_pixels,
            deadline: now() + 180,
        };
        let subscribers = events.clone();
        let (worker, output) = self
            .client
            .execute_with(
                input,
                limits,
                Some(&self.worker),
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
            .map_err(|error| {
                tracing::warn!(detail = %error, "direct attested exchange failed");
                if cancel.is_cancelled() {
                    SecureClientError::cancelled()
                } else {
                    SecureClientError::attestation(
                        "Verda response incomplete or unauthenticated; inference was not replayed",
                    )
                }
            })?;
        if !self.policy.is_valid_at(now()) {
            return Err(SecureClientError::attestation(
                "native trust policy expired before completion",
            ));
        }
        self.evidence = evidence(
            &self.model,
            &self.policy,
            &worker,
            self.client.public_evidence().await,
        );
        self.worker = worker;
        let usage = output.terminal.usage;
        let finish = if output.terminal.finish_reason == "length" {
            FinishReason::Length
        } else {
            FinishReason::Stop
        };
        if let Some(events) = events {
            for event in [
                ProviderEvent::SecurityEvidence(Box::new(self.evidence.clone())),
                ProviderEvent::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                },
                ProviderEvent::ResponseVerified,
            ] {
                events
                    .send(event)
                    .await
                    .map_err(|_| SecureClientError::cancelled())?;
            }
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
fn evidence(
    model: &ModelInfo,
    policy: &TrustPolicy,
    worker: &VerifiedWorker,
    document: Option<serde_json::Value>,
) -> SecurityEvidence {
    SecurityEvidence {
        state: SecurityState::Verified,
        provider_id: ID.into(),
        model_id: model.id.clone(),
        attestation_protocol: PROFILE.into(),
        e2ee_protocol: axiom_pool_protocol::PROTOCOL.into(),
        e2ee_encryption_version: 1,
        trust_policy_version: format!(
            "{} / direct policy {}",
            policy.version,
            worker.policy_sequence()
        ),
        verified_at_unix_seconds: now(),
        attestation_generation: Some(worker.generation()),
        hard_expires_at_unix_seconds: Some(worker.expires_at().min(policy.expires_at_unix_seconds)),
        model_key_fingerprint: axiom_pool_protocol::sha256(
            hex::decode(&worker.identity().encryption_key).unwrap_or_default(),
        ),
        tls_spki_fingerprint: None,
        checks: [
            ("cpu_tee", "AMD SEV-SNP"),
            ("nvidia_gpu", "NVIDIA confidential GPU"),
            ("worker_binding", "Measured workload and worker keys"),
            ("model_binding", "Verified model inventory"),
            ("tls_binding", "CPU-bound TLS certificate"),
        ]
        .into_iter()
        .map(|(id, label)| EvidenceCheck {
            id: id.into(),
            label: label.into(),
            status: "verified".into(),
            passed: true,
        })
        .collect(),
        provider_claims: [
            ("worker_id", worker.identity().worker_id.clone()),
            ("worker_boot_id", worker.identity().boot_id.clone()),
            (
                "model_manifest_sha256",
                worker.identity().model_manifest.clone(),
            ),
            (
                "deployment_digest",
                worker.identity().deployment_digest.clone(),
            ),
        ]
        .into_iter()
        .map(|(name, value)| EvidenceClaim {
            name: name.into(),
            value,
        })
        .collect(),
        workload_manifest: document.map(|v| v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> SecureClientConfig {
        let mut config = SecureClientConfig::new("https://127.0.0.1:9443").unwrap();
        config.direct = Some(
            serde_json::from_value(serde_json::json!({
                "origin":"https://worker.example:8443", "connect_ip":null,
                "model_manifest":"aa".repeat(32), "issuer_key_file":"/private/operator.pem",
                "verifier":{"binary":"/installed/verifier", "binary_sha256":"bb".repeat(32),
                    "publisher_key":"cc".repeat(32), "minimum_policy_sequence":10}
            }))
            .unwrap(),
        );
        config
    }

    #[test]
    fn installed_direct_contract_rejects_catalog_substitution() {
        let config = installed();
        let provider = provider(&config).unwrap().unwrap();
        let model = models(&config).remove(0);
        assert!(provider.supports(&model).is_ok());
        assert!(model.supports_streaming && !model.supports_tools && !model.supports_images);
        for altered in [
            ModelInfo {
                supports_images: true,
                ..model.clone()
            },
            ModelInfo {
                provider_base_url: "https://attacker.example".into(),
                ..model.clone()
            },
            ModelInfo {
                upstream_model: "dd".repeat(32),
                ..model.clone()
            },
            ModelInfo {
                attestation_protocol: "chutes-gpu-v1".into(),
                ..model.clone()
            },
            ModelInfo {
                e2ee_encryption_version: 2,
                ..model.clone()
            },
            ModelInfo {
                max_output_tokens: 8192,
                ..model.clone()
            },
        ] {
            assert!(provider.supports(&altered).is_err());
        }
    }

    #[test]
    fn vision_requires_local_installation_and_never_advertises_tools_or_files() {
        let mut config = installed();
        assert!(!models(&config)[0].supports_images);
        config.direct.as_mut().unwrap().vision = true;
        let model = models(&config).remove(0);
        assert!(model.supports_images);
        assert!(!model.supports_tools && !model.supports_parallel_tools);
        assert!(model.file_mime_types.is_empty());
        assert!(provider(&config).unwrap().unwrap().supports(&model).is_ok());
    }

    #[tokio::test]
    async fn relaxed_policy_is_rejected_before_any_worker_contact() {
        let config = installed();
        let provider = provider(&config).unwrap().unwrap();
        let model = models(&config).remove(0);
        for policy in [
            TrustPolicy {
                require_nvidia_verified: false,
                ..TrustPolicy::default()
            },
            TrustPolicy {
                require_model_binding: false,
                ..TrustPolicy::default()
            },
            TrustPolicy {
                require_live_tls_binding: false,
                ..TrustPolicy::default()
            },
            TrustPolicy {
                expires_at_unix_seconds: 1,
                ..TrustPolicy::default()
            },
        ] {
            let result = provider
                .establish(&model, &policy, CancellationToken::new())
                .await;
            assert!(result.is_err());
            assert!(
                result
                    .err()
                    .unwrap()
                    .safe_detail()
                    .contains("strict direct attestation policy")
            );
        }
    }
}

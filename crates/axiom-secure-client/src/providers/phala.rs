//! Phala ACI v2 provider adapter.
//!
//! The cryptographic and attestation implementation is maintained in the
//! in-tree `axiom-phala-core` fork. This adapter retains `AxiomCLI`'s bounded
//! relay transport. Decrypted text streams provisionally; successful completion
//! remains gated on the provider receipt and the exact complete transcript.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axiom_inference::{
    AssistantTurn, ChatRole, FinishReason, InferenceRequest, InferenceResponse, ModelInfo,
    ProviderEvent, ResponseFormat, ToolChoice, Usage,
};
use axiom_phala_core::{
    CoreError,
    provider::{
        AttestationRequest, Attestor as _, CipherSession, ProviderCipher as _, ProviderWireMessage,
        ProviderWireRequest, VerifiedModel,
    },
    providers::phala::{
        PHALA_ACI_V2_ENCRYPTION_VERSION, PHALA_ACI_V2_PROTOCOL, PHALA_ATTESTATION_PROTOCOL,
        PHALA_PROVIDER_ID, PhalaAttestor, PhalaCipher,
    },
    relay::{ProviderAttestationEvidence, RelayApi, RelayCompletion as CoreCompletion},
};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecurityEvidence, SecurityState, TrustPolicy,
    lease::{AttestationBinding, VerifiedMaterialCache, key_fingerprint},
    provider::{SecureProvider, VerifiedSession, sealed},
    relay::{
        client::RelayClient,
        dto::{
            CiphertextHex, EncryptedMessage, ProviderKeyLease, RelayChatRequest, RelayCompletion,
            RelayUsage,
        },
        sse::{RelayEvent, SseDecoder},
    },
};

const INFERENCE_ENCRYPTION: &str = "provider_e2ee_v2";

pub(crate) struct PhalaProvider {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    cache: VerifiedMaterialCache<PhalaVerifiedMaterial>,
}

impl PhalaProvider {
    pub(crate) fn new(config: SecureClientConfig, credential: Arc<ApiCredential>) -> Self {
        Self {
            config,
            credential,
            cache: VerifiedMaterialCache::default(),
        }
    }
}

#[derive(Clone)]
struct PhalaVerifiedMaterial {
    verified: VerifiedModel,
    evidence: SecurityEvidence,
    binding: AttestationBinding,
}

impl sealed::Provider for PhalaProvider {}

#[async_trait]
impl SecureProvider for PhalaProvider {
    fn id(&self) -> &'static str {
        PHALA_PROVIDER_ID
    }

    fn supports(&self, model: &ModelInfo) -> Result<()> {
        if model.provider_id != PHALA_PROVIDER_ID
            || model.e2ee_protocol != PHALA_ACI_V2_PROTOCOL
            || model.e2ee_encryption_version != u16::from(PHALA_ACI_V2_ENCRYPTION_VERSION)
            || model.attestation_protocol != PHALA_ATTESTATION_PROTOCOL
        {
            return Err(SecureClientError::capability(
                "model does not match the compiled Phala ACI v2 contract",
            ));
        }
        if model.upstream_model.trim().is_empty() || model.provider_base_url.trim().is_empty() {
            return Err(SecureClientError::capability(
                "Phala model omits required routing metadata",
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
        e2ee_protocol == PHALA_ACI_V2_PROTOCOL
            && e2ee_encryption_version == u16::from(PHALA_ACI_V2_ENCRYPTION_VERSION)
            && attestation_protocol == PHALA_ATTESTATION_PROTOCOL
    }

    fn populate_capabilities(&self, model: &mut ModelInfo) {
        // ACI v2 has field-specific AAD for chat text and reasoning. Tool
        // fields stay disabled until the upstream protocol covers every tool
        // definition, call, argument, and result field.
        model.supports_tools = false;
        model.supports_parallel_tools = false;
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
        let relay = AttestationRelay {
            config: self.config.clone(),
            credential: Arc::clone(&self.credential),
            e2ee_protocol: model.e2ee_protocol.clone(),
            e2ee_encryption_version: model.e2ee_encryption_version,
            attestation_protocol: model.attestation_protocol.clone(),
        };
        let request = AttestationRequest {
            model_id: &model.id,
            expected_model: &model.upstream_model,
            relay: &relay,
            nvidia_anchor_sha256: &policy.nvidia_anchor_sha256,
        };
        let verified = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            result = PhalaAttestor.verify_model(request) => result
                .map_err(|error| map_attestation_error(&error))?,
        };
        let cipher = PhalaCipher
            .new_session(&verified)
            .map_err(|error| map_attestation_error(&error))?;
        if !cipher.is_ready() {
            return Err(SecureClientError::attestation(
                "verified Phala session is not ready",
            ));
        }
        let locally_verified_keyset = verified
            .provider_state
            .as_ref()
            .and_then(|state| state.get("keyset_digest"))
            .and_then(serde_json::Value::as_str);
        let observed_key_fingerprint = key_fingerprint(&verified.model_public_key_hex)?;
        let wire: ProviderKeyLease = RelayClient::new(&self.config, &self.credential)
            .provider_e2ee_key(
                model,
                Some(&observed_key_fingerprint),
                locally_verified_keyset,
                &cancellation,
            )
            .await?;
        let binding = AttestationBinding::verify(wire, model, &verified.model_public_key_hex)?;
        if binding.keyset_digest.as_deref() != locally_verified_keyset {
            return Err(SecureClientError::attestation(
                "backend Phala keyset lease does not match local evidence",
            ));
        }
        let mut evidence = evidence_for(model, &verified, policy);
        evidence.attestation_generation = Some(binding.generation);
        evidence.hard_expires_at_unix_seconds = Some(binding.hard_expires_at_unix_seconds);
        let provider_expiry = verified
            .expires_at_unix
            .unwrap_or(binding.hard_expires_at_unix_seconds)
            .min(binding.hard_expires_at_unix_seconds)
            .min(policy.expires_at_unix_seconds);
        let material = PhalaVerifiedMaterial {
            verified,
            evidence,
            binding,
        };
        self.cache
            .insert(
                model.clone(),
                policy.clone(),
                material.clone(),
                provider_expiry,
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
    material: PhalaVerifiedMaterial,
    config: &SecureClientConfig,
    credential: &Arc<ApiCredential>,
    model: &ModelInfo,
) -> Box<dyn VerifiedSession> {
    Box::new(PhalaVerifiedSession {
        model: model.clone(),
        config: config.clone(),
        credential: Arc::clone(credential),
        verified: material.verified,
        evidence: material.evidence,
        binding: material.binding,
    })
}

struct AttestationRelay {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    e2ee_protocol: String,
    e2ee_encryption_version: u16,
    attestation_protocol: String,
}

#[async_trait]
impl RelayApi for AttestationRelay {
    async fn provider_attestation_report(
        &self,
        model_id: &str,
        nonce: &str,
    ) -> axiom_phala_core::Result<ProviderAttestationEvidence> {
        RelayClient::new(&self.config, &self.credential)
            .provider_attestation_report(
                model_id,
                nonce,
                &self.e2ee_protocol,
                self.e2ee_encryption_version,
                &self.attestation_protocol,
                &CancellationToken::new(),
            )
            .await
            .map_err(|error| map_transport_to_core(&error))
    }
}

struct PhalaVerifiedSession {
    model: ModelInfo,
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    verified: VerifiedModel,
    evidence: SecurityEvidence,
    binding: AttestationBinding,
}

impl sealed::Session for PhalaVerifiedSession {}

#[async_trait]
impl VerifiedSession for PhalaVerifiedSession {
    fn evidence(&self) -> &SecurityEvidence {
        &self.evidence
    }

    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        let (mut cipher, relay_request) = self.build_request(&request, false)?;
        let completion = RelayClient::new(&self.config, &self.credential)
            .complete(&relay_request, &cancellation)
            .await?;
        decrypt_completion(&self.model, cipher.as_mut(), completion)
    }

    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        let (mut cipher, relay_request) = self.build_request(&request, true)?;
        let response = RelayClient::new(&self.config, &self.credential)
            .stream(&relay_request, &cancellation)
            .await?;
        stream_verified(
            response,
            cipher.as_mut(),
            &events,
            &cancellation,
            self.config.limits.relay_sse_event_bytes,
            self.config.limits.relay_stream_bytes,
        )
        .await
    }
}

impl PhalaVerifiedSession {
    fn build_request(
        &self,
        request: &InferenceRequest,
        stream: bool,
    ) -> Result<(Box<dyn CipherSession>, RelayChatRequest)> {
        request
            .validate()
            .map_err(|_| invalid_request("inference request validation failed"))?;
        let response_format = public_response_format(&request.response_format)?;
        if request.model != self.model.id {
            return Err(invalid_request(
                "request model does not match verified session",
            ));
        }
        if request.thinking_mode != axiom_inference::ThinkingMode::ProviderDefault {
            return Err(invalid_request(
                "explicit thinking mode is unsupported by this Phala model",
            ));
        }
        let reasoning_effort = provider_reasoning_effort(&self.model, request.reasoning_effort)?;
        if matches!(
            request.tool_choice,
            ToolChoice::Required | ToolChoice::Named { .. }
        ) {
            return Err(invalid_request(
                "Phala ACI v2 does not support tool choices",
            ));
        }
        if request.parallel_tool_calls.is_some() {
            return Err(invalid_request(
                "Phala ACI v2 does not support parallel tools",
            ));
        }
        let max_tokens = request
            .max_output_tokens
            .unwrap_or(self.model.max_output_tokens);
        if max_tokens == 0 || max_tokens > self.model.max_output_tokens {
            return Err(invalid_request("output limit exceeds the model capability"));
        }

        let mut cipher = PhalaCipher
            .new_session(&self.verified)
            .map_err(|error| map_provider_error(&error))?;
        if !cipher.is_ready() {
            return Err(SecureClientError::session(
                "Phala cipher session is not ready",
            ));
        }

        let mut encrypted_messages = Vec::with_capacity(request.messages.len());
        let mut provider_wire_messages = Vec::with_capacity(request.messages.len());
        for (index, message) in request.messages.iter().enumerate() {
            let assistant_null_content = message.role == ChatRole::Assistant
                && message.content.is_empty()
                && message.refusal.is_none()
                && message.tool_calls.is_empty();
            let plaintext_content = if assistant_null_content {
                None
            } else {
                Some(message.content.clone())
            };
            let encrypted_content = plaintext_content
                .as_deref()
                .map(|content| {
                    cipher
                        .encrypt_field(content.as_bytes(), &format!("messages.{index}.content"))
                        .map_err(|error| map_provider_error(&error))
                        .and_then(CiphertextHex::new)
                })
                .transpose()?;
            let has_extended_fields = message.reasoning_content.is_some()
                || message.name.is_some()
                || message.refusal.is_some()
                || !message.tool_calls.is_empty()
                || message.tool_call_id.is_some()
                || message.role == ChatRole::Tool;
            provider_wire_messages.push(ProviderWireMessage {
                role: role_name(&message.role).to_owned(),
                content: encrypted_content
                    .as_ref()
                    .map(|ciphertext| ciphertext.as_str().to_owned()),
                assistant_null_content,
                has_extended_fields,
            });
            encrypted_messages.push(EncryptedMessage {
                role: role_name(&message.role),
                encrypted_content,
                encrypted_reasoning_content: None,
                encrypted_name: None,
                encrypted_refusal: None,
                encrypted_tool_calls: None,
                tool_call_id: None,
            });
        }

        let sampling = sampling_value(request);
        let provider_wire_request = ProviderWireRequest {
            model: self.model.upstream_model.clone(),
            messages: provider_wire_messages,
            max_tokens,
            stream,
            sampling: sampling.clone(),
            response_format: response_format.clone(),
            reasoning_effort: reasoning_effort.clone(),
            // Tool definitions are intentionally omitted for Phala. Auto/none
            // turns remain ordinary chat; required/named choices were rejected.
            has_tools: false,
        };
        let provider_e2ee_context = Some(
            cipher
                .prepare_request_context(&provider_wire_request)
                .map_err(|error| map_provider_error(&error))?,
        );
        let client_public_key_hex = cipher.client_public_key_hex().to_owned();
        if client_public_key_hex.is_empty() {
            return Err(SecureClientError::session(
                "Phala client key is unavailable",
            ));
        }

        let relay_request = RelayChatRequest {
            client_request_id: request.request_id.clone(),
            thinking_mode: None,
            provider_id: self.model.provider_id.clone(),
            inference_encryption: INFERENCE_ENCRYPTION,
            model_id: self.model.id.clone(),
            encryption_version: self.model.e2ee_encryption_version,
            e2ee_protocol: self.model.e2ee_protocol.clone(),
            client_public_key_hex,
            model_public_key_hex: self.verified.model_public_key_hex.clone(),
            encrypted_messages,
            // Phala ACI v2 currently encrypts message content only. The backend rejects the
            // all-fields flag until tools and every extended field have a defined AAD contract.
            encrypt_all_fields: false,
            encrypted_tools: None,
            encrypted_tool_choice: None,
            parallel_tool_calls: None,
            stream,
            max_tokens: Some(max_tokens),
            sampling,
            response_format,
            reasoning_effort,
            provider_e2ee_context,
            attestation_generation: self.binding.generation,
            verified_key_fingerprint: self.binding.model_key_fingerprint.clone(),
            verified_keyset_digest: self.binding.keyset_digest.clone(),
        };
        Ok((cipher, relay_request))
    }
}

fn decrypt_completion(
    _model: &ModelInfo,
    cipher: &mut dyn CipherSession,
    completion: RelayCompletion,
) -> Result<InferenceResponse> {
    if !completion.encrypted_tool_calls.is_empty() {
        return Err(invalid_response("Phala returned unsupported tool calls"));
    }
    let core = CoreCompletion {
        proof: completion
            .proof
            .ok_or_else(|| invalid_response("Phala completion omitted its proof"))?,
    };
    let decrypted = cipher
        .decrypt_verified_completion(&core)
        .map_err(|error| map_provider_error(&error))?;
    let finish_reason = parse_finish_reason(&completion.finish_reason)?;
    if matches!(finish_reason, FinishReason::ToolCalls) {
        return Err(invalid_response(
            "Phala returned an unsupported tool finish reason",
        ));
    }
    let text = decrypted.content.unwrap_or_default();
    if text.is_empty() && decrypted.reasoning_content.is_none() && decrypted.refusal.is_none() {
        return Err(invalid_response("Phala completion has no usable output"));
    }
    Ok(InferenceResponse {
        assistant: AssistantTurn {
            reasoning: None,
            text,
            tool_calls: Vec::new(),
        },
        reasoning: decrypted.reasoning_content,
        refusal: decrypted.refusal,
        usage: normalize_usage(&completion.usage)?,
        finish_reason: Some(finish_reason),
    })
}

async fn stream_verified(
    response: reqwest::Response,
    cipher: &mut dyn CipherSession,
    sink: &mpsc::Sender<ProviderEvent>,
    cancellation: &CancellationToken,
    event_limit: usize,
    stream_limit: usize,
) -> Result<InferenceResponse> {
    use futures_util::StreamExt as _;

    let mut bytes = response.bytes_stream();
    let mut decoder = SseDecoder::new(event_limit, stream_limit);
    let mut run_id = None;
    let mut next_sequence = None;
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage = None;
    let mut finish_reason = None;
    let mut proof = None;
    let mut message_completed = false;
    let mut run_completed = false;

    let mut deadline = crate::relay::deadline::StreamDeadline::new();
    loop {
        let chunk = tokio::select! {
            () = tokio::time::sleep_until(deadline.at()) => return Err(deadline.error()),
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            chunk = bytes.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| {
            SecureClientError::new(
                axiom_inference::ProviderFailureKind::Transient,
                "relay stream failed",
            )
        })?;
        for event in decoder.feed(&chunk)? {
            deadline.observe(&event);
            if run_completed {
                return Err(invalid_response(
                    "relay emitted data after terminal completion",
                ));
            }
            match event {
                RelayEvent::Finalizing(payload) => {
                    require_run(run_id.as_deref(), &payload.run_id)?;
                }
                RelayEvent::Accounting(usage) => {
                    require_run(run_id.as_deref(), &usage.request_id)?;
                    if !usage.validate_counters() || usage.response_verified {
                        return Err(invalid_response("invalid operational accounting"));
                    }
                    send(
                        sink,
                        ProviderEvent::Accounting(Box::new(usage)),
                        cancellation,
                    )
                    .await?;
                }
                RelayEvent::RunCreated(payload) => {
                    if run_id.is_some() || payload.inference_encryption != INFERENCE_ENCRYPTION {
                        return Err(invalid_response("relay run creation contract is invalid"));
                    }
                    validate_run_id(&payload.run_id)?;
                    run_id = Some(payload.run_id);
                }
                RelayEvent::Delta(payload) => {
                    require_run(run_id.as_deref(), &payload.run_id)?;
                    if message_completed {
                        return Err(invalid_response("relay emitted a delta after completion"));
                    }
                    if let Some(expected) = next_sequence
                        && payload.sequence != expected
                    {
                        return Err(invalid_response("relay event sequence is invalid"));
                    }
                    next_sequence = Some(
                        payload
                            .sequence
                            .checked_add(1)
                            .ok_or_else(|| invalid_response("relay event sequence overflowed"))?,
                    );
                    if payload.encrypted_refusal_delta.is_some()
                        || !payload.encrypted_tool_calls.is_empty()
                    {
                        return Err(invalid_response(
                            "Phala returned unsupported encrypted fields",
                        ));
                    }
                    if let Some(value) = payload.encrypted_delta {
                        let value = cipher
                            .decrypt_stream_field(
                                value.as_str(),
                                payload.response_id.as_deref(),
                                payload.content_field.as_deref(),
                            )
                            .map_err(|error| map_provider_error(&error))?;
                        let value = String::from_utf8(value)
                            .map_err(|_| invalid_response("Phala plaintext is not UTF-8"))?;
                        append_bounded(&mut text, &value)?;
                        send(sink, ProviderEvent::TextDelta(value), cancellation).await?;
                    }
                    if let Some(value) = payload.encrypted_reasoning_delta {
                        let value = cipher
                            .decrypt_stream_field(
                                value.as_str(),
                                payload.response_id.as_deref(),
                                payload.reasoning_field.as_deref(),
                            )
                            .map_err(|error| map_provider_error(&error))?;
                        let value = String::from_utf8(value)
                            .map_err(|_| invalid_response("Phala reasoning is not UTF-8"))?;
                        append_bounded(&mut reasoning, &value)?;
                        send(sink, ProviderEvent::ReasoningDelta(value), cancellation).await?;
                    }
                }
                RelayEvent::MessageCompleted(payload) => {
                    require_run(run_id.as_deref(), &payload.run_id)?;
                    if message_completed {
                        return Err(invalid_response("relay completion is duplicated"));
                    }
                    usage = Some(normalize_usage(&payload.usage)?);
                    finish_reason = Some(parse_finish_reason(&payload.finish_reason)?);
                    proof = payload.proof;
                    message_completed = true;
                }
                RelayEvent::RunCompleted(payload) => {
                    require_run(run_id.as_deref(), &payload.run_id)?;
                    if !message_completed {
                        return Err(invalid_response("relay run completed before its message"));
                    }
                    run_completed = true;
                }
                RelayEvent::Cancelled(payload) => {
                    require_run(run_id.as_deref(), &payload.run_id)?;
                    if payload.reason.as_deref() == Some("insufficient_credit") {
                        return Err(SecureClientError::new(
                            axiom_inference::ProviderFailureKind::InsufficientCredit,
                            "Axiom credit is exhausted",
                        ));
                    }
                    return Err(SecureClientError::cancelled());
                }
                RelayEvent::Failed(payload) => {
                    require_run(run_id.as_deref(), &payload.run_id)?;
                    return Err(crate::relay::client::stream_failure(
                        payload.code.as_deref(),
                    ));
                }
            }
        }
    }
    decoder.finish()?;
    if !message_completed || !run_completed {
        return Err(invalid_response(
            "relay stream ended without terminal completion",
        ));
    }
    let proof = proof
        .as_ref()
        .ok_or_else(|| invalid_response("Phala stream omitted its final proof"))?;
    cipher
        .verify_stream_completion(proof)
        .map_err(|error| map_provider_error(&error))?;

    // Previewed output is not successful/verified until the receipt binds the
    // exact stream and exact pre-verified worker, and transport reaches EOF.
    send(
        sink,
        ProviderEvent::Status {
            connected: true,
            detail: "Phala ACI receipt-verified inference connected".to_owned(),
        },
        cancellation,
    )
    .await?;
    let usage = usage.unwrap_or_default();
    send(
        sink,
        ProviderEvent::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        },
        cancellation,
    )
    .await?;
    let finish_reason =
        finish_reason.ok_or_else(|| invalid_response("relay stream has no finish reason"))?;
    if matches!(finish_reason, FinishReason::ToolCalls) {
        return Err(invalid_response(
            "Phala returned an unsupported tool finish reason",
        ));
    }
    send(
        sink,
        ProviderEvent::Finished(finish_reason.clone()),
        cancellation,
    )
    .await?;
    Ok(InferenceResponse {
        assistant: AssistantTurn {
            reasoning: None,
            text,
            tool_calls: Vec::new(),
        },
        reasoning: (!reasoning.is_empty()).then_some(reasoning),
        refusal: None,
        usage,
        finish_reason: Some(finish_reason),
    })
}

fn evidence_for(
    model: &ModelInfo,
    verified: &VerifiedModel,
    policy: &TrustPolicy,
) -> SecurityEvidence {
    let provider_state = verified
        .provider_state
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let keyset_digest = provider_state
        .and_then(|state| state.get("keyset_digest"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let worker_count = provider_state
        .and_then(|state| state.get("worker_sessions"))
        .and_then(serde_json::Value::as_object)
        .map_or(0, serde_json::Map::len);
    let workload = phala_workload_identity(verified);
    let mut claims = vec![
        EvidenceClaim {
            name: "workload_keyset_digest".into(),
            value: keyset_digest.to_owned(),
        },
        EvidenceClaim {
            name: "preverified_worker_sessions".into(),
            value: worker_count.to_string(),
        },
    ];
    if let Some(source) = verified
        .checks
        .iter()
        .find(|check| check.id == "gateway_source")
    {
        claims.push(EvidenceClaim {
            name: "gateway_source_commit".into(),
            value: source.status.clone(),
        });
    }
    if let Some(repository) = &workload.repository {
        claims.push(EvidenceClaim {
            name: "gateway_source_repository".into(),
            value: repository.clone(),
        });
    }
    if let Some(commit) = &workload.commit {
        claims.push(EvidenceClaim {
            name: "gateway_source_commit_full".into(),
            value: commit.clone(),
        });
    }
    SecurityEvidence {
        state: SecurityState::Verified,
        provider_id: PHALA_PROVIDER_ID.into(),
        model_id: model.id.clone(),
        attestation_protocol: PHALA_ATTESTATION_PROTOCOL.into(),
        e2ee_protocol: PHALA_ACI_V2_PROTOCOL.into(),
        e2ee_encryption_version: u16::from(PHALA_ACI_V2_ENCRYPTION_VERSION),
        trust_policy_version: policy.version.clone(),
        verified_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        attestation_generation: None,
        hard_expires_at_unix_seconds: verified.expires_at_unix,
        model_key_fingerprint: hex::decode(&verified.model_public_key_hex)
            .map(|key| hex::encode(Sha256::digest(key)))
            .unwrap_or_default(),
        tls_spki_fingerprint: verified.tls_fingerprint.clone(),
        checks: verified
            .checks
            .iter()
            .map(|check| EvidenceCheck {
                id: check.id.clone(),
                label: check.label.clone(),
                status: check.status.clone(),
                passed: check.ok,
            })
            .collect(),
        provider_claims: claims,
        workload_manifest: None,
        // Source is quote-bound provenance, not an Axiom code-approval claim.
        workload_identity_allowlist_configured: false,
        approved_workload_identity: false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PhalaWorkloadIdentity {
    repository: Option<String>,
    commit: Option<String>,
}

fn phala_workload_identity(verified: &VerifiedModel) -> PhalaWorkloadIdentity {
    let state = verified
        .provider_state
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let repository = state
        .and_then(|value| value.get("source_repository"))
        .and_then(serde_json::Value::as_str)
        .map(canonical_repository);
    let commit = state
        .and_then(|value| value.get("source_commit"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_ascii_lowercase);
    PhalaWorkloadIdentity { repository, commit }
}

fn canonical_repository(repository: &str) -> String {
    repository
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase()
}

fn sampling_value(request: &InferenceRequest) -> Option<serde_json::Value> {
    let mut values = serde_json::Map::new();
    if let Some(value) = request.sampling.temperature {
        values.insert("temperature".into(), serde_json::json!(value));
    }
    if let Some(value) = request.sampling.top_p {
        values.insert("top_p".into(), serde_json::json!(value));
    }
    (!values.is_empty()).then_some(serde_json::Value::Object(values))
}

fn public_response_format(format: &ResponseFormat) -> Result<Option<serde_json::Value>> {
    match format {
        ResponseFormat::Text => Ok(None),
        ResponseFormat::JsonObject => Ok(Some(serde_json::json!({"type": "json_object"}))),
        ResponseFormat::JsonSchema { .. } => Err(invalid_request(
            "JSON Schema response format is unavailable until its metadata is provider-E2EE",
        )),
    }
}

fn provider_reasoning_effort(
    model: &ModelInfo,
    requested: axiom_inference::ReasoningEffort,
) -> Result<Option<String>> {
    if requested == axiom_inference::ReasoningEffort::Minimal
        || (!model.supported_reasoning_efforts.is_empty()
            && !model.supported_reasoning_efforts.contains(&requested))
    {
        return Err(invalid_request(
            "reasoning effort is not supported by the model",
        ));
    }
    Ok((!model.supported_reasoning_efforts.is_empty()).then(|| requested.as_str().to_owned()))
}

fn role_name(role: &ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::Developer => "developer",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    }
}

fn parse_finish_reason(value: &str) -> Result<FinishReason> {
    match value {
        "stop" => Ok(FinishReason::Stop),
        "length" => Ok(FinishReason::Length),
        "tool_calls" => Ok(FinishReason::ToolCalls),
        "content_filter" => Ok(FinishReason::ContentFilter),
        _ => Err(invalid_response("relay finish reason is invalid")),
    }
}

fn normalize_usage(usage: &RelayUsage) -> Result<Usage> {
    let input_tokens = usage.prompt_tokens.unwrap_or(0);
    let output_tokens = usage.completion_tokens.unwrap_or(0);
    let total_tokens = usage
        .total_tokens
        .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
    if total_tokens < input_tokens || total_tokens < output_tokens {
        return Err(invalid_response("relay token usage is inconsistent"));
    }
    Ok(Usage {
        input_tokens,
        output_tokens,
        total_tokens,
    })
}

fn append_bounded(output: &mut String, value: &str) -> Result<()> {
    if output.len().saturating_add(value.len()) > axiom_inference::MAX_MESSAGE_TEXT_BYTES {
        return Err(invalid_response(
            "decrypted Phala output exceeds the configured limit",
        ));
    }
    output.push_str(value);
    Ok(())
}

fn validate_run_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.bytes().any(|byte| byte.is_ascii_control()) {
        Err(invalid_response("relay run ID is invalid"))
    } else {
        Ok(())
    }
}

fn require_run(expected: Option<&str>, actual: &str) -> Result<()> {
    if expected == Some(actual) {
        Ok(())
    } else {
        Err(invalid_response("relay event run ID is inconsistent"))
    }
}

async fn send(
    sink: &mpsc::Sender<ProviderEvent>,
    event: ProviderEvent,
    cancellation: &CancellationToken,
) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(SecureClientError::cancelled());
    }
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(SecureClientError::cancelled()),
        result = sink.send(event) => result.map_err(|_| SecureClientError::cancelled()),
    }
}

fn invalid_request(detail: &'static str) -> SecureClientError {
    SecureClientError::new(axiom_inference::ProviderFailureKind::InvalidRequest, detail)
}

fn invalid_response(detail: &'static str) -> SecureClientError {
    SecureClientError::new(
        axiom_inference::ProviderFailureKind::InvalidResponse,
        detail,
    )
}

fn map_transport_to_core(error: &SecureClientError) -> CoreError {
    CoreError::Relay(format!("{:?}: {}", error.kind(), error.safe_detail()))
}

fn map_attestation_error(error: &CoreError) -> SecureClientError {
    tracing::warn!(reason = %error, "Phala attestation was rejected locally");
    let kind = match error {
        CoreError::Relay(_) => axiom_inference::ProviderFailureKind::AttestationUnavailable,
        _ => axiom_inference::ProviderFailureKind::AttestationRejected,
    };
    SecureClientError::new(kind, "Phala attestation verification failed")
}

fn map_provider_error(error: &CoreError) -> SecureClientError {
    let kind = match error {
        CoreError::Relay(_) => axiom_inference::ProviderFailureKind::Transient,
        CoreError::Attestation(_) => axiom_inference::ProviderFailureKind::AttestationRejected,
        CoreError::Provider(_) => axiom_inference::ProviderFailureKind::InvalidResponse,
    };
    SecureClientError::new(kind, "Phala ACI v2 verification failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_contract_is_exact_and_tools_stay_disabled() {
        let provider = PhalaProvider::new(
            SecureClientConfig::new("https://api.axiom.stream").unwrap(),
            Arc::new(ApiCredential::new("test")),
        );
        let mut model = ModelInfo {
            id: "phala-glm-5-2".into(),
            provider_id: PHALA_PROVIDER_ID.into(),
            upstream_model: "z-ai/glm-5.2".into(),
            provider_base_url: "https://inference.phala.com/v1".into(),
            e2ee_protocol: PHALA_ACI_V2_PROTOCOL.into(),
            e2ee_encryption_version: u16::from(PHALA_ACI_V2_ENCRYPTION_VERSION),
            attestation_protocol: PHALA_ATTESTATION_PROTOCOL.into(),
            ..ModelInfo::default()
        };
        provider.supports(&model).unwrap();
        provider.populate_capabilities(&mut model);
        assert!(model.supports_streaming);
        assert!(!model.supports_tools);
        model.attestation_protocol = "other".into();
        assert!(provider.supports(&model).is_err());
    }

    #[test]
    fn provider_reasoning_is_omitted_or_rejected_from_exact_model_capabilities() {
        let mut model = ModelInfo::default();
        assert_eq!(
            provider_reasoning_effort(&model, axiom_inference::ReasoningEffort::ExtraHigh).unwrap(),
            None
        );

        model.supported_reasoning_efforts = vec![axiom_inference::ReasoningEffort::Medium];
        assert_eq!(
            provider_reasoning_effort(&model, axiom_inference::ReasoningEffort::Medium).unwrap(),
            Some("medium".into())
        );
        assert!(
            provider_reasoning_effort(&model, axiom_inference::ReasoningEffort::ExtraHigh).is_err()
        );
    }

    #[test]
    fn json_schema_metadata_is_rejected_before_phala_provider_construction() {
        let sentinel = "plaintext-schema-instruction-must-not-cross";
        let format = ResponseFormat::JsonSchema {
            name: sentinel.into(),
            schema: serde_json::json!({"description": sentinel}),
            strict: true,
        };
        let error = public_response_format(&format).expect_err("schema metadata must fail closed");
        assert_eq!(
            error.kind(),
            axiom_inference::ProviderFailureKind::InvalidRequest
        );
        assert!(!error.safe_detail().contains(sentinel));
        assert_eq!(
            public_response_format(&ResponseFormat::JsonObject).unwrap(),
            Some(serde_json::json!({"type": "json_object"}))
        );
    }

    #[test]
    fn phala_usage_and_bounds_fail_closed() {
        assert!(
            normalize_usage(&RelayUsage {
                prompt_tokens: Some(8),
                completion_tokens: Some(5),
                total_tokens: Some(4),
                cached_prompt_tokens: None,
            })
            .is_err()
        );
        let mut text = String::new();
        assert!(
            append_bounded(
                &mut text,
                &"x".repeat(axiom_inference::MAX_MESSAGE_TEXT_BYTES + 1)
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_provisional_send_never_publishes_more_text() {
        let (sink, mut output) = mpsc::channel(1);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(
            send(
                &sink,
                ProviderEvent::TextDelta("never publish".into()),
                &cancellation,
            )
            .await
            .is_err()
        );
        assert!(output.try_recv().is_err());
    }

    // Transport regression fixture using the real ACI cipher and request AAD.
    // Its synthetic attestation state is confined to this unit test; it never
    // establishes a live provider session or bypasses a production verifier.
    fn stream_cipher_fixture() -> (Box<dyn CipherSession>, serde_json::Value) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let worker_id = "aa".repeat(32);
        let verified = VerifiedModel {
            model_public_key_hex: "34".repeat(32),
            tls_fingerprint: None,
            checks: Vec::new(),
            expires_at_unix: Some(now + 300),
            provider_state: Some(serde_json::json!({
                "profile": "dstack-aci-draft-d567f6",
                "model": "org/model",
                "workload_id": null,
                "keyset_digest": format!("sha256:{}", "12".repeat(32)),
                "source_repository": "https://github.com/Dstack-TEE/private-ai-gateway",
                "source_commit": "ab".repeat(20),
                "gateway_public_key_hex": "34".repeat(32),
                "receipt_public_keys": {},
                "worker_sessions": {
                    (worker_id.clone()): {
                        "session_id": worker_id,
                        "established_at": now - 1,
                        "expires_at": now + 300,
                        "record": {}
                    }
                }
            })),
        };
        let mut cipher = PhalaCipher.new_session(&verified).unwrap();
        let encrypted = cipher
            .encrypt_field(b"test prompt", "messages.0.content")
            .unwrap();
        let context = cipher
            .prepare_request_context(&ProviderWireRequest {
                model: "org/model".into(),
                messages: vec![ProviderWireMessage {
                    role: "user".into(),
                    content: Some(encrypted),
                    assistant_null_content: false,
                    has_extended_fields: false,
                }],
                max_tokens: 32,
                stream: true,
                sampling: None,
                response_format: None,
                reasoning_effort: None,
                has_tools: false,
            })
            .unwrap();
        (cipher, context)
    }

    fn encrypted_stream_fixture(
        cipher: &dyn CipherSession,
        context: &serde_json::Value,
        field: &str,
        plaintext: &[u8],
    ) -> String {
        use aes_gcm::{
            Aes256Gcm, KeyInit as _, Nonce,
            aead::{Aead as _, Payload},
        };
        use x25519_dalek::{PublicKey, StaticSecret};

        let recipient = PublicKey::from(
            <[u8; 32]>::try_from(hex::decode(cipher.client_public_key_hex()).unwrap()).unwrap(),
        );
        let ephemeral = StaticSecret::random_from_rng(rand_core::OsRng);
        let public = PublicKey::from(&ephemeral);
        let mut key = [0_u8; 32];
        hkdf::Hkdf::<Sha256>::new(None, ephemeral.diffie_hellman(&recipient).as_bytes())
            .expand(b"aci.e2ee.v2.x25519", &mut key)
            .unwrap();
        let aad = serde_json::json!({
            "purpose": "aci.e2ee.response.v2",
            "algo": "x25519-aes-256-gcm-hkdf-sha256",
            "model": "org/model",
            "id": "chat-1",
            "field": field,
            "nonce": context["nonce"],
            "ts": context["timestamp"],
        });
        let canonical: std::collections::BTreeMap<_, _> = aad.as_object().unwrap().iter().collect();
        let aad = serde_json::to_vec(&canonical).unwrap();
        // Each field uses a fresh ephemeral key, so the fixed test nonce is safe.
        let nonce = [7_u8; 12];
        let encrypted = Aes256Gcm::new_from_slice(&key)
            .unwrap()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .unwrap();
        hex::encode([public.as_bytes().as_slice(), &nonce, &encrypted].concat())
    }

    fn sse_fixture(name: &str, payload: &serde_json::Value) -> String {
        format!("event: {name}\ndata: {payload}\n\n")
    }

    #[tokio::test]
    async fn phala_streams_before_terminal_and_never_succeeds_without_valid_proof() {
        for ending in [
            "eof",
            "missing-proof",
            "invalid-proof",
            "reordered",
            "tampered",
            "cancelled",
        ] {
            let (mut cipher, context) = stream_cipher_fixture();
            let encrypted_content = encrypted_stream_fixture(
                cipher.as_ref(),
                &context,
                "choices.0.delta.content",
                b"partial answer",
            );
            let reasoning = encrypted_stream_fixture(
                cipher.as_ref(),
                &context,
                "choices.0.delta.reasoning_content",
                b"thinking",
            );
            let delta = serde_json::json!({
                "run_id": "run-1", "sequence": 1, "response_id": "chat-1",
                "encrypted_delta": encrypted_content,
                "encrypted_reasoning_delta": reasoning,
                "content_field": "choices.0.delta.content",
                "reasoning_field": "choices.0.delta.reasoning_content"
            });
            let (wire_tx, wire_rx) = mpsc::channel::<String>(8);
            let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(
                wire_rx,
                |mut rx| async move {
                    rx.recv()
                        .await
                        .map(|chunk| (Ok::<_, std::io::Error>(chunk), rx))
                },
            ));
            let response = reqwest::Response::from(::http::Response::new(body));
            let (sink, mut output) = mpsc::channel(8);
            let cancellation = CancellationToken::new();
            let stream_cancellation = cancellation.clone();
            let task = tokio::spawn(async move {
                stream_verified(
                    response,
                    cipher.as_mut(),
                    &sink,
                    &stream_cancellation,
                    8192,
                    65536,
                )
                .await
            });
            wire_tx
                .send(sse_fixture(
                    "run.created",
                    &serde_json::json!({
                        "run_id": "run-1", "inference_encryption": INFERENCE_ENCRYPTION
                    }),
                ))
                .await
                .unwrap();
            wire_tx
                .send(sse_fixture("message.encrypted_delta", &delta))
                .await
                .unwrap();
            assert!(
                matches!(
                    tokio::time::timeout(std::time::Duration::from_secs(2), output.recv()).await.unwrap(),
                    Some(ProviderEvent::TextDelta(text)) if text == "partial answer"
                ),
                "{ending}: text must arrive while the network stream is still open"
            );
            assert!(matches!(
                tokio::time::timeout(std::time::Duration::from_secs(2), output.recv()).await.unwrap(),
                Some(ProviderEvent::ReasoningDelta(text)) if text == "thinking"
            ));
            assert!(!task.is_finished());
            assert!(output.try_recv().is_err());
            match ending {
                "missing-proof" | "invalid-proof" => {
                    let proof = if ending == "invalid-proof" {
                        serde_json::json!({})
                    } else {
                        serde_json::Value::Null
                    };
                    wire_tx.send(sse_fixture("message.encrypted_completed", &serde_json::json!({
                        "run_id": "run-1", "finish_reason": "stop", "usage": {}, "proof": proof
                    }))).await.unwrap();
                    wire_tx
                        .send(sse_fixture(
                            "run.completed",
                            &serde_json::json!({"run_id": "run-1"}),
                        ))
                        .await
                        .unwrap();
                }
                "reordered" | "tampered" => {
                    let mut next = delta;
                    next["sequence"] = serde_json::json!(if ending == "reordered" { 3 } else { 2 });
                    if ending == "tampered" {
                        let mut wire =
                            hex::decode(next["encrypted_delta"].as_str().unwrap()).unwrap();
                        *wire.last_mut().unwrap() ^= 1;
                        next["encrypted_delta"] = serde_json::json!(hex::encode(wire));
                    }
                    wire_tx
                        .send(sse_fixture("message.encrypted_delta", &next))
                        .await
                        .unwrap();
                }
                "cancelled" => cancellation.cancel(),
                _ => {}
            }
            drop(wire_tx);
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(2), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err(),
                "{ending}"
            );
            assert!(
                output.try_recv().is_err(),
                "{ending}: no more text or successful terminal event"
            );
        }
    }

    #[test]
    fn workload_identity_exposes_source_without_release_approval() {
        let verified = VerifiedModel {
            model_public_key_hex: "12".repeat(32),
            tls_fingerprint: None,
            checks: Vec::new(),
            provider_state: Some(serde_json::json!({
                "source_repository": "https://github.com/Dstack-TEE/private-ai-gateway.git/",
                "source_commit": "ABCDabcdABCDabcdABCDabcdABCDabcdABCDabcd"
            })),
            expires_at_unix: None,
        };
        let identity = phala_workload_identity(&verified);
        assert_eq!(
            identity.commit.as_deref(),
            Some("abcdabcdabcdabcdabcdabcdabcdabcdabcdabcd")
        );
        assert_eq!(
            identity.repository.as_deref(),
            Some("https://github.com/dstack-tee/private-ai-gateway")
        );

        let policy = TrustPolicy {
            workload_enforcement: crate::WorkloadEnforcement::Enforce,
            ..TrustPolicy::default()
        };
        let evidence = evidence_for(&ModelInfo::default(), &verified, &policy);
        assert!(!evidence.workload_identity_allowlist_configured);
        assert!(!evidence.approved_workload_identity);
    }
}

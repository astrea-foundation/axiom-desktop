//! Locally attested Tinfoil EHBP sessions. Only ciphertext crosses the relay.
mod response;
#[cfg(test)]
mod tests;

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axiom_inference::{
    InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent, ReasoningEffort, ResponseFormat,
    ThinkingMode, ToolChoice,
};
use futures_util::StreamExt as _;
use rand_core::{OsRng, RngCore as _};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecurityEvidence, SecurityState, TrustPolicy,
    catalog::relay_endpoint,
    http::{bounded_body, pinned_client},
    lease::{VerifiedMaterialCache, key_fingerprint},
    provider::{SecureProvider, VerifiedSession, sealed},
    relay::client::{RelayClient, error_for_status_body},
};

const HOST: &str = "inference.tinfoil.sh";
const REPOSITORY: &str = "tinfoilsh/confidential-model-router";
const PROTOCOL: &str = "tinfoil-ehbp-v1";
const ATTESTATION: &str = "tinfoil-snp-sigstore-v1";
const BASE_URL: &str = "https://inference.tinfoil.sh/v1";
const MODELS: &[&str] = &["gpt-oss-120b", "glm-5-3", "kimi-k3", "llama3-3-70b"];

pub(crate) struct TinfoilProvider {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    cache_secret: Arc<SecretString>,
    cache: Arc<VerifiedMaterialCache<Material>>,
}

#[derive(Clone)]
struct Material {
    key: String,
    evidence: SecurityEvidence,
}

// Operational timings only. In particular, never log decrypted chunks or keys.
struct ExchangeTiming<'a> {
    model: &'a str,
    request: &'a str,
    started: Instant,
    headers_ms: Option<u128>,
    first_delta_ms: Option<u128>,
    last_chunk: Option<Instant>,
    max_idle_ms: u128,
    complete: bool,
}

impl Drop for ExchangeTiming<'_> {
    fn drop(&mut self) {
        let total_ms = self.started.elapsed().as_millis();
        let max_idle_ms = self
            .max_idle_ms
            .max(self.last_chunk.map_or(0, |last| last.elapsed().as_millis()));
        tracing::info!(
            model = self.model,
            request = self.request,
            headers_ms = self.headers_ms,
            first_authenticated_delta_ms = self.first_delta_ms,
            max_body_idle_ms = max_idle_ms,
            total_ms,
            complete = self.complete,
            "Tinfoil native exchange timing"
        );
    }
}

impl TinfoilProvider {
    pub(crate) fn new(config: SecureClientConfig, credential: Arc<ApiCredential>) -> Self {
        let mut secret = [0u8; 32];
        OsRng.fill_bytes(&mut secret);
        Self {
            config,
            credential,
            cache_secret: Arc::new(SecretString::new(hex::encode(secret).into_boxed_str())),
            cache: Arc::new(VerifiedMaterialCache::default()),
        }
    }

    fn session(&self, model: &ModelInfo, material: Material) -> Box<dyn VerifiedSession> {
        Box::new(Session {
            config: self.config.clone(),
            credential: Arc::clone(&self.credential),
            cache_secret: Arc::clone(&self.cache_secret),
            model: model.clone(),
            material,
            cache: Arc::clone(&self.cache),
        })
    }
}

impl sealed::Provider for TinfoilProvider {}

#[async_trait]
impl SecureProvider for TinfoilProvider {
    fn id(&self) -> &'static str {
        "tinfoil"
    }

    fn supports(&self, model: &ModelInfo) -> Result<()> {
        if model.provider_id != self.id()
            || !self.supports_contract(
                &model.e2ee_protocol,
                model.e2ee_encryption_version,
                &model.attestation_protocol,
            )
            || model.provider_base_url.trim_end_matches('/') != BASE_URL
            || !MODELS.contains(&model.upstream_model.as_str())
            || model.id != format!("tinfoil-{}", model.upstream_model)
        {
            return Err(SecureClientError::capability(
                "model does not match the compiled Tinfoil contract",
            ));
        }
        Ok(())
    }

    fn supports_contract(&self, protocol: &str, version: u16, attestation: &str) -> bool {
        protocol == PROTOCOL && version == 1 && attestation == ATTESTATION
    }

    fn populate_capabilities(&self, model: &mut ModelInfo) {
        model.supports_tools = true;
        // The GPT-OSS deployment selects one call even when asked to batch.
        model.supports_parallel_tools = model.upstream_model != "gpt-oss-120b";
        model.supports_streaming = true;
        // Kimi K3 exposes native image input on this same attested whole-body
        // transport. Other offerings stay text-only until verified separately.
        model.supports_images = model.upstream_model == "kimi-k3";
    }

    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        if !policy.is_valid_at(now()) {
            return Err(SecureClientError::attestation("trust policy expired"));
        }
        if let Some(material) = self.cache.get(model, policy).await {
            return Ok(self.session(model, material));
        }
        let _lock = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            lock = self.cache.model_lock(&model.id) => lock,
        };
        if let Some(material) = self.cache.get(model, policy).await {
            return Ok(self.session(model, material));
        }
        // Only this locally verified result can populate a production session.
        // No provider API key is needed for public attestation verification.
        let mut verifier = ::tinfoil::SecureClient::new(HOST, REPOSITORY, "");
        let verification_started = Instant::now();
        let ground = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            result = tokio::time::timeout(self.config.attestation_timeout, verifier.verify()) => {
                result.map_err(|_| SecureClientError::session("Tinfoil verification timed out"))?
                    .map_err(|_| SecureClientError::attestation("Tinfoil enclave verification failed"))?
            },
        };
        let key = ground
            .hpke_public_key
            .clone()
            .ok_or_else(|| SecureClientError::attestation("Tinfoil evidence has no HPKE key"))?;
        let fingerprint = key_fingerprint(&key)?;
        let verified_at = now();
        let expires = verified_at
            .saturating_add(self.config.verified_session_ttl.as_secs().min(240))
            .min(policy.expires_at_unix_seconds);
        let evidence = SecurityEvidence {
            state: SecurityState::Verified,
            provider_id: self.id().into(),
            model_id: model.id.clone(),
            attestation_protocol: ATTESTATION.into(),
            e2ee_protocol: PROTOCOL.into(),
            e2ee_encryption_version: 1,
            trust_policy_version: policy.version.clone(),
            verified_at_unix_seconds: verified_at,
            attestation_generation: None,
            hard_expires_at_unix_seconds: Some(expires),
            model_key_fingerprint: fingerprint,
            tls_spki_fingerprint: ground.tls_public_key.clone(),
            checks: [
                ("amd_snp", "AMD SEV-SNP router attestation"),
                (
                    "source_measurement",
                    "Measured router matches Sigstore build provenance",
                ),
                (
                    "live_key_binding",
                    "Live TLS identity and attested HPKE key",
                ),
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
                    name: "source_repository".into(),
                    value: format!("https://github.com/{REPOSITORY}"),
                },
                EvidenceClaim {
                    name: "source_release".into(),
                    value: ground.release_tag.clone().unwrap_or_default(),
                },
                EvidenceClaim {
                    name: "worker_verification".into(),
                    value: "Model workers verified by the measured Tinfoil router".into(),
                },
            ],
            workload_manifest: Some(
                serde_json::to_string(&ground).map_err(|_| invalid("invalid Tinfoil evidence"))?,
            ),
            workload_identity_allowlist_configured: false,
            approved_workload_identity: false,
        };
        let material = Material { key, evidence };
        tracing::info!(
            model = model.id,
            elapsed_ms = verification_started.elapsed().as_millis(),
            "Tinfoil attestation verified"
        );
        self.cache
            .insert(
                model.clone(),
                policy.clone(),
                material.clone(),
                expires,
                self.config.verified_session_ttl,
            )
            .await;
        Ok(self.session(model, material))
    }

    async fn invalidate(&self, model_id: &str) {
        self.cache.invalidate(model_id).await;
    }
}

struct Session {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    cache_secret: Arc<SecretString>,
    model: ModelInfo,
    material: Material,
    cache: Arc<VerifiedMaterialCache<Material>>,
}
impl sealed::Session for Session {}

#[async_trait]
impl VerifiedSession for Session {
    fn evidence(&self) -> &SecurityEvidence {
        &self.material.evidence
    }
    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        let result = self.run(request, None, cancellation).await;
        self.invalidate_failed_exchange(&result).await;
        result
    }
    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        let result = self.run(request, Some(events), cancellation).await;
        self.invalidate_failed_exchange(&result).await;
        result
    }
}

impl Session {
    async fn invalidate_failed_exchange(&self, result: &Result<InferenceResponse>) {
        if result.as_ref().is_err_and(|error| {
            !matches!(
                error.kind(),
                axiom_inference::ProviderFailureKind::Cancelled
                    | axiom_inference::ProviderFailureKind::InvalidRequest
            )
        }) {
            // Refresh on the next user attempt after rotation or transport
            // failure. Never automatically replay a possibly billable request.
            self.cache.invalidate(&self.model.id).await;
        }
    }

    async fn run(
        &self,
        request: InferenceRequest,
        sink: Option<mpsc::Sender<ProviderEvent>>,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse> {
        if self
            .material
            .evidence
            .hard_expires_at_unix_seconds
            .is_none_or(|expiry| expiry <= now())
        {
            return Err(SecureClientError::with_code(
                axiom_inference::ProviderFailureKind::AttestationRejected,
                crate::SecureErrorCode::AttestationExpired,
                "Tinfoil session expired; verify again",
                true,
            ));
        }
        let stream = sink.is_some();
        let mut payload = request_body(&self.model, &request, stream)?;
        // The provider key is shared by Axiom users. A separate random secret
        // partitions prompt caching per native client, inside the encrypted body.
        payload["user_cache_secret"] = json!(self.cache_secret.expose_secret());
        let plaintext = serde_json::to_vec(&payload)
            .map_err(|_| invalid_request("request serialization failed"))?;
        if plaintext.len().saturating_add(20) > self.config.limits.serialized_relay_request_bytes {
            return Err(invalid_request("encrypted request exceeds size limit"));
        }
        let request_id = request_id(request.request_id.as_deref())?;
        let endpoint = relay_endpoint(
            &self.config.relay_base_url,
            "/api/v1/relay/tinfoil/chat/completions",
        )?;
        let http = pinned_client(&endpoint, &self.config.endpoint_policy, None, false).await?;
        let wire = http
            .post(endpoint)
            .bearer_auth(self.credential.expose())
            .header("Content-Type", "application/json")
            .header("X-Axiom-Model-Id", &self.model.id)
            .header("X-Axiom-Request-Id", &request_id)
            .header("X-Axiom-E2EE-Protocol", PROTOCOL)
            .body(plaintext)
            .build()
            .map_err(|_| invalid_request("request construction failed"))?;
        // execute() seals the entire body with a fresh HPKE context and wraps
        // the response in ordered AES-GCM decryption. It never sends plaintext.
        let encrypted = tinfoil_ehbp::Client::with_public_key_hex_and_http_client(
            self.config.relay_base_url.as_str(),
            &self.material.key,
            http,
        )
        .map_err(|_| SecureClientError::attestation("invalid attested Tinfoil key"))?;
        let mut timing = ExchangeTiming {
            model: &self.model.id,
            request: &request_id,
            started: Instant::now(),
            headers_ms: None,
            first_delta_ms: None,
            last_chunk: None,
            max_idle_ms: 0,
            complete: false,
        };
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            result = tokio::time::timeout(Duration::from_secs(300), encrypted.execute(wire)) => {
                result.map_err(|_| invalid("Tinfoil request timed out"))?
                    .map_err(|_| invalid("Tinfoil encrypted exchange failed"))?
            },
        };
        timing.headers_ms = Some(timing.started.elapsed().as_millis());
        if response.status() != reqwest::StatusCode::OK {
            let status = response.status();
            let body = bounded_body(response, self.config.limits.relay_error_bytes)
                .await
                .unwrap_or_default();
            return Err(error_for_status_body(status, &body));
        }
        if response
            .headers()
            .get("X-Axiom-Request-Id")
            .and_then(|v| v.to_str().ok())
            != Some(&request_id)
        {
            return Err(invalid("relay response has a different request identity"));
        }
        let mut source = response.bytes_stream();
        let mut parser = response::StreamParser::new(
            &self.model.upstream_model,
            self.config.limits.relay_sse_event_bytes,
        );
        let mut body = Vec::new();
        let mut received = 0usize;
        let hard_deadline = tokio::time::Instant::now() + Duration::from_secs(900);
        loop {
            let chunk = tokio::select! {
                () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
                () = tokio::time::sleep_until(hard_deadline) => return Err(invalid("Tinfoil response timed out")),
                next = tokio::time::timeout(Duration::from_secs(120), source.next()) => next.map_err(|_| invalid("Tinfoil response stalled"))?,
            };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk
                .map_err(|_| invalid("Tinfoil response authentication or transport failed"))?;
            if let Some(last) = timing.last_chunk.replace(Instant::now()) {
                timing.max_idle_ms = timing.max_idle_ms.max(last.elapsed().as_millis());
            }
            received = received.saturating_add(chunk.len());
            let limit = if stream {
                self.config.limits.relay_stream_bytes
            } else {
                self.config.limits.relay_response_bytes
            };
            if received > limit {
                return Err(invalid("Tinfoil response exceeds size limit"));
            }
            if stream {
                for event in parser.push(&chunk)? {
                    if timing.first_delta_ms.is_none()
                        && matches!(
                            event,
                            ProviderEvent::TextDelta(_)
                                | ProviderEvent::ReasoningDelta(_)
                                | ProviderEvent::RefusalDelta(_)
                                | ProviderEvent::ToolCallDelta(_)
                        )
                    {
                        timing.first_delta_ms = Some(timing.started.elapsed().as_millis());
                    }
                    send(sink.as_ref(), event, &cancellation).await?;
                }
            } else {
                body.extend_from_slice(&chunk);
            }
        }
        let output = if stream {
            parser.finish()?
        } else {
            response::complete(&body, &self.model.upstream_model)?
        };
        validate_requested_tools(&request, &output)?;
        // HTTP EOF is delivered only after provider usage has been settled.
        // Accounting remains operational metadata, never a verification proof.
        let records = RelayClient::new(&self.config, &self.credential)
            .accounting(std::slice::from_ref(&request_id), &cancellation)
            .await?;
        let record = records
            .into_iter()
            .next()
            .ok_or_else(|| invalid("Tinfoil accounting is missing"))?;
        if record.provider_id != "tinfoil"
            || record.model_id != self.model.id
            || record.state != axiom_inference::InvocationState::Completed
            || record.completeness != axiom_inference::UsageCompleteness::Final
            || !record.settled
            || record
                .input_tokens
                .as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                != Some(output.usage.input_tokens)
            || record
                .output_tokens
                .as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                != Some(output.usage.output_tokens)
        {
            return Err(invalid(
                "Tinfoil accounting does not match the authenticated response",
            ));
        }
        send(
            sink.as_ref(),
            ProviderEvent::Accounting(Box::new(record)),
            &cancellation,
        )
        .await?;
        send(
            sink.as_ref(),
            ProviderEvent::Usage {
                input_tokens: output.usage.input_tokens,
                output_tokens: output.usage.output_tokens,
            },
            &cancellation,
        )
        .await?;
        if let Some(reason) = &output.finish_reason {
            send(
                sink.as_ref(),
                ProviderEvent::Finished(reason.clone()),
                &cancellation,
            )
            .await?;
        }
        timing.complete = true;
        Ok(output)
    }
}

fn request_body(model: &ModelInfo, request: &InferenceRequest, stream: bool) -> Result<Value> {
    request
        .validate()
        .map_err(|_| invalid_request("inference request validation failed"))?;
    if request.model != model.id || request.thinking_mode != ThinkingMode::ProviderDefault {
        return Err(invalid_request(
            "request does not match Tinfoil model capabilities",
        ));
    }
    if request.parallel_tool_calls == Some(true) && model.upstream_model == "gpt-oss-120b" {
        return Err(invalid_request(
            "Tinfoil GPT-OSS supports sequential tool calls",
        ));
    }
    if request
        .messages
        .iter()
        .any(|message| !message.images.is_empty())
        && model.upstream_model != "kimi-k3"
    {
        return Err(invalid_request(
            "this model does not support encrypted image input",
        ));
    }
    let max_tokens = request.max_output_tokens;
    if max_tokens.is_some_and(|tokens| tokens == 0 || tokens > model.max_output_tokens) {
        return Err(invalid_request("invalid output limit"));
    }
    let mut messages =
        serde_json::to_value(&request.messages).map_err(|_| invalid_request("invalid messages"))?;
    // GPT-OSS replays reasoning only during a tool roundtrip; other models
    // advertise whether full reasoning history is accepted.
    for message in messages
        .as_array_mut()
        .ok_or_else(|| invalid_request("invalid messages"))?
    {
        if let Some(images) = message.as_object_mut().unwrap().remove("images") {
            let images: Vec<axiom_inference::ImageContent> = serde_json::from_value(images)
                .map_err(|_| invalid_request("invalid image content"))?;
            let mut parts = vec![json!({"type": "text", "text": message["content"]})];
            parts.extend(
                images.iter().map(
                    |image| json!({"type": "image_url", "image_url": {"url": image.data_url()}}),
                ),
            );
            message["content"] = Value::Array(parts);
        }
        if !model.reasoning_replay && message.get("tool_calls").is_none() {
            message.as_object_mut().unwrap().remove("reasoning_content");
        }
        if message["role"] == "assistant"
            && message["content"] == ""
            && message.get("tool_calls").is_some()
        {
            message["content"] = Value::Null;
        }
    }
    let mut body = json!({"model": model.upstream_model, "messages": messages, "stream": stream});
    if let Some(max_tokens) = max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    if !model.supported_reasoning_efforts.is_empty() {
        if !model
            .supported_reasoning_efforts
            .contains(&request.reasoning_effort)
        {
            return Err(invalid_request("unsupported reasoning effort"));
        }
        if matches!(model.upstream_model.as_str(), "glm-5-3" | "kimi-k3") {
            let effort = match request.reasoning_effort {
                ReasoningEffort::Low => "low",
                ReasoningEffort::Medium => "high",
                ReasoningEffort::High => "max",
                _ => return Err(invalid_request("unsupported reasoning effort")),
            };
            // Axiom's three effort levels map to the provider's low/high/max.
            if model.upstream_model == "glm-5-3" {
                body["chat_template_kwargs"] = json!({"reasoning_effort": effort});
            } else {
                body["reasoning_effort"] = json!(effort);
            }
        } else {
            body["reasoning_effort"] = json!(request.reasoning_effort.as_str());
        }
    }
    if let Some(value) = request.sampling.temperature {
        body["temperature"] = json!(value);
    }
    if let Some(value) = request.sampling.top_p {
        body["top_p"] = json!(value);
    }
    match &request.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::JsonObject => body["response_format"] = json!({"type":"json_object"}),
        ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            body["response_format"] = json!({"type":"json_schema", "json_schema":{"name":name,"schema":schema,"strict":strict}});
        }
    }
    if request.tools.is_empty() {
        if !matches!(request.tool_choice, ToolChoice::Auto | ToolChoice::None)
            || request.parallel_tool_calls.is_some()
        {
            return Err(invalid_request("tool controls require definitions"));
        }
    } else {
        body["tools"] = json!(request.tools);
        body["tool_choice"] = match &request.tool_choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::None => json!("none"),
            ToolChoice::Required => json!("required"),
            ToolChoice::Named { name } => {
                if !request.tools.iter().any(|tool| tool.function.name == *name) {
                    return Err(invalid_request("named tool is not defined"));
                }
                json!({"type":"function","function":{"name":name}})
            }
        };
        if let Some(value) = request.parallel_tool_calls {
            body["parallel_tool_calls"] = json!(value);
        }
    }
    Ok(body)
}

fn request_id(provided: Option<&str>) -> Result<String> {
    if let Some(id) = provided {
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid_request("invalid request id"));
        }
        return Ok(id.into());
    }
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    Ok(hex::encode(bytes))
}

fn validate_requested_tools(request: &InferenceRequest, output: &InferenceResponse) -> Result<()> {
    let calls = &output.assistant.tool_calls;
    if calls.iter().any(|call| {
        !request
            .tools
            .iter()
            .any(|tool| tool.function.name == call.function.name)
    }) || (request.parallel_tool_calls == Some(false) && calls.len() > 1)
    {
        return Err(invalid("provider returned an unrequested tool call"));
    }
    let valid = match &request.tool_choice {
        ToolChoice::Auto => true,
        ToolChoice::None => calls.is_empty(),
        ToolChoice::Required => !calls.is_empty(),
        ToolChoice::Named { name } => {
            !calls.is_empty() && calls.iter().all(|call| call.function.name == *name)
        }
    };
    if !valid {
        return Err(invalid("provider did not honor the requested tool choice"));
    }
    Ok(())
}

async fn send(
    sink: Option<&mpsc::Sender<ProviderEvent>>,
    event: ProviderEvent,
    cancellation: &CancellationToken,
) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(SecureClientError::cancelled());
    }
    if let Some(sink) = sink {
        tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            result = sink.send(event) => result.map_err(|_| SecureClientError::cancelled())?,
        }
    }
    Ok(())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn invalid(detail: &'static str) -> SecureClientError {
    SecureClientError::new(
        axiom_inference::ProviderFailureKind::InvalidResponse,
        detail,
    )
}
fn invalid_request(detail: &'static str) -> SecureClientError {
    SecureClientError::new(axiom_inference::ProviderFailureKind::InvalidRequest, detail)
}

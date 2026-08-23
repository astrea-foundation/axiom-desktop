use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use axiom_inference::{
    AssistantTurn, ChatMessage, ChatRole, FinishReason, InferenceRequest, InferenceResponse,
    ModelInfo, ProviderEvent, ReasoningEffort, ResponseFormat, ToolCall, ToolCallAccumulator,
    ToolCallDelta, ToolChoice, Usage,
};

use crate::{
    ApiCredential, Result, SecureClientConfig, SecureClientError,
    relay::{
        client::RelayClient,
        dto::{
            CiphertextHex, EncryptedFunctionCall, EncryptedFunctionDefinition, EncryptedMessage,
            EncryptedNamedFunction, EncryptedNamedToolChoice, EncryptedTool, EncryptedToolCall,
            EncryptedToolChoice, RelayChatRequest, RelayCompletion, RelayUsage,
        },
        sse::{RelayEvent, SseDecoder},
    },
};

use super::crypto;

const INFERENCE_ENCRYPTION: &str = "provider_e2ee_v2";
const MAX_PLAINTEXT_STREAM_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn complete(
    config: &SecureClientConfig,
    credential: &ApiCredential,
    model: &ModelInfo,
    model_key: crypto::Ed25519PublicKey,
    request: InferenceRequest,
    cancellation: &CancellationToken,
) -> Result<InferenceResponse> {
    let (identity, relay_request) = build_request(model, model_key, request, false)?;
    let completion = RelayClient::new(config, credential)
        .complete(&relay_request, cancellation)
        .await?;
    decrypt_completion(model, &identity, completion)
}

pub(crate) async fn stream(
    config: &SecureClientConfig,
    credential: &ApiCredential,
    model: &ModelInfo,
    model_key: crypto::Ed25519PublicKey,
    request: InferenceRequest,
    events: mpsc::Sender<ProviderEvent>,
    cancellation: &CancellationToken,
) -> Result<InferenceResponse> {
    let (identity, relay_request) = build_request(model, model_key, request, true)?;
    let response = RelayClient::new(config, credential)
        .stream(&relay_request, cancellation)
        .await?;
    let mut bytes = response.bytes_stream();
    let mut decoder = SseDecoder::new(
        config.limits.relay_sse_event_bytes,
        config.limits.relay_stream_bytes,
    );
    let mut state = StreamState::default();

    loop {
        let next = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            next = bytes.next() => next,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|_| invalid("relay stream transport failed"))?;
        for event in decoder.feed(&chunk)? {
            state
                .accept(event, &identity, &events, cancellation)
                .await?;
        }
    }
    decoder.finish()?;
    state.finish()
}

fn build_request(
    model: &ModelInfo,
    model_key: crypto::Ed25519PublicKey,
    request: InferenceRequest,
    stream: bool,
) -> Result<(crypto::ClientIdentity, RelayChatRequest)> {
    request
        .validate()
        .map_err(|_| invalid_request("inference request validation failed"))?;
    if request.model != model.id {
        return Err(invalid_request(
            "request model does not match verified session",
        ));
    }
    if request.reasoning_effort == ReasoningEffort::Minimal
        || (!model.supported_reasoning_efforts.is_empty()
            && !model
                .supported_reasoning_efforts
                .contains(&request.reasoning_effort))
    {
        return Err(invalid_request(
            "reasoning effort is not supported by the model",
        ));
    }
    if !request.tools.is_empty() && !model.supports_tools {
        return Err(invalid_request("model does not support tools"));
    }
    if request.tools.is_empty()
        && !matches!(request.tool_choice, ToolChoice::Auto | ToolChoice::None)
    {
        return Err(invalid_request("tool choice requires at least one tool"));
    }
    if let ToolChoice::Named { name } = &request.tool_choice
        && !request.tools.iter().any(|tool| tool.function.name == *name)
    {
        return Err(invalid_request("named tool choice is not defined"));
    }
    if request.parallel_tool_calls.is_some() && !model.supports_parallel_tools {
        return Err(invalid_request("model does not support parallel tools"));
    }
    if request.parallel_tool_calls.is_some() && request.tools.is_empty() {
        return Err(invalid_request("parallel tool calls require tools"));
    }
    if request
        .max_output_tokens
        .is_some_and(|limit| limit > model.max_output_tokens)
    {
        return Err(invalid_request("output limit exceeds the model capability"));
    }
    if stream && !model.supports_streaming {
        return Err(invalid_request("model does not support streaming"));
    }

    let identity = crypto::ClientIdentity::generate();
    let encrypted_messages = request
        .messages
        .iter()
        .map(|message| encrypt_message(model_key, message))
        .collect::<Result<Vec<_>>>()?;
    let encrypted_tools = (!request.tools.is_empty())
        .then(|| {
            request
                .tools
                .iter()
                .map(|tool| {
                    let parameters = serde_json::to_vec(&tool.function.parameters)
                        .map_err(|_| invalid_request("tool parameters are not serializable"))?;
                    Ok(EncryptedTool {
                        kind: "function",
                        function: EncryptedFunctionDefinition {
                            encrypted_name: encrypt(model_key, tool.function.name.as_bytes())?,
                            encrypted_description: Some(encrypt(
                                model_key,
                                tool.function.description.as_bytes(),
                            )?),
                            encrypted_parameters: encrypt(model_key, &parameters)?,
                            strict: tool.function.strict,
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let encrypted_tool_choice = match &request.tool_choice {
        ToolChoice::Auto => encrypted_tools
            .as_ref()
            .map(|_| EncryptedToolChoice::Mode("auto")),
        ToolChoice::None => Some(EncryptedToolChoice::Mode("none")),
        ToolChoice::Required => Some(EncryptedToolChoice::Mode("required")),
        ToolChoice::Named { name } => Some(EncryptedToolChoice::Named(EncryptedNamedToolChoice {
            kind: "function",
            function: EncryptedNamedFunction {
                encrypted_name: encrypt(model_key, name.as_bytes())?,
            },
        })),
    };
    let sampling = (request.sampling.temperature.is_some() || request.sampling.top_p.is_some())
        .then(|| {
            serde_json::to_value(&request.sampling)
                .map_err(|_| invalid_request("sampling parameters are invalid"))
        })
        .transpose()?;
    let response_format = match request.response_format {
        ResponseFormat::Text => None,
        other => Some(
            serde_json::to_value(other)
                .map_err(|_| invalid_request("response format is invalid"))?,
        ),
    };
    let relay = RelayChatRequest {
        model_id: model.id.clone(),
        encryption_version: model.e2ee_encryption_version,
        e2ee_protocol: model.e2ee_protocol.clone(),
        client_public_key_hex: identity.public_key_hex(),
        model_public_key_hex: model_key.to_hex(),
        encrypted_messages,
        encrypt_all_fields: true,
        encrypted_tools,
        encrypted_tool_choice,
        parallel_tool_calls: request.parallel_tool_calls,
        stream,
        max_tokens: request.max_output_tokens,
        sampling,
        response_format,
        reasoning_effort: (!model.supported_reasoning_efforts.is_empty())
            .then(|| request.reasoning_effort.as_str().to_owned()),
    };
    Ok((identity, relay))
}

fn encrypt_message(
    model_key: crypto::Ed25519PublicKey,
    message: &ChatMessage,
) -> Result<EncryptedMessage> {
    validate_message_shape(message)?;
    let required_content = matches!(
        message.role,
        ChatRole::System | ChatRole::Developer | ChatRole::User | ChatRole::Tool
    );
    let encrypted_content = (required_content || !message.content.is_empty())
        .then(|| encrypt(model_key, message.content.as_bytes()))
        .transpose()?;
    let encrypted_tool_calls = (!message.tool_calls.is_empty())
        .then(|| {
            message
                .tool_calls
                .iter()
                .map(|call| {
                    validate_call_id(&call.id)?;
                    Ok(EncryptedToolCall {
                        id: call.id.clone(),
                        kind: "function".to_owned(),
                        function: EncryptedFunctionCall {
                            encrypted_name: encrypt(model_key, call.function.name.as_bytes())?,
                            encrypted_arguments: encrypt(
                                model_key,
                                call.function.arguments.as_bytes(),
                            )?,
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    Ok(EncryptedMessage {
        role: role_name(&message.role),
        encrypted_content,
        encrypted_reasoning_content: encrypt_optional(
            model_key,
            message.reasoning_content.as_deref(),
        )?,
        encrypted_name: encrypt_optional(model_key, message.name.as_deref())?,
        encrypted_refusal: encrypt_optional(model_key, message.refusal.as_deref())?,
        encrypted_tool_calls,
        tool_call_id: message.tool_call_id.clone(),
    })
}

fn validate_message_shape(message: &ChatMessage) -> Result<()> {
    match message.role {
        ChatRole::System | ChatRole::Developer | ChatRole::User => {
            if message.reasoning_content.is_some()
                || message.refusal.is_some()
                || message.tool_call_id.is_some()
                || !message.tool_calls.is_empty()
            {
                return Err(invalid_request("message fields do not match its role"));
            }
        }
        ChatRole::Assistant => {
            if message.tool_call_id.is_some()
                || (message.content.is_empty()
                    && message.refusal.is_none()
                    && message.tool_calls.is_empty())
            {
                return Err(invalid_request("assistant message shape is invalid"));
            }
        }
        ChatRole::Tool => {
            if message.tool_call_id.is_none()
                || message.reasoning_content.is_some()
                || message.refusal.is_some()
                || message.name.is_some()
                || !message.tool_calls.is_empty()
            {
                return Err(invalid_request("tool message shape is invalid"));
            }
        }
    }
    if let Some(id) = &message.tool_call_id {
        validate_call_id(id)?;
    }
    Ok(())
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

fn validate_call_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.chars().count() > axiom_inference::MAX_TOOL_CALL_ID_CHARS
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
    {
        Err(invalid_request("tool call ID is invalid"))
    } else {
        Ok(())
    }
}

fn encrypt_optional(
    model_key: crypto::Ed25519PublicKey,
    value: Option<&str>,
) -> Result<Option<CiphertextHex>> {
    value
        .map(|value| encrypt(model_key, value.as_bytes()))
        .transpose()
}

fn encrypt(model_key: crypto::Ed25519PublicKey, bytes: &[u8]) -> Result<CiphertextHex> {
    CiphertextHex::new(crypto::encrypt_hex(model_key, bytes)?)
}

fn decrypt(identity: &crypto::ClientIdentity, ciphertext: &CiphertextHex) -> Result<String> {
    let bytes = identity.decrypt_hex(ciphertext.as_str())?;
    String::from_utf8(bytes).map_err(|_| {
        SecureClientError::new(
            axiom_inference::ProviderFailureKind::Decryption,
            "decrypted relay value is not UTF-8",
        )
    })
}

fn decrypt_completion(
    model: &ModelInfo,
    identity: &crypto::ClientIdentity,
    completion: RelayCompletion,
) -> Result<InferenceResponse> {
    validate_run_id(&completion.id)?;
    if completion
        .model
        .as_ref()
        .is_some_and(|value| value != &model.upstream_model && value != &model.id)
    {
        return Err(invalid(
            "relay response model does not match verified session",
        ));
    }
    let text = completion
        .encrypted_content
        .as_ref()
        .map(|value| decrypt(identity, value))
        .transpose()?
        .unwrap_or_default();
    let reasoning = completion
        .encrypted_reasoning_content
        .as_ref()
        .map(|value| decrypt(identity, value))
        .transpose()?;
    let refusal = completion
        .encrypted_refusal
        .as_ref()
        .map(|value| decrypt(identity, value))
        .transpose()?;
    let tool_calls = decrypt_tool_calls(identity, completion.encrypted_tool_calls)?;
    if text.is_empty() && refusal.is_none() && tool_calls.is_empty() {
        return Err(invalid("relay completion has no usable output"));
    }
    let finish_reason = parse_finish_reason(&completion.finish_reason)?;
    validate_finish_shape(&finish_reason, &tool_calls)?;
    Ok(InferenceResponse {
        assistant: AssistantTurn { text, tool_calls },
        reasoning,
        refusal,
        usage: normalize_usage(&completion.usage)?,
        finish_reason: Some(finish_reason),
    })
}

fn decrypt_tool_calls(
    identity: &crypto::ClientIdentity,
    encrypted: Vec<EncryptedToolCall>,
) -> Result<Vec<ToolCall>> {
    if encrypted.len() > axiom_inference::MAX_TOOL_CALLS {
        return Err(invalid("relay returned too many tool calls"));
    }
    let mut calls = Vec::with_capacity(encrypted.len());
    let mut seen = std::collections::BTreeSet::new();
    for call in encrypted {
        validate_call_id(&call.id)?;
        if call.kind != "function" || !seen.insert(call.id.clone()) {
            return Err(invalid("relay tool call metadata is invalid"));
        }
        let name = decrypt(identity, &call.function.encrypted_name)?;
        let arguments = decrypt(identity, &call.function.encrypted_arguments)?;
        validate_tool_arguments(&arguments)?;
        calls.push(ToolCall {
            id: call.id,
            kind: call.kind,
            function: axiom_inference::FunctionCall { name, arguments },
        });
    }
    Ok(calls)
}

fn validate_tool_arguments(arguments: &str) -> Result<()> {
    let _: serde_json::Value = serde_json::from_str(arguments)
        .map_err(|_| invalid("decrypted tool arguments are not valid JSON"))?;
    Ok(())
}

fn parse_finish_reason(value: &str) -> Result<FinishReason> {
    match value {
        "stop" => Ok(FinishReason::Stop),
        "length" => Ok(FinishReason::Length),
        "tool_calls" => Ok(FinishReason::ToolCalls),
        "content_filter" => Ok(FinishReason::ContentFilter),
        _ => Err(invalid("relay finish reason is invalid")),
    }
}

fn validate_finish_shape(reason: &FinishReason, calls: &[ToolCall]) -> Result<()> {
    if matches!(reason, FinishReason::ToolCalls) != !calls.is_empty() {
        return Err(invalid("relay finish reason conflicts with tool calls"));
    }
    Ok(())
}

fn normalize_usage(usage: &RelayUsage) -> Result<Usage> {
    let input_tokens = usage.prompt_tokens.unwrap_or(0);
    let output_tokens = usage.completion_tokens.unwrap_or(0);
    if usage
        .cached_prompt_tokens
        .is_some_and(|cached| usage.prompt_tokens.is_none_or(|prompt| cached > prompt))
    {
        return Err(invalid("relay cached-token usage is inconsistent"));
    }
    Ok(Usage {
        input_tokens,
        output_tokens,
        total_tokens: usage
            .total_tokens
            .unwrap_or_else(|| input_tokens.saturating_add(output_tokens)),
    })
}

fn validate_run_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(invalid("relay run ID is invalid"))
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct StreamState {
    run_id: Option<String>,
    next_sequence: Option<u64>,
    text: String,
    reasoning: String,
    refusal: String,
    tool_calls: ToolCallAccumulator,
    usage: Option<Usage>,
    finish_reason: Option<FinishReason>,
    message_completed: bool,
    run_completed: bool,
}

impl StreamState {
    async fn accept(
        &mut self,
        event: RelayEvent,
        identity: &crypto::ClientIdentity,
        sink: &mpsc::Sender<ProviderEvent>,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        if self.run_completed {
            return Err(invalid("relay emitted data after terminal completion"));
        }
        match event {
            RelayEvent::RunCreated(payload) => {
                if self.run_id.is_some() || payload.inference_encryption != INFERENCE_ENCRYPTION {
                    return Err(invalid("relay run creation contract is invalid"));
                }
                validate_run_id(&payload.run_id)?;
                self.run_id = Some(payload.run_id);
                send(
                    sink,
                    ProviderEvent::Status {
                        connected: true,
                        detail: "Attested end-to-end encrypted inference connected".to_owned(),
                    },
                    cancellation,
                )
                .await?;
            }
            RelayEvent::Delta(payload) => {
                self.require_run(&payload.run_id)?;
                if self.message_completed {
                    return Err(invalid("relay emitted a delta after message completion"));
                }
                if let Some(expected) = self.next_sequence
                    && payload.sequence != expected
                {
                    return Err(invalid("relay event sequence is invalid"));
                }
                self.next_sequence = Some(
                    payload
                        .sequence
                        .checked_add(1)
                        .ok_or_else(|| invalid("relay event sequence overflowed"))?,
                );
                if let Some(value) = payload.encrypted_delta {
                    let value = decrypt(identity, &value)?;
                    append_bounded(&mut self.text, &value)?;
                    send(sink, ProviderEvent::TextDelta(value), cancellation).await?;
                }
                if let Some(value) = payload.encrypted_reasoning_delta {
                    let value = decrypt(identity, &value)?;
                    append_bounded(&mut self.reasoning, &value)?;
                    send(sink, ProviderEvent::ReasoningDelta(value), cancellation).await?;
                }
                if let Some(value) = payload.encrypted_refusal_delta {
                    let value = decrypt(identity, &value)?;
                    append_bounded(&mut self.refusal, &value)?;
                    send(sink, ProviderEvent::RefusalDelta(value), cancellation).await?;
                }
                for encrypted in payload.encrypted_tool_calls {
                    let delta = ToolCallDelta {
                        index: encrypted.index,
                        id: encrypted.id,
                        kind: encrypted.kind,
                        name: encrypted
                            .function
                            .as_ref()
                            .and_then(|function| function.encrypted_name.as_ref())
                            .map(|value| decrypt(identity, value))
                            .transpose()?,
                        arguments: encrypted
                            .function
                            .as_ref()
                            .and_then(|function| function.encrypted_arguments.as_ref())
                            .map(|value| decrypt(identity, value))
                            .transpose()?,
                    };
                    self.tool_calls
                        .push(delta.clone())
                        .map_err(|_| invalid("relay tool-call fragments are invalid"))?;
                    send(sink, ProviderEvent::ToolCallDelta(delta), cancellation).await?;
                }
            }
            RelayEvent::MessageCompleted(payload) => {
                self.require_run(&payload.run_id)?;
                if self.message_completed {
                    return Err(invalid("relay message completion is duplicated"));
                }
                let reason = parse_finish_reason(&payload.finish_reason)?;
                let usage = normalize_usage(&payload.usage)?;
                send(
                    sink,
                    ProviderEvent::Usage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                    },
                    cancellation,
                )
                .await?;
                send(sink, ProviderEvent::Finished(reason.clone()), cancellation).await?;
                self.usage = Some(usage);
                self.finish_reason = Some(reason);
                self.message_completed = true;
            }
            RelayEvent::RunCompleted(payload) => {
                self.require_run(&payload.run_id)?;
                if !self.message_completed {
                    return Err(invalid("relay run completed before its message"));
                }
                self.run_completed = true;
            }
            RelayEvent::Cancelled(payload) => {
                self.require_run(&payload.run_id)?;
                if payload.reason.as_deref() == Some("insufficient_credit") {
                    return Err(SecureClientError::new(
                        axiom_inference::ProviderFailureKind::InsufficientCredit,
                        "Axiom credit is exhausted",
                    ));
                }
                return Err(SecureClientError::cancelled());
            }
            RelayEvent::Failed(payload) => {
                self.require_run(&payload.run_id)?;
                let _ = (payload.error, payload.code);
                return Err(SecureClientError::new(
                    axiom_inference::ProviderFailureKind::Transient,
                    "remote encrypted inference failed",
                ));
            }
        }
        Ok(())
    }

    fn require_run(&self, run_id: &str) -> Result<()> {
        if self.run_id.as_deref() == Some(run_id) {
            Ok(())
        } else {
            Err(invalid("relay event run ID is inconsistent"))
        }
    }

    fn finish(self) -> Result<InferenceResponse> {
        if !self.message_completed || !self.run_completed {
            return Err(invalid("relay stream ended without terminal completion"));
        }
        let tool_calls = self
            .tool_calls
            .finish()
            .map_err(|_| invalid("relay tool-call stream is incomplete"))?;
        for call in &tool_calls {
            validate_call_id(&call.id)?;
            validate_tool_arguments(&call.function.arguments)?;
        }
        let finish_reason = self
            .finish_reason
            .ok_or_else(|| invalid("relay stream has no finish reason"))?;
        validate_finish_shape(&finish_reason, &tool_calls)?;
        Ok(InferenceResponse {
            assistant: AssistantTurn {
                text: self.text,
                tool_calls,
            },
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            refusal: (!self.refusal.is_empty()).then_some(self.refusal),
            usage: self.usage.unwrap_or_default(),
            finish_reason: Some(finish_reason),
        })
    }
}

fn append_bounded(target: &mut String, fragment: &str) -> Result<()> {
    if target.len().saturating_add(fragment.len()) > MAX_PLAINTEXT_STREAM_BYTES {
        return Err(invalid(
            "decrypted relay stream exceeds the configured limit",
        ));
    }
    target.push_str(fragment);
    Ok(())
}

async fn send(
    sink: &mpsc::Sender<ProviderEvent>,
    event: ProviderEvent,
    cancellation: &CancellationToken,
) -> Result<()> {
    tokio::select! {
        () = cancellation.cancelled() => Err(SecureClientError::cancelled()),
        result = sink.send(event) => result.map_err(|_| SecureClientError::cancelled()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::dto::{
        CompletedPayload, CreatedPayload, DeltaPayload, EncryptedFunctionCallDelta,
        EncryptedToolCallDelta, RunPayload,
    };
    use axiom_inference::{FunctionDefinition, ToolDefinition};

    fn model() -> ModelInfo {
        ModelInfo {
            id: "secure-model".to_owned(),
            upstream_model: "provider/model".to_owned(),
            max_output_tokens: 8_192,
            supported_reasoning_efforts: vec![ReasoningEffort::Medium],
            supports_tools: true,
            supports_parallel_tools: true,
            supports_streaming: true,
            ..ModelInfo::default()
        }
    }

    #[test]
    fn relay_serialization_contains_no_secret_bearing_sentinels() {
        let recipient = crypto::ClientIdentity::generate();
        let recipient_key =
            crypto::Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        let sentinel = "SENTINEL-private-🌸";
        let request = InferenceRequest {
            model: "secure-model".to_owned(),
            messages: vec![ChatMessage::text(ChatRole::User, sentinel)],
            tools: vec![ToolDefinition {
                kind: "function".to_owned(),
                function: FunctionDefinition {
                    name: format!("tool_{sentinel}"),
                    description: sentinel.to_owned(),
                    parameters: serde_json::json!({"secret": sentinel}),
                    strict: Some(true),
                },
            }],
            tool_choice: ToolChoice::Named {
                name: format!("tool_{sentinel}"),
            },
            parallel_tool_calls: Some(false),
            reasoning_effort: ReasoningEffort::Medium,
            max_output_tokens: Some(128),
            sampling: axiom_inference::SamplingParameters::default(),
            response_format: ResponseFormat::Text,
            stream: false,
        };
        // The function-name validation catches the deliberately invalid test
        // name, so use a valid but unique name while retaining sentinels in all
        // arbitrary secret-bearing strings.
        let mut request = request;
        request.tools[0].function.name = "sentinel_tool".to_owned();
        request.tool_choice = ToolChoice::Named {
            name: "sentinel_tool".to_owned(),
        };
        let (_, relay) = build_request(&model(), recipient_key, request, false).unwrap();
        let json = serde_json::to_string(&relay).unwrap();
        assert!(!json.contains(sentinel));
        assert!(!json.contains("sentinel_tool"));
        assert!(json.contains("encrypted_messages"));
        assert!(json.contains("encrypt_all_fields"));
    }

    #[test]
    fn non_reasoning_models_omit_the_provider_parameter_and_usage_is_consistent() {
        let recipient = crypto::ClientIdentity::generate();
        let recipient_key =
            crypto::Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        let mut non_reasoning = model();
        non_reasoning.supported_reasoning_efforts.clear();
        let mut request = InferenceRequest::streaming(
            non_reasoning.id.clone(),
            vec![ChatMessage::text(ChatRole::User, "hello")],
            Vec::new(),
        );
        request.reasoning_effort = ReasoningEffort::Medium;
        request.stream = false;
        let (_, relay) = build_request(&non_reasoning, recipient_key, request, false).unwrap();
        assert!(relay.reasoning_effort.is_none());

        assert!(
            normalize_usage(&RelayUsage {
                prompt_tokens: Some(10),
                completion_tokens: Some(5),
                total_tokens: Some(15),
                cached_prompt_tokens: Some(4),
            })
            .is_ok()
        );
        assert!(
            normalize_usage(&RelayUsage {
                prompt_tokens: Some(3),
                cached_prompt_tokens: Some(4),
                ..RelayUsage::default()
            })
            .is_err()
        );
    }

    #[test]
    fn tool_results_and_assistant_history_are_encrypted() {
        let recipient = crypto::ClientIdentity::generate();
        let recipient_key =
            crypto::Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        let messages = [
            ChatMessage {
                role: ChatRole::Assistant,
                content: String::new(),
                reasoning_content: None,
                name: None,
                refusal: None,
                tool_call_id: None,
                tool_calls: vec![ToolCall {
                    id: "call:1".to_owned(),
                    kind: "function".to_owned(),
                    function: axiom_inference::FunctionCall {
                        name: "read_file".to_owned(),
                        arguments: "{\"path\":\"SECRET_PATH\"}".to_owned(),
                    },
                }],
            },
            ChatMessage {
                role: ChatRole::Tool,
                content: "SECRET_RESULT".to_owned(),
                reasoning_content: None,
                name: None,
                refusal: None,
                tool_call_id: Some("call:1".to_owned()),
                tool_calls: Vec::new(),
            },
        ];
        let encrypted = messages
            .iter()
            .map(|message| encrypt_message(recipient_key, message))
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let json = serde_json::to_string(&encrypted).unwrap();
        for secret in ["read_file", "SECRET_PATH", "SECRET_RESULT"] {
            assert!(!json.contains(secret));
        }
        assert!(json.contains("call:1"));
    }

    #[test]
    fn decrypts_a_bounded_nonstream_completion_and_rejects_tampering() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let completion = RelayCompletion {
            id: "run-1".to_owned(),
            model: Some("provider/model".to_owned()),
            encrypted_content: Some(encrypt(client_key, b"finished").unwrap()),
            encrypted_reasoning_content: Some(encrypt(client_key, b"reasoned").unwrap()),
            encrypted_refusal: None,
            encrypted_tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            usage: RelayUsage {
                prompt_tokens: Some(10),
                completion_tokens: Some(4),
                total_tokens: Some(14),
                cached_prompt_tokens: None,
            },
        };
        let response = decrypt_completion(&model(), &identity, completion).unwrap();
        assert_eq!(response.assistant.text, "finished");
        assert_eq!(response.reasoning.as_deref(), Some("reasoned"));
        assert_eq!(response.usage.total_tokens, 14);
        assert_eq!(response.finish_reason, Some(FinishReason::Stop));

        let mut tampered = crypto::encrypt_hex(client_key, b"secret").unwrap();
        let replacement = if &tampered[150..152] == "00" {
            "01"
        } else {
            "00"
        };
        tampered.replace_range(150..152, replacement);
        let tampered = RelayCompletion {
            id: "run-2".to_owned(),
            model: Some("provider/model".to_owned()),
            encrypted_content: Some(CiphertextHex::new(tampered).unwrap()),
            encrypted_reasoning_content: None,
            encrypted_refusal: None,
            encrypted_tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            usage: RelayUsage::default(),
        };
        assert!(decrypt_completion(&model(), &identity, tampered).is_err());
    }

    #[tokio::test]
    async fn assembles_ordered_stream_content_and_tool_calls() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let (sink, mut output) = mpsc::channel(16);
        let cancellation = CancellationToken::new();
        let mut state = StreamState::default();
        state
            .accept(
                RelayEvent::RunCreated(CreatedPayload {
                    run_id: "run-1".to_owned(),
                    inference_encryption: INFERENCE_ENCRYPTION.to_owned(),
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        state
            .accept(
                RelayEvent::Delta(DeltaPayload {
                    run_id: "run-1".to_owned(),
                    encrypted_delta: Some(encrypt(client_key, b"working ").unwrap()),
                    encrypted_reasoning_delta: None,
                    encrypted_refusal_delta: None,
                    encrypted_tool_calls: vec![EncryptedToolCallDelta {
                        index: 0,
                        id: Some("call-1".to_owned()),
                        kind: Some("function".to_owned()),
                        function: Some(EncryptedFunctionCallDelta {
                            encrypted_name: Some(encrypt(client_key, b"read_").unwrap()),
                            encrypted_arguments: Some(encrypt(client_key, b"{\"path\":").unwrap()),
                        }),
                    }],
                    sequence: 1,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        state
            .accept(
                RelayEvent::Delta(DeltaPayload {
                    run_id: "run-1".to_owned(),
                    encrypted_delta: Some(encrypt(client_key, b"done").unwrap()),
                    encrypted_reasoning_delta: None,
                    encrypted_refusal_delta: None,
                    encrypted_tool_calls: vec![EncryptedToolCallDelta {
                        index: 0,
                        id: None,
                        kind: None,
                        function: Some(EncryptedFunctionCallDelta {
                            encrypted_name: Some(encrypt(client_key, b"file").unwrap()),
                            encrypted_arguments: Some(encrypt(client_key, b"\"a.rs\"}").unwrap()),
                        }),
                    }],
                    sequence: 2,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        state
            .accept(
                RelayEvent::MessageCompleted(CompletedPayload {
                    run_id: "run-1".to_owned(),
                    usage: RelayUsage {
                        prompt_tokens: Some(8),
                        completion_tokens: Some(5),
                        total_tokens: Some(13),
                        cached_prompt_tokens: None,
                    },
                    finish_reason: "tool_calls".to_owned(),
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        state
            .accept(
                RelayEvent::RunCompleted(RunPayload {
                    run_id: "run-1".to_owned(),
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        let response = state.finish().unwrap();
        assert_eq!(response.assistant.text, "working done");
        assert_eq!(response.assistant.tool_calls[0].function.name, "read_file");
        assert_eq!(
            response.assistant.tool_calls[0].function.arguments,
            "{\"path\":\"a.rs\"}"
        );
        assert_eq!(response.usage.total_tokens, 13);
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::Status { .. })
        ));
    }

    #[tokio::test]
    async fn stream_rejects_out_of_order_and_incomplete_terminal_state() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let (sink, _output) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let mut state = StreamState::default();
        state
            .accept(
                RelayEvent::RunCreated(CreatedPayload {
                    run_id: "run-1".to_owned(),
                    inference_encryption: INFERENCE_ENCRYPTION.to_owned(),
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        for sequence in [1, 3] {
            let result = state
                .accept(
                    RelayEvent::Delta(DeltaPayload {
                        run_id: "run-1".to_owned(),
                        encrypted_delta: Some(encrypt(client_key, b"x").unwrap()),
                        encrypted_reasoning_delta: None,
                        encrypted_refusal_delta: None,
                        encrypted_tool_calls: Vec::new(),
                        sequence,
                    }),
                    &identity,
                    &sink,
                    &cancellation,
                )
                .await;
            if sequence == 1 {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
        }
        assert!(state.finish().is_err());
    }
}

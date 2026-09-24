use std::io::Write;

use base64::{Engine, engine::general_purpose};
use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::ser::Formatter;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use axiom_inference::{
    AssistantTurn, ChatMessage, ChatRole, FinishReason, InferenceRequest, InferenceResponse,
    ModelInfo, ProviderEvent, ReasoningEffort, ResponseFormat, ToolCall, ToolCallAccumulator,
    ToolCallDelta, ToolChoice, Usage,
};

use crate::{
    ApiCredential, Result, SecureClientConfig, SecureClientError,
    lease::AttestationBinding,
    relay::{
        client::RelayClient,
        dto::{
            CiphertextHex, EncryptedFunctionCall, EncryptedFunctionDefinition, EncryptedMessage,
            EncryptedNamedFunction, EncryptedNamedToolChoice, EncryptedTool, EncryptedToolCall,
            EncryptedToolChoice, NearCompletionProof, RelayChatRequest, RelayCompletion,
            RelayUsage,
        },
        sse::{RelayEvent, SseDecoder},
    },
};

use super::crypto;

const INFERENCE_ENCRYPTION: &str = "provider_e2ee_v2";
const MAX_PLAINTEXT_STREAM_BYTES: usize = axiom_inference::MAX_MESSAGE_TEXT_BYTES;
const NEAR_RECEIPT_RAW_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct NearInferenceContext<'a> {
    pub(crate) config: &'a SecureClientConfig,
    pub(crate) credential: &'a ApiCredential,
    pub(crate) model: &'a ModelInfo,
    pub(crate) model_key: crypto::Ed25519PublicKey,
    pub(crate) response_signing_address: &'a str,
    pub(crate) binding: &'a AttestationBinding,
    pub(crate) worker_session_id: &'a str,
}

pub(crate) async fn complete(
    context: NearInferenceContext<'_>,
    request: InferenceRequest,
    cancellation: &CancellationToken,
) -> Result<InferenceResponse> {
    let NearInferenceContext {
        config,
        credential,
        model,
        model_key,
        response_signing_address,
        binding,
        worker_session_id,
    } = context;
    let (identity, mut relay_request, request_hash) = build_request(
        model,
        model_key,
        response_signing_address,
        binding,
        &request,
        false,
    )?;
    relay_request
        .provider_e2ee_context
        .as_mut()
        .expect("built context")["worker_session_id"] = worker_session_id.into();
    let completion = RelayClient::new(config, credential)
        .complete(&relay_request, cancellation)
        .await?;
    decrypt_completion(
        model,
        &identity,
        &request_hash,
        response_signing_address,
        completion,
    )
}

pub(crate) async fn stream(
    context: NearInferenceContext<'_>,
    request: InferenceRequest,
    events: mpsc::Sender<ProviderEvent>,
    cancellation: &CancellationToken,
) -> Result<InferenceResponse> {
    let NearInferenceContext {
        config,
        credential,
        model,
        model_key,
        response_signing_address,
        binding,
        worker_session_id,
    } = context;
    let (identity, mut relay_request, request_hash) = build_request(
        model,
        model_key,
        response_signing_address,
        binding,
        &request,
        true,
    )?;
    relay_request
        .provider_e2ee_context
        .as_mut()
        .expect("built context")["worker_session_id"] = worker_session_id.into();
    let response = RelayClient::new(config, credential)
        .stream(&relay_request, cancellation)
        .await?;
    let mut bytes = response.bytes_stream();
    let mut decoder = SseDecoder::new(
        config.limits.relay_sse_event_bytes,
        config.limits.relay_stream_bytes,
    );
    let mut state = StreamState::new(
        request_hash,
        response_signing_address.to_owned(),
        model.upstream_model.clone(),
    );

    let mut deadline = crate::relay::deadline::StreamDeadline::new();
    loop {
        let next = tokio::select! {
            () = tokio::time::sleep_until(deadline.at()) => return Err(deadline.error()),
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            next = bytes.next() => next,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|_| {
            SecureClientError::with_code(
                axiom_inference::ProviderFailureKind::Transient,
                crate::SecureErrorCode::ProviderDisconnected,
                "The connection to the provider was interrupted.",
                true,
            )
        })?;
        for event in decoder.feed(&chunk)? {
            deadline.observe(&event);
            state
                .accept(event, &identity, &events, cancellation)
                .await?;
        }
    }
    decoder.finish()?;
    let (response, verified_events) = state.finish()?;
    emit_verified_events(verified_events, &events, cancellation).await?;
    Ok(response)
}

fn build_request(
    model: &ModelInfo,
    model_key: crypto::Ed25519PublicKey,
    response_signing_address: &str,
    binding: &AttestationBinding,
    request: &InferenceRequest,
    stream: bool,
) -> Result<(crypto::ClientIdentity, RelayChatRequest, String)> {
    request
        .validate()
        .map_err(|_| invalid_request("inference request validation failed"))?;
    let response_format = public_response_format(&request.response_format)?;
    if request.messages.iter().any(|message| {
        (!model.supports_images && !message.images.is_empty()) || !message.files.is_empty()
    }) {
        return Err(invalid_request(
            "this NEAR model does not support the attached file format",
        ));
    }
    if request.model != model.id {
        return Err(invalid_request(
            "request model does not match verified session",
        ));
    }
    if request.thinking_mode != axiom_inference::ThinkingMode::ProviderDefault
        && !model
            .supported_thinking_modes
            .contains(&request.thinking_mode)
    {
        return Err(invalid_request(
            "thinking mode is not supported by the model",
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
    // The catalog exposes the backend's effective provider cap. Materialize it into the relay
    // request even when the caller omitted a limit so the locally hashed value and the value
    // admitted by the backend cannot take separate defaulting paths.
    let max_tokens = request.max_output_tokens.unwrap_or(model.max_output_tokens);

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
    let mut relay = RelayChatRequest {
        client_request_id: request.request_id.clone(),
        provider_id: model.provider_id.clone(),
        inference_encryption: INFERENCE_ENCRYPTION,
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
        max_tokens: Some(max_tokens),
        sampling,
        response_format,
        thinking_mode: (request.thinking_mode != axiom_inference::ThinkingMode::ProviderDefault)
            .then_some(request.thinking_mode),
        reasoning_effort: (!model.supported_reasoning_efforts.is_empty())
            .then(|| request.reasoning_effort.as_str().to_owned()),
        provider_e2ee_context: None,
        attestation_generation: binding.generation,
        verified_key_fingerprint: binding.model_key_fingerprint.clone(),
        verified_keyset_digest: binding.keyset_digest.clone(),
    };
    let request_body = near_upstream_request_bytes(model, &relay)?;
    let request_hash = hex::encode(Sha256::digest(request_body));
    relay.provider_e2ee_context = Some(serde_json::json!({
        "request_body_hash": request_hash,
        "response_signing_address": response_signing_address,
    }));
    Ok((identity, relay, request_hash))
}

#[derive(Serialize)]
struct NearUpstreamRequest<'a> {
    model: &'a str,
    messages: Vec<NearUpstreamMessage<'a>>,
    stream: bool,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_template_kwargs: Option<NearThinkingControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<NearUpstreamTool<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<NearUpstreamToolChoice<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
}

#[derive(Serialize)]
struct NearThinkingControl {
    thinking: bool,
}

#[derive(Serialize)]
struct NearUpstreamMessage<'a> {
    role: &'a str,
    // The upstream distinguishes an omitted field, an explicit assistant null, and ciphertext.
    #[allow(clippy::option_option)]
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Option<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<NearUpstreamToolCall<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct NearUpstreamToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    function: NearUpstreamFunctionCall<'a>,
}

#[derive(Serialize)]
struct NearUpstreamFunctionCall<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct NearUpstreamTool<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    function: NearUpstreamFunctionDefinition<'a>,
}

#[derive(Serialize)]
struct NearUpstreamFunctionDefinition<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum NearUpstreamToolChoice<'a> {
    Mode(&'a str),
    Named(NearUpstreamNamedToolChoice<'a>),
}

#[derive(Serialize)]
struct NearUpstreamNamedToolChoice<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    function: NearUpstreamNamedFunction<'a>,
}

#[derive(Serialize)]
struct NearUpstreamNamedFunction<'a> {
    name: &'a str,
}

fn near_upstream_request_bytes(model: &ModelInfo, relay: &RelayChatRequest) -> Result<Vec<u8>> {
    if relay
        .response_format
        .as_ref()
        .is_some_and(|value| !is_fixed_json_object_format(value))
    {
        return Err(invalid_request(
            "schema-bearing response formats cannot cross the provider-E2EE boundary",
        ));
    }
    let messages = relay
        .encrypted_messages
        .iter()
        .map(|message| NearUpstreamMessage {
            role: message.role,
            content: message
                .encrypted_content
                .as_ref()
                .map(|value| Some(value.as_str()))
                .or_else(|| (message.role == "assistant").then_some(None)),
            reasoning_content: message
                .encrypted_reasoning_content
                .as_ref()
                .map(CiphertextHex::as_str),
            name: message.encrypted_name.as_ref().map(CiphertextHex::as_str),
            refusal: message
                .encrypted_refusal
                .as_ref()
                .map(CiphertextHex::as_str),
            tool_calls: message.encrypted_tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|call| NearUpstreamToolCall {
                        id: &call.id,
                        kind: &call.kind,
                        function: NearUpstreamFunctionCall {
                            name: call.function.encrypted_name.as_str(),
                            arguments: call.function.encrypted_arguments.as_str(),
                        },
                    })
                    .collect()
            }),
            tool_call_id: message.tool_call_id.as_deref(),
        })
        .collect();
    let tools = relay.encrypted_tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|tool| NearUpstreamTool {
                kind: tool.kind,
                function: NearUpstreamFunctionDefinition {
                    name: tool.function.encrypted_name.as_str(),
                    description: tool
                        .function
                        .encrypted_description
                        .as_ref()
                        .map(CiphertextHex::as_str),
                    parameters: tool.function.encrypted_parameters.as_str(),
                    strict: tool.function.strict,
                },
            })
            .collect()
    });
    let tool_choice = relay
        .encrypted_tool_choice
        .as_ref()
        .map(|choice| match choice {
            EncryptedToolChoice::Mode(mode) => NearUpstreamToolChoice::Mode(mode),
            EncryptedToolChoice::Named(choice) => {
                NearUpstreamToolChoice::Named(NearUpstreamNamedToolChoice {
                    kind: choice.kind,
                    function: NearUpstreamNamedFunction {
                        name: choice.function.encrypted_name.as_str(),
                    },
                })
            }
        });
    let sampling = relay
        .sampling
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let number = |name: &str| sampling.and_then(|values| values.get(name)?.as_f64());
    let reasoning_effort = relay
        .reasoning_effort
        .as_deref()
        .map(|effort| if effort == "xhigh" { "max" } else { effort });
    let request = NearUpstreamRequest {
        model: &model.upstream_model,
        messages,
        stream: relay.stream,
        max_tokens: relay.max_tokens.unwrap_or(model.max_output_tokens),
        temperature: number("temperature"),
        top_p: number("top_p"),
        response_format: relay.response_format.as_ref(),
        reasoning_effort,
        chat_template_kwargs: relay.thinking_mode.map(|mode| NearThinkingControl {
            thinking: mode == axiom_inference::ThinkingMode::Enabled,
        }),
        tools,
        tool_choice,
        parallel_tool_calls: relay.parallel_tool_calls,
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, PythonCompactUtf8Formatter);
    request
        .serialize(&mut serializer)
        .map_err(|_| invalid_request("NEAR upstream request could not be serialized"))?;
    Ok(bytes)
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

fn is_fixed_json_object_format(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 1
            && object.get("type").and_then(serde_json::Value::as_str) == Some("json_object")
    })
}

struct PythonCompactUtf8Formatter;

impl Formatter for PythonCompactUtf8Formatter {
    fn write_f32<W>(&mut self, writer: &mut W, value: f32) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        writer.write_all(python_float_repr(f64::from(value)).as_bytes())
    }

    fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        writer.write_all(python_float_repr(value).as_bytes())
    }

    fn write_number_str<W>(&mut self, writer: &mut W, value: &str) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        if (value.contains('.') || value.contains('e') || value.contains('E'))
            && let Ok(number) = value.parse::<f64>()
            && number.is_finite()
        {
            return writer.write_all(python_float_repr(number).as_bytes());
        }
        writer.write_all(value.as_bytes())
    }
}

fn python_float_repr(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    let mut buffer = ryu::Buffer::new();
    let rendered = buffer.format_finite(value);
    let (negative, unsigned) = rendered
        .strip_prefix('-')
        .map_or((false, rendered), |value| (true, value));
    let (mantissa, explicit_exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, None), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().ok())
        });
    let decimal_index = mantissa.find('.').unwrap_or(mantissa.len());
    let mut digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let first_nonzero = digits.find(|character| character != '0').unwrap_or(0);
    let decimal_index = i32::try_from(decimal_index).expect("a float spelling is bounded");
    let first_nonzero_i32 = i32::try_from(first_nonzero).expect("a float spelling is bounded");
    let exponent = explicit_exponent.unwrap_or(decimal_index - first_nonzero_i32 - 1);
    digits.drain(..first_nonzero);
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }

    let mut output = String::new();
    if negative {
        output.push('-');
    }
    if !(-4..16).contains(&exponent) {
        output.push(digits.as_bytes()[0] as char);
        if digits.len() > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        output.push('e');
        output.push(if exponent < 0 { '-' } else { '+' });
        let absolute = exponent.unsigned_abs();
        if absolute < 10 {
            output.push('0');
        }
        output.push_str(&absolute.to_string());
    } else if exponent < 0 {
        output.push_str("0.");
        let zero_count = usize::try_from(-exponent - 1).expect("negative exponent was checked");
        output.extend(std::iter::repeat_n('0', zero_count));
        output.push_str(&digits);
    } else {
        let integer_digits = usize::try_from(exponent).expect("positive exponent was checked") + 1;
        if digits.len() <= integer_digits {
            output.push_str(&digits);
            output.extend(std::iter::repeat_n('0', integer_digits - digits.len()));
            output.push_str(".0");
        } else {
            output.push_str(&digits[..integer_digits]);
            output.push('.');
            output.push_str(&digits[integer_digits..]);
        }
    }
    output
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
    // The upstream proxy decrypts and restores JSON content arrays. Inline
    // image bytes and text are encrypted together, never placed in relay JSON.
    let content = if message.images.is_empty() {
        message.content.clone()
    } else {
        let mut parts = vec![serde_json::json!({"type": "text", "text": message.content})];
        parts.extend(message.images.iter().map(|image| serde_json::json!({"type": "image_url", "image_url": {"url": image.data_url()}})));
        serde_json::to_string(&parts).map_err(|_| invalid_request("invalid image content"))?
    };
    let encrypted_content = (required_content || !content.is_empty())
        .then(|| encrypt(model_key, content.as_bytes()))
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
    expected_request_hash: &str,
    expected_signing_address: &str,
    completion: RelayCompletion,
) -> Result<InferenceResponse> {
    let proof_value = completion
        .proof
        .clone()
        .ok_or_else(|| invalid("NEAR completion omitted its signed receipt"))?;
    let (proof, raw_body) = verify_near_proof(
        proof_value,
        expected_request_hash,
        expected_signing_address,
        &model.upstream_model,
    )?;
    if proof.chat_id != completion.id {
        return Err(invalid(
            "NEAR receipt chat ID does not match the relay response",
        ));
    }
    let raw_completion = parse_raw_completion(&raw_body)?;
    if raw_completion.id != proof.chat_id {
        return Err(invalid(
            "NEAR receipt chat ID does not match its response bytes",
        ));
    }
    let verified = decrypt_relay_completion(model, identity, raw_completion)?;
    let provisional = decrypt_relay_completion(model, identity, completion)?;
    if provisional != verified {
        return Err(invalid(
            "relay completion does not match the signed NEAR response",
        ));
    }
    Ok(verified)
}

fn decrypt_relay_completion(
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
        assistant: AssistantTurn {
            reasoning: None,
            text,
            tool_calls,
        },
        reasoning,
        refusal,
        usage: normalize_usage(&completion.usage)?,
        finish_reason: Some(finish_reason),
    })
}

#[derive(Deserialize)]
struct RawNearCompletion {
    id: String,
    #[serde(default)]
    model: Option<String>,
    choices: Vec<RawNearCompletionChoice>,
    #[serde(default)]
    usage: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawNearCompletionChoice {
    message: RawNearMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct RawNearMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Vec<RawNearToolCall>,
}

#[derive(Deserialize)]
struct RawNearToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: RawNearFunctionCall,
}

#[derive(Deserialize)]
struct RawNearFunctionCall {
    name: String,
    arguments: String,
}

fn parse_raw_completion(bytes: &[u8]) -> Result<RelayCompletion> {
    let raw: RawNearCompletion = serde_json::from_slice(bytes)
        .map_err(|_| invalid("signed NEAR completion JSON is invalid"))?;
    validate_run_id(&raw.id)?;
    let choice = raw
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| invalid("signed NEAR completion has no choice"))?;
    let encrypted_tool_calls = choice
        .message
        .tool_calls
        .into_iter()
        .map(|call| {
            Ok(EncryptedToolCall {
                id: call.id,
                kind: call.kind,
                function: EncryptedFunctionCall {
                    encrypted_name: required_ciphertext(
                        call.function.name,
                        "signed NEAR tool name is invalid",
                    )?,
                    encrypted_arguments: required_ciphertext(
                        call.function.arguments,
                        "signed NEAR tool arguments are invalid",
                    )?,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RelayCompletion {
        id: raw.id,
        model: raw.model,
        encrypted_content: optional_ciphertext(choice.message.content)?,
        encrypted_reasoning_content: optional_ciphertext(choice.message.reasoning_content)?,
        encrypted_refusal: optional_ciphertext(choice.message.refusal)?,
        encrypted_tool_calls,
        finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".to_owned()),
        usage: provider_usage(raw.usage.as_ref()),
        proof: None,
    })
}

fn optional_ciphertext(value: Option<String>) -> Result<Option<CiphertextHex>> {
    value
        .filter(|value| !value.is_empty())
        .map(CiphertextHex::new)
        .transpose()
}

fn required_ciphertext(value: String, detail: &'static str) -> Result<CiphertextHex> {
    if value.is_empty() {
        return Err(invalid(detail));
    }
    CiphertextHex::new(value).map_err(|_| invalid(detail))
}

fn provider_usage(value: Option<&serde_json::Value>) -> RelayUsage {
    let object = value.and_then(serde_json::Value::as_object);
    let token = |names: &[&str]| {
        names.iter().find_map(|name| {
            object
                .and_then(|object| object.get(*name))
                .and_then(token_value)
        })
    };
    let prompt_tokens = token(&["prompt_tokens", "input_tokens"]);
    let cached = token(&["cached_prompt_tokens"]).or_else(|| {
        ["prompt_tokens_details", "input_tokens_details"]
            .iter()
            .find_map(|name| {
                object
                    .and_then(|object| object.get(*name))
                    .and_then(serde_json::Value::as_object)
                    .and_then(|details| details.get("cached_tokens"))
                    .and_then(token_value)
            })
    });
    RelayUsage {
        prompt_tokens,
        completion_tokens: token(&["completion_tokens", "output_tokens"]),
        total_tokens: token(&["total_tokens"]),
        cached_prompt_tokens: cached
            .map(|value| prompt_tokens.map_or(value, |prompt| value.min(prompt))),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn token_value(value: &serde_json::Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return Some(value);
    }
    if let Some(value) = value.as_i64() {
        return Some(value.max(0).cast_unsigned());
    }
    if let Some(value) = value.as_f64()
        && value.is_finite()
    {
        return Some(value.max(0.0).trunc() as u64);
    }
    value
        .as_str()
        .and_then(|value| value.parse::<i128>().ok())
        .map(|value| {
            u64::try_from(value.max(0).min(i128::from(u64::MAX)))
                .expect("the provider token count was clamped to u64")
        })
}

fn verify_near_proof(
    value: serde_json::Value,
    expected_request_hash: &str,
    expected_signing_address: &str,
    expected_model: &str,
) -> Result<(NearCompletionProof, Vec<u8>)> {
    let proof: NearCompletionProof = serde_json::from_value(value)
        .map_err(|_| invalid("NEAR completion receipt schema is invalid"))?;
    if proof.provider != super::PROVIDER_ID || proof.protocol != super::E2EE_PROTOCOL {
        return Err(invalid("NEAR completion receipt protocol is invalid"));
    }
    if proof.chat_id.is_empty()
        || proof.chat_id.len() > 256
        || proof.chat_id.chars().any(char::is_control)
        || proof.model != expected_model
        || !canonical_hash(&proof.request_hash)
        || !canonical_hash(&proof.response_hash)
        || proof.request_hash != expected_request_hash
        || !canonical_signing_address(&proof.signing_address)
        || proof.signing_address != expected_signing_address
    {
        return Err(invalid("NEAR completion receipt binding is invalid"));
    }
    let maximum_encoded = NEAR_RECEIPT_RAW_RESPONSE_BYTES
        .div_ceil(3)
        .saturating_mul(4);
    if proof.response_body_base64.len() > maximum_encoded {
        return Err(invalid(
            "NEAR completion receipt response exceeds its bound",
        ));
    }
    let raw_body = general_purpose::STANDARD
        .decode(&proof.response_body_base64)
        .map_err(|_| invalid("NEAR completion receipt response encoding is invalid"))?;
    if raw_body.is_empty() || raw_body.len() > NEAR_RECEIPT_RAW_RESPONSE_BYTES {
        return Err(invalid(
            "NEAR completion receipt response exceeds its bound",
        ));
    }
    if hex::encode(Sha256::digest(&raw_body)) != proof.response_hash {
        return Err(invalid("NEAR completion receipt response hash is invalid"));
    }
    let hash_pair = format!("{}:{}", proof.request_hash, proof.response_hash);
    let model_hash_pair = format!("{}:{hash_pair}", proof.model);
    if proof.signed_text != model_hash_pair {
        return Err(invalid("NEAR completion receipt signed text is invalid"));
    }
    verify_ed25519_signature(&proof.signed_text, &proof.signature, &proof.signing_address)?;
    Ok((proof, raw_body))
}

fn canonical_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn canonical_signing_address(value: &str) -> bool {
    canonical_hash(value)
}

fn verify_ed25519_signature(
    signed_text: &str,
    encoded_signature: &str,
    expected_address: &str,
) -> Result<()> {
    if signed_text.len() > 1024
        || !canonical_signing_address(expected_address)
        || encoded_signature.len() != 128
        || !encoded_signature
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(invalid("NEAR completion signature encoding is invalid"));
    }
    let key_bytes: [u8; 32] = hex::decode(expected_address)
        .map_err(|_| invalid("NEAR receipt key is invalid"))?
        .try_into()
        .map_err(|_| invalid("NEAR receipt key is invalid"))?;
    let key =
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| invalid("NEAR receipt key is invalid"))?;
    let signature_bytes =
        hex::decode(encoded_signature).map_err(|_| invalid("NEAR receipt signature is invalid"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| invalid("NEAR receipt signature is invalid"))?;
    key.verify_strict(signed_text.as_bytes(), &signature)
        .map_err(|_| invalid("NEAR completion signature does not match attestation"))
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

#[derive(Default, Deserialize)]
struct RawNearStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<RawNearToolCallDelta>>,
}

#[derive(Deserialize)]
struct RawNearToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    function: Option<RawNearFunctionCallDelta>,
}

#[derive(Deserialize)]
struct RawNearFunctionCallDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Default, Deserialize)]
struct RawNearStreamChoice {
    #[serde(default)]
    delta: RawNearStreamDelta,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct RawNearStreamChunk {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<RawNearStreamChoice>,
    #[serde(default)]
    usage: Option<serde_json::Value>,
}

struct VerifiedStreamTranscript {
    chat_id: String,
    model: Option<String>,
    sequence_count: u64,
    response: InferenceResponse,
}

fn parse_raw_stream(
    bytes: &[u8],
    identity: &crypto::ClientIdentity,
) -> Result<VerifiedStreamTranscript> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("signed NEAR stream is not valid UTF-8"))?;
    let mut chat_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut refusal = String::new();
    let mut tool_calls = ToolCallAccumulator::default();
    let mut usage = provider_usage(None);
    let mut finish_reason = "stop".to_owned();
    let mut sequence_count = 0_u64;
    let mut done = false;
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.strip_prefix(' ').unwrap_or(payload).trim();
        if payload == "[DONE]" {
            if done {
                return Err(invalid("signed NEAR stream repeats its terminal marker"));
            }
            done = true;
            continue;
        }
        if done {
            return Err(invalid("signed NEAR stream contains data after completion"));
        }
        let chunk: RawNearStreamChunk = serde_json::from_str(payload)
            .map_err(|_| invalid("signed NEAR stream event is invalid"))?;
        if let Some(value) = chunk.id {
            validate_run_id(&value)?;
            if chat_id.as_ref().is_some_and(|existing| existing != &value) {
                return Err(invalid("signed NEAR stream changes its chat ID"));
            }
            chat_id = Some(value);
        }
        if let Some(value) = chunk.model {
            if model.as_ref().is_some_and(|existing| existing != &value) {
                return Err(invalid("signed NEAR stream changes its model"));
            }
            model = Some(value);
        }
        if chunk.usage.as_ref().is_some_and(json_value_is_truthy) {
            usage = provider_usage(chunk.usage.as_ref());
        }
        let choice = chunk.choices.into_iter().next().unwrap_or_default();
        if let Some(value) = choice.finish_reason {
            finish_reason = value;
        }
        let encrypted_content = optional_ciphertext(choice.delta.content)?;
        let encrypted_content = if encrypted_content.is_some() {
            encrypted_content
        } else {
            optional_ciphertext(choice.text)?
        };
        let encrypted_reasoning = optional_ciphertext(choice.delta.reasoning_content)?;
        let encrypted_refusal = optional_ciphertext(choice.delta.refusal)?;
        let has_tool_calls = choice
            .delta
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty());
        if encrypted_content.is_some()
            || encrypted_reasoning.is_some()
            || encrypted_refusal.is_some()
            || has_tool_calls
        {
            sequence_count = sequence_count
                .checked_add(1)
                .ok_or_else(|| invalid("signed NEAR stream sequence overflowed"))?;
        }
        if let Some(value) = encrypted_content {
            append_bounded(&mut content, &decrypt(identity, &value)?)?;
        }
        if let Some(value) = encrypted_reasoning {
            append_bounded(&mut reasoning, &decrypt(identity, &value)?)?;
        }
        if let Some(value) = encrypted_refusal {
            append_bounded(&mut refusal, &decrypt(identity, &value)?)?;
        }
        for encrypted in choice.delta.tool_calls.unwrap_or_default() {
            let delta = ToolCallDelta {
                index: encrypted.index,
                id: encrypted.id,
                kind: encrypted.kind,
                name: encrypted
                    .function
                    .as_ref()
                    .and_then(|function| function.name.clone())
                    .filter(|value| !value.is_empty())
                    .map(CiphertextHex::new)
                    .transpose()?
                    .as_ref()
                    .map(|value| decrypt(identity, value))
                    .transpose()?,
                arguments: encrypted
                    .function
                    .as_ref()
                    .and_then(|function| function.arguments.clone())
                    .filter(|value| !value.is_empty())
                    .map(CiphertextHex::new)
                    .transpose()?
                    .as_ref()
                    .map(|value| decrypt(identity, value))
                    .transpose()?,
            };
            tool_calls
                .push(delta)
                .map_err(|_| invalid("signed NEAR tool-call fragments are invalid"))?;
        }
    }
    if !done {
        return Err(invalid("signed NEAR stream omitted its terminal marker"));
    }
    let chat_id = chat_id.ok_or_else(|| invalid("signed NEAR stream omitted its chat ID"))?;
    let calls = tool_calls
        .finish()
        .map_err(|_| invalid("signed NEAR tool-call stream is incomplete"))?;
    for call in &calls {
        validate_call_id(&call.id)?;
        validate_tool_arguments(&call.function.arguments)?;
    }
    let finish_reason = parse_finish_reason(&finish_reason)?;
    validate_finish_shape(&finish_reason, &calls)?;
    Ok(VerifiedStreamTranscript {
        chat_id,
        model,
        sequence_count,
        response: InferenceResponse {
            assistant: AssistantTurn {
                reasoning: None,
                text: content,
                tool_calls: calls,
            },
            reasoning: (!reasoning.is_empty()).then_some(reasoning),
            refusal: (!refusal.is_empty()).then_some(refusal),
            usage: normalize_usage(&usage)?,
            finish_reason: Some(finish_reason),
        },
    })
}

fn json_value_is_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

struct StreamState {
    expected_request_hash: String,
    expected_signing_address: String,
    expected_model: String,
    run_id: Option<String>,
    response_id: Option<String>,
    next_sequence: Option<u64>,
    text: String,
    reasoning: String,
    refusal: String,
    tool_calls: ToolCallAccumulator,
    usage: Option<Usage>,
    finish_reason: Option<FinishReason>,
    /// Text is emitted as provisional after AEAD decryption. Tool-call deltas,
    /// usage and success remain gated on the exact signed transcript and EOF.
    verified_events: Vec<ProviderEvent>,
    message_completed: bool,
    run_completed: bool,
}

impl StreamState {
    fn new(
        expected_request_hash: String,
        expected_signing_address: String,
        expected_model: String,
    ) -> Self {
        Self {
            expected_request_hash,
            expected_signing_address,
            expected_model,
            run_id: None,
            response_id: None,
            next_sequence: Some(1),
            text: String::new(),
            reasoning: String::new(),
            refusal: String::new(),
            tool_calls: ToolCallAccumulator::default(),
            usage: None,
            finish_reason: None,
            verified_events: Vec::new(),
            message_completed: false,
            run_completed: false,
        }
    }

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
            RelayEvent::Finalizing(payload) => {
                self.require_run(&payload.run_id)?;
            }
            RelayEvent::Accounting(usage) => {
                self.require_run(&usage.request_id)?;
                if !usage.validate_counters() || usage.response_verified {
                    return Err(invalid("invalid operational accounting"));
                }
                send(
                    sink,
                    ProviderEvent::Accounting(Box::new(usage)),
                    cancellation,
                )
                .await?;
            }
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
                let _provider_field_metadata = (
                    payload.content_field.as_deref(),
                    payload.reasoning_field.as_deref(),
                    payload.refusal_field.as_deref(),
                );
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
                if let Some(response_id) = payload.response_id.as_deref() {
                    validate_run_id(response_id)?;
                    if self
                        .response_id
                        .as_ref()
                        .is_some_and(|existing| existing != response_id)
                    {
                        return Err(invalid("NEAR relay delta changed its response ID"));
                    }
                    self.response_id = Some(response_id.to_owned());
                }
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
                    self.verified_events
                        .push(ProviderEvent::ToolCallDelta(delta));
                }
            }
            RelayEvent::MessageCompleted(payload) => {
                self.require_run(&payload.run_id)?;
                if self.message_completed {
                    return Err(invalid("relay message completion is duplicated"));
                }
                let proof_value = payload
                    .proof
                    .ok_or_else(|| invalid("NEAR stream omitted its signed receipt"))?;
                let (proof, raw_body) = verify_near_proof(
                    proof_value,
                    &self.expected_request_hash,
                    &self.expected_signing_address,
                    &self.expected_model,
                )?;
                if self
                    .response_id
                    .as_ref()
                    .is_some_and(|response_id| response_id != &proof.chat_id)
                {
                    return Err(invalid(
                        "NEAR stream receipt chat ID does not match provisional deltas",
                    ));
                }
                let verified = parse_raw_stream(&raw_body, identity)?;
                if verified.chat_id != proof.chat_id
                    || verified
                        .model
                        .as_ref()
                        .is_some_and(|model| model != &self.expected_model)
                {
                    return Err(invalid(
                        "NEAR stream receipt metadata does not match its response bytes",
                    ));
                }
                let processed_sequences = self
                    .next_sequence
                    .unwrap_or(1)
                    .checked_sub(1)
                    .ok_or_else(|| invalid("relay sequence state is invalid"))?;
                if verified.sequence_count != processed_sequences {
                    return Err(invalid(
                        "relay stream delta count does not match the signed NEAR response",
                    ));
                }
                let reason = parse_finish_reason(&payload.finish_reason)?;
                let usage = normalize_usage(&payload.usage)?;
                let provisional = self.provisional_response(usage.clone(), reason.clone())?;
                if provisional != verified.response {
                    return Err(invalid(
                        "relay stream does not match the signed NEAR response",
                    ));
                }
                self.verified_events.push(ProviderEvent::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                });
                self.verified_events
                    .push(ProviderEvent::Finished(reason.clone()));
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
                return Err(crate::relay::client::stream_failure(
                    payload.code.as_deref(),
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

    fn provisional_response(
        &self,
        usage: Usage,
        finish_reason: FinishReason,
    ) -> Result<InferenceResponse> {
        let tool_calls = self
            .tool_calls
            .clone()
            .finish()
            .map_err(|_| invalid("relay tool-call stream is incomplete"))?;
        for call in &tool_calls {
            validate_call_id(&call.id)?;
            validate_tool_arguments(&call.function.arguments)?;
        }
        validate_finish_shape(&finish_reason, &tool_calls)?;
        Ok(InferenceResponse {
            assistant: AssistantTurn {
                reasoning: None,
                text: self.text.clone(),
                tool_calls,
            },
            reasoning: (!self.reasoning.is_empty()).then(|| self.reasoning.clone()),
            refusal: (!self.refusal.is_empty()).then(|| self.refusal.clone()),
            usage,
            finish_reason: Some(finish_reason),
        })
    }

    fn finish(self) -> Result<(InferenceResponse, Vec<ProviderEvent>)> {
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
        let response = InferenceResponse {
            assistant: AssistantTurn {
                reasoning: None,
                text: self.text,
                tool_calls,
            },
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            refusal: (!self.refusal.is_empty()).then_some(self.refusal),
            usage: self.usage.unwrap_or_default(),
            finish_reason: Some(finish_reason),
        };
        Ok((response, self.verified_events))
    }
}

async fn emit_verified_events(
    events: Vec<ProviderEvent>,
    sink: &mpsc::Sender<ProviderEvent>,
    cancellation: &CancellationToken,
) -> Result<()> {
    for event in events {
        send(sink, event, cancellation).await?;
    }
    Ok(())
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
    if cancellation.is_cancelled() {
        return Err(SecureClientError::cancelled());
    }
    tokio::select! {
        biased;
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
    use ed25519_dalek::{Signer, SigningKey};

    #[derive(Serialize)]
    struct SerializationVector<'a> {
        text: &'a str,
        value: f64,
    }

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

    fn binding() -> AttestationBinding {
        AttestationBinding {
            generation: 1,
            model_key_fingerprint: "00".repeat(32),
            keyset_digest: None,
            hard_expires_at_unix_seconds: u64::MAX,
        }
    }

    #[test]
    fn image_parts_are_encrypted_together_and_decrypt_to_the_upstream_array() {
        let worker = crypto::ClientIdentity::generate();
        let key = crypto::Ed25519PublicKey::from_hex(&worker.public_key_hex()).unwrap();
        let mut message = ChatMessage::text(ChatRole::User, "private image question");
        let image = axiom_inference::ImageContent {
            mime_type: "image/png".into(),
            data: "iVBORw0KGgpmaXh0dXJl".into(),
        };
        message.images.push(image.clone());
        let encrypted = encrypt_message(key, &message).unwrap();
        let wire = serde_json::to_string(&encrypted).unwrap();
        assert!(!wire.contains("private image question"));
        assert!(!wire.contains(&image.data));
        assert!(!wire.contains("image_url"));
        let plaintext = worker
            .decrypt_hex(encrypted.encrypted_content.as_ref().unwrap().as_str())
            .unwrap();
        let content: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(content[0]["text"], message.content);
        assert_eq!(content[1]["image_url"]["url"], image.data_url());
        let other_worker = crypto::ClientIdentity::generate();
        assert!(
            other_worker
                .decrypt_hex(encrypted.encrypted_content.as_ref().unwrap().as_str())
                .is_err()
        );
    }

    #[test]
    fn signed_stream_optional_tool_calls_accepts_null_but_not_wrong_types() {
        let identity = crypto::ClientIdentity::generate();
        for delta in [
            serde_json::json!({}),
            serde_json::json!({"tool_calls": null}),
            serde_json::json!({"tool_calls": []}),
        ] {
            let chunk = serde_json::json!({"id": "signed-chat", "model": "provider/model", "choices": [{"delta": delta, "finish_reason": "stop"}]});
            let raw = format!("data: {chunk}\n\ndata: [DONE]\n\n");
            let transcript = parse_raw_stream(raw.as_bytes(), &identity).unwrap();
            assert_eq!(transcript.sequence_count, 0);
            assert!(transcript.response.assistant.tool_calls.is_empty());
        }
        for invalid in [
            serde_json::json!({}),
            serde_json::json!(false),
            serde_json::json!("not-an-array"),
            serde_json::json!([null]),
        ] {
            let chunk = serde_json::json!({"id": "signed-chat", "choices": [{"delta": {"tool_calls": invalid}}]});
            let raw = format!("data: {chunk}\n\ndata: [DONE]\n\n");
            assert!(parse_raw_stream(raw.as_bytes(), &identity).is_err());
        }
    }

    #[test]
    fn python_compact_utf8_serialization_matches_backend_vectors() {
        for (value, expected) in [
            (1e-5, "1e-05"),
            (1e-4, "0.0001"),
            (1e16, "1e+16"),
            (1.2e15, "1200000000000000.0"),
            (-0.0, "-0.0"),
        ] {
            assert_eq!(python_float_repr(value), expected);
        }

        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, PythonCompactUtf8Formatter);
        SerializationVector {
            text: "café 😀\u{7f}",
            value: 1e-5,
        }
        .serialize(&mut serializer)
        .unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\"text\":\"café 😀\u{7f}\",\"value\":1e-05}"
        );
    }

    #[test]
    fn json_schema_metadata_is_rejected_before_near_relay_construction() {
        let recipient = crypto::ClientIdentity::generate();
        let recipient_key =
            crypto::Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        let sentinel = "plaintext-schema-instruction-must-not-cross";
        let mut request = InferenceRequest::streaming(
            "secure-model",
            vec![ChatMessage::text(ChatRole::User, "encrypted message")],
            Vec::new(),
        );
        request.response_format = ResponseFormat::JsonSchema {
            name: sentinel.into(),
            schema: serde_json::json!({"description": sentinel}),
            strict: true,
        };

        let error = build_request(
            &model(),
            recipient_key,
            &response_signer().1,
            &binding(),
            &request,
            true,
        )
        .expect_err("schema metadata must fail closed");
        assert_eq!(
            error.kind(),
            axiom_inference::ProviderFailureKind::InvalidRequest
        );
        assert!(!error.safe_detail().contains(sentinel));
    }

    #[test]
    fn upstream_request_hash_uses_effective_cap_and_raw_utf8() {
        let mut offered_model = model();
        offered_model.max_output_tokens = 8_192;
        let ciphertext = "ab".repeat(72);
        let mut relay = RelayChatRequest {
            client_request_id: None,
            thinking_mode: None,
            provider_id: offered_model.provider_id.clone(),
            inference_encryption: INFERENCE_ENCRYPTION,
            model_id: offered_model.id.clone(),
            encryption_version: offered_model.e2ee_encryption_version,
            e2ee_protocol: offered_model.e2ee_protocol.clone(),
            client_public_key_hex: "11".repeat(32),
            model_public_key_hex: "22".repeat(32),
            encrypted_messages: vec![EncryptedMessage {
                role: "user",
                encrypted_content: Some(CiphertextHex::new(ciphertext.clone()).unwrap()),
                encrypted_reasoning_content: None,
                encrypted_name: None,
                encrypted_refusal: None,
                encrypted_tool_calls: None,
                tool_call_id: None,
            }],
            encrypt_all_fields: true,
            encrypted_tools: None,
            encrypted_tool_choice: Some(EncryptedToolChoice::Mode("none")),
            parallel_tool_calls: None,
            stream: false,
            max_tokens: Some(offered_model.max_output_tokens),
            sampling: Some(serde_json::json!({"temperature": 1e-5})),
            response_format: Some(serde_json::json!({"type": "json_object"})),
            reasoning_effort: Some("medium".into()),
            provider_e2ee_context: None,
            attestation_generation: 1,
            verified_key_fingerprint: "33".repeat(32),
            verified_keyset_digest: None,
        };
        let bytes = near_upstream_request_bytes(&offered_model, &relay).unwrap();
        let body = String::from_utf8(bytes.clone()).unwrap();
        assert_eq!(
            body,
            format!(
                "{{\"model\":\"provider/model\",\"messages\":[{{\"role\":\"user\",\"content\":\"{ciphertext}\"}}],\"stream\":false,\"max_tokens\":8192,\"temperature\":1e-05,\"response_format\":{{\"type\":\"json_object\"}},\"reasoning_effort\":\"medium\",\"tool_choice\":\"none\"}}"
            )
        );
        assert!(!body.contains("\\u00e9"));
        assert_eq!(
            hex::encode(Sha256::digest(&bytes)),
            "3f4a823af653ec0d07fd480eaecbe3bbbcec9ea9ad82ec9e6f3d8a29177610c3"
        );
        relay.response_format = Some(serde_json::json!({
            "type": "json_schema",
            "json_schema": {"description": "plaintext must not cross"},
        }));
        assert!(near_upstream_request_bytes(&offered_model, &relay).is_err());
    }

    #[test]
    fn upstream_request_matches_shared_backend_hash_vector() {
        #[derive(Deserialize)]
        struct Fixture {
            vectors: Vec<Vector>,
        }
        #[derive(Deserialize)]
        struct Vector {
            provider: String,
            protocol: String,
            body_utf8: String,
            sha256: String,
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../../../fixtures/request_hash_vectors/v1.json"
        ))
        .unwrap();
        let vector = fixture
            .vectors
            .iter()
            .find(|vector| vector.provider == "near" && vector.protocol == "near-v2")
            .unwrap();
        let mut offered_model = model();
        offered_model.upstream_model = "deepseek-ai/DeepSeek-V4-Flash".into();
        offered_model.max_output_tokens = 8_192;
        let relay = RelayChatRequest {
            client_request_id: None,
            thinking_mode: None,
            provider_id: offered_model.provider_id.clone(),
            inference_encryption: INFERENCE_ENCRYPTION,
            model_id: offered_model.id.clone(),
            encryption_version: offered_model.e2ee_encryption_version,
            e2ee_protocol: offered_model.e2ee_protocol.clone(),
            client_public_key_hex: "11".repeat(32),
            model_public_key_hex: "22".repeat(32),
            encrypted_messages: vec![
                EncryptedMessage {
                    role: "system",
                    encrypted_content: Some(CiphertextHex::new("ab".repeat(80)).unwrap()),
                    encrypted_reasoning_content: None,
                    encrypted_name: None,
                    encrypted_refusal: None,
                    encrypted_tool_calls: None,
                    tool_call_id: None,
                },
                EncryptedMessage {
                    role: "user",
                    encrypted_content: Some(CiphertextHex::new("cd".repeat(80)).unwrap()),
                    encrypted_reasoning_content: None,
                    encrypted_name: None,
                    encrypted_refusal: None,
                    encrypted_tool_calls: None,
                    tool_call_id: None,
                },
            ],
            encrypt_all_fields: true,
            encrypted_tools: None,
            encrypted_tool_choice: None,
            parallel_tool_calls: None,
            stream: true,
            max_tokens: Some(offered_model.max_output_tokens),
            sampling: Some(serde_json::json!({"temperature": 0.25, "top_p": 0.9})),
            response_format: Some(serde_json::json!({"type": "json_object"})),
            reasoning_effort: Some("medium".into()),
            provider_e2ee_context: None,
            attestation_generation: 1,
            verified_key_fingerprint: "33".repeat(32),
            verified_keyset_digest: None,
        };
        let bytes = near_upstream_request_bytes(&offered_model, &relay).unwrap();
        assert_eq!(bytes, vector.body_utf8.as_bytes());
        assert_eq!(hex::encode(Sha256::digest(&bytes)), vector.sha256);
    }

    fn response_signer() -> (SigningKey, String) {
        let key = SigningKey::from_bytes(&[7_u8; 32]);
        let address = hex::encode(key.verifying_key().to_bytes());
        (key, address)
    }

    fn signed_proof(
        raw_body: &[u8],
        request_hash: &str,
        chat_id: &str,
        signing_key: &SigningKey,
        signing_address: &str,
    ) -> serde_json::Value {
        let response_hash = hex::encode(Sha256::digest(raw_body));
        let signed_text = format!("{}:{request_hash}:{response_hash}", model().upstream_model);
        let signature_bytes = signing_key.sign(signed_text.as_bytes()).to_bytes();
        serde_json::json!({
            "provider": "near",
            "protocol": "near-v3",
            "chat_id": chat_id,
            "model": model().upstream_model,
            "request_hash": request_hash,
            "response_hash": response_hash,
            "response_body_base64": general_purpose::STANDARD.encode(raw_body),
            "signed_text": signed_text,
            "signature": hex::encode(signature_bytes),
            "signing_address": signing_address,
        })
    }

    #[test]
    fn relay_serialization_contains_no_secret_bearing_sentinels() {
        let recipient = crypto::ClientIdentity::generate();
        let recipient_key =
            crypto::Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        let sentinel = "SENTINEL-private-🌸";
        let request = InferenceRequest {
            request_id: None,
            thinking_mode: axiom_inference::ThinkingMode::ProviderDefault,
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
        let (_, relay, _) = build_request(
            &model(),
            recipient_key,
            &response_signer().1,
            &binding(),
            &request,
            false,
        )
        .unwrap();
        let json = serde_json::to_string(&relay).unwrap();
        let wire: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(wire["provider_id"], model().provider_id);
        assert_eq!(wire["inference_encryption"], "provider_e2ee_v2");
        assert!(!json.contains(sentinel));
        assert!(!json.contains("sentinel_tool"));
        assert!(json.contains("encrypted_messages"));
        assert!(json.contains("encrypt_all_fields"));
        assert!(json.contains("\"attestation_generation\":1"));
        assert!(json.contains("\"verified_key_fingerprint\""));
        assert!(json.contains("\"verified_keyset_digest\":null"));
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
        let (_, relay, _) = build_request(
            &non_reasoning,
            recipient_key,
            &response_signer().1,
            &binding(),
            &request,
            false,
        )
        .unwrap();
        assert!(relay.reasoning_effort.is_none());
        assert_eq!(relay.max_tokens, Some(non_reasoning.max_output_tokens));

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
                images: Vec::new(),
                files: Vec::new(),
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
                images: Vec::new(),
                files: Vec::new(),
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
        let encrypted_content = encrypt(client_key, b"finished").unwrap();
        let encrypted_reasoning = encrypt(client_key, b"reasoned").unwrap();
        let raw_body = serde_json::to_vec(&serde_json::json!({
            "id": "chat-1",
            "model": "provider/model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": encrypted_content.as_str(),
                    "reasoning_content": encrypted_reasoning.as_str(),
                },
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 4,
                "total_tokens": 14,
            },
        }))
        .unwrap();
        let request_hash = "11".repeat(32);
        let (signing_key, signing_address) = response_signer();
        let proof = signed_proof(
            &raw_body,
            &request_hash,
            "chat-1",
            &signing_key,
            &signing_address,
        );
        let completion = RelayCompletion {
            id: "chat-1".to_owned(),
            model: Some("provider/model".to_owned()),
            encrypted_content: Some(encrypted_content),
            encrypted_reasoning_content: Some(encrypted_reasoning),
            encrypted_refusal: None,
            encrypted_tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            usage: RelayUsage {
                prompt_tokens: Some(10),
                completion_tokens: Some(4),
                total_tokens: Some(14),
                cached_prompt_tokens: None,
            },
            proof: Some(proof.clone()),
        };
        let response = decrypt_completion(
            &model(),
            &identity,
            &request_hash,
            &signing_address,
            completion,
        )
        .unwrap();
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
            id: "chat-1".to_owned(),
            model: Some("provider/model".to_owned()),
            encrypted_content: Some(CiphertextHex::new(tampered).unwrap()),
            encrypted_reasoning_content: None,
            encrypted_refusal: None,
            encrypted_tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            usage: RelayUsage::default(),
            proof: Some(proof),
        };
        assert!(
            decrypt_completion(
                &model(),
                &identity,
                &request_hash,
                &signing_address,
                tampered,
            )
            .is_err()
        );
    }

    #[test]
    fn signed_receipt_rejects_every_critical_binding_mutation() {
        let raw_body = br#"{"id":"chat-1","choices":[]}"#;
        let request_hash = "44".repeat(32);
        let (signing_key, signing_address) = response_signer();
        let proof = signed_proof(
            raw_body,
            &request_hash,
            "chat-1",
            &signing_key,
            &signing_address,
        );
        assert!(
            verify_near_proof(
                proof.clone(),
                &request_hash,
                &signing_address,
                &model().upstream_model,
            )
            .is_ok()
        );

        for (field, replacement) in [
            ("provider", serde_json::json!("other")),
            ("protocol", serde_json::json!("near-v2")),
            ("model", serde_json::json!("other/model")),
            ("request_hash", serde_json::json!("55".repeat(32))),
            ("response_hash", serde_json::json!("66".repeat(32))),
            ("response_body_base64", serde_json::json!("e30=")),
            ("signed_text", serde_json::json!("unsigned")),
            (
                "signing_address",
                serde_json::json!(format!("0x{}", "00".repeat(20))),
            ),
        ] {
            let mut candidate = proof.clone();
            candidate[field] = replacement;
            assert!(
                verify_near_proof(
                    candidate,
                    &request_hash,
                    &signing_address,
                    &model().upstream_model,
                )
                .is_err(),
                "mutated {field} was accepted"
            );
        }

        let mut signature = proof.clone();
        let encoded = signature["signature"].as_str().unwrap();
        let mut changed = encoded.to_owned();
        changed.replace_range(10..12, if &encoded[10..12] == "00" { "01" } else { "00" });
        signature["signature"] = serde_json::json!(changed);
        assert!(
            verify_near_proof(
                signature,
                &request_hash,
                &signing_address,
                &model().upstream_model,
            )
            .is_err()
        );

        let mut extra = proof;
        extra["unexpected"] = serde_json::json!(true);
        assert!(
            verify_near_proof(
                extra,
                &request_hash,
                &signing_address,
                &model().upstream_model,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn large_encrypted_stream_verifies_chunked_receipt_and_replays_in_next_request() {
        let identity = crypto::ClientIdentity::generate();
        let key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let text = "a".repeat(3 * 1024 * 1024);
        let encrypted = encrypt(key, text.as_bytes()).unwrap();
        let raw = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({
                "id":"chat-1", "model":"provider/model",
                "choices":[{"delta":{"content":encrypted.as_str()},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":2,"completion_tokens":8,"total_tokens":10}
            })
        );
        assert!(raw.len() > 2 * 1024 * 1024);
        let hash = "55".repeat(32);
        let (signer, address) = response_signer();
        let mut proof = signed_proof(raw.as_bytes(), &hash, "chat-1", &signer, &address);
        let encoded = proof
            .as_object_mut()
            .unwrap()
            .remove("response_body_base64")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let limits = crate::SecureClientLimits::default();
        let mut decoder = SseDecoder::new(limits.relay_sse_event_bytes, limits.relay_stream_bytes);
        let mut state = StreamState::new(hash, address, model().upstream_model);
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let mut frames = vec![
            (
                "run.created",
                serde_json::json!({"run_id":"r","inference_encryption":INFERENCE_ENCRYPTION}),
            ),
            (
                "message.encrypted_delta",
                serde_json::json!({"run_id":"r","sequence":1,"encrypted_delta":encrypted.as_str()}),
            ),
        ];
        for (index, chunk) in encoded.as_bytes().chunks(256 * 1024).enumerate() {
            frames.push(("message.proof_chunk", serde_json::json!({"run_id":"r","index":index,"data":std::str::from_utf8(chunk).unwrap()})));
        }
        proof["response_body_chunk_count"] = serde_json::json!(encoded.len().div_ceil(256 * 1024));
        frames.push((
            "message.encrypted_completed",
            serde_json::json!({"run_id":"r","finish_reason":"stop",
            "usage":{"prompt_tokens":2,"completion_tokens":8,"total_tokens":10},"proof":proof}),
        ));
        frames.push(("run.completed", serde_json::json!({"run_id":"r"})));
        for (name, value) in frames {
            let frame = format!("event: {name}\ndata: {value}\n\n");
            for chunk in frame.as_bytes().chunks(8191) {
                for event in decoder.feed(chunk).unwrap() {
                    state.accept(event, &identity, &tx, &cancel).await.unwrap();
                }
            }
        }
        decoder.finish().unwrap();
        let (response, _) = state.finish().unwrap();
        assert_eq!(response.assistant.text, text);
        assert!(rx.try_recv().is_ok());
        let request = InferenceRequest::streaming(
            "secure-model",
            vec![
                ChatMessage::text(ChatRole::Assistant, response.assistant.text),
                ChatMessage::text(ChatRole::User, "Continue."),
            ],
            vec![],
        );
        request.validate().unwrap();
        let (_, relay, _) = build_request(
            &model(),
            key,
            &response_signer().1,
            &binding(),
            &request,
            true,
        )
        .unwrap();
        assert!(
            relay
                .serialize_bounded(limits.serialized_relay_request_bytes)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn stream_requires_receipt_before_run_completion() {
        let identity = crypto::ClientIdentity::generate();
        let (sink, _output) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let mut state =
            StreamState::new("77".repeat(32), response_signer().1, model().upstream_model);
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
        assert!(
            state
                .accept(
                    RelayEvent::MessageCompleted(CompletedPayload {
                        run_id: "run-1".to_owned(),
                        usage: RelayUsage::default(),
                        finish_reason: "stop".to_owned(),
                        proof: None,
                    }),
                    &identity,
                    &sink,
                    &cancellation,
                )
                .await
                .is_err()
        );

        let mut state =
            StreamState::new("77".repeat(32), response_signer().1, model().upstream_model);
        state.run_id = Some("run-1".to_owned());
        assert!(
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
                .is_err()
        );
    }

    #[tokio::test]
    async fn assembles_ordered_stream_content_and_tool_calls() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let (sink, mut output) = mpsc::channel(16);
        let cancellation = CancellationToken::new();
        let request_hash = "22".repeat(32);
        let (signing_key, signing_address) = response_signer();
        let content_one = encrypt(client_key, b"working ").unwrap();
        let name_one = encrypt(client_key, b"read_").unwrap();
        let arguments_one = encrypt(client_key, b"{\"path\":").unwrap();
        let content_two = encrypt(client_key, b"done").unwrap();
        let name_two = encrypt(client_key, b"file").unwrap();
        let arguments_two = encrypt(client_key, b"\"a.rs\"}").unwrap();
        let raw_stream = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::to_string(&serde_json::json!({
                "id": "chat-1",
                "model": "provider/model",
                "choices": [{"delta": {
                    "content": content_one.as_str(),
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": name_one.as_str(),
                            "arguments": arguments_one.as_str(),
                        },
                    }],
                }}],
            }))
            .unwrap(),
            serde_json::to_string(&serde_json::json!({
                "id": "chat-1",
                "model": "provider/model",
                "choices": [{
                    "delta": {
                        "content": content_two.as_str(),
                        "tool_calls": [{
                            "index": 0,
                            "function": {
                                "name": name_two.as_str(),
                                "arguments": arguments_two.as_str(),
                            },
                        }],
                    },
                    "finish_reason": "tool_calls",
                }],
                "usage": {
                    "prompt_tokens": 8,
                    "completion_tokens": 5,
                    "total_tokens": 13,
                },
            }))
            .unwrap(),
        );
        let proof = signed_proof(
            raw_stream.as_bytes(),
            &request_hash,
            "chat-1",
            &signing_key,
            &signing_address,
        );
        let mut state = StreamState::new(request_hash, signing_address, model().upstream_model);
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
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::Status { .. })
        ));
        state
            .accept(
                RelayEvent::Delta(DeltaPayload {
                    run_id: "run-1".to_owned(),
                    encrypted_delta: Some(content_one),
                    encrypted_reasoning_delta: None,
                    encrypted_refusal_delta: None,
                    encrypted_tool_calls: vec![EncryptedToolCallDelta {
                        index: 0,
                        id: Some("call-1".to_owned()),
                        kind: Some("function".to_owned()),
                        function: Some(EncryptedFunctionCallDelta {
                            encrypted_name: Some(name_one),
                            encrypted_arguments: Some(arguments_one),
                        }),
                    }],
                    sequence: 1,
                    response_id: None,
                    content_field: None,
                    reasoning_field: None,
                    refusal_field: None,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(matches!(
            output.try_recv(),
            Ok(ProviderEvent::TextDelta(value)) if value == "working "
        ));
        assert!(output.try_recv().is_err(), "tools must await verification");
        state
            .accept(
                RelayEvent::Delta(DeltaPayload {
                    run_id: "run-1".to_owned(),
                    encrypted_delta: Some(content_two),
                    encrypted_reasoning_delta: None,
                    encrypted_refusal_delta: None,
                    encrypted_tool_calls: vec![EncryptedToolCallDelta {
                        index: 0,
                        id: None,
                        kind: None,
                        function: Some(EncryptedFunctionCallDelta {
                            encrypted_name: Some(name_two),
                            encrypted_arguments: Some(arguments_two),
                        }),
                    }],
                    sequence: 2,
                    response_id: None,
                    content_field: None,
                    reasoning_field: None,
                    refusal_field: None,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(matches!(
            output.try_recv(),
            Ok(ProviderEvent::TextDelta(value)) if value == "done"
        ));
        assert!(output.try_recv().is_err(), "tools must await verification");
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
                    proof: Some(proof),
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(
            output.try_recv().is_err(),
            "receipt alone released output before terminal run completion"
        );
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
        let (response, verified_events) = state.finish().unwrap();
        assert_eq!(response.assistant.text, "working done");
        assert_eq!(response.assistant.tool_calls[0].function.name, "read_file");
        assert_eq!(
            response.assistant.tool_calls[0].function.arguments,
            "{\"path\":\"a.rs\"}"
        );
        assert_eq!(response.usage.total_tokens, 13);
        assert!(output.try_recv().is_err());
        emit_verified_events(verified_events, &sink, &cancellation)
            .await
            .unwrap();
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::ToolCallDelta(_))
        ));
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::ToolCallDelta(_))
        ));
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::Usage {
                input_tokens: 8,
                output_tokens: 5
            })
        ));
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::Finished(FinishReason::ToolCalls))
        ));
        assert!(output.try_recv().is_err());
    }

    #[tokio::test]
    async fn invalid_or_missing_receipt_never_completes_provisional_output() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let (sink, mut output) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let request_hash = "44".repeat(32);
        let (_, signing_address) = response_signer();
        let mut state = StreamState::new(
            request_hash.clone(),
            signing_address.clone(),
            model().upstream_model,
        );
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
        assert!(matches!(
            output.recv().await,
            Some(ProviderEvent::Status { .. })
        ));
        state
            .accept(
                RelayEvent::Delta(DeltaPayload {
                    run_id: "run-1".to_owned(),
                    encrypted_delta: Some(encrypt(client_key, b"provisional secret").unwrap()),
                    encrypted_reasoning_delta: None,
                    encrypted_refusal_delta: None,
                    encrypted_tool_calls: Vec::new(),
                    sequence: 1,
                    response_id: Some("chat-1".to_owned()),
                    content_field: None,
                    reasoning_field: None,
                    refusal_field: None,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(matches!(
            output.try_recv(),
            Ok(ProviderEvent::TextDelta(value)) if value == "provisional secret"
        ));
        assert!(output.try_recv().is_err());

        let missing = state
            .accept(
                RelayEvent::MessageCompleted(CompletedPayload {
                    run_id: "run-1".to_owned(),
                    usage: RelayUsage::default(),
                    finish_reason: "stop".to_owned(),
                    proof: None,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await;
        assert!(missing.is_err());
        assert!(output.try_recv().is_err());

        let (wrong_key, wrong_address) = response_signer();
        let raw_stream = "data: [DONE]\n\n";
        let invalid_proof = signed_proof(
            raw_stream.as_bytes(),
            &request_hash,
            "chat-1",
            &wrong_key,
            &wrong_address,
        );
        let mut invalid_state =
            StreamState::new(request_hash, signing_address, model().upstream_model);
        invalid_state.run_id = Some("run-1".to_owned());
        invalid_state.response_id = Some("chat-1".to_owned());
        invalid_state.next_sequence = Some(2);
        invalid_state.text = "provisional secret".to_owned();
        let invalid = invalid_state
            .accept(
                RelayEvent::MessageCompleted(CompletedPayload {
                    run_id: "run-1".to_owned(),
                    usage: RelayUsage::default(),
                    finish_reason: "stop".to_owned(),
                    proof: Some(invalid_proof),
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await;
        assert!(invalid.is_err());
        assert!(output.try_recv().is_err());
    }

    #[tokio::test]
    async fn interruption_and_cancellation_never_complete_provisional_output() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let (sink, mut output) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let mut state =
            StreamState::new("33".repeat(32), response_signer().1, model().upstream_model);
        state.run_id = Some("run-1".to_owned());
        state
            .accept(
                RelayEvent::Delta(DeltaPayload {
                    run_id: "run-1".to_owned(),
                    encrypted_delta: Some(encrypt(client_key, b"partial answer").unwrap()),
                    encrypted_reasoning_delta: None,
                    encrypted_refusal_delta: None,
                    encrypted_tool_calls: Vec::new(),
                    sequence: 1,
                    response_id: Some("chat-1".to_owned()),
                    content_field: None,
                    reasoning_field: None,
                    refusal_field: None,
                }),
                &identity,
                &sink,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(matches!(
            output.try_recv(),
            Ok(ProviderEvent::TextDelta(value)) if value == "partial answer"
        ));
        assert!(
            state.finish().is_err(),
            "an interrupted stream was accepted"
        );
        assert!(output.try_recv().is_err());

        cancellation.cancel();
        assert!(
            emit_verified_events(
                vec![ProviderEvent::TextDelta("never publish".to_owned())],
                &sink,
                &cancellation,
            )
            .await
            .is_err()
        );
        assert!(output.try_recv().is_err());
    }

    #[tokio::test]
    async fn stream_rejects_out_of_order_and_incomplete_terminal_state() {
        let identity = crypto::ClientIdentity::generate();
        let client_key = crypto::Ed25519PublicKey::from_hex(&identity.public_key_hex()).unwrap();
        let (sink, _output) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let mut state =
            StreamState::new("33".repeat(32), response_signer().1, model().upstream_model);
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
                        response_id: Some("chat-1".to_owned()),
                        content_field: None,
                        reasoning_field: None,
                        refusal_field: None,
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

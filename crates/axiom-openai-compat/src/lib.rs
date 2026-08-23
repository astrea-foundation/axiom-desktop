//! Pure OpenAI-compatible wire translation for Axiom inference.
//!
//! This crate does not perform HTTP, attestation, encryption, environment
//! access, tool execution, or authorization.

use axiom_inference::{
    AssistantTurn, ChatMessage, ChatRole, FinishReason, FunctionCall, FunctionDefinition,
    InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent, ReasoningEffort, ResponseFormat,
    SamplingParameters, ToolCall, ToolCallDelta, ToolChoice, ToolDefinition, Usage,
};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, CompatError>;

#[derive(Debug, thiserror::Error)]
#[error("invalid OpenAI-compatible request: {detail}")]
pub struct CompatError {
    detail: &'static str,
}

impl CompatError {
    const fn new(detail: &'static str) -> Self {
        Self { detail }
    }

    #[must_use]
    pub const fn safe_detail(&self) -> &'static str {
        self.detail
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<WireMessage>,
    #[serde(default)]
    pub tools: Vec<WireTool>,
    #[serde(default)]
    pub tool_choice: Option<WireToolChoice>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_completion_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub response_format: Option<WireResponseFormat>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
}

impl ChatCompletionRequest {
    pub fn into_domain(self) -> Result<InferenceRequest> {
        if self.max_tokens.is_some()
            && self.max_completion_tokens.is_some()
            && self.max_tokens != self.max_completion_tokens
        {
            return Err(CompatError::new(
                "max_tokens and max_completion_tokens conflict",
            ));
        }
        if self.stream_options.is_some() && !self.stream {
            return Err(CompatError::new("stream_options requires stream=true"));
        }
        let reasoning_effort = parse_reasoning(self.reasoning_effort.as_deref())?;
        let tools = self
            .tools
            .into_iter()
            .map(WireTool::into_domain)
            .collect::<Result<Vec<_>>>()?;
        let request = InferenceRequest {
            model: self.model,
            messages: self
                .messages
                .into_iter()
                .map(WireMessage::into_domain)
                .collect::<Result<Vec<_>>>()?,
            tools,
            tool_choice: self
                .tool_choice
                .map_or(Ok(ToolChoice::Auto), WireToolChoice::into_domain)?,
            parallel_tool_calls: self.parallel_tool_calls,
            reasoning_effort,
            max_output_tokens: self.max_completion_tokens.or(self.max_tokens),
            sampling: SamplingParameters {
                temperature: self.temperature,
                top_p: self.top_p,
            },
            response_format: self
                .response_format
                .map_or(ResponseFormat::Text, WireResponseFormat::into_domain),
            stream: self.stream,
        };
        request
            .validate()
            .map_err(|_| CompatError::new("request fields failed validation"))?;
        Ok(request)
    }

    #[must_use]
    pub fn include_stream_usage(&self) -> bool {
        self.stream_options
            .as_ref()
            .is_some_and(|options| options.include_usage)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<WireToolCall>,
}

impl WireMessage {
    fn into_domain(self) -> Result<ChatMessage> {
        let role = match self.role.as_str() {
            "system" => ChatRole::System,
            "developer" => ChatRole::Developer,
            "user" => ChatRole::User,
            "assistant" => ChatRole::Assistant,
            "tool" => ChatRole::Tool,
            _ => return Err(CompatError::new("message role is not supported")),
        };
        Ok(ChatMessage {
            role,
            content: self.content.unwrap_or_default(),
            reasoning_content: self.reasoning_content,
            name: self.name,
            refusal: self.refusal,
            tool_call_id: self.tool_call_id,
            tool_calls: self
                .tool_calls
                .into_iter()
                .map(WireToolCall::into_domain)
                .collect::<Result<Vec<_>>>()?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunctionDefinition,
}

impl WireTool {
    fn into_domain(self) -> Result<ToolDefinition> {
        if self.kind != "function" {
            return Err(CompatError::new("only function tools are supported"));
        }
        Ok(ToolDefinition {
            kind: self.kind,
            function: FunctionDefinition {
                name: self.function.name,
                description: self.function.description.unwrap_or_default(),
                parameters: self.function.parameters,
                strict: self.function.strict,
            },
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireFunctionDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "empty_schema")]
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub strict: Option<bool>,
}

fn empty_schema() -> serde_json::Value {
    serde_json::json!({"type":"object","properties":{}})
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum WireToolChoice {
    Mode(String),
    Named(WireNamedToolChoice),
}

impl WireToolChoice {
    fn into_domain(self) -> Result<ToolChoice> {
        match self {
            Self::Mode(mode) => match mode.as_str() {
                "auto" => Ok(ToolChoice::Auto),
                "none" => Ok(ToolChoice::None),
                "required" => Ok(ToolChoice::Required),
                _ => Err(CompatError::new("tool_choice mode is not supported")),
            },
            Self::Named(choice) if choice.kind == "function" => Ok(ToolChoice::Named {
                name: choice.function.name,
            }),
            Self::Named(_) => Err(CompatError::new("named tool_choice type is invalid")),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireNamedToolChoice {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireNamedFunction,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireNamedFunction {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunctionCall,
}

impl WireToolCall {
    fn into_domain(self) -> Result<ToolCall> {
        if self.kind != "function" {
            return Err(CompatError::new("tool call type is invalid"));
        }
        Ok(ToolCall {
            id: self.id,
            kind: self.kind,
            function: FunctionCall {
                name: self.function.name,
                arguments: self.function.arguments,
            },
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: WireJsonSchema },
}

impl WireResponseFormat {
    fn into_domain(self) -> ResponseFormat {
        match self {
            Self::Text => ResponseFormat::Text,
            Self::JsonObject => ResponseFormat::JsonObject,
            Self::JsonSchema { json_schema } => ResponseFormat::JsonSchema {
                name: json_schema.name,
                schema: json_schema.schema,
                strict: json_schema.strict,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireJsonSchema {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(default)]
    pub strict: bool,
}

fn parse_reasoning(value: Option<&str>) -> Result<ReasoningEffort> {
    match value.unwrap_or("medium") {
        "minimal" => Ok(ReasoningEffort::Minimal),
        "low" => Ok(ReasoningEffort::Low),
        "medium" => Ok(ReasoningEffort::Medium),
        "high" => Ok(ReasoningEffort::High),
        "xhigh" => Ok(ReasoningEffort::ExtraHigh),
        _ => Err(CompatError::new("reasoning_effort is not supported")),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<WireModel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireModel {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: String,
}

#[must_use]
pub fn model_list(models: &[ModelInfo]) -> ModelList {
    ModelList {
        object: "list",
        data: models
            .iter()
            .map(|model| WireModel {
                id: model.id.clone(),
                object: "model",
                created: 0,
                owned_by: model.provider_label.clone(),
            })
            .collect(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatCompletion {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: WireUsage,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompletionChoice {
    pub index: usize,
    pub message: AssistantMessage,
    pub finish_reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantMessage {
    pub role: &'static str,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ResponseToolCall>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResponseToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ResponseFunctionCall,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResponseFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct WireUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl From<&Usage> for WireUsage {
    fn from(value: &Usage) -> Self {
        Self {
            prompt_tokens: value.input_tokens,
            completion_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
        }
    }
}

#[must_use]
pub fn completion(
    id: String,
    model: String,
    created: u64,
    response: InferenceResponse,
) -> ChatCompletion {
    let finish_reason = response
        .finish_reason
        .as_ref()
        .map_or_else(|| "stop".to_owned(), finish_reason_name);
    ChatCompletion {
        id,
        object: "chat.completion",
        created,
        model,
        choices: vec![CompletionChoice {
            index: 0,
            message: assistant_message(response.assistant, response.reasoning, response.refusal),
            finish_reason,
        }],
        usage: WireUsage::from(&response.usage),
    }
}

fn assistant_message(
    assistant: AssistantTurn,
    reasoning: Option<String>,
    refusal: Option<String>,
) -> AssistantMessage {
    AssistantMessage {
        role: "assistant",
        content: (!assistant.text.is_empty()).then_some(assistant.text),
        reasoning_content: reasoning,
        refusal,
        tool_calls: assistant
            .tool_calls
            .into_iter()
            .map(|call| ResponseToolCall {
                id: call.id,
                kind: call.kind,
                function: ResponseFunctionCall {
                    name: call.function.name,
                    arguments: call.function.arguments,
                },
            })
            .collect(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatChunk {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<WireUsage>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChunkChoice {
    pub index: usize,
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChunkToolCall>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChunkToolCall {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<ChunkFunctionCall>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChunkFunctionCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[must_use]
pub fn stream_start(id: &str, model: &str, created: u64) -> ChatChunk {
    chunk(
        id,
        model,
        created,
        ChunkDelta {
            role: Some("assistant"),
            ..ChunkDelta::default()
        },
        None,
        None,
    )
}

#[must_use]
pub fn stream_event(
    id: &str,
    model: &str,
    created: u64,
    event: ProviderEvent,
    include_usage: bool,
) -> Option<ChatChunk> {
    match event {
        ProviderEvent::TextDelta(value) => Some(chunk(
            id,
            model,
            created,
            ChunkDelta {
                content: Some(value),
                ..ChunkDelta::default()
            },
            None,
            None,
        )),
        ProviderEvent::ReasoningDelta(value) => Some(chunk(
            id,
            model,
            created,
            ChunkDelta {
                reasoning_content: Some(value),
                ..ChunkDelta::default()
            },
            None,
            None,
        )),
        ProviderEvent::RefusalDelta(value) => Some(chunk(
            id,
            model,
            created,
            ChunkDelta {
                refusal: Some(value),
                ..ChunkDelta::default()
            },
            None,
            None,
        )),
        ProviderEvent::ToolCallDelta(delta) => Some(tool_chunk(id, model, created, delta)),
        ProviderEvent::Usage {
            input_tokens,
            output_tokens,
        } if include_usage => Some(chunk(
            id,
            model,
            created,
            ChunkDelta::default(),
            None,
            Some(WireUsage {
                prompt_tokens: input_tokens,
                completion_tokens: output_tokens,
                total_tokens: input_tokens.saturating_add(output_tokens),
            }),
        )),
        ProviderEvent::Finished(reason) => Some(chunk(
            id,
            model,
            created,
            ChunkDelta::default(),
            Some(finish_reason_name(&reason)),
            None,
        )),
        ProviderEvent::SecurityState(_)
        | ProviderEvent::Status { .. }
        | ProviderEvent::Usage { .. } => None,
    }
}

fn tool_chunk(id: &str, model: &str, created: u64, delta: ToolCallDelta) -> ChatChunk {
    chunk(
        id,
        model,
        created,
        ChunkDelta {
            tool_calls: vec![ChunkToolCall {
                index: delta.index,
                id: delta.id,
                kind: delta.kind,
                function: (delta.name.is_some() || delta.arguments.is_some()).then_some(
                    ChunkFunctionCall {
                        name: delta.name,
                        arguments: delta.arguments,
                    },
                ),
            }],
            ..ChunkDelta::default()
        },
        None,
        None,
    )
}

fn chunk(
    id: &str,
    model: &str,
    created: u64,
    delta: ChunkDelta,
    finish_reason: Option<String>,
    usage: Option<WireUsage>,
) -> ChatChunk {
    ChatChunk {
        id: id.to_owned(),
        object: "chat.completion.chunk",
        created,
        model: model.to_owned(),
        choices: vec![ChunkChoice {
            index: 0,
            delta,
            finish_reason,
        }],
        usage,
    }
}

fn finish_reason_name(reason: &FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop".to_owned(),
        FinishReason::Length => "length".to_owned(),
        FinishReason::ToolCalls => "tool_calls".to_owned(),
        FinishReason::ContentFilter => "content_filter".to_owned(),
        FinishReason::Cancelled => "cancelled".to_owned(),
        FinishReason::Other(value) => value.clone(),
    }
}

pub fn encode_sse<T: Serialize>(value: &T) -> Result<String> {
    let json = serde_json::to_string(value)
        .map_err(|_| CompatError::new("stream chunk serialization failed"))?;
    Ok(format!("data: {json}\n\n"))
}

#[must_use]
pub fn done_sse() -> &'static str {
    "data: [DONE]\n\n"
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorBody {
    pub message: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub code: &'static str,
}

#[must_use]
pub const fn error_envelope(
    message: &'static str,
    kind: &'static str,
    code: &'static str,
) -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message,
            kind,
            code,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_messages_tools_choice_reasoning_and_response_format_losslessly() {
        let wire: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
            "model": "cherry",
            "messages": [
                {"role":"developer","content":"rules"},
                {"role":"assistant","content":null,"tool_calls":[{
                    "id":"call:1","type":"function","function":{"name":"read","arguments":"{\"p\":\"a\"}"}
                }]},
                {"role":"tool","tool_call_id":"call:1","content":"result"}
            ],
            "tools":[{"type":"function","function":{
                "name":"read","description":"read it","parameters":{"type":"object"},"strict":true
            }}],
            "tool_choice":{"type":"function","function":{"name":"read"}},
            "parallel_tool_calls":false,
            "reasoning_effort":"high",
            "max_completion_tokens":1024,
            "temperature":0.2,
            "response_format":{"type":"json_schema","json_schema":{
                "name":"answer","schema":{"type":"object"},"strict":true
            }},
            "stream":true,
            "stream_options":{"include_usage":true}
        }))
        .unwrap();
        assert!(wire.include_stream_usage());
        let domain = wire.into_domain().unwrap();
        assert_eq!(domain.messages[0].role, ChatRole::Developer);
        assert_eq!(domain.messages[1].tool_calls[0].id, "call:1");
        assert_eq!(domain.reasoning_effort, ReasoningEffort::High);
        assert_eq!(domain.max_output_tokens, Some(1024));
        assert!(matches!(domain.tool_choice, ToolChoice::Named { .. }));
        assert!(matches!(
            domain.response_format,
            ResponseFormat::JsonSchema { .. }
        ));
    }

    #[test]
    fn rejects_unknown_fields_conflicts_and_legacy_shapes() {
        assert!(
            serde_json::from_value::<ChatCompletionRequest>(serde_json::json!({
                "model":"x","messages":[],"prompt":"legacy"
            }))
            .is_err()
        );
        let wire: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
            "model":"x","messages":[{"role":"user","content":"hi"}],
            "max_tokens":1,"max_completion_tokens":2
        }))
        .unwrap();
        assert!(wire.into_domain().is_err());
    }

    #[test]
    fn emits_openai_nonstream_stream_usage_tools_and_done_shapes() {
        let response = InferenceResponse {
            assistant: AssistantTurn {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_owned(),
                    kind: "function".to_owned(),
                    function: FunctionCall {
                        name: "read".to_owned(),
                        arguments: "{}".to_owned(),
                    },
                }],
            },
            reasoning: Some("thinking".to_owned()),
            refusal: None,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 3,
                total_tokens: 13,
            },
            finish_reason: Some(FinishReason::ToolCalls),
        };
        let value = serde_json::to_value(completion(
            "chatcmpl-1".to_owned(),
            "cherry".to_owned(),
            1,
            response,
        ))
        .unwrap();
        assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(value["usage"]["total_tokens"], 13);
        let event = ProviderEvent::ToolCallDelta(ToolCallDelta {
            index: 0,
            id: Some("call-1".to_owned()),
            kind: Some("function".to_owned()),
            name: Some("read".to_owned()),
            arguments: Some("{}".to_owned()),
        });
        let encoded = encode_sse(&stream_event("id", "cherry", 1, event, true).unwrap()).unwrap();
        assert!(encoded.starts_with("data: {"));
        assert_eq!(done_sse(), "data: [DONE]\n\n");
    }
}

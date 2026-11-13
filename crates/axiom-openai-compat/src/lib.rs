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
    parameters: Vec<String>,
}

impl CompatError {
    const fn new(detail: &'static str) -> Self {
        Self {
            detail,
            parameters: Vec::new(),
        }
    }

    fn unsupported(parameters: Vec<String>) -> Self {
        Self {
            detail: "request contains parameters this proxy does not support",
            parameters,
        }
    }

    #[must_use]
    pub const fn safe_detail(&self) -> &'static str {
        self.detail
    }

    /// Sanitized names of the parameters that caused a strict-mode rejection.
    ///
    /// Empty for every other translation failure.
    #[must_use]
    pub fn parameters(&self) -> &[String] {
        &self.parameters
    }
}

/// How the translator treats request parameters it cannot honor.
///
/// Axiom's relay accepts a fixed field set, so a parameter with no relay
/// equivalent can only be dropped or refused. The composition root chooses;
/// this crate never reads configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompatMode {
    /// Drop unsupported parameters and report the ones that could have changed
    /// the answer. A request from an application the operator cannot modify
    /// still succeeds.
    #[default]
    Lenient,
    /// Refuse any request carrying a parameter that cannot be honored.
    Strict,
}

/// Parameters that cannot reach the relay but also cannot change the answer.
///
/// Dropping these is invisible, so lenient mode does not report them.
const INERT_PARAMETERS: [&str; 4] = ["user", "metadata", "store", "service_tier"];

/// Upper bound on reported parameter names, keeping the response header and
/// the supervisor log bounded regardless of request content.
const MAX_REPORTED_PARAMETERS: usize = 16;
const MAX_REPORTED_PARAMETER_LEN: usize = 64;

/// Reduces a client-supplied key to a token that is safe to place in a response
/// header and a structured log line.
///
/// Returns `None` when nothing usable survives.
fn sanitize_parameter_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(MAX_REPORTED_PARAMETER_LEN)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// A translated request plus the parameters that were dropped to produce it.
#[derive(Clone, Debug)]
pub struct TranslatedRequest {
    pub request: InferenceRequest,
    /// Sorted, deduplicated, sanitized names of dropped parameters that could
    /// have changed the answer. Empty when the request was honored in full.
    pub ignored_parameters: Vec<String>,
}

/// An OpenAI-compatible chat completion request.
///
/// Unknown keys are captured rather than refused at parse time so that
/// [`CompatMode`] can decide their fate. This applies recursively: lenient
/// mode can survive additive SDK metadata while strict mode still refuses any
/// field that cannot be represented faithfully.
#[derive(Clone, Debug, Deserialize)]
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
    pub thinking_mode: axiom_inference::ThinkingMode,
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
    /// Requested completion count. Only `1` is representable, so anything
    /// greater is refused in every mode rather than silently under-delivered.
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

impl ChatCompletionRequest {
    /// Names the captured keys that lenient mode must report.
    ///
    /// Everything outside [`INERT_PARAMETERS`] is reported: both parameters
    /// `OpenAI` defines that the relay cannot honor (`stop`, `seed`,
    /// `frequency_penalty`, `presence_penalty`, `logit_bias`, `logprobs`,
    /// `top_logprobs`) and keys this build does not recognize at all, which may
    /// be a caller typo or a newer API revision.
    fn classify_parameters(&self) -> Vec<String> {
        let mut reported = Vec::new();
        for key in self.unsupported.keys() {
            if INERT_PARAMETERS.contains(&key.as_str()) {
                continue;
            }
            if let Some(name) = sanitize_parameter_name(key) {
                reported.push(name);
            }
        }
        reported.sort_unstable();
        reported.dedup();
        reported.truncate(MAX_REPORTED_PARAMETERS);
        reported
    }

    fn nested_parameters(&self) -> Vec<String> {
        let mut reported = Vec::new();
        collect_keys(
            "stream_options",
            self.stream_options.as_ref().map(|value| &value.unsupported),
            &mut reported,
        );
        for message in &self.messages {
            collect_keys("messages", Some(&message.unsupported), &mut reported);
            if let Some(content) = &message.content {
                content.collect_parameters(&mut reported);
            }
            for call in &message.tool_calls {
                collect_keys(
                    "messages.tool_calls",
                    Some(&call.unsupported),
                    &mut reported,
                );
                collect_keys(
                    "messages.tool_calls.function",
                    Some(&call.function.unsupported),
                    &mut reported,
                );
            }
        }
        for tool in &self.tools {
            collect_keys("tools", Some(&tool.unsupported), &mut reported);
            collect_keys(
                "tools.function",
                Some(&tool.function.unsupported),
                &mut reported,
            );
        }
        if let Some(WireToolChoice::Named(choice)) = &self.tool_choice {
            collect_keys("tool_choice", Some(&choice.unsupported), &mut reported);
            collect_keys(
                "tool_choice.function",
                Some(&choice.function.unsupported),
                &mut reported,
            );
        }
        if let Some(format) = &self.response_format {
            format.collect_parameters(&mut reported);
        }
        reported.sort_unstable();
        reported.dedup();
        reported.truncate(MAX_REPORTED_PARAMETERS);
        reported
    }

    pub fn into_domain(self, mode: CompatMode) -> Result<TranslatedRequest> {
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
        if self.n.is_some_and(|count| count != 1) {
            return Err(CompatError::new(
                "n must be 1: additional choices cannot be produced",
            ));
        }
        let nested_parameters = self.nested_parameters();
        let mut ignored_parameters = self.classify_parameters();
        ignored_parameters.extend(nested_parameters.iter().cloned());
        ignored_parameters.sort_unstable();
        ignored_parameters.dedup();
        ignored_parameters.truncate(MAX_REPORTED_PARAMETERS);
        if mode == CompatMode::Strict
            && (!self.unsupported.is_empty() || !nested_parameters.is_empty())
        {
            let mut refused: Vec<String> = self
                .unsupported
                .keys()
                .filter_map(|key| sanitize_parameter_name(key))
                .collect();
            refused.extend(nested_parameters);
            refused.sort_unstable();
            refused.dedup();
            refused.truncate(MAX_REPORTED_PARAMETERS);
            return Err(CompatError::unsupported(refused));
        }
        let reasoning_effort = parse_reasoning(self.reasoning_effort.as_deref())?;
        let tools = self
            .tools
            .into_iter()
            .map(WireTool::into_domain)
            .collect::<Result<Vec<_>>>()?;
        let request = InferenceRequest {
            model: self.model,
            request_id: None,
            thinking_mode: self.thinking_mode,
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
        Ok(TranslatedRequest {
            request,
            ignored_parameters,
        })
    }

    #[must_use]
    pub fn include_stream_usage(&self) -> bool {
        self.stream_options
            .as_ref()
            .is_some_and(|options| options.include_usage)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<WireContent>,
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
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
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
        let (content, images, files) = self.content.map_or_else(
            || Ok((String::new(), Vec::new(), Vec::new())),
            WireContent::into_parts,
        )?;
        Ok(ChatMessage {
            role,
            content,
            images,
            files,
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

/// Message content as either a bare string or the part array that the
/// `OpenAI` SDKs and most agent frameworks emit even for plain text.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum WireContent {
    Text(String),
    Parts(Vec<WireContentPart>),
}

impl WireContent {
    fn into_parts(self) -> Result<ContentParts> {
        match self {
            Self::Text(text) => Ok((text, Vec::new(), Vec::new())),
            Self::Parts(parts) => {
                let mut texts = Vec::with_capacity(parts.len());
                let mut images = Vec::new();
                let mut files = Vec::new();
                for part in parts {
                    match part {
                        WireContentPart::Text { text, .. } => texts.push(text),
                        WireContentPart::ImageUrl { image_url, .. } => {
                            images.push(axiom_inference::ImageContent::from_data_url(&image_url.url)
                                .map_err(|_| CompatError::new("images must be valid inline PNG, JPEG, WebP, or GIF data URLs (up to 5 MiB each)"))?);
                        }
                        WireContentPart::File { file } => {
                            let (mime_type, data) = file
                                .file_data
                                .strip_prefix("data:")
                                .and_then(|s| s.split_once(";base64,"))
                                .ok_or_else(|| {
                                    CompatError::new("files require inline base64 data URLs")
                                })?;
                            let content = axiom_inference::FileContent {
                                name: file.filename,
                                mime_type: mime_type.into(),
                                data: data.into(),
                            };
                            content
                                .validate()
                                .map_err(|_| CompatError::new("invalid file upload"))?;
                            files.push(content);
                        }
                        WireContentPart::Unsupported => {
                            return Err(CompatError::new(
                                "message content part type is not supported",
                            ));
                        }
                    }
                }
                Ok((texts.join("\n"), images, files))
            }
        }
    }

    fn collect_parameters(&self, reported: &mut Vec<String>) {
        if let Self::Parts(parts) = self {
            for part in parts {
                if let WireContentPart::Text { unsupported, .. } = part {
                    collect_keys("messages.content", Some(unsupported), reported);
                } else if let WireContentPart::ImageUrl {
                    image_url,
                    unsupported,
                } = part
                {
                    collect_keys("messages.content", Some(unsupported), reported);
                    collect_keys(
                        "messages.content.image_url",
                        Some(&image_url.unsupported),
                        reported,
                    );
                }
            }
        }
    }
}

type ContentParts = (
    String,
    Vec<axiom_inference::ImageContent>,
    Vec<axiom_inference::FileContent>,
);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireFile {
    pub filename: String,
    pub file_data: String,
}

/// A single content part.
///
/// Text and inline image bytes are representable. Provider adapters enforce
/// authenticated image support; unsupported content is never silently dropped.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContentPart {
    File {
        file: WireFile,
    },
    Text {
        text: String,
        #[serde(flatten)]
        unsupported: serde_json::Map<String, serde_json::Value>,
    },
    ImageUrl {
        image_url: WireImageUrl,
        #[serde(flatten)]
        unsupported: serde_json::Map<String, serde_json::Value>,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireImageUrl {
    pub url: String,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunctionDefinition,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
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
pub struct WireFunctionDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "empty_schema")]
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub strict: Option<bool>,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
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
pub struct WireNamedToolChoice {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireNamedFunction,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireNamedFunction {
    pub name: String,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunctionCall,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
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
pub struct WireFunctionCall {
    pub name: String,
    pub arguments: String,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireResponseFormat {
    Text {
        #[serde(flatten)]
        unsupported: serde_json::Map<String, serde_json::Value>,
    },
    JsonObject {
        #[serde(flatten)]
        unsupported: serde_json::Map<String, serde_json::Value>,
    },
    JsonSchema {
        json_schema: WireJsonSchema,
        #[serde(flatten)]
        unsupported: serde_json::Map<String, serde_json::Value>,
    },
}

impl WireResponseFormat {
    fn into_domain(self) -> ResponseFormat {
        match self {
            Self::Text { .. } => ResponseFormat::Text,
            Self::JsonObject { .. } => ResponseFormat::JsonObject,
            Self::JsonSchema { json_schema, .. } => ResponseFormat::JsonSchema {
                name: json_schema.name,
                schema: json_schema.schema,
                strict: json_schema.strict,
            },
        }
    }

    fn collect_parameters(&self, reported: &mut Vec<String>) {
        match self {
            Self::Text { unsupported } | Self::JsonObject { unsupported } => {
                collect_keys("response_format", Some(unsupported), reported);
            }
            Self::JsonSchema {
                json_schema,
                unsupported,
            } => {
                collect_keys("response_format", Some(unsupported), reported);
                collect_keys(
                    "response_format.json_schema",
                    Some(&json_schema.unsupported),
                    reported,
                );
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct WireJsonSchema {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(default)]
    pub strict: bool,
    #[serde(flatten)]
    pub unsupported: serde_json::Map<String, serde_json::Value>,
}

fn collect_keys(
    prefix: &str,
    fields: Option<&serde_json::Map<String, serde_json::Value>>,
    reported: &mut Vec<String>,
) {
    let Some(fields) = fields else {
        return;
    };
    for key in fields.keys() {
        if let Some(name) = sanitize_parameter_name(&format!("{prefix}.{key}")) {
            reported.push(name);
        }
    }
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
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    pub supported_reasoning_efforts: Vec<String>,
    pub supports_tools: bool,
    pub supports_parallel_tools: bool,
    pub supports_streaming: bool,
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
                context_window_tokens: model.context_window_tokens,
                max_output_tokens: model.max_output_tokens,
                supported_reasoning_efforts: model
                    .supported_reasoning_efforts
                    .iter()
                    .map(|effort| effort.as_str().to_owned())
                    .collect(),
                supports_tools: model.supports_tools,
                supports_parallel_tools: model.supports_parallel_tools,
                supports_streaming: model.supports_streaming,
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
        ProviderEvent::Accounting(_)
        | ProviderEvent::SecurityState(_)
        | ProviderEvent::ResponseVerified
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
    fn direct_files_preserve_bytes_and_reject_remote_or_non_user_inputs() {
        let raw = serde_json::json!({"model":"new-model","messages":[{"role":"user","content":[
            {"type":"file","file":{"filename":"notes.txt","file_data":"data:text/plain;base64,aGVsbG8="}}
        ]}]});
        let parsed: ChatCompletionRequest = serde_json::from_value(raw.clone()).unwrap();
        let translated = parsed.into_domain(CompatMode::Strict).unwrap();
        assert!(translated.request.messages[0].content.is_empty());
        assert_eq!(translated.request.messages[0].files[0].data, "aGVsbG8=");
        for role in ["assistant", "system", "tool"] {
            let mut invalid = raw.clone();
            invalid["messages"][0]["role"] = serde_json::json!(role);
            let parsed: ChatCompletionRequest = serde_json::from_value(invalid).unwrap();
            assert!(parsed.into_domain(CompatMode::Strict).is_err());
        }
        let mut invalid = raw;
        invalid["messages"][0]["content"][0]["file"]["file_data"] =
            serde_json::json!("https://example.com/a.pdf");
        let parsed: ChatCompletionRequest = serde_json::from_value(invalid).unwrap();
        assert!(parsed.into_domain(CompatMode::Lenient).is_err());
    }

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
        let translated = wire.into_domain(CompatMode::Strict).unwrap();
        assert!(translated.ignored_parameters.is_empty());
        let domain = translated.request;
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
    fn inline_images_are_preserved_and_unknown_options_follow_compatibility_mode() {
        let raw = serde_json::json!({"model":"x","messages":[{"role":"user","content":[
            {"type":"text","text":"describe"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgpmaXh0dXJl", "detail":"high"}, "extra":true}
        ]}]});
        let parsed: ChatCompletionRequest = serde_json::from_value(raw.clone()).unwrap();
        assert!(parsed.into_domain(CompatMode::Strict).is_err());
        let parsed: ChatCompletionRequest = serde_json::from_value(raw).unwrap();
        let translated = parsed.into_domain(CompatMode::Lenient).unwrap();
        assert_eq!(translated.request.messages[0].images.len(), 1);
        assert_eq!(translated.request.messages[0].content, "describe");
        assert!(
            translated
                .ignored_parameters
                .contains(&"messages.content.image_url.detail".into())
        );
        assert!(
            translated
                .ignored_parameters
                .contains(&"messages.content.extra".into())
        );
    }

    #[test]
    fn rejects_conflicting_and_unrepresentable_fields_in_every_mode() {
        for mode in [CompatMode::Lenient, CompatMode::Strict] {
            let conflict: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
                "model":"x","messages":[{"role":"user","content":"hi"}],
                "max_tokens":1,"max_completion_tokens":2
            }))
            .unwrap();
            assert!(conflict.into_domain(mode).is_err());

            // n > 1 cannot be under-delivered silently: the caller indexes the
            // choices it asked for.
            let choices: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
                "model":"x","messages":[{"role":"user","content":"hi"}],"n":3
            }))
            .unwrap();
            assert!(choices.into_domain(mode).is_err());

            // n = 1 is exactly the default, so it is honored rather than refused.
            let single: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
                "model":"x","messages":[{"role":"user","content":"hi"}],"n":1
            }))
            .unwrap();
            assert!(
                single
                    .into_domain(mode)
                    .unwrap()
                    .ignored_parameters
                    .is_empty()
            );

            // A content part type with no representable meaning must not be
            // dropped: answering without the image is worse than refusing.
            let image: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
                "model":"x","messages":[{"role":"user","content":[
                    {"type":"image_url","image_url":{"url":"https://example/a.png"}}
                ]}]
            }))
            .expect("an unsupported part must reach translation, not fail parsing");
            assert!(image.into_domain(mode).is_err());
        }
    }

    #[test]
    fn lenient_drops_unsupported_parameters_and_strict_refuses_them() {
        let body = serde_json::json!({
            "model":"x","messages":[{"role":"user","content":"hi"}],
            // Answer-affecting: reported.
            "stop":["\n\n"],"seed":7,"frequency_penalty":0.5,"logit_bias":{"1":1},
            "logprobs":true,"top_logprobs":3,"presence_penalty":0.1,
            // Inert: dropped without noise.
            "user":"u-1","metadata":{"k":"v"},"store":false,"service_tier":"auto",
            // Unrecognized, possibly a typo or a newer revision: reported.
            "temperatur":0.4
        });

        let lenient: ChatCompletionRequest = serde_json::from_value(body.clone()).unwrap();
        let translated = lenient.into_domain(CompatMode::Lenient).unwrap();
        assert_eq!(
            translated.ignored_parameters,
            [
                "frequency_penalty",
                "logit_bias",
                "logprobs",
                "presence_penalty",
                "seed",
                "stop",
                "temperatur",
                "top_logprobs"
            ]
        );

        let strict: ChatCompletionRequest = serde_json::from_value(body).unwrap();
        let error = strict.into_domain(CompatMode::Strict).unwrap_err();
        // Strict names the inert parameters too: it promises full fidelity.
        assert!(error.parameters().contains(&"user".to_owned()));
        assert!(error.parameters().contains(&"stop".to_owned()));
    }

    #[test]
    fn accepts_inert_parameters_silently_in_lenient_mode() {
        let wire: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
            "model":"x","messages":[{"role":"user","content":"hi"}],
            "user":"u-1","store":true
        }))
        .unwrap();
        let translated = wire.into_domain(CompatMode::Lenient).unwrap();
        assert!(translated.ignored_parameters.is_empty());
    }

    #[test]
    fn lenient_mode_survives_additive_nested_metadata_while_strict_reports_it() {
        let body = serde_json::json!({
            "model":"x",
            "messages":[{
                "role":"user",
                "content":[{"type":"text","text":"hi","provider_cache_hint":true}],
                "cache_control":{"type":"ephemeral"}
            }],
            "tools":[{"type":"function","provider_hint":"future","function":{
                "name":"lookup",
                "parameters":{"type":"object"},
                "future_schema_dialect":"v2"
            }}],
            "stream":true,
            "stream_options":{"include_usage":true,"continuous_usage_stats":true},
            "response_format":{"type":"json_object","future_format_hint":true}
        });

        let lenient: ChatCompletionRequest = serde_json::from_value(body.clone()).unwrap();
        let translated = lenient.into_domain(CompatMode::Lenient).unwrap();
        assert!(
            translated
                .ignored_parameters
                .contains(&"messages.cache_control".into())
        );
        assert!(
            translated
                .ignored_parameters
                .contains(&"messages.content.provider_cache_hint".into())
        );
        assert!(
            translated
                .ignored_parameters
                .contains(&"tools.provider_hint".into())
        );
        assert!(
            translated
                .ignored_parameters
                .contains(&"tools.function.future_schema_dialect".into())
        );
        assert!(
            translated
                .ignored_parameters
                .contains(&"stream_options.continuous_usage_stats".into())
        );
        assert!(
            translated
                .ignored_parameters
                .contains(&"response_format.future_format_hint".into())
        );

        let strict: ChatCompletionRequest = serde_json::from_value(body).unwrap();
        assert!(strict.into_domain(CompatMode::Strict).is_err());
    }

    #[test]
    fn joins_text_content_parts_and_keeps_bare_strings() {
        let wire: ChatCompletionRequest = serde_json::from_value(serde_json::json!({
            "model":"x","messages":[
                {"role":"user","content":[{"type":"text","text":"first"},
                                          {"type":"text","text":"second"}]},
                {"role":"assistant","content":"plain"}
            ]
        }))
        .unwrap();
        let domain = wire.into_domain(CompatMode::Strict).unwrap().request;
        assert_eq!(domain.messages[0].content, "first\nsecond");
        assert_eq!(domain.messages[1].content, "plain");
    }

    #[test]
    fn sanitizes_and_bounds_reported_parameter_names() {
        // Header and log injection must be impossible regardless of the key.
        assert_eq!(sanitize_parameter_name("a\r\nb: c").as_deref(), Some("abc"));
        assert_eq!(sanitize_parameter_name("\u{202e}\u{202e}"), None);
        assert_eq!(
            sanitize_parameter_name(&"x".repeat(200)).map(|n| n.len()),
            Some(MAX_REPORTED_PARAMETER_LEN)
        );

        let mut body = serde_json::Map::new();
        body.insert("model".into(), serde_json::json!("x"));
        body.insert(
            "messages".into(),
            serde_json::json!([{"role":"user","content":"hi"}]),
        );
        for index in 0..50 {
            body.insert(format!("unknown_{index}"), serde_json::json!(1));
        }
        let wire: ChatCompletionRequest =
            serde_json::from_value(serde_json::Value::Object(body)).unwrap();
        let translated = wire.into_domain(CompatMode::Lenient).unwrap();
        assert_eq!(translated.ignored_parameters.len(), MAX_REPORTED_PARAMETERS);
    }

    #[test]
    fn emits_openai_nonstream_stream_usage_tools_and_done_shapes() {
        let models = serde_json::to_value(model_list(&[ModelInfo {
            id: "cherry".to_owned(),
            provider_label: "Axiom".to_owned(),
            context_window_tokens: 131_072,
            max_output_tokens: 8_192,
            supported_reasoning_efforts: vec![ReasoningEffort::Medium],
            supports_tools: true,
            supports_parallel_tools: true,
            supports_streaming: true,
            ..ModelInfo::default()
        }]))
        .unwrap();
        assert_eq!(models["data"][0]["context_window_tokens"], 131_072);
        assert_eq!(
            models["data"][0]["supported_reasoning_efforts"],
            serde_json::json!(["medium"])
        );

        let response = InferenceResponse {
            assistant: AssistantTurn {
                reasoning: None,
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

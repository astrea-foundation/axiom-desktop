//! Transport-independent inference contracts shared by Axiom applications.
//!
//! This crate contains domain values only. HTTP compatibility, secure
//! transport, attestation, encryption, agent orchestration, persistence, and
//! presentation belong to adapters and applications around these types.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
mod accounting;
mod attachments;
pub use accounting::{InvocationPurpose, InvocationState, RequestUsage, UsageCompleteness};
pub use attachments::{
    FILE_MIME_TYPES, FileContent, ImageContent, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS,
    MAX_FILE_BYTES, MAX_IMAGE_BYTES, PromptAttachment, user_message, validate_prompt,
    validate_stored_prompt,
};

pub const MAX_TOOLS: usize = 128;
pub const MAX_TOOL_CALLS: usize = 128;
pub const MAX_TOOL_CALL_ID_CHARS: usize = 256;
pub const MAX_FUNCTION_NAME_CHARS: usize = 64;
pub const MAX_MESSAGE_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOOL_DESCRIPTION_BYTES: usize = 16_384;
pub const MAX_TOOL_PARAMETERS_BYTES: usize = 131_072;
pub const MAX_TOOL_ARGUMENTS_BYTES: usize = 262_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    #[must_use]
    pub fn text(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            images: Vec::new(),
            files: Vec::new(),
            reasoning_content: None,
            name: None,
            refusal: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDefinition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    #[serde(rename = "xhigh")]
    ExtraHigh,
}

impl ReasoningEffort {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::ExtraHigh => "xhigh",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named {
        name: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SamplingParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ResponseFormat {
    #[default]
    Text,
    JsonObject,
    JsonSchema {
        name: String,
        schema: serde_json::Value,
        #[serde(default)]
        strict: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    #[default]
    ProviderDefault,
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InferenceRequest {
    pub model: String,
    pub request_id: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoice,
    pub parallel_tool_calls: Option<bool>,
    pub reasoning_effort: ReasoningEffort,
    pub thinking_mode: ThinkingMode,
    pub max_output_tokens: Option<u32>,
    pub sampling: SamplingParameters,
    pub response_format: ResponseFormat,
    pub stream: bool,
}

impl InferenceRequest {
    #[must_use]
    pub fn streaming(
        model: impl Into<String>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            model: model.into(),
            request_id: None,
            messages,
            tools,
            tool_choice: ToolChoice::Auto,
            parallel_tool_calls: None,
            reasoning_effort: ReasoningEffort::Medium,
            thinking_mode: ThinkingMode::ProviderDefault,
            max_output_tokens: None,
            sampling: SamplingParameters::default(),
            response_format: ResponseFormat::Text,
            stream: true,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.model.trim().is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_control)
        {
            return Err(ValidationError::invalid("model"));
        }
        if self.messages.is_empty() {
            return Err(ValidationError::count("messages"));
        }
        if self.tools.len() > MAX_TOOLS {
            return Err(ValidationError::count("tools"));
        }
        for message in &self.messages {
            if !message.files.is_empty()
                && (message.role != ChatRole::User
                    || message.files.len() + message.images.len() > MAX_ATTACHMENTS)
            {
                return Err(ValidationError::invalid("message.files"));
            }
            let mut attachment_bytes = message.content.len();
            for file in &message.files {
                file.validate()?;
                attachment_bytes = attachment_bytes.saturating_add(file.data.len());
            }
            for image in &message.images {
                attachment_bytes = attachment_bytes.saturating_add(image.data.len());
            }
            if attachment_bytes > MAX_ATTACHMENT_BYTES {
                return Err(ValidationError::invalid("message.attachments"));
            }
            if !message.images.is_empty()
                && (message.role != ChatRole::User || message.images.len() > MAX_ATTACHMENTS)
            {
                return Err(ValidationError::invalid("message.images"));
            }
            for image in &message.images {
                image.validate()?;
            }
            validate_bytes("message.content", &message.content, MAX_MESSAGE_TEXT_BYTES)?;
            validate_optional_bytes(
                "message.reasoning_content",
                message.reasoning_content.as_deref(),
                MAX_MESSAGE_TEXT_BYTES,
            )?;
            validate_optional_bytes(
                "message.refusal",
                message.refusal.as_deref(),
                MAX_MESSAGE_TEXT_BYTES,
            )?;
            if let Some(name) = &message.name {
                validate_identifier("message.name", name, MAX_FUNCTION_NAME_CHARS)?;
            }
            if let Some(call_id) = &message.tool_call_id {
                validate_call_id(call_id)?;
            }
            validate_tool_calls(&message.tool_calls)?;
        }
        for tool in &self.tools {
            if tool.kind != "function" {
                return Err(ValidationError::unsupported("tool.type"));
            }
            validate_identifier(
                "tool.function.name",
                &tool.function.name,
                MAX_FUNCTION_NAME_CHARS,
            )?;
            validate_bytes(
                "tool.function.description",
                &tool.function.description,
                MAX_TOOL_DESCRIPTION_BYTES,
            )?;
            let parameters = serde_json::to_vec(&tool.function.parameters)
                .map_err(|_| ValidationError::invalid("tool.function.parameters"))?;
            if parameters.len() > MAX_TOOL_PARAMETERS_BYTES {
                return Err(ValidationError::size("tool.function.parameters"));
            }
        }
        if let ToolChoice::Named { name } = &self.tool_choice {
            validate_identifier("tool_choice.name", name, MAX_FUNCTION_NAME_CHARS)?;
        }
        if self.max_output_tokens == Some(0) {
            return Err(ValidationError::invalid("max_output_tokens"));
        }
        if self
            .sampling
            .temperature
            .is_some_and(|number| !number.is_finite() || !(0.0..=2.0).contains(&number))
        {
            return Err(ValidationError::invalid("temperature"));
        }
        if self
            .sampling
            .top_p
            .is_some_and(|number| !number.is_finite() || !(0.0..=1.0).contains(&number))
        {
            return Err(ValidationError::invalid("top_p"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationErrorKind {
    Invalid,
    Count,
    Size,
    Unsupported,
    Conflict,
    Incomplete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub kind: ValidationErrorKind,
}

impl ValidationError {
    const fn invalid(field: &'static str) -> Self {
        Self {
            field,
            kind: ValidationErrorKind::Invalid,
        }
    }

    const fn count(field: &'static str) -> Self {
        Self {
            field,
            kind: ValidationErrorKind::Count,
        }
    }

    const fn size(field: &'static str) -> Self {
        Self {
            field,
            kind: ValidationErrorKind::Size,
        }
    }

    const fn unsupported(field: &'static str) -> Self {
        Self {
            field,
            kind: ValidationErrorKind::Unsupported,
        }
    }

    const fn conflict(field: &'static str) -> Self {
        Self {
            field,
            kind: ValidationErrorKind::Conflict,
        }
    }

    const fn incomplete(field: &'static str) -> Self {
        Self {
            field,
            kind: ValidationErrorKind::Incomplete,
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} inference field: {}", self.kind, self.field)
    }
}

impl std::error::Error for ValidationError {}

fn validate_optional_bytes(
    field: &'static str,
    value: Option<&str>,
    maximum: usize,
) -> Result<(), ValidationError> {
    value.map_or(Ok(()), |value| validate_bytes(field, value, maximum))
}

fn validate_bytes(field: &'static str, value: &str, maximum: usize) -> Result<(), ValidationError> {
    if value.len() > maximum {
        Err(ValidationError::size(field))
    } else {
        Ok(())
    }
}

fn validate_identifier(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.chars().count() > maximum
        || value.chars().any(|character| {
            character.is_control()
                || !(character.is_ascii_alphanumeric() || "_-.$/".contains(character))
        })
    {
        Err(ValidationError::invalid(field))
    } else {
        Ok(())
    }
}

fn validate_call_id(value: &str) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.chars().count() > MAX_TOOL_CALL_ID_CHARS
        || value.chars().any(char::is_control)
    {
        Err(ValidationError::invalid("tool_call_id"))
    } else {
        Ok(())
    }
}

pub fn validate_tool_calls(calls: &[ToolCall]) -> Result<(), ValidationError> {
    if calls.len() > MAX_TOOL_CALLS {
        return Err(ValidationError::count("tool_calls"));
    }
    for call in calls {
        validate_call_id(&call.id)?;
        if call.kind != "function" {
            return Err(ValidationError::unsupported("tool_call.type"));
        }
        validate_identifier(
            "tool_call.function.name",
            &call.function.name,
            MAX_FUNCTION_NAME_CHARS,
        )?;
        validate_bytes(
            "tool_call.function.arguments",
            &call.function.arguments,
            MAX_TOOL_ARGUMENTS_BYTES,
        )?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Cancelled,
    Other(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ToolCallAccumulator {
    calls: BTreeMap<usize, PartialToolCall>,
}

#[derive(Clone, Debug, Default)]
struct PartialToolCall {
    id: Option<String>,
    kind: Option<String>,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    pub fn push(&mut self, delta: ToolCallDelta) -> Result<(), ValidationError> {
        if delta.index >= MAX_TOOL_CALLS {
            return Err(ValidationError::count("tool_call_delta.index"));
        }
        let call = self.calls.entry(delta.index).or_default();
        merge_stable(&mut call.id, delta.id, "tool_call_delta.id")?;
        merge_stable(&mut call.kind, delta.kind, "tool_call_delta.type")?;
        if let Some(name) = delta.name {
            call.name.push_str(&name);
            if call.name.len() > MAX_FUNCTION_NAME_CHARS {
                return Err(ValidationError::size("tool_call_delta.name"));
            }
        }
        if let Some(arguments) = delta.arguments {
            call.arguments.push_str(&arguments);
            if call.arguments.len() > MAX_TOOL_ARGUMENTS_BYTES {
                return Err(ValidationError::size("tool_call_delta.arguments"));
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<ToolCall>, ValidationError> {
        let mut completed = Vec::with_capacity(self.calls.len());
        for (expected, (index, call)) in self.calls.into_iter().enumerate() {
            if index != expected {
                return Err(ValidationError::incomplete("tool_call_delta.index"));
            }
            let id = call
                .id
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ValidationError::incomplete("tool_call_delta.id"))?;
            let kind = call.kind.unwrap_or_else(|| "function".to_owned());
            let tool = ToolCall {
                id,
                kind,
                function: FunctionCall {
                    name: call.name,
                    arguments: call.arguments,
                },
            };
            validate_tool_calls(std::slice::from_ref(&tool))?;
            completed.push(tool);
        }
        Ok(completed)
    }
}

fn merge_stable(
    destination: &mut Option<String>,
    fragment: Option<String>,
    field: &'static str,
) -> Result<(), ValidationError> {
    if let Some(fragment) = fragment {
        match destination {
            Some(existing) if existing != &fragment => Err(ValidationError::conflict(field)),
            Some(_) => Ok(()),
            None => {
                *destination = Some(fragment);
                Ok(())
            }
        }
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    Accounting(Box<RequestUsage>),
    SecurityState(ProviderSecurityState),
    /// The exact response associated with this provider call passed its
    /// terminal cryptographic verification. Secure adapters emit this only
    /// after the verified stream has returned successfully; preflight
    /// attestation alone must never produce it.
    ResponseVerified,
    Status {
        connected: bool,
        detail: String,
    },
    /// Incremental display material, provisional until terminal verification.
    /// A delta alone does not establish successful completion or a verified receipt.
    TextDelta(String),
    ReasoningDelta(String),
    RefusalDelta(String),
    ToolCallDelta(ToolCallDelta),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Finished(FinishReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderSecurityState {
    Unverified,
    Verifying,
    Verified,
    Degraded,
    Outdated,
    Failed,
    UnattestedDevelopment,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssistantTurn {
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InferenceResponse {
    pub assistant: AssistantTurn,
    pub reasoning: Option<String>,
    pub refusal: Option<String>,
    pub usage: Usage,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Provider capabilities are independent catalog flags, not mutually exclusive states"
)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub short_label: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub provider_label: String,
    #[serde(default)]
    pub upstream_model: String,
    #[serde(default)]
    pub provider_base_url: String,
    #[serde(default)]
    pub e2ee_protocol: String,
    #[serde(default)]
    pub e2ee_encryption_version: u16,
    #[serde(default)]
    pub attestation_protocol: String,
    #[serde(default)]
    pub context_window_tokens: u32,
    #[serde(default)]
    pub max_output_tokens: u32,
    /// Exact display price in micro-USD per one million uncached input tokens.
    #[serde(default)]
    pub input_price_microusd_per_million_tokens: Option<u64>,
    /// Exact display price in micro-USD per one million output tokens.
    #[serde(default)]
    pub output_price_microusd_per_million_tokens: Option<u64>,
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<ReasoningEffort>,
    #[serde(default)]
    pub supported_thinking_modes: Vec<ThinkingMode>,
    #[serde(default)]
    pub reasoning_replay: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_parallel_tools: bool,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_images: bool,
    #[serde(default)]
    pub file_mime_types: Vec<String>,
    #[serde(default)]
    pub reasoning_parameters: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub thinking_parameters: std::collections::BTreeMap<String, serde_json::Value>,
}

pub const AUTO_COMPACT_CONTEXT_PERCENT: u32 = 85;

impl ModelInfo {
    /// Token occupancy at which conversation context should be compacted.
    /// The provider's live full context window is the source of truth.
    #[must_use]
    pub fn auto_compact_threshold_tokens(&self) -> u32 {
        self.context_window_tokens
            .saturating_mul(AUTO_COMPACT_CONTEXT_PERCENT)
            / 100
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderFailureKind {
    Configuration,
    LocalAuthentication,
    Authentication,
    InsufficientCredit,
    RateLimited,
    ModelUnavailable,
    CapabilityMismatch,
    AttestationUnavailable,
    AttestationRejected,
    SessionEstablishment,
    Encryption,
    Decryption,
    Transient,
    InvalidRequest,
    InvalidResponse,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{
        ChatMessage, ChatRole, InferenceRequest, ModelInfo, ReasoningEffort, ResponseFormat,
        ToolCallAccumulator, ToolCallDelta, ToolChoice, ValidationErrorKind,
    };

    #[test]
    fn text_message_has_no_tool_or_optional_metadata() {
        let message = ChatMessage::text(ChatRole::User, "hello");

        assert_eq!(message.role, ChatRole::User);
        assert_eq!(message.content, "hello");
        assert!(message.reasoning_content.is_none());
        assert!(message.name.is_none());
        assert!(message.refusal.is_none());
        assert!(message.tool_call_id.is_none());
        assert!(message.tool_calls.is_empty());
    }

    #[test]
    fn streaming_request_uses_explicit_provider_neutral_defaults() {
        let request = InferenceRequest::streaming(
            "cherry",
            vec![ChatMessage::text(ChatRole::User, "hello")],
            Vec::new(),
        );

        assert!(request.stream);
        assert_eq!(request.reasoning_effort, ReasoningEffort::Medium);
        assert_eq!(request.tool_choice, ToolChoice::Auto);
        assert_eq!(request.response_format, ResponseFormat::Text);
    }

    #[test]
    fn requests_still_require_at_least_one_message() {
        let request = InferenceRequest::streaming("cherry", Vec::new(), Vec::new());

        let error = request.validate().unwrap_err();
        assert_eq!(error.kind, ValidationErrorKind::Count);
        assert_eq!(error.field, "messages");
    }

    #[test]
    fn model_catalog_defaults_do_not_imply_capabilities() {
        let model: ModelInfo = serde_json::from_str(r#"{"id":"cherry"}"#).unwrap();

        assert_eq!(model.id, "cherry");
        assert!(!model.supports_tools);
        assert!(!model.supports_streaming);
        assert!(model.supported_reasoning_efforts.is_empty());
        assert_eq!(model.auto_compact_threshold_tokens(), 0);
    }

    #[test]
    fn auto_compaction_uses_eighty_five_percent_of_live_context() {
        let model = ModelInfo {
            id: "cherry".into(),
            context_window_tokens: 1_048_576,
            ..ModelInfo::default()
        };

        assert_eq!(model.auto_compact_threshold_tokens(), 891_289);
    }

    #[test]
    fn validation_errors_do_not_echo_untrusted_values() {
        let secret = "axm_do_not_echo";
        let mut request = InferenceRequest::streaming(
            "cherry",
            vec![ChatMessage::text(
                ChatRole::User,
                format!("{secret}{}", "x".repeat(super::MAX_MESSAGE_TEXT_BYTES)),
            )],
            Vec::new(),
        );
        request.sampling.temperature = Some(f64::NAN);

        let error = request.validate().unwrap_err();
        assert_eq!(error.kind, ValidationErrorKind::Size);
        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn sampling_metadata_is_finite_and_bounded() {
        let base = InferenceRequest::streaming(
            "cherry",
            vec![ChatMessage::text(ChatRole::User, "hello")],
            Vec::new(),
        );
        for temperature in [-0.01, 2.01, f64::INFINITY, f64::NAN] {
            let mut request = base.clone();
            request.sampling.temperature = Some(temperature);
            assert_eq!(request.validate().unwrap_err().field, "temperature");
        }
        for top_p in [-0.01, 1.01, f64::INFINITY, f64::NAN] {
            let mut request = base.clone();
            request.sampling.top_p = Some(top_p);
            assert_eq!(request.validate().unwrap_err().field, "top_p");
        }
        for (temperature, top_p) in [(0.0, 0.0), (1.0, 0.9), (2.0, 1.0)] {
            let mut request = base.clone();
            request.sampling.temperature = Some(temperature);
            request.sampling.top_p = Some(top_p);
            request.validate().expect("bounded numeric sampling");
        }
    }

    #[test]
    fn tool_fragments_assemble_in_index_order_and_reject_conflicts() {
        let mut calls = ToolCallAccumulator::default();
        calls
            .push(ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                kind: Some("function".into()),
                name: Some("read_".into()),
                arguments: Some("{\"path\":".into()),
            })
            .unwrap();
        calls
            .push(ToolCallDelta {
                index: 0,
                id: None,
                kind: None,
                name: Some("file".into()),
                arguments: Some("\"README.md\"}".into()),
            })
            .unwrap();
        let completed = calls.finish().unwrap();
        assert_eq!(completed[0].function.name, "read_file");
        assert_eq!(completed[0].function.arguments, r#"{"path":"README.md"}"#);

        let mut conflict = ToolCallAccumulator::default();
        conflict
            .push(ToolCallDelta {
                index: 0,
                id: Some("one".into()),
                ..ToolCallDelta::default()
            })
            .unwrap();
        let error = conflict
            .push(ToolCallDelta {
                index: 0,
                id: Some("two".into()),
                ..ToolCallDelta::default()
            })
            .unwrap_err();
        assert_eq!(error.kind, ValidationErrorKind::Conflict);
    }

    proptest! {
        #[test]
        fn arbitrary_ascii_argument_fragments_assemble_without_loss(
            fragments in proptest::collection::vec("[ -~]{0,24}", 0..32)
        ) {
            let expected = fragments.concat();
            let mut calls = ToolCallAccumulator::default();
            calls.push(ToolCallDelta {
                index: 0,
                id: Some("call_property".into()),
                kind: Some("function".into()),
                name: Some("read_file".into()),
                arguments: None,
            }).unwrap();
            for fragment in fragments {
                calls.push(ToolCallDelta {
                    index: 0,
                    arguments: Some(fragment),
                    ..ToolCallDelta::default()
                }).unwrap();
            }
            let completed = calls.finish().unwrap();
            prop_assert_eq!(&completed[0].function.arguments, &expected);
        }
    }
}

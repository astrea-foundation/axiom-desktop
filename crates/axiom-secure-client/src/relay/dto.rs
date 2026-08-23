use serde::{Deserialize, Deserializer, Serialize};

use crate::{Result, SecureClientError};

pub(crate) const MAX_CIPHERTEXT_HEX_CHARS: usize = 524_288;
const MIN_CIPHERTEXT_HEX_CHARS: usize = 144;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct CiphertextHex(String);

impl CiphertextHex {
    pub(crate) fn new(value: String) -> Result<Self> {
        if value.len() < MIN_CIPHERTEXT_HEX_CHARS
            || value.len() > MAX_CIPHERTEXT_HEX_CHARS
            || !value.len().is_multiple_of(2)
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SecureClientError::new(
                axiom_inference::ProviderFailureKind::InvalidResponse,
                "relay ciphertext is invalid",
            ));
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CiphertextHex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CiphertextHex([CIPHERTEXT])")
    }
}

impl<'de> Deserialize<'de> for CiphertextHex {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EncryptedFunctionDefinition {
    pub encrypted_name: CiphertextHex,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_description: Option<CiphertextHex>,
    pub encrypted_parameters: CiphertextHex,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EncryptedTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: EncryptedFunctionDefinition,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EncryptedNamedFunction {
    pub encrypted_name: CiphertextHex,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EncryptedNamedToolChoice {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: EncryptedNamedFunction,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum EncryptedToolChoice {
    Mode(&'static str),
    Named(EncryptedNamedToolChoice),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EncryptedFunctionCall {
    pub encrypted_name: CiphertextHex,
    pub encrypted_arguments: CiphertextHex,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EncryptedToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: EncryptedFunctionCall,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EncryptedMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<CiphertextHex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_reasoning_content: Option<CiphertextHex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_name: Option<CiphertextHex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_refusal: Option<CiphertextHex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_tool_calls: Option<Vec<EncryptedToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RelayChatRequest {
    pub model_id: String,
    pub encryption_version: u16,
    pub e2ee_protocol: String,
    pub client_public_key_hex: String,
    pub model_public_key_hex: String,
    pub encrypted_messages: Vec<EncryptedMessage>,
    pub encrypt_all_fields: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_tools: Option<Vec<EncryptedTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_tool_choice: Option<EncryptedToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl RelayChatRequest {
    pub(crate) fn serialize_bounded(&self, limit: usize) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self).map_err(|_| {
            SecureClientError::new(
                axiom_inference::ProviderFailureKind::InvalidRequest,
                "relay request could not be serialized",
            )
        })?;
        if bytes.len() > limit {
            return Err(SecureClientError::new(
                axiom_inference::ProviderFailureKind::InvalidRequest,
                "relay request exceeds the configured limit",
            ));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)]
pub(crate) struct RelayUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelayCompletion {
    pub id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub encrypted_content: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_reasoning_content: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_refusal: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_tool_calls: Vec<EncryptedToolCall>,
    pub finish_reason: String,
    #[serde(default)]
    pub usage: RelayUsage,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EncryptedFunctionCallDelta {
    #[serde(default)]
    pub encrypted_name: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_arguments: Option<CiphertextHex>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EncryptedToolCallDelta {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub function: Option<EncryptedFunctionCallDelta>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatedPayload {
    pub run_id: String,
    pub inference_encryption: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeltaPayload {
    pub run_id: String,
    #[serde(default)]
    pub encrypted_delta: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_reasoning_delta: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_refusal_delta: Option<CiphertextHex>,
    #[serde(default)]
    pub encrypted_tool_calls: Vec<EncryptedToolCallDelta>,
    pub sequence: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompletedPayload {
    pub run_id: String,
    #[serde(default)]
    pub usage: RelayUsage,
    pub finish_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunPayload {
    pub run_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CancelledPayload {
    pub run_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FailedPayload {
    pub run_id: String,
    pub error: String,
    #[serde(default)]
    pub code: Option<String>,
}

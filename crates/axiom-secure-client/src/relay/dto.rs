use serde::{Deserialize, Deserializer, Serialize};

use crate::{Result, SecureClientError};

pub(crate) const MAX_CIPHERTEXT_HEX_CHARS: usize = 40 * 1024 * 1024 + 1024;
// Version-2 NEAR envelopes contain a 32-byte ephemeral key, 24-byte
// nonce and 16-byte authentication tag before any plaintext content.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    pub provider_id: String,
    pub inference_encryption: &'static str,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_mode: Option<axiom_inference::ThinkingMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_e2ee_context: Option<serde_json::Value>,
    pub attestation_generation: u64,
    pub verified_key_fingerprint: String,
    pub verified_keyset_digest: Option<String>,
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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ProviderKeyLease {
    pub model_id: String,
    pub model: String,
    pub base_url: String,
    pub provider: String,
    pub attestation_protocol: String,
    pub encryption_version: u16,
    pub e2ee_protocol: String,
    pub model_public_key: String,
    pub verified: bool,
    pub attestation_generation: u64,
    pub model_key_fingerprint: String,
    #[serde(default)]
    pub keyset_digest: Option<String>,
    #[serde(default)]
    pub response_signing_address: Option<String>,
    #[serde(default)]
    pub response_signing_key_fingerprint: Option<String>,
    pub hard_expires_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NearCompletionProof {
    pub provider: String,
    pub protocol: String,
    pub chat_id: String,
    pub model: String,
    pub request_hash: String,
    pub response_hash: String,
    pub response_body_base64: String,
    pub signed_text: String,
    pub signature: String,
    pub signing_address: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct RelayUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub cached_prompt_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
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
    #[serde(default)]
    pub proof: Option<serde_json::Value>,
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
pub(crate) struct CreatedPayload {
    pub run_id: String,
    pub inference_encryption: String,
}

#[derive(Debug, Deserialize)]
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
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub content_field: Option<String>,
    #[serde(default)]
    pub reasoning_field: Option<String>,
    #[serde(default)]
    pub refusal_field: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CompletedPayload {
    pub run_id: String,
    #[serde(default)]
    pub usage: RelayUsage,
    pub finish_reason: String,
    #[serde(default)]
    pub proof: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RunPayload {
    pub run_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CancelledPayload {
    pub run_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FailedPayload {
    pub run_id: String,
    // Preserve wire validation but never reflect untrusted remote error text.
    #[serde(rename = "error")]
    pub _error: String,
    #[serde(default)]
    pub code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_response_metadata_is_additive_but_encrypted_tool_calls_stay_exact() {
        let completion: RelayCompletion = serde_json::from_value(serde_json::json!({
            "id": "response-1",
            "encrypted_content": "a".repeat(144),
            "finish_reason": "stop",
            "usage": {
                "prompt_tokens": 4,
                "future_cached_tokens": 2
            },
            "future_provider_metadata": {"revision": 2}
        }))
        .unwrap();
        assert_eq!(completion.usage.prompt_tokens, Some(4));

        let strict_tool_call = serde_json::from_value::<EncryptedToolCall>(serde_json::json!({
            "id": "call-1",
            "type": "function",
            "function": {
                "encrypted_name": "a".repeat(144),
                "encrypted_arguments": "b".repeat(144)
            },
            "future_execution_semantics": true
        }));
        assert!(strict_tool_call.is_err());
    }
}

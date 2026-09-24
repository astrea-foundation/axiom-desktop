//! Operational accounting is deliberately independent of response verification.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageCompleteness {
    #[default]
    Unknown,
    Live,
    Final,
    Partial,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvocationState {
    #[default]
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvocationPurpose {
    #[default]
    Conversation,
    Title,
    Compaction,
}

/// Decimal strings remain exact in JavaScript, on disk, and over ACP.
/// A settled charge does not imply an authenticated/complete model response.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestUsage {
    pub request_id: String,
    pub model_id: String,
    pub provider_id: String,
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    #[serde(default)]
    pub auto_compact_threshold_tokens: Option<u32>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub purpose: InvocationPurpose,
    #[serde(default)]
    pub state: InvocationState,
    #[serde(default)]
    pub completeness: UsageCompleteness,
    #[serde(default)]
    pub input_tokens: Option<String>,
    #[serde(default)]
    pub cached_input_tokens: Option<String>,
    #[serde(default)]
    pub output_tokens: Option<String>,
    #[serde(default)]
    pub reasoning_tokens: Option<String>,
    #[serde(default)]
    pub cost_microusd: Option<String>,
    #[serde(default)]
    pub settled: bool,
    #[serde(default)]
    pub response_verified: bool,
    #[serde(default)]
    pub started_at_ms: String,
    #[serde(default)]
    pub finished_at_ms: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

impl RequestUsage {
    #[must_use]
    pub fn validate_counters(&self) -> bool {
        let parse = |v: &Option<String>| -> Option<Option<u64>> {
            match v {
                None => Some(None),
                Some(s)
                    if !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    s.parse::<u64>().ok().map(Some)
                }
                Some(_) => None,
            }
        };
        let (Some(input), Some(cached), Some(output), Some(reasoning), Some(_)) = (
            parse(&self.input_tokens),
            parse(&self.cached_input_tokens),
            parse(&self.output_tokens),
            parse(&self.reasoning_tokens),
            parse(&self.cost_microusd),
        ) else {
            return false;
        };
        !matches!((input, cached), (Some(i), Some(c)) if c > i)
            && !matches!((output, reasoning), (Some(o), Some(r)) if r > o)
    }
}

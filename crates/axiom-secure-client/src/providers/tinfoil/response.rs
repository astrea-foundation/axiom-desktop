//! `OpenAI` response parsing after EHBP authentication. An authenticated [DONE]
//! plus a finish reason and usage are mandatory; transport EOF is insufficient.
use super::invalid;
use crate::Result;
use axiom_inference::{
    AssistantTurn, FinishReason, InferenceResponse, ProviderEvent, ToolCallAccumulator,
    ToolCallDelta, Usage,
};
use serde_json::Value;

pub(super) struct StreamParser {
    model: String,
    id: Option<String>,
    wire_model: Option<String>,
    buffer: Vec<u8>,
    data: String,
    limit: usize,
    done: bool,
    output: InferenceResponse,
    usage: Option<Usage>,
    calls: ToolCallAccumulator,
}

impl StreamParser {
    pub(super) fn new(model: &str, limit: usize) -> Self {
        Self {
            model: model.into(),
            id: None,
            wire_model: None,
            buffer: Vec::new(),
            data: String::new(),
            limit,
            done: false,
            output: InferenceResponse::default(),
            usage: None,
            calls: ToolCallAccumulator::default(),
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<ProviderEvent>> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line[..end])
                .map_err(|_| invalid("invalid UTF-8 in authenticated stream"))?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    let data = std::mem::take(&mut self.data);
                    events.extend(self.event(data.trim_end_matches('\n'))?);
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if self.done {
                    return Err(invalid("data after authenticated stream completion"));
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
                self.data.push('\n');
                if self.data.len() > self.limit {
                    return Err(invalid("Tinfoil stream event is oversized"));
                }
            } else if !line.starts_with(':')
                && !line.starts_with("event:")
                && !line.starts_with("id:")
                && !line.starts_with("retry:")
            {
                return Err(invalid("invalid authenticated SSE framing"));
            }
        }
        if self.buffer.len().saturating_add(self.data.len()) > self.limit {
            return Err(invalid("Tinfoil stream event is oversized"));
        }
        Ok(events)
    }

    fn event(&mut self, data: &str) -> Result<Vec<ProviderEvent>> {
        if self.done {
            return Err(invalid("duplicate stream completion"));
        }
        if data == "[DONE]" {
            if self.output.finish_reason.is_none() || self.usage.is_none() {
                return Err(invalid("incomplete authenticated terminal event"));
            }
            self.done = true;
            return Ok(Vec::new());
        }
        let value: Value =
            serde_json::from_str(data).map_err(|_| invalid("invalid authenticated stream JSON"))?;
        if value.get("error").is_some() {
            return Err(invalid("Tinfoil reported an inference error"));
        }
        identity(&value, &mut self.id, &mut self.wire_model)?;
        if let Some(raw) = value.get("usage").filter(|v| !v.is_null()) {
            let current = usage(raw)?;
            if let Some(previous) = &self.usage
                && (current.input_tokens < previous.input_tokens
                    || current.output_tokens < previous.output_tokens)
            {
                return Err(invalid("Tinfoil usage went backwards"));
            }
            self.usage = Some(current);
        }
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("missing stream choices"))?;
        if choices.len() > 1 {
            return Err(invalid("unexpected number of stream choices"));
        }
        let Some(choice) = choices.first() else {
            return Ok(Vec::new());
        };
        if choice.get("index").and_then(Value::as_u64) != Some(0)
            || self.output.finish_reason.is_some()
        {
            return Err(invalid("invalid stream choice ordering"));
        }
        let delta = choice
            .get("delta")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("missing stream delta"))?;
        if delta
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|r| r != "assistant")
        {
            return Err(invalid("invalid assistant role"));
        }
        let mut events = Vec::new();
        for (value, target) in [
            (delta.get("content"), 0),
            (reasoning_value(delta)?, 1),
            (delta.get("refusal"), 2),
        ] {
            if let Some(value) = value.filter(|v| !v.is_null()) {
                let text = value
                    .as_str()
                    .ok_or_else(|| invalid("nontext assistant delta"))?;
                if text.is_empty() {
                    continue;
                }
                let destination = match target {
                    0 => &mut self.output.assistant.text,
                    1 => self.output.reasoning.get_or_insert_with(String::new),
                    _ => self.output.refusal.get_or_insert_with(String::new),
                };
                append(destination, text)?;
                events.push(match target {
                    0 => ProviderEvent::TextDelta(text.into()),
                    1 => ProviderEvent::ReasoningDelta(text.into()),
                    _ => ProviderEvent::RefusalDelta(text.into()),
                });
            }
        }
        if let Some(raw) = delta.get("tool_calls").filter(|v| !v.is_null()) {
            let calls = raw
                .as_array()
                .ok_or_else(|| invalid("invalid tool deltas"))?;
            if calls.len() > axiom_inference::MAX_TOOL_CALLS {
                return Err(invalid("too many tool deltas"));
            }
            for call in calls {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| invalid("missing tool delta index"))?;
                let delta = ToolCallDelta {
                    index,
                    id: optional_text(call.get("id"))?,
                    kind: optional_text(call.get("type"))?,
                    name: optional_text(call.pointer("/function/name"))?,
                    arguments: optional_text(call.pointer("/function/arguments"))?,
                };
                self.calls
                    .push(delta.clone())
                    .map_err(|_| invalid("invalid tool-call sequence"))?;
                events.push(ProviderEvent::ToolCallDelta(delta));
            }
        }
        if let Some(raw) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            self.output.finish_reason = Some(finish_reason(raw)?);
        }
        Ok(events)
    }

    pub(super) fn finish(mut self) -> Result<InferenceResponse> {
        if !self.done || !self.buffer.is_empty() || !self.data.is_empty() || self.id.is_none() {
            return Err(invalid(
                "Tinfoil stream ended without authenticated completion",
            ));
        }
        // Response model names may be deployment aliases. The encrypted request
        // selects the exact model; the attested router enforces X-Tinfoil-Model.
        if self.model.is_empty() {
            return Err(invalid("missing requested model"));
        }
        self.output.usage = self
            .usage
            .ok_or_else(|| invalid("missing authenticated usage"))?;
        self.output.assistant.tool_calls = self
            .calls
            .finish()
            .map_err(|_| invalid("incomplete tool calls"))?;
        validate_finish(&mut self.output)?;
        self.output
            .assistant
            .reasoning
            .clone_from(&self.output.reasoning);
        Ok(self.output)
    }
}

pub(super) fn complete(body: &[u8], _model: &str) -> Result<InferenceResponse> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| invalid("incomplete authenticated response JSON"))?;
    if value.get("error").is_some() {
        return Err(invalid("Tinfoil reported an inference error"));
    }
    identity(&value, &mut None, &mut None)?;
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .filter(|v| v.len() == 1)
        .ok_or_else(|| invalid("invalid completion choices"))?;
    let choice = &choices[0];
    if choice.get("index").and_then(Value::as_u64) != Some(0) {
        return Err(invalid("invalid completion index"));
    }
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("missing assistant message"))?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(invalid("invalid completion role"));
    }
    let calls = match message.get("tool_calls").filter(|v| !v.is_null()) {
        Some(raw) => {
            serde_json::from_value(raw.clone()).map_err(|_| invalid("invalid tool calls"))?
        }
        None => Vec::new(),
    };
    axiom_inference::validate_tool_calls(&calls)
        .map_err(|_| invalid("invalid completion tool calls"))?;
    let reasoning = optional_text(reasoning_value(message)?)?;
    let mut output = InferenceResponse {
        assistant: AssistantTurn {
            text: optional_text(message.get("content"))?.unwrap_or_default(),
            reasoning: reasoning.clone(),
            tool_calls: calls,
        },
        reasoning,
        refusal: optional_text(message.get("refusal"))?,
        usage: usage(
            value
                .get("usage")
                .ok_or_else(|| invalid("missing authenticated usage"))?,
        )?,
        finish_reason: Some(finish_reason(
            choice
                .get("finish_reason")
                .ok_or_else(|| invalid("missing finish reason"))?,
        )?),
    };
    for text in [
        Some(output.assistant.text.as_str()),
        output.reasoning.as_deref(),
        output.refusal.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if text.len() > axiom_inference::MAX_MESSAGE_TEXT_BYTES {
            return Err(invalid("completion text is oversized"));
        }
    }
    validate_finish(&mut output)?;
    Ok(output)
}

fn identity(value: &Value, id: &mut Option<String>, model: &mut Option<String>) -> Result<()> {
    for (key, saved) in [("id", id), ("model", model)] {
        let actual = value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
            .ok_or_else(|| invalid("missing authenticated response identity"))?;
        if saved.as_deref().is_some_and(|s| s != actual) {
            return Err(invalid("stream response identity changed"));
        }
        *saved = Some(actual.into());
    }
    Ok(())
}

fn usage(value: &Value) -> Result<Usage> {
    let count = |field| {
        value
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("invalid authenticated token usage"))
    };
    let input_tokens = count("prompt_tokens")?;
    let output_tokens = count("completion_tokens")?;
    let total_tokens = count("total_tokens")?;
    if input_tokens.checked_add(output_tokens) != Some(total_tokens) {
        return Err(invalid("incoherent authenticated token usage"));
    }
    Ok(Usage {
        input_tokens,
        output_tokens,
        total_tokens,
    })
}
fn optional_text(value: Option<&Value>) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(invalid("invalid assistant text field")),
    }
}
fn reasoning_value(fields: &serde_json::Map<String, Value>) -> Result<Option<&Value>> {
    let reasoning = fields.get("reasoning").filter(|value| !value.is_null());
    let compatibility = fields
        .get("reasoning_content")
        .filter(|value| !value.is_null());
    for value in [reasoning, compatibility].into_iter().flatten() {
        if !value.is_string() {
            return Err(invalid("invalid assistant reasoning field"));
        }
    }
    // Tinfoil documents `reasoning`; keep the compatibility alias as a fallback.
    // Select one field so providers returning both do not duplicate the trace.
    Ok(reasoning
        .filter(|value| value.as_str() != Some(""))
        .or(compatibility)
        .or(reasoning))
}
fn append(target: &mut String, text: &str) -> Result<()> {
    if target.len().saturating_add(text.len()) > axiom_inference::MAX_MESSAGE_TEXT_BYTES {
        return Err(invalid("assistant text is oversized"));
    }
    target.push_str(text);
    Ok(())
}
fn finish_reason(value: &Value) -> Result<FinishReason> {
    Ok(match value.as_str() {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("content_filter") => FinishReason::ContentFilter,
        _ => return Err(invalid("unsupported authenticated finish reason")),
    })
}
fn validate_finish(output: &mut InferenceResponse) -> Result<()> {
    for call in &output.assistant.tool_calls {
        if !serde_json::from_str::<Value>(&call.function.arguments)
            .is_ok_and(|arguments| arguments.is_object())
        {
            return Err(invalid("incomplete or malformed tool arguments"));
        }
    }
    // Tinfoil's GLM, Kimi, Gemma and Llama deployments use `stop` for
    // complete forced tool calls. Normalize only fully validated calls.
    if !output.assistant.tool_calls.is_empty() && output.finish_reason == Some(FinishReason::Stop) {
        output.finish_reason = Some(FinishReason::ToolCalls);
    }
    if !output.assistant.tool_calls.is_empty()
        && output.finish_reason != Some(FinishReason::ToolCalls)
        || output.assistant.tool_calls.is_empty()
            && output.finish_reason == Some(FinishReason::ToolCalls)
    {
        return Err(invalid("tool calls disagree with finish reason"));
    }
    Ok(())
}

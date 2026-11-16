//! Shared planning for NEAR's encrypted JSON, before any worker dispatch.
use crate::Result;
use axiom_inference::{InferenceRequest, ModelInfo, ToolChoice};

/// Conservative operating budget matching NEAR's published JSON proxy default.
/// Axiom's larger inner/outer hard bounds do not prove an upstream deployment
/// accepts them. Keep room for the model context independently of this budget.
pub const NEAR_REQUEST_BUDGET_BYTES: usize = 10 * 1024 * 1024;

fn json_size(value: &impl serde::Serialize) -> Result<usize> {
    #[derive(Default)]
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(value.len());
            Ok(value.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count::default();
    serde_json::to_writer(&mut count, value)
        .map_err(|_| crate::SecureClientError::capability("Invalid request metadata."))?;
    Ok(count.0)
}

/// Upper bound after images have been prepared. Includes JSON content-array
/// escaping, the 72-byte per-field encryption envelope, hex and tool fields.
/// Structural headroom makes the same calculation safe for compaction plans.
pub fn near_request_bytes(model: &ModelInfo, request: &InferenceRequest) -> Result<usize> {
    let encrypted = |bytes: usize| bytes.saturating_add(72).saturating_mul(2);
    let mut size = 16 * 1024_usize + model.upstream_model.len().saturating_mul(6);
    for message in &request.messages {
        size = size.saturating_add(512);
        if let Some(id) = &message.tool_call_id {
            size = size.saturating_add(json_size(id)?);
        }
        let content = if message.images.is_empty() {
            message.content.len()
        } else {
            // Quotes/control characters in text are escaped inside the encrypted
            // content array. Canonical image base64 itself never needs escaping.
            message.images.iter().fold(
                json_size(&message.content)?.saturating_add(64),
                |total, image| {
                    total
                        .saturating_add(image.data.len())
                        .saturating_add(image.mime_type.len())
                        .saturating_add(128)
                },
            )
        };
        size = size.saturating_add(encrypted(content));
        for value in [&message.name, &message.refusal, &message.reasoning_content]
            .into_iter()
            .flatten()
        {
            size = size.saturating_add(encrypted(value.len()));
        }
        for call in &message.tool_calls {
            size = size
                .saturating_add(512)
                .saturating_add(json_size(&call.id)?)
                .saturating_add(encrypted(call.function.name.len()))
                .saturating_add(encrypted(call.function.arguments.len()));
        }
    }
    for tool in &request.tools {
        size = size
            .saturating_add(512)
            .saturating_add(encrypted(tool.function.name.len()))
            .saturating_add(encrypted(tool.function.description.len()))
            .saturating_add(encrypted(json_size(&tool.function.parameters)?));
    }
    if let ToolChoice::Named { name } = &request.tool_choice {
        size = size.saturating_add(encrypted(name.len()));
    }
    Ok(size)
}

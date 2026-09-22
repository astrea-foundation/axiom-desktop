//! Synthetic diagnostics: only metadata is printed, never message/key material.
//! Every request uses production fresh attestation, E2EE and receipt verification.
use axiom_inference::{
    AssistantTurn, ChatMessage, ChatRole, FunctionDefinition, InferenceRequest, ProviderEvent,
    ThinkingMode, ToolChoice, ToolDefinition,
};
use axiomcli::{
    auth::{AuthManager, ValidationStatus},
    provider::{InferenceProvider, SecureAxiomProvider},
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

async fn run(
    provider: &SecureAxiomProvider,
    request: InferenceRequest,
    phase: &str,
) -> anyhow::Result<AssistantTurn> {
    let (tx, mut rx) = mpsc::channel(64);
    let operation = provider.stream(request, tx, CancellationToken::new());
    tokio::pin!(operation);
    let start = Instant::now();
    let mut text = 0usize;
    let mut reasoning = 0usize;
    let mut first_delta_ms = None;
    let mut verified = false;
    let mut accounting = None;
    let mut observe = |event| match event {
        ProviderEvent::TextDelta(delta) => {
            text += delta.len();
            first_delta_ms.get_or_insert(start.elapsed().as_millis());
        }
        ProviderEvent::ReasoningDelta(delta) => {
            reasoning += delta.len();
            first_delta_ms.get_or_insert(start.elapsed().as_millis());
        }
        ProviderEvent::ResponseVerified => verified = true,
        ProviderEvent::Accounting(record) => accounting = Some(record),
        _ => {}
    };
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            event = rx.recv() => if let Some(event) = event { observe(event); }
        }
    };
    while let Ok(event) = rx.try_recv() {
        observe(event);
    }
    println!(
        "{}",
        serde_json::json!({"phase":phase, "verified": verified, "answer_bytes":text,
        "reasoning_bytes":reasoning, "first_delta_ms":first_delta_ms,
        "elapsed_ms":start.elapsed().as_millis(), "accounting":accounting,
        "success":result.is_ok()})
    );
    let assistant = result?;
    if let Some(record) = accounting {
        let recovered = provider
            .request_accounting(
                std::slice::from_ref(&record.request_id),
                CancellationToken::new(),
            )
            .await?;
        anyhow::ensure!(
            recovered.len() == 1,
            "Accounting lookup did not return the invocation"
        );
        anyhow::ensure!(
            recovered[0].cost_microusd == record.cost_microusd
                && recovered[0].input_tokens == record.input_tokens
                && recovered[0].output_tokens == record.output_tokens
                && recovered[0].reasoning_tokens == record.reasoning_tokens
                && recovered[0].settled
                && !recovered[0].response_verified,
            "Accounting lookup disagrees with the stream, or claims verification"
        );
        println!(
            "{}",
            serde_json::json!({"phase":phase, "accounting_reconciled":true})
        );
    }
    anyhow::ensure!(
        verified,
        "The response did not pass final receipt verification"
    );
    Ok(assistant)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let origin = std::env::var("AXIOM_BASE_URL")?;
    let auth = AuthManager::new(&origin, Duration::from_secs(20))?;
    match auth.validate().await {
        ValidationStatus::Valid(_) => {}
        ValidationStatus::Missing => anyhow::bail!("Native credential is missing"),
        ValidationStatus::Expired => anyhow::bail!("Native session has expired"),
        ValidationStatus::Unavailable(reason) => {
            anyhow::bail!("Native credential validation unavailable: {reason}")
        }
    }
    if std::env::args().nth(1).as_deref() == Some("status") {
        println!("Native authentication is valid");
        return Ok(());
    }
    let provider = SecureAxiomProvider::with_auth(&origin, auth, Duration::from_secs(120))?;
    let model = std::env::var("AXIOM_PROBE_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into());
    let tool_test = std::env::args().nth(2).as_deref() == Some("tool");
    let mut request = InferenceRequest::streaming(
        model,
        vec![
            ChatMessage::text(
                ChatRole::System,
                "You are a concise mathematical assistant. Use a provided tool when asked. Put only the answer and a short explanation in the answer channel.",
            ),
            ChatMessage::text(
                ChatRole::User,
                if tool_test {
                    "Use rectangle_dimensions to get a rectangle's dimensions, then give its area."
                } else {
                    "A rectangle has integer side lengths and perimeter 50. What is the largest possible area? Explain briefly."
                },
            ),
        ],
        vec![],
    );
    request.max_output_tokens = Some(2048);
    request.thinking_mode = match std::env::args().nth(1).as_deref() {
        Some("enabled") => ThinkingMode::Enabled,
        Some("disabled") => ThinkingMode::Disabled,
        _ => ThinkingMode::ProviderDefault,
    };
    if tool_test {
        request.tools.push(ToolDefinition { kind: "function".into(), function: FunctionDefinition {
            name: "rectangle_dimensions".into(), description: "Return the synthetic rectangle's integer side lengths.".into(),
            parameters: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}), strict: None,
        }});
        request.tool_choice = ToolChoice::Required;
    }
    let answer = run(&provider, request.clone(), "initial").await?;
    if tool_test {
        anyhow::ensure!(
            answer.tool_calls.len() == 1
                && answer.tool_calls[0].function.name == "rectangle_dimensions",
            "Expected the synthetic tool call"
        );
        let tool_id = answer.tool_calls[0].id.clone();
        let mut assistant = ChatMessage::text(ChatRole::Assistant, answer.text);
        assistant.reasoning_content = answer.reasoning;
        assistant.tool_calls = answer.tool_calls;
        request.messages.push(assistant);
        let mut tool = ChatMessage::text(ChatRole::Tool, r#"{"width":12,"height":13}"#);
        tool.tool_call_id = Some(tool_id);
        request.messages.push(tool);
        // Exercise lossless local history serialization before replay.
        request.messages = serde_json::from_slice(&serde_json::to_vec(&request.messages)?)?;
        request.tool_choice = ToolChoice::None;
        let answer = run(&provider, request, "tool_continuation").await?;
        anyhow::ensure!(
            answer.text.contains("156"),
            "Synthetic tool answer was incorrect"
        );
    }
    Ok(())
}

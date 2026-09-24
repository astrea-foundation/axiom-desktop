//! Bounded paid billing workload through Axiom's real attested E2EE relay.
//! Set `AXIOM_BILLING_MODEL` to one exact offering; logs only operational metadata.
//! Reads `AXIOM_API_KEY`, `AXIOM_RELAY_URL`, optional `AXIOM_BILLING_MODEL`,
//! `AXIOM_BILLING_SMOKE_ROUNDS` and `AXIOM_BILLING_SMOKE_PARALLEL`.
//! Never reads a provider credential.
use std::{env, fmt::Write as _, time::Duration};

use axiom_inference::{
    ChatMessage, ChatRole, InferenceRequest, ProviderEvent, ReasoningEffort, ToolChoice,
    ToolDefinition,
};
use axiom_secure_client::{
    ApiCredential, SecureClient, SecureClientConfig, TrustPolicy, VerifiedSession,
};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

async fn streamed(
    session: &mut dyn VerifiedSession,
    request: InferenceRequest,
) -> anyhow::Result<axiom_inference::InferenceResponse> {
    let (tx, mut rx) = mpsc::channel(128);
    let consumer = tokio::spawn(async move {
        let mut deltas = 0;
        let mut terminal = 0;
        while let Some(event) = rx.recv().await {
            match event {
                ProviderEvent::TextDelta(_) | ProviderEvent::ToolCallDelta(_) => deltas += 1,
                ProviderEvent::Finished(_) => terminal += 1,
                ProviderEvent::Accounting(usage) => {
                    println!("{}", json!({"kind":"accounting","usage":usage}));
                }
                _ => {}
            }
        }
        (deltas, terminal)
    });
    let result = session.stream(request, tx, CancellationToken::new()).await;
    let (deltas, terminal) = consumer.await?;
    let output = result?;
    anyhow::ensure!(
        deltas > 0 && terminal == 1,
        "stream omitted deltas or terminal event"
    );
    Ok(output)
}

async fn measured(
    session: &mut dyn VerifiedSession,
    mut request: InferenceRequest,
    label: &str,
) -> anyhow::Result<axiom_inference::InferenceResponse> {
    use rand_core::RngCore;
    let mut bytes = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut bytes);
    let id = hex::encode(bytes);
    request.request_id = Some(id.clone());
    let stream = request.stream;
    println!(
        "{}",
        json!({"kind":"request_start","request_id":id,"label":label,"stream":stream})
    );
    let result = if stream {
        streamed(session, request).await
    } else {
        session
            .complete(request, CancellationToken::new())
            .await
            .map_err(anyhow::Error::from)
    };
    match result {
        Ok(output) => {
            println!(
                "{}",
                json!({"kind":"request_verified","request_id":id,"label":label,"stream":stream,"usage":output.usage,"finish_reason":output.finish_reason})
            );
            Ok(output)
        }
        Err(error) => {
            println!(
                "{}",
                json!({"kind":"request_error","request_id":id,"label":label,"error":error.to_string()})
            );
            Err(error)
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let relay = env::var("AXIOM_RELAY_URL")?;
    let mut config = if relay.starts_with("http://") {
        #[cfg(feature = "test-fixture")]
        {
            anyhow::ensure!(
                env::var("AXIOM_BILLING_ALLOW_LOCAL_RELAY").as_deref() == Ok("1"),
                "local relay requires explicit opt-in"
            );
            let url = url::Url::parse(&relay)?;
            anyhow::ensure!(
                url.host_str() == Some("127.0.0.1"),
                "local canary requires loopback"
            );
            SecureClientConfig::new_test_fixture(&relay)?
        }
        #[cfg(not(feature = "test-fixture"))]
        anyhow::bail!("local relay requires test-fixture feature and explicit opt-in");
    } else {
        SecureClientConfig::new(&relay)?
    };
    config.attestation_timeout = Duration::from_secs(180);
    config.request_timeout = Duration::from_secs(120);
    config.verified_session_ttl = Duration::from_secs(240);
    let client = SecureClient::new(config, ApiCredential::new(env::var("AXIOM_API_KEY")?))?;
    let models = client.models(CancellationToken::new()).await?;
    let policy = TrustPolicy::production()?;
    let selected = env::var("AXIOM_BILLING_MODEL")?;
    let effort = match env::var("AXIOM_BILLING_REASONING_EFFORT")
        .as_deref()
        .unwrap_or("low")
    {
        "low" => ReasoningEffort::Low,
        "medium" => ReasoningEffort::Medium,
        "high" => ReasoningEffort::High,
        _ => anyhow::bail!("canary effort must be low, medium or high"),
    };
    let rounds = env::var("AXIOM_BILLING_SMOKE_ROUNDS")
        .ok()
        .map_or(Ok(1), |n| n.parse::<u32>())?;
    anyhow::ensure!(
        (1..=3).contains(&rounds),
        "canary is limited to three rounds"
    );
    let mut tested = 0;
    let mut failed = 0;
    for model in models.iter().filter(|m| m.id == selected) {
        tested += 1;
        eprintln!("Verifying and testing {}", model.id);
        let result: anyhow::Result<()> = async {
            anyhow::ensure!(model.context_window_tokens > 0 && model.auto_compact_threshold_tokens() > 0, "missing context window");
            let mut session = client.establish(model, &policy, CancellationToken::new()).await?;
            let mut totals = [0u64; 2];
            let mut records = String::new();
            for i in 0..200 {
                writeln!(records, "Synthetic record {i:03}: category=test, units={}, status=ready.", i % 10)?;
            }
            for (label, prompt, stream) in [
                ("input-heavy", format!("Count the records below and report the count in one sentence.\n{records}"), false),
                ("input-heavy-repeat", format!("Count the records below and report the count in one sentence.\n{records}"), true),
                ("output-heavy", "Write 250 to 350 words explaining how to compare two usage bills. Use numbered steps. This is a synthetic test; do not call tools.".to_string(), true),
            ] {
                let mut request = InferenceRequest::streaming(&model.id, vec![ChatMessage::text(ChatRole::User, prompt)], vec![]);
                request.stream = stream;
                request.max_output_tokens = Some(1024);
                request.reasoning_effort = effort;
                let output = measured(session.as_mut(), request, label).await?;
                totals[0] += output.usage.input_tokens;
                totals[1] += output.usage.output_tokens;
                anyhow::ensure!(!output.assistant.text.is_empty(), "missing synthetic workload response");
            }
            let tool: ToolDefinition = serde_json::from_value(json!({"type":"function","function":{
                "name":"lookup_code", "description":"Return the private test code for a key.",
                "parameters":{"type":"object","properties":{"key":{"type":"string","enum":["alpha","beta","gamma"]}},"required":["key"],"additionalProperties":false}
            }}))?;
            let mut messages = vec![ChatMessage::text(ChatRole::System, "Use the provided tool to retrieve codes. Keep responses brief.")];
            for round in 0..rounds {
                let key = ["alpha", "beta", "gamma"][usize::try_from(round)?];
                let code = format!("CANARY-{}", 914 + round);
                messages.push(ChatMessage::text(ChatRole::User, format!("Call lookup_code with key {key}, then reply with only its returned code.")));
                let mut request = InferenceRequest::streaming(&model.id, messages.clone(), vec![tool.clone()]);
                request.reasoning_effort = effort;
                request.max_output_tokens = Some(2048);
                request.tool_choice = ToolChoice::Named { name: "lookup_code".into() };
                request.stream = round % 2 == 1;
                let turn = measured(session.as_mut(), request, &format!("tool-call-{round}")).await?;
                totals[0] += turn.usage.input_tokens;
                totals[1] += turn.usage.output_tokens;
                anyhow::ensure!(turn.assistant.tool_calls.len() == 1, "expected one real tool call");
                let call = &turn.assistant.tool_calls[0];
                anyhow::ensure!(call.function.name == "lookup_code" && serde_json::from_str::<serde_json::Value>(&call.function.arguments)? == json!({"key":key}), "unexpected tool call");
                let mut assistant = ChatMessage::text(ChatRole::Assistant, &turn.assistant.text);
                assistant.reasoning_content.clone_from(&turn.reasoning);
                assistant.tool_calls.clone_from(&turn.assistant.tool_calls);
                messages.push(assistant);
                let mut result = ChatMessage::text(ChatRole::Tool, json!({"code":code}).to_string());
                result.tool_call_id = Some(call.id.clone());
                messages.push(result);
                let mut request = InferenceRequest::streaming(&model.id, messages.clone(), vec![tool.clone()]);
                request.reasoning_effort = effort;
                request.max_output_tokens = Some(2048);
                request.tool_choice = ToolChoice::None;
                let answer = measured(session.as_mut(), request, &format!("tool-result-{round}")).await?;
                anyhow::ensure!(answer.assistant.text.contains(&code) && answer.assistant.tool_calls.is_empty(), "tool result was not used");
                totals[0] += answer.usage.input_tokens;
                totals[1] += answer.usage.output_tokens;
                let mut assistant = ChatMessage::text(ChatRole::Assistant, &answer.assistant.text);
                assistant.reasoning_content = answer.reasoning;
                messages.push(assistant);
            }
            let test_parallel = model.supports_parallel_tools && env::var("AXIOM_BILLING_SMOKE_PARALLEL").as_deref() == Ok("1");
            if test_parallel {
                let mut request = InferenceRequest::streaming(&model.id,
                    vec![ChatMessage::text(ChatRole::User, "Call lookup_code twice now, once for alpha and once for beta. Return both function calls in this turn; do not answer in text.")], vec![tool]);
                request.tool_choice = ToolChoice::Required;
                request.parallel_tool_calls = Some(true);
                request.reasoning_effort = effort;
                request.max_output_tokens = Some(2048);
                let output = measured(session.as_mut(), request, "parallel-tools").await?;
                let mut keys = output.assistant.tool_calls.iter().map(|call| {
                    serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                        .ok().and_then(|value| value["key"].as_str().map(str::to_owned)).unwrap_or_default()
                }).collect::<Vec<_>>();
                keys.sort();
                anyhow::ensure!(keys == ["alpha", "beta"], "parallel tool calls were not honored");
                totals[0] += output.usage.input_tokens;
                totals[1] += output.usage.output_tokens;
            }
            println!("{}", json!({"model":model.id,"status":"passed","reasoning_effort":effort.as_str(),"rounds":rounds,"parallel_tools_supported":model.supports_parallel_tools,"parallel_tools_tested":test_parallel,"input_tokens":totals[0],"output_tokens":totals[1],"context_window":model.context_window_tokens,"auto_compact_at":model.auto_compact_threshold_tokens(),"protocol":session.evidence().e2ee_protocol}));
            Ok(())
        }.await;
        if let Err(error) = result {
            failed += 1;
            println!(
                "{}",
                json!({"model":model.id,"status":"failed","error":error.to_string()})
            );
        }
    }
    anyhow::ensure!(
        tested > 0 && failed == 0,
        "{failed} of {tested} model canaries failed"
    );
    Ok(())
}

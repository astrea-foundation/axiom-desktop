//! Explicit live smoke test: fresh native verification and authenticated E2EE
//! completion for every discovered chat model. Never opens conversation storage.
use std::{
    env,
    time::{Duration, Instant},
};

use axiom_inference::{
    ChatMessage, ChatRole, InferenceRequest, ProviderEvent, ReasoningEffort, ThinkingMode,
};
use axiom_secure_client::{ApiCredential, SecureClient, SecureClientConfig, TrustPolicy};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let relay = env::var("AXIOM_RELAY_URL").unwrap_or_else(|_| "https://api.axiom.stream".into());
    let local = relay.starts_with("http://");
    let mut config = if local {
        let url = url::Url::parse(&relay)?;
        anyhow::ensure!(
            url.host_str() == Some("127.0.0.1")
                && env::var("AXIOM_CANARY_LOCAL_RELAY").as_deref() == Ok("1"),
            "local canary requires explicit loopback opt-in"
        );
        #[cfg(feature = "test-fixture")]
        {
            SecureClientConfig::new_test_fixture(&relay)?
        }
        #[cfg(not(feature = "test-fixture"))]
        {
            anyhow::bail!("local canary requires the test-fixture feature");
        }
    } else {
        SecureClientConfig::new(&relay)?
    };
    config.attestation_timeout = Duration::from_secs(90);
    config.request_timeout = Duration::from_secs(150);
    // Explicit automation credential only: never open the OS credential store
    // or read/refresh an interactive Desktop account during unattended tests.
    let credential = ApiCredential::new(env::var("AXIOM_API_KEY").map_err(|_| {
        anyhow::anyhow!("AXIOM_API_KEY is required; this canary never opens the system Keychain")
    })?);
    let client = SecureClient::new(config, credential)?;
    let policy = TrustPolicy::production()?;
    let models = client.models(CancellationToken::new()).await?;
    println!(
        "{}",
        json!({"kind":"catalog", "relay":relay, "count":models.len(),
        "models":models.iter().map(|m| json!({"id":m.id,"provider":m.provider_id,"label":m.provider_label,"upstream":m.upstream_model,"attestation":m.attestation_protocol,"context_window":m.context_window_tokens})).collect::<Vec<_>>()})
    );
    if env::var("AXIOM_CANARY_LIST_ONLY").as_deref() == Ok("1") {
        return Ok(());
    }
    let selected = env::var("AXIOM_CANARY_MODEL").ok();
    let provider = env::var("AXIOM_CANARY_PROVIDER").ok();
    let mut passed = 0;
    let mut failed = 0;
    for model in models.iter().filter(|m| {
        selected.as_ref().is_none_or(|id| &m.id == id)
            && provider.as_ref().is_none_or(|p| &m.provider_id == p)
    }) {
        let start = Instant::now();
        let mut stage = "attestation";
        let result: anyhow::Result<serde_json::Value> = async {
            let mut session = tokio::time::timeout(Duration::from_secs(150), client.establish(model, &policy, CancellationToken::new())).await??;
            let evidence = session.evidence().clone();
            stage = "completion";
            let mut request = InferenceRequest::streaming(&model.id, vec![ChatMessage::text(ChatRole::User, "Reply with just the word OK.")], vec![]);
            request.max_output_tokens = Some(512);
            if model.supported_reasoning_efforts.contains(&ReasoningEffort::Low) { request.reasoning_effort = ReasoningEffort::Low; }
            if model.supported_thinking_modes.contains(&ThinkingMode::Disabled) { request.thinking_mode = ThinkingMode::Disabled; }
            let (tx, mut rx) = mpsc::channel(128);
            let consumer = tokio::spawn(async move {
                let (mut verified, mut terminal, mut ordered) = (0,0,true);
                while let Some(event) = rx.recv().await {
                    match event {
                        ProviderEvent::ResponseVerified => verified += 1,
                        ProviderEvent::Finished(_) => { terminal += 1; ordered &= verified == 1; },
                        _ => {},
                    }
                }
                (verified,terminal,ordered)
            });
            let response = tokio::time::timeout(Duration::from_secs(180), session.stream(request, tx, CancellationToken::new())).await;
            let (verified,terminal,ordered) = consumer.await?;
            let response = response??;
            // NEAR's provider finish is provisional until its later signed
            // receipt; successful stream return plus ResponseVerified is final.
            anyhow::ensure!(verified == 1 && terminal == 1, "missing authenticated completion");
            anyhow::ensure!(model.provider_id != "tinfoil" || ordered, "Tinfoil verification followed its terminal event");
            anyhow::ensure!(response.assistant.text.trim() == "OK", "authenticated reply did not match the test word OK");
            Ok(json!({"reply":response.assistant.text,"input_tokens":response.usage.input_tokens,"output_tokens":response.usage.output_tokens,
                "protocol":evidence.e2ee_protocol,"attestation":evidence.attestation_protocol,"security_state":format!("{:?}",evidence.state),"verified_terminal":true,"verification_before_provider_finish":ordered}))
        }.await;
        match result {
            Ok(detail) => {
                passed += 1;
                println!(
                    "{}",
                    json!({"kind":"result","status":"passed","model":model.id,"provider":model.provider_id,"seconds":start.elapsed().as_secs_f64(),"detail":detail})
                );
            }
            Err(error) => {
                failed += 1;
                println!(
                    "{}",
                    json!({"kind":"result","status":"failed","stage":stage,"model":model.id,"provider":model.provider_id,"seconds":start.elapsed().as_secs_f64(),"error":error.to_string()})
                );
            }
        }
    }
    println!(
        "{}",
        json!({"kind":"summary","passed":passed,"failed":failed})
    );
    anyhow::ensure!(
        passed + failed > 0 && failed == 0,
        "one or more model canaries failed"
    );
    Ok(())
}

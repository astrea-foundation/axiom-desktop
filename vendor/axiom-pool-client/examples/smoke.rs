//! Synthetic HTTP/GPU qualification only. This binary cannot attest hardware.
use anyhow::{Context, Result, ensure};
use axiom_pool_client::{PoolClient, attestation::VerifierConfig};
use axiom_pool_protocol::{
    strict,
    types::{Input, Limits, Message},
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    origin: String,
    issuer_key: String,
    worker_identity_key: String,
    credential: String,
    model_manifest: String,
    operation: String,
    expected_text: Option<String>,
    #[serde(default)]
    image_size: Option<u32>,
    #[serde(default)]
    maximum_charge_microusd: u64,
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("synthetic smoke failed; no inference was replayed");
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    ensure!(
        std::env::var("AXIOM_POOL_TEST_ONLY").as_deref() == Ok("1"),
        "test-only process required"
    );
    let raw = std::fs::read(std::env::args().nth(1).context("fixture config")?)?;
    ensure!(raw.len() < 128 * 1024, "config size");
    let cfg: Config = strict::decode(&raw)?;
    let verifier = VerifierConfig::fixture(&cfg.worker_identity_key)?;
    let client = PoolClient::fixture(
        cfg.origin,
        &cfg.issuer_key,
        secrecy::SecretString::new(cfg.credential.into_boxed_str()),
        verifier,
    )?
    .with_maximum_charge(cfg.maximum_charge_microusd)?;
    let cancel = CancellationToken::new();
    let pinned = client.probe(&cfg.model_manifest, &cancel).await?;
    let input = if cfg.operation == "chat" {
        Input::Chat {
            messages: vec![Message {
                tool_calls: vec![],
                tool_call_id: None,
                role: "user".into(),
                images: vec![],
                content: "Write a Python function that adds two integers. Synthetic test.".into(),
            }],
            max_tokens: 256,
            reasoning_effort: "medium".into(),
            tools: vec![],
        }
    } else {
        ensure!(cfg.operation == "image", "operation");
        Input::Image {
            uncensored: false,
            prompt: "A blue square on a white background. Synthetic test.".into(),
            width: cfg.image_size.unwrap_or(512),
            height: cfg.image_size.unwrap_or(512),
            steps: 8,
            seed: 42,
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let limits = Limits {
        context_tokens: 2048,
        output_tokens: 256,
        input_bytes: 1024 * 1024,
        output_bytes: 16 * 1024 * 1024,
        image_pixels: 1024 * 1024,
        image_steps: 8,
        input_image_pixels: 0,
        deadline: now + 600,
    };
    let prepared = client
        .prepare_pinned(
            &cfg.model_manifest,
            input,
            limits,
            None,
            Some(&pinned),
            &cancel,
        )
        .await?;
    let output = client.execute(prepared, |_| {}, cancel).await?;
    if let Some(expected) = cfg.expected_text {
        ensure!(output.text == expected, "synthetic answer mismatch")
    }
    ensure!(
        if cfg.operation == "image" {
            output.image.is_some()
        } else {
            !output.text.is_empty()
        },
        "missing result"
    );
    println!(
        "{}",
        serde_json::json!({"test_fixture_only":true,"hardware_attested":false,"authenticated_completion":true,"operation":cfg.operation,"text_bytes":output.text.len(),"image_bytes":output.image.as_ref().map(Vec::len),"usage":output.terminal.usage,"billing":output.billing})
    );
    Ok(())
}

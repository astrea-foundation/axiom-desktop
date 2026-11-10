//! Measure one cold preparation and independently authenticated warm requests.
use anyhow::{Context, Result};
use axiom_pool_client::direct::{DirectClient, DirectConfig};
use axiom_pool_protocol::types::{Input, Limits, Message};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("direct configuration required")?;
    let config: DirectConfig = axiom_pool_protocol::strict::decode(&std::fs::read(path)?)?;
    let client = DirectClient::new(config)?;
    let start = Instant::now();
    let worker = client.prepare(&CancellationToken::new()).await?;
    println!(
        "{}",
        serde_json::json!({"preparation_ms":start.elapsed().as_millis(),"generation":worker.generation(),"policy_sequence":worker.policy_sequence()})
    );
    for index in 1..=3 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let input = Input::Chat {
            messages: vec![Message::text(
                "user",
                format!("Reply with exactly LIME-{index}."),
            )],
            max_tokens: 128,
            reasoning_effort: "none".into(),
            tools: vec![],
        };
        let (_, output) = client
            .execute_with(
                input,
                Limits {
                    context_tokens: 8192,
                    output_tokens: 128,
                    input_bytes: 1024 * 1024,
                    output_bytes: 4 * 1024 * 1024,
                    image_pixels: 0,
                    image_steps: 0,
                    input_image_pixels: 0,
                    deadline: now + 180,
                },
                Some(&worker),
                |_| async { Ok(()) },
                CancellationToken::new(),
            )
            .await?;
        anyhow::ensure!(
            output.text.trim() == format!("LIME-{index}"),
            "unexpected authenticated response"
        );
        println!(
            "{}",
            serde_json::json!({"index":index,"text":output.text,"timings":output.direct_timings,"terminal":output.terminal,"response_authenticated":true})
        );
    }
    Ok(())
}

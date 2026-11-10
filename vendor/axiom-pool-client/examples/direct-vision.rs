//! Real, attested inline-image inference using an installed operator config.
use anyhow::{Context, Result, ensure};
use axiom_pool_client::direct::{DirectClient, DirectConfig};
use axiom_pool_protocol::{
    types::{Input, Limits, Message},
    vision::InlineImage,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("attested vision request failed: {error}; inference was not replayed");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let cfg = PathBuf::from(
        args.next()
            .context("installed direct configuration required")?,
    );
    let image = PathBuf::from(args.next().context("local image required")?);
    let prompt = args
        .next()
        .context("text question required")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("UTF-8 question required"))?;
    ensure!(args.next().is_none(), "unexpected arguments");
    let config: DirectConfig = axiom_pool_protocol::strict::decode(&std::fs::read(cfg)?)?;
    ensure!(config.vision, "installed vision capability required");
    let mime = match image
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => anyhow::bail!("unsupported image extension"),
    };
    let raw = std::fs::read(image)?;
    let image = InlineImage::from_upload(mime, &STANDARD.encode(raw))?;
    let pixels = image.pixels();
    let client = DirectClient::new(config)?;
    let input = Input::Chat {
        messages: vec![Message {
            tool_calls: vec![],
            tool_call_id: None,
            role: "user".into(),
            content: prompt,
            images: vec![image],
        }],
        max_tokens: 768,
        reasoning_effort: "medium".into(),
        tools: vec![],
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let limits = Limits {
        context_tokens: 8192,
        output_tokens: 768,
        input_bytes: axiom_pool_protocol::MAX_REQUEST as u64,
        output_bytes: 32 * 1024 * 1024,
        image_pixels: 0,
        image_steps: 0,
        input_image_pixels: pixels,
        deadline: now + 180,
    };
    let (worker, output) = client
        .execute_with(
            input,
            limits,
            None,
            |_| async { Ok(()) },
            CancellationToken::new(),
        )
        .await?;
    println!(
        "{}",
        json!({"text":output.text,"terminal":output.terminal,"worker":worker.identity(),
        "generation":worker.generation(),"policy_sequence":worker.policy_sequence(),
        "capabilities":worker.capabilities(),"hardware_verified":true,"response_authenticated":true})
    );
    Ok(())
}

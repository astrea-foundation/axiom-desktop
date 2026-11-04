//! Synthetic direct-worker load/recovery qualification. Never reads chat storage.
use anyhow::{Context, Result, ensure};
use axiom_pool_client::direct::{DirectClient, DirectConfig};
use axiom_pool_protocol::{
    frames::Rejection,
    types::{Input, Limits, Message},
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let config: DirectConfig = axiom_pool_protocol::strict::decode(&std::fs::read(
        args.get(1).context("config file required")?,
    )?)?;
    let count: usize = args.get(2).context("request count required")?.parse()?;
    ensure!(
        (1..=24).contains(&count),
        "1..24 synthetic requests required"
    );
    let cancel_active = args.get(3).is_some_and(|v| v == "cancel-active");
    ensure!(
        !cancel_active || count == 1,
        "recovery qualification uses one synthetic request"
    );
    let client = Arc::new(DirectClient::new(config)?);
    let barrier = Arc::new(tokio::sync::Barrier::new(count));
    let mut tasks = Vec::new();
    for index in 0..count {
        let client = client.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let canary=format!("SYNTHETIC-CANARY-{index:02}");
            let input=Input::Chat { messages:vec![Message::text("user",format!(
                "Start your final answer with the exact marker {canary}. Then write a numbered list of 150 different two-word fictional animal names. This is a synthetic capacity test."))],
                max_tokens:768,reasoning_effort:"medium".into(),tools:vec![] };
            let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let limits=Limits{context_tokens:32768,output_tokens:768,input_bytes:1024*1024,
                output_bytes:1024*1024,image_pixels:0,image_steps:0,input_image_pixels:0,deadline:now+180};
            let cancel=CancellationToken::new(); let stop=cancel.clone();
            barrier.wait().await; let started=Instant::now();
            let result=client.execute_with(input,limits,None,move |frame| {let stop=stop.clone();async move {
                if cancel_active && matches!(frame.kind.as_str(),"text_delta"|"reasoning_delta") { stop.cancel(); }
                Ok(())
            }},cancel.clone()).await;
            match result {
                Ok((worker,output))=>json!({"index":index,"status":"succeeded","ms":started.elapsed().as_millis(),
                    "canary_present":output.text.contains(&canary),"text_bytes":output.text.len(),
                    "policy_sequence":worker.policy_sequence(),"boot_id":worker.identity().boot_id,
                    "timings":output.direct_timings,"usage":output.terminal.usage}),
                Err(error) if error.downcast_ref::<Rejection>().is_some()=>json!({"index":index,"status":"busy",
                    "ms":started.elapsed().as_millis(),"code":error.downcast_ref::<Rejection>().unwrap().code}),
                Err(_) if cancel.is_cancelled()=>json!({"index":index,"status":"synthetic-cancelled","ms":started.elapsed().as_millis()}),
                Err(_)=>json!({"index":index,"status":"failed","ms":started.elapsed().as_millis()}),
            }
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await?);
    }
    println!(
        "{}",
        json!({"thinking":"on","synthetic_only":true,"requests":results})
    );
    Ok(())
}

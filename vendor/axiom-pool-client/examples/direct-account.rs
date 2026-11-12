//! Account-authorized direct E2EE qualification, using only a private token file.
use anyhow::{Context, Result, ensure};
use axiom_pool_client::direct::{DirectClient, DirectConfig};
use axiom_pool_protocol::types::{Input, Limits};
use serde::Deserialize;
use std::{path::PathBuf, time::Instant};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    worker: DirectConfig,
    credential_file: PathBuf,
    input: Input,
    limits: Limits,
    image_output: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("provide a private request configuration")?;
    let cfg: Config = axiom_pool_protocol::strict::decode(&std::fs::read(path)?)?;
    let meta = std::fs::symlink_metadata(&cfg.credential_file)?;
    ensure!(
        cfg.credential_file.is_absolute() && meta.is_file(),
        "ordinary absolute token file required"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            meta.permissions().mode() & 0o077 == 0,
            "private token file permissions required"
        );
    }
    let token = zeroize::Zeroizing::new(std::fs::read_to_string(&cfg.credential_file)?);
    let client = DirectClient::new_account(
        cfg.worker,
        secrecy::SecretString::from(token.trim().to_owned()),
    )?;
    let started = Instant::now();
    let (worker, output) = client
        .execute_with(
            cfg.input,
            cfg.limits,
            None,
            |_| async { Ok(()) },
            CancellationToken::new(),
        )
        .await?;
    let image_sha256 = output.image.as_ref().map(axiom_pool_protocol::sha256);
    if let (Some(bytes), Some(path)) = (&output.image, cfg.image_output) {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)?.write_all(bytes)?;
    }
    println!(
        "{}",
        serde_json::json!({
            "worker":worker.identity(),"text":output.text,"reasoning":output.reasoning,
            "tool_calls":output.tool_calls,"terminal":output.terminal,"billing":output.billing,
            "timings":output.direct_timings,"image_sha256":image_sha256,
            "total_ms":started.elapsed().as_millis()
        })
    );
    Ok(())
}

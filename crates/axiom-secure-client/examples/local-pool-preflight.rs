//! Public local metadata and native attestation preflight. Sends no prompt and
//! has no inference credential. An unqualified worker exits with status 2.
use axiom_secure_client::{
    ApiCredential, ProviderRegistry, SecureClientConfig, TrustPolicy, local_relay_ca_pem,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Local preflight failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let origin = std::env::var("AXIOM_BASE_URL")?;
    let config = SecureClientConfig::new(&origin)?;
    anyhow::ensure!(
        matches!(
            config.relay_base_url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        ),
        "preflight is limited to HTTPS loopback"
    );
    let installed = config
        .pool
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no installed pool"))?;
    anyhow::ensure!(
        installed.models.len() == 1 && installed.models[0].attestation_protocol == "chutes-gpu-v1",
        "this public-only preflight is limited to the unqualified Chutes offering"
    );
    let offering = &installed.models[0];
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10));
    if let Some(pem) = local_relay_ca_pem(&config.relay_base_url)? {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);
    }
    let http = builder.build()?;
    let mut response = http
        .get(format!(
            "{}/api/v1/pool/models",
            origin.trim_end_matches('/')
        ))
        .send()
        .await?
        .error_for_status()?;
    let mut raw = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(raw.len() + chunk.len() <= 128 * 1024, "metadata size limit");
        raw.extend_from_slice(&chunk);
    }
    let metadata: serde_json::Value = serde_json::from_slice(&raw)?;
    anyhow::ensure!(
        metadata["issuer_key"].as_str() == Some(installed.issuer_key.as_str())
            && metadata["models"].as_array().is_some_and(|models| models
                .iter()
                .any(|model| model["model_manifest"].as_str()
                    == Some(offering.model_manifest.as_str()))),
        "local public configuration differs from installed pins"
    );

    let model = axiom_inference::ModelInfo {
        id: format!("pool:{}", offering.model_manifest),
        provider_id: "axiom-pool".into(),
        provider_base_url: origin.trim_end_matches('/').into(),
        upstream_model: offering.model_manifest.clone(),
        e2ee_protocol: "axiom-pool-e2ee-v1".into(),
        e2ee_encryption_version: 1,
        attestation_protocol: offering.attestation_protocol.clone(),
        context_window_tokens: offering.context_tokens,
        max_output_tokens: offering.output_tokens,
        ..Default::default()
    };
    // No inference credential is loaded or granted by a public metadata check.
    let registry = ProviderRegistry::production(
        config,
        Arc::new(ApiCredential::new("public-metadata-preflight")),
    )?;
    let policy = TrustPolicy::production()?;
    match registry
        .establish(&model, &policy, CancellationToken::new())
        .await
    {
        Ok(_) => anyhow::bail!("unexpectedly admitted an unqualified preview"),
        Err(error) => {
            println!(
                "{}",
                json!({"backend_https_verified":true, "installed_pins_match":true,
                "model":model.id, "security_state":"blocked", "code":error.code().as_str(),
                "message":error.safe_detail(), "prompt_sent":false,
                "desktop_launch_permitted":false})
            );
            std::process::exit(2);
        }
    }
}

//! Explicit local operator connection to a measured SNP/NVIDIA worker.
use crate::{
    ApiCredential, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecurityEvidence, SecurityState, TrustPolicy,
    provider::{SecureProvider, VerifiedSession, sealed},
};
use async_trait::async_trait;
use axiom_inference::{
    AssistantTurn, ChatRole, FinishReason, InferenceRequest, InferenceResponse, ModelInfo,
    ProviderEvent, ReasoningEffort, ResponseFormat, SamplingParameters, ThinkingMode, Usage,
};
use axiom_pool_client::{
    attestation::VerifiedWorker,
    direct::{DirectClient, DirectConfig, ExecutionOptions},
};
use axiom_pool_protocol::types::{Input, Limits, Message};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn exchange_error(error: &anyhow::Error, cancelled: bool, image: bool) -> SecureClientError {
    if cancelled {
        return SecureClientError::cancelled();
    }
    if error.is::<axiom_pool_client::InsufficientCredit>() {
        return SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::InsufficientCredit,
            crate::error::SecureErrorCode::InsufficientCredit,
            "Insufficient balance. Top up your Axiom balance to continue.",
            false,
        );
    }
    if error.is::<axiom_pool_client::AuthenticationRejected>() {
        return SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::Authentication,
            crate::error::SecureErrorCode::Authentication,
            "Your Axiom session is no longer valid. Sign in again.",
            false,
        );
    }
    if let Some(capacity) = error.downcast_ref::<axiom_pool_client::direct::CapacityUnavailable>() {
        let detail = match capacity {
            axiom_pool_client::direct::CapacityUnavailable::Preparation => {
                "AstreaAI is busy preparing a secure connection. Your request was not submitted; try again shortly."
            }
            axiom_pool_client::direct::CapacityUnavailable::Exchange => {
                "AstreaAI is overloaded. Your request did not complete; it was not replayed."
            }
        };
        return SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::RateLimited,
            crate::error::SecureErrorCode::RateLimited,
            detail,
            false,
        )
        .with_retry_at(Some(
            i64::try_from(now().saturating_add(2)).unwrap_or(i64::MAX),
        ));
    }
    if let Some(refusal) = error.downcast_ref::<axiom_pool_protocol::frames::Rejection>() {
        // Only the request-bound encrypted refusal followed by clean EOF can
        // reach this type. Do not automatically replay an inference request.
        return SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::RateLimited,
            crate::error::SecureErrorCode::RateLimited,
            "AstreaAI is busy. Your request was not started; try again shortly.",
            false,
        )
        .with_retry_at(Some(
            i64::try_from(now().saturating_add(u64::from(refusal.retry_after_secs)))
                .unwrap_or(i64::MAX),
        ));
    }
    SecureClientError::attestation(if image {
        "AstreaAI image response incomplete or unauthenticated; inference was not replayed"
    } else {
        "AstreaAI response incomplete or unauthenticated; inference was not replayed"
    })
}

pub const ID: &str = "axiom-direct";
pub const MODEL: &str = "direct:qwen3.8-27b";
const PROFILE: &str = "axiom-snp-nvidia-v1";

pub(crate) fn installed_config(relay: &url::Url) -> Result<Option<DirectConfig>> {
    let (path, bundled) = if let Some(path) = std::env::var_os("AXIOM_DIRECT_CLIENT_CONFIG_FILE") {
        (PathBuf::from(path), false)
    } else {
        if relay.as_str().trim_end_matches('/') != "https://api.axiom.stream" {
            return Ok(None);
        }
        let executable = std::env::current_exe().map_err(|_| {
            SecureClientError::configuration("native executable location unavailable")
        })?;
        let Some(path) = bundled_config(&executable)? else {
            return Ok(None);
        };
        (path, true)
    };
    load_config(&path, bundled, relay).map(Some)
}

fn bundled_config(executable: &std::path::Path) -> Result<Option<PathBuf>> {
    // Unix may report the command symlink rather than its installed target.
    // The installer places its trust bundle beside the actual binary.
    #[cfg(unix)]
    let executable = executable
        .canonicalize()
        .map_err(|_| SecureClientError::configuration("native executable location unavailable"))?;
    let Some(directory) = executable.parent().and_then(std::path::Path::parent) else {
        return Ok(None);
    };
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("windows", "aarch64") if directory.join("direct/win32-arm64/native.json").exists() => {
            "win32-arm64"
        }
        ("windows", "aarch64" | "x86_64") => "win32-x64",
        ("linux", "x86_64") => "linux-x64",
        _ => return Ok(None),
    };
    let path = directory.join("direct").join(target).join("native.json");
    if !path.exists() {
        return Ok(None);
    }
    let identity_path = directory.join("axiom-install.json");
    let metadata = std::fs::symlink_metadata(&identity_path).map_err(|_| {
        SecureClientError::configuration("bundled AstreaAI installation identity unavailable")
    })?;
    if !metadata.is_file() || metadata.len() > 16384 {
        return Err(SecureClientError::configuration(
            "invalid bundled AstreaAI installation identity",
        ));
    }
    let identity = std::fs::read(identity_path).map_err(|_| {
        SecureClientError::configuration("bundled AstreaAI installation identity unavailable")
    })?;
    let identity: serde_json::Value = serde_json::from_slice(&identity).map_err(|_| {
        SecureClientError::configuration("invalid bundled AstreaAI installation identity")
    })?;
    if identity["schemaVersion"] != 1
        || !matches!(identity["product"].as_str(), Some("cli" | "desktop"))
    {
        return Err(SecureClientError::configuration(
            "unrecognized bundled AstreaAI installation",
        ));
    }
    Ok(Some(path))
}

fn load_config(path: &std::path::Path, bundled: bool, relay: &url::Url) -> Result<DirectConfig> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| {
        SecureClientError::configuration("direct operator configuration unavailable")
    })?;
    if !path.is_absolute() || !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(SecureClientError::configuration(
            "ordinary absolute direct configuration required",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !bundled && metadata.permissions().mode() & 0o077 != 0 {
            return Err(SecureClientError::configuration(
                "private direct configuration required",
            ));
        }
    }
    let raw = std::fs::read(path).map_err(|_| {
        SecureClientError::configuration("direct operator configuration unavailable")
    })?;
    let mut cfg: DirectConfig = axiom_pool_protocol::strict::decode(&raw)
        .map_err(|_| SecureClientError::configuration("invalid direct operator configuration"))?;
    if bundled {
        let binary = if cfg!(windows) {
            "runtime/axiom-direct-verify.exe"
        } else {
            "runtime/axiom-direct-verify"
        };
        if cfg.verifier.binary != std::path::Path::new(binary)
            || cfg.account.is_none()
            || !cfg.issuer_key_file.as_os_str().is_empty()
        {
            return Err(SecureClientError::configuration(
                "bundled AstreaAI requires a pinned verifier and account authorization",
            ));
        }
        cfg.verifier.binary = path
            .parent()
            .ok_or_else(|| SecureClientError::configuration("invalid AstreaAI bundle path"))?
            .join(binary);
    }
    if let Some(authority) = &cfg.account {
        if authority.origin != relay.as_str().trim_end_matches('/') {
            return Err(SecureClientError::configuration(
                "AstreaAI account authority must match the signed-in account API",
            ));
        }
    } else if !matches!(relay.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err(SecureClientError::configuration(
            "direct operator configuration requires a local account API",
        ));
    }
    DirectClient::validate_config(&cfg)
        .map_err(|_| SecureClientError::configuration("invalid direct worker pins"))?;
    Ok(cfg)
}

fn capabilities(model: &mut ModelInfo, config: &SecureClientConfig) {
    model.supports_streaming = true;
    model.supports_tools = config.direct.as_ref().is_some_and(|direct| direct.tools);
    model.supports_parallel_tools = false;
    model.supports_images = config.direct.as_ref().is_some_and(|direct| direct.vision);
    model.file_mime_types.clear();
    model.reasoning_replay = false;
    // This measured worker tokenizes with its installed medium template.
    // Advertising another effort would let background titles select a
    // different prompt template and invalidate the exact token accounting.
    model.supported_reasoning_efforts = vec![ReasoningEffort::Medium];
    model.supported_thinking_modes = vec![ThinkingMode::ProviderDefault, ThinkingMode::Enabled];
    if model.supports_tools {
        model.supported_thinking_modes.push(ThinkingMode::Disabled);
    }
}

#[cfg(test)]
mod installed_bundle_tests {
    use super::*;

    fn installed_executable(directory: &std::path::Path) -> PathBuf {
        for target in [
            "darwin-arm64",
            "darwin-x64",
            "win32-x64",
            "win32-arm64",
            "linux-x64",
        ] {
            let path = directory.join("direct").join(target);
            std::fs::create_dir_all(&path).unwrap();
            installed_config(&path);
        }
        let executable = directory.join("bin/axiomcli");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"synthetic executable fixture").unwrap();
        executable
    }

    fn installed_config(directory: &std::path::Path) -> PathBuf {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../../../deploy/astreaai-production.json"))
                .unwrap();
        if cfg!(windows) {
            value["verifier"]["binary"] = "runtime/axiom-direct-verify.exe".into();
        }
        let path = directory.join("native.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn public_bundle_binds_its_account_origin_and_relative_verifier() {
        let directory = tempfile::tempdir().unwrap();
        let path = installed_config(directory.path());
        let origin = url::Url::parse("https://api.axiom.stream").unwrap();
        let cfg = load_config(&path, true, &origin).unwrap();
        assert!(cfg.verifier.binary.is_absolute());
        assert!(cfg.issuer_key_file.as_os_str().is_empty());
        assert!(
            load_config(
                &path,
                true,
                &url::Url::parse("https://api-staging.axiom.stream").unwrap()
            )
            .is_err()
        );
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["verifier"]["binary"] = "../outside".into();
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_config(&path, true, &origin).is_err());
        value["verifier"]["binary"] = if cfg!(windows) {
            "runtime/axiom-direct-verify.exe"
        } else {
            "runtime/axiom-direct-verify"
        }
        .into();
        value["issuer_key_file"] = "/private/operator.pem".into();
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_config(&path, true, &origin).is_err());
    }

    #[test]
    fn bundle_discovery_requires_a_recognized_installation_identity() {
        let directory = tempfile::tempdir().unwrap();
        let executable = installed_executable(directory.path());
        assert!(bundled_config(&executable).is_err());
        let identity = directory.path().join("axiom-install.json");
        std::fs::write(&identity, br#"{"schemaVersion":1,"product":"unknown"}"#).unwrap();
        assert!(bundled_config(&executable).is_err());
        std::fs::write(&identity, br#"{"schemaVersion":1,"product":"desktop"}"#).unwrap();
        assert!(bundled_config(&executable).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn command_symlink_discovers_the_actual_installation_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let resources = directory.path().join("lib/axiom-cli");
        let executable = installed_executable(&resources);
        std::fs::write(
            resources.join("axiom-install.json"),
            br#"{"schemaVersion":1,"product":"cli"}"#,
        )
        .unwrap();
        let command = directory.path().join("bin/axiomcli");
        std::fs::create_dir_all(command.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../lib/axiom-cli/bin/axiomcli", &command).unwrap();
        let expected = bundled_config(&executable).unwrap().unwrap();
        assert_eq!(bundled_config(&command).unwrap(), Some(expected));
    }
}
pub(crate) fn models(config: &SecureClientConfig) -> Vec<ModelInfo> {
    let Some(direct) = &config.direct else {
        return vec![];
    };
    let mut model = ModelInfo {
        id: MODEL.into(),
        label: "Qwen3.8 Uncensored · AstreaAI".into(),
        // Locally pinned offering: carries its own tag so image policy follows
        // the same catalog-tag rule as relay offerings.
        tags: vec![axiom_inference::ModelTag {
            id: axiom_inference::UNCENSORED_MODEL_TAG.into(),
            label: "Uncensored".into(),
            tone: axiom_inference::ModelTagTone::Pink,
            position: 50,
        }],
        short_label: "Qwen3.8 Uncensored".into(),
        provider_id: ID.into(),
        provider_label: "AstreaAI".into(),
        upstream_model: direct.model_manifest.clone(),
        provider_base_url: direct.origin.clone(),
        e2ee_protocol: axiom_pool_protocol::PROTOCOL.into(),
        e2ee_encryption_version: 1,
        attestation_protocol: PROFILE.into(),
        context_window_tokens: 32768,
        max_output_tokens: 4096,
        input_price_microusd_per_million_tokens: direct
            .account
            .as_ref()
            .map(|a| a.chat_price.input_microusd_per_million),
        output_price_microusd_per_million_tokens: direct
            .account
            .as_ref()
            .map(|a| a.chat_price.output_microusd_per_million),
        ..ModelInfo::default()
    };
    capabilities(&mut model, config);
    vec![model]
}

pub(crate) fn provider(
    config: &SecureClientConfig,
    credential: &Arc<ApiCredential>,
) -> Result<Option<Arc<dyn SecureProvider>>> {
    if config.direct.is_none() {
        return Ok(None);
    }
    let installed = config.direct.clone().unwrap();
    let client = if installed.account.is_some() {
        DirectClient::new_account(
            installed,
            secrecy::SecretString::from(credential.expose().to_owned()),
        )
    } else {
        DirectClient::new(installed)
    };
    Ok(Some(Arc::new(DirectProvider {
        client: Arc::new(
            client.map_err(|_| SecureClientError::configuration("invalid direct installation"))?,
        ),
        config: config.clone(),
        warm_started: std::sync::atomic::AtomicBool::new(false),
        warm_cancel: CancellationToken::new(),
    })))
}
struct DirectProvider {
    config: SecureClientConfig,
    client: Arc<DirectClient>,
    warm_started: std::sync::atomic::AtomicBool,
    warm_cancel: CancellationToken,
}
impl Drop for DirectProvider {
    fn drop(&mut self) {
        self.warm_cancel.cancel();
    }
}
impl DirectProvider {
    fn keep_warm(&self) {
        if !self.config.direct.as_ref().is_some_and(|cfg| cfg.tools)
            || self
                .warm_started
                .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let weak = Arc::downgrade(&self.client);
        let cancel = self.warm_cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {()=cancel.cancelled()=>break,()=tokio::time::sleep(std::time::Duration::from_secs(20))=>{}}
                let Some(client) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = client.prewarm(&cancel).await {
                    if cancel.is_cancelled() {
                        break;
                    }
                    tracing::warn!(detail=%error,"direct background verification renewal failed");
                }
            }
        });
    }

    async fn image_policy(
        &self,
        policy: &TrustPolicy,
        cancel: &CancellationToken,
    ) -> Result<axiom_inference::ImageRuntimePolicy> {
        let cfg = self
            .config
            .direct
            .as_ref()
            .ok_or_else(|| SecureClientError::configuration("direct installation missing"))?;
        if !cfg.image_generation || !cfg.tools || !cfg.vision {
            return Err(SecureClientError::capability(
                "Image generation is not enabled by this installed Direct configuration",
            ));
        }
        if !policy.is_valid_at(now())
            || !policy.require_nvidia_verified
            || !policy.require_model_binding
            || !policy.require_live_tls_binding
        {
            return Err(SecureClientError::attestation(
                "strict image attestation policy required",
            ));
        }
        let worker = self.client.prepare(cancel).await.map_err(|e| {
            tracing::warn!(detail=%e,"direct image preflight failed");
            if cancel.is_cancelled() {
                SecureClientError::cancelled()
            } else if e
                .downcast_ref::<axiom_pool_client::direct::CapacityUnavailable>()
                .is_some()
            {
                exchange_error(&e, false, true)
            } else {
                SecureClientError::attestation("AstreaAI image workload verification failed")
            }
        })?;
        self.keep_warm();
        if !worker.supports("image") || !worker.supports("tools") || !worker.supports("vision") {
            return Err(SecureClientError::capability(
                "signed release does not support image generation tools",
            ));
        }
        let runtime = direct_image_runtime();
        runtime.validate().map_err(SecureClientError::capability)?;
        Ok(runtime)
    }
    async fn generate(
        &self,
        policy: &TrustPolicy,
        request: axiom_inference::ImageGenerationRequest<'_>,
        uncensored: bool,
        progress: Option<mpsc::Sender<axiom_inference::ImageGenerationProgress>>,
        cancel: CancellationToken,
    ) -> Result<crate::GeneratedImage> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        if request.chat_model_id.trim().is_empty()
            || request.chat_model_id.len() > 256
            || request.chat_model_id.chars().any(char::is_control)
        {
            return Err(SecureClientError::capability(
                "Choose a valid chat model before generating an image",
            ));
        }
        if request.prompt.trim().is_empty()
            || request.prompt.len() > 8192
            || request.references.len() > axiom_pool_protocol::vision::MAX_REFERENCES
        {
            return Err(SecureClientError::capability(
                "Provide a prompt and at most five local reference images",
            ));
        }
        let runtime = self.image_policy(policy, &cancel).await?;
        let size = request.dimensions.unwrap_or(
            runtime
                .dimensions(&runtime.image.default_aspect_ratio)
                .map_err(SecureClientError::capability)?,
        );
        if size.validate().is_err()
            || u64::from(size.width) * u64::from(size.height) > runtime.image.max_pixels
        {
            return Err(SecureClientError::capability(
                "Image dimensions exceed the installed image runtime limits",
            ));
        }
        let mut source_image_ids = Vec::new();
        let mut identities = std::collections::BTreeSet::new();
        let mut references = Vec::new();
        for (id, image) in request.references {
            if id.is_empty() || id.len() > 128 || !identities.insert(*id) {
                return Err(SecureClientError::capability(
                    "invalid or duplicate local image ID",
                ));
            }
            references.push(
                axiom_pool_protocol::vision::InlineImage::from_reference_upload(
                    &image.mime_type,
                    &image.data,
                )
                .map_err(|_| SecureClientError::capability("invalid image reference"))?,
            );
            source_image_ids.push((*id).to_owned());
        }
        let input_image_pixels: u64 = references
            .iter()
            .map(axiom_pool_protocol::vision::InlineImage::pixels)
            .sum();
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        if let Some(tx) = &progress {
            let _ = tx
                .send(axiom_inference::ImageGenerationProgress {
                    stage: axiom_inference::ImageGenerationStage::Queued,
                    percent: 0,
                })
                .await;
        }
        let updates = progress.clone();
        let (worker, output) = self
            .client
            .execute_with(
                Input::Image {
                    prompt: request.prompt.into(),
                    references,
                    uncensored,
                    width: size.width,
                    height: size.height,
                    steps: 8,
                    seed: rand_core::RngCore::next_u32(&mut rand_core::OsRng),
                },
                Limits {
                    context_tokens: 8192,
                    output_tokens: 1,
                    input_bytes: axiom_pool_protocol::MAX_REQUEST as u64,
                    output_bytes: 32 * 1024 * 1024,
                    image_pixels: u64::from(size.width) * u64::from(size.height),
                    image_steps: 8,
                    input_image_pixels,
                    deadline: now() + 180,
                },
                None,
                move |frame| {
                    let updates = updates.clone();
                    async move {
                        if let Some(tx) = updates {
                            let update = match frame.kind.as_str() {
                                "accepted" => Some(axiom_inference::ImageGenerationProgress {
                                    stage: axiom_inference::ImageGenerationStage::Generating,
                                    percent: 1,
                                }),
                                "image_manifest" => {
                                    Some(axiom_inference::ImageGenerationProgress {
                                        stage: axiom_inference::ImageGenerationStage::Finishing,
                                        percent: 99,
                                    })
                                }
                                _ => None,
                            };
                            if let Some(update) = update {
                                let _ = tx.send(update).await;
                            }
                        }
                        Ok(())
                    }
                },
                cancel.clone(),
            )
            .await
            .map_err(|e| {
                tracing::warn!(detail=%e,"direct image exchange failed");
                exchange_error(&e, cancel.is_cancelled(), true)
            })?;
        if !policy.is_valid_at(now()) {
            return Err(SecureClientError::attestation(
                "trust policy expired before image completion",
            ));
        }
        let bytes = output
            .image
            .ok_or_else(|| SecureClientError::attestation("verified image missing"))?;
        let data = STANDARD.encode(&bytes);
        axiom_pool_protocol::vision::validate_generated_png(&bytes, size.width, size.height)
            .map_err(|_| {
                SecureClientError::attestation("generated PNG dimensions or bytes differ")
            })?;
        let key: [u8; 32] = axiom_pool_protocol::hex_bytes(&worker.identity().identity_key, 32)
            .map_err(|_| SecureClientError::attestation("image signing key invalid"))?
            .try_into()
            .map_err(|_| SecureClientError::attestation("image signing key invalid"))?;
        let receipt: axiom_pool_protocol::types::Receipt = axiom_pool_protocol::verify(
            &output.terminal.receipt,
            &ed25519_dalek::VerifyingKey::from_bytes(&key)
                .map_err(|_| SecureClientError::attestation("image signing key invalid"))?,
            axiom_pool_protocol::RECEIPT_DOMAIN,
        )
        .map_err(|_| SecureClientError::attestation("image receipt invalid"))?;
        let request_id = receipt.binding.request_id.clone();
        let model = models(&self.config)
            .into_iter()
            .next()
            .ok_or_else(|| SecureClientError::configuration("image model missing"))?;
        if let Some(timings) = &output.direct_timings {
            tracing::info!(timings=?timings,"direct image timing");
        }
        Ok(crate::GeneratedImage {
            request_id: request_id.clone(),
            data,
            mime_type: "image/png".into(),
            width: size.width,
            height: size.height,
            sha256: axiom_pool_protocol::sha256(&bytes),
            source_image_ids,
            chat_model_id: Some(request.chat_model_id.into()),
            evidence: evidence(&model, policy, &worker, worker.public_evidence()),
            usage: axiom_inference::RequestUsage {
                request_id,
                model_id: MODEL.into(),
                provider_id: ID.into(),
                state: axiom_inference::InvocationState::Completed,
                completeness: axiom_inference::UsageCompleteness::Final,
                image_pixels: Some(receipt.usage.image_pixels.to_string()),
                image_steps: Some(receipt.usage.image_steps.to_string()),
                input_image_pixels: Some(receipt.usage.input_image_pixels.to_string()),
                cost_microusd: Some(output.billing.charge_microusd.to_string()),
                settled: output.billing.settled,
                response_verified: true,
                started_at_ms,
                finished_at_ms: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        .to_string(),
                ),
                finish_reason: Some(output.terminal.finish_reason),
                ..Default::default()
            },
        })
    }
}
fn direct_image_runtime() -> axiom_inference::ImageRuntimePolicy {
    let mut runtime = axiom_inference::ImageRuntimePolicy::default();
    runtime.image.default_aspect_ratio = "1:1".into();
    runtime.image.uncensored_models = vec![MODEL.into()];
    runtime.extensions.insert(
        "reference_uses_default_size".into(),
        serde_json::json!(true),
    );
    runtime
}
impl sealed::Provider for DirectProvider {}
#[async_trait]
impl SecureProvider for DirectProvider {
    async fn invalidate(&self, _model: &str) {
        self.client.invalidate().await;
    }
    async fn image_capabilities(
        &self,
        policy: &TrustPolicy,
        cancel: CancellationToken,
    ) -> Result<axiom_inference::ImageRuntimePolicy> {
        self.image_policy(policy, &cancel).await
    }
    async fn image_request(
        &self,
        policy: &TrustPolicy,
        request: axiom_inference::ImageGenerationRequest<'_>,
        uncensored: bool,
        progress: Option<mpsc::Sender<axiom_inference::ImageGenerationProgress>>,
        cancel: CancellationToken,
    ) -> Result<crate::GeneratedImage> {
        self.generate(policy, request, uncensored, progress, cancel)
            .await
    }
    fn id(&self) -> &'static str {
        ID
    }
    fn supports(&self, model: &ModelInfo) -> Result<()> {
        if !models(&self.config).iter().any(|m| {
            m.id == model.id
                && m.provider_id == model.provider_id
                && m.upstream_model == model.upstream_model
                && m.provider_base_url == model.provider_base_url
                && m.attestation_protocol == model.attestation_protocol
                && m.e2ee_protocol == model.e2ee_protocol
                && m.e2ee_encryption_version == model.e2ee_encryption_version
                && m.context_window_tokens == model.context_window_tokens
                && m.max_output_tokens == model.max_output_tokens
                && m.supports_images == model.supports_images
                && m.supports_tools == model.supports_tools
        }) {
            return Err(SecureClientError::capability(
                "unconfigured direct worker contract",
            ));
        }
        Ok(())
    }
    fn supports_contract(&self, protocol: &str, version: u16, profile: &str) -> bool {
        protocol == axiom_pool_protocol::PROTOCOL && version == 1 && profile == PROFILE
    }
    fn populate_capabilities(&self, model: &mut ModelInfo) {
        capabilities(model, &self.config);
    }
    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancel: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        if !policy.is_valid_at(now())
            || !policy.require_nvidia_verified
            || !policy.require_model_binding
            || !policy.require_live_tls_binding
        {
            return Err(SecureClientError::attestation(
                "strict direct attestation policy required",
            ));
        }
        let client = Arc::clone(&self.client);
        let started = std::time::Instant::now();
        let worker = client.prepare(&cancel).await.map_err(|error| {
            tracing::warn!(detail = %error, "direct attestation preflight failed");
            if cancel.is_cancelled() {
                SecureClientError::cancelled()
            } else if error
                .downcast_ref::<axiom_pool_client::direct::CapacityUnavailable>()
                .is_some()
            {
                exchange_error(&error, false, false)
            } else {
                SecureClientError::attestation(
                    "AstreaAI workload attestation failed; no message was sent",
                )
            }
        })?;
        self.keep_warm();
        tracing::info!(
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "direct session prepared"
        );
        let evidence = evidence(model, policy, &worker, worker.public_evidence());
        Ok(Box::new(Session {
            client,
            worker,
            evidence,
            model: model.clone(),
            policy: policy.clone(),
        }))
    }
}
struct Session {
    client: Arc<DirectClient>,
    worker: VerifiedWorker,
    evidence: SecurityEvidence,
    model: ModelInfo,
    policy: TrustPolicy,
}
impl sealed::Session for Session {}
#[async_trait]
impl VerifiedSession for Session {
    fn evidence(&self) -> &SecurityEvidence {
        &self.evidence
    }
    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancel: CancellationToken,
    ) -> Result<InferenceResponse> {
        self.run(request, None, cancel).await
    }
    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<InferenceResponse> {
        self.run(request, Some(events), cancel).await
    }
}
impl Session {
    async fn run(
        &mut self,
        request: InferenceRequest,
        events: Option<mpsc::Sender<ProviderEvent>>,
        cancel: CancellationToken,
    ) -> Result<InferenceResponse> {
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        request
            .validate()
            .map_err(|_| SecureClientError::capability("invalid direct request"))?;
        if request.model != self.model.id
            || !self.policy.is_valid_at(now())
            || (!request.tools.is_empty() && !self.model.supports_tools)
            || !self
                .model
                .supported_thinking_modes
                .contains(&request.thinking_mode)
            || request.sampling != SamplingParameters::default()
            || request.response_format != ResponseFormat::Text
            || request.reasoning_effort != ReasoningEffort::Medium
        {
            return Err(SecureClientError::capability(
                "direct worker request controls unsupported",
            ));
        }
        let mut messages = Vec::new();
        let mut input_image_pixels = 0u64;
        let mut input_image_count = 0usize;
        for m in request.messages {
            if (!m.images.is_empty() && !self.model.supports_images)
                || !m.files.is_empty()
                || ((!m.tool_calls.is_empty() || m.tool_call_id.is_some())
                    && !self.model.supports_tools)
                || (m.name.is_some() && m.role != ChatRole::Tool)
                || m.refusal.is_some()
                || m.reasoning_content.is_some()
            {
                return Err(SecureClientError::capability(
                    "direct worker request contains unsupported attachments or message fields",
                ));
            }
            let role = serde_json::to_value(m.role)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| SecureClientError::capability("invalid direct message role"))?;
            let images = m
                .images
                .into_iter()
                .map(|image| {
                    axiom_pool_protocol::vision::InlineImage::from_upload(
                        &image.mime_type,
                        &image.data,
                    )
                    .map_err(|_| {
                        SecureClientError::capability(
                            "Image is invalid or exceeds the installed vision limits",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            input_image_count += images.len();
            if input_image_count > axiom_pool_protocol::vision::MAX_IMAGES {
                return Err(SecureClientError::capability(
                    "This connection supports at most eight images in the current context",
                ));
            }
            for image in &images {
                input_image_pixels = input_image_pixels
                    .checked_add(image.pixels())
                    .ok_or_else(|| SecureClientError::capability("Image pixel budget exceeded"))?;
            }
            messages.push(Message {
                role,
                content: m.content,
                images,
                tool_calls: m
                    .tool_calls
                    .into_iter()
                    .map(|call| axiom_pool_protocol::tools::ToolCall {
                        id: call.id,
                        kind: call.kind,
                        function: axiom_pool_protocol::tools::FunctionCall {
                            name: call.function.name,
                            arguments: call.function.arguments,
                        },
                    })
                    .collect(),
                tool_call_id: m.tool_call_id,
            });
        }
        let effort = if request.thinking_mode == ThinkingMode::Disabled {
            "none"
        } else {
            match request.reasoning_effort {
                ReasoningEffort::Low => "low",
                ReasoningEffort::Medium => "medium",
                ReasoningEffort::High => "high",
                _ => {
                    return Err(SecureClientError::capability(
                        "unsupported direct reasoning effort",
                    ));
                }
            }
        };
        let max_tokens = u64::from(request.max_output_tokens.unwrap_or(4096));
        if max_tokens > 4096 {
            return Err(SecureClientError::capability(
                "direct worker output limit is 4096 tokens",
            ));
        }
        let input = Input::Chat {
            messages,
            max_tokens,
            reasoning_effort: effort.into(),
            tools: request
                .tools
                .into_iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| SecureClientError::capability("invalid tool contract"))?,
        };
        let limits = Limits {
            context_tokens: 32768,
            output_tokens: max_tokens,
            input_bytes: axiom_pool_protocol::MAX_REQUEST as u64,
            output_bytes: 32 * 1024 * 1024,
            image_pixels: 0,
            image_steps: 0,
            input_image_pixels,
            deadline: now() + 180,
        };
        let subscribers = events.clone();
        let request_id = request.request_id.clone();
        let (worker, output) = self
            .client
            .execute_with_options(
                input,
                limits,
                ExecutionOptions {
                    pinned: Some(&self.worker),
                    request_id: request_id.as_deref(),
                },
                move |frame| {
                    let events = subscribers.clone();
                    async move {
                        if let Some(events) = events {
                            let event = match frame.kind.as_str() {
                                "text_delta" => Some(ProviderEvent::TextDelta(
                                    frame.data["text"].as_str().unwrap_or_default().into(),
                                )),
                                "reasoning_delta" => Some(ProviderEvent::ReasoningDelta(
                                    frame.data["text"].as_str().unwrap_or_default().into(),
                                )),
                                _ => None,
                            };
                            if let Some(event) = event {
                                events
                                    .send(event)
                                    .await
                                    .map_err(|_| anyhow::anyhow!("subscriber closed"))?;
                            }
                        }
                        Ok(())
                    }
                },
                cancel.clone(),
            )
            .await
            .map_err(|error| {
                tracing::warn!(detail = %error, "direct attested exchange failed");
                exchange_error(&error, cancel.is_cancelled(), false)
            })?;
        if !self.policy.is_valid_at(now()) {
            return Err(SecureClientError::attestation(
                "native trust policy expired before completion",
            ));
        }
        self.evidence = evidence(&self.model, &self.policy, &worker, worker.public_evidence());
        self.worker = worker;
        if let Some(timings) = &output.direct_timings {
            tracing::info!(timings=?timings,"direct request timing");
        }
        let tool_calls = output
            .tool_calls
            .into_iter()
            .map(|call| axiom_inference::ToolCall {
                id: call.id,
                kind: call.kind,
                function: axiom_inference::FunctionCall {
                    name: call.function.name,
                    arguments: call.function.arguments,
                },
            })
            .collect::<Vec<_>>();
        let receipt: axiom_pool_protocol::types::Receipt = axiom_pool_protocol::verify(
            &output.terminal.receipt,
            &ed25519_dalek::VerifyingKey::from_bytes(
                &axiom_pool_protocol::hex_bytes(&self.worker.identity().identity_key, 32)
                    .map_err(|_| SecureClientError::attestation("receipt signing key invalid"))?
                    .try_into()
                    .map_err(|_| SecureClientError::attestation("receipt signing key invalid"))?,
            )
            .map_err(|_| SecureClientError::attestation("receipt signing key invalid"))?,
            axiom_pool_protocol::RECEIPT_DOMAIN,
        )
        .map_err(|_| SecureClientError::attestation("chat receipt invalid"))?;
        let usage = output.terminal.usage;
        let accounting = axiom_inference::RequestUsage {
            request_id: receipt.binding.request_id,
            model_id: MODEL.into(),
            provider_id: ID.into(),
            context_window_tokens: Some(self.model.context_window_tokens),
            state: axiom_inference::InvocationState::Completed,
            completeness: axiom_inference::UsageCompleteness::Final,
            input_tokens: Some(usage.input_tokens.to_string()),
            cached_input_tokens: Some(usage.cached_input_tokens.to_string()),
            output_tokens: Some(usage.output_tokens.to_string()),
            reasoning_tokens: Some(usage.reasoning_tokens.to_string()),
            input_image_pixels: Some(usage.input_image_pixels.to_string()),
            cost_microusd: Some(output.billing.charge_microusd.to_string()),
            settled: output.billing.settled,
            response_verified: true,
            started_at_ms,
            finished_at_ms: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .to_string(),
            ),
            finish_reason: Some(output.terminal.finish_reason.clone()),
            ..Default::default()
        };
        let finish = if output.terminal.finish_reason == "length" {
            FinishReason::Length
        } else if output.terminal.finish_reason == "tool_calls" {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        };
        if let Some(events) = events {
            for (index, call) in tool_calls.iter().enumerate() {
                events
                    .send(ProviderEvent::ToolCallDelta(
                        axiom_inference::ToolCallDelta {
                            index,
                            id: Some(call.id.clone()),
                            kind: Some(call.kind.clone()),
                            name: Some(call.function.name.clone()),
                            arguments: Some(call.function.arguments.clone()),
                        },
                    ))
                    .await
                    .map_err(|_| SecureClientError::cancelled())?;
            }
            for event in [
                ProviderEvent::SecurityEvidence(Box::new(self.evidence.clone())),
                ProviderEvent::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                },
                ProviderEvent::ResponseVerified,
                ProviderEvent::Accounting(Box::new(accounting)),
            ] {
                events
                    .send(event)
                    .await
                    .map_err(|_| SecureClientError::cancelled())?;
            }
        }
        let reasoning = if output.reasoning.is_empty() {
            None
        } else {
            Some(output.reasoning)
        };
        Ok(InferenceResponse {
            assistant: AssistantTurn {
                text: output.text,
                reasoning: reasoning.clone(),
                tool_calls,
            },
            reasoning,
            refusal: None,
            usage: Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage.input_tokens + usage.output_tokens,
            },
            finish_reason: Some(finish),
        })
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
fn evidence(
    model: &ModelInfo,
    policy: &TrustPolicy,
    worker: &VerifiedWorker,
    document: Option<serde_json::Value>,
) -> SecurityEvidence {
    SecurityEvidence {
        state: SecurityState::Verified,
        provider_id: ID.into(),
        model_id: model.id.clone(),
        attestation_protocol: PROFILE.into(),
        e2ee_protocol: axiom_pool_protocol::PROTOCOL.into(),
        e2ee_encryption_version: 1,
        trust_policy_version: format!(
            "{} / direct policy {}",
            policy.version,
            worker.policy_sequence()
        ),
        verified_at_unix_seconds: worker.verified_at(),
        attestation_generation: Some(worker.generation()),
        hard_expires_at_unix_seconds: Some(worker.expires_at().min(policy.expires_at_unix_seconds)),
        model_key_fingerprint: axiom_pool_protocol::sha256(
            hex::decode(&worker.identity().encryption_key).unwrap_or_default(),
        ),
        tls_spki_fingerprint: None,
        checks: [
            ("cpu_tee", "AMD SEV-SNP"),
            ("nvidia_gpu", "NVIDIA confidential GPU"),
            ("worker_binding", "Measured workload and worker keys"),
            ("model_binding", "Verified model inventory"),
            ("tls_binding", "CPU-bound TLS certificate"),
        ]
        .into_iter()
        .map(|(id, label)| EvidenceCheck {
            id: id.into(),
            label: label.into(),
            status: "verified".into(),
            passed: true,
        })
        .collect(),
        provider_claims: [
            ("worker_id", worker.identity().worker_id.clone()),
            ("worker_boot_id", worker.identity().boot_id.clone()),
            (
                "model_manifest_sha256",
                worker.identity().model_manifest.clone(),
            ),
            (
                "deployment_digest",
                worker.identity().deployment_digest.clone(),
            ),
        ]
        .into_iter()
        .map(|(name, value)| EvidenceClaim {
            name: name.into(),
            value,
        })
        .collect(),
        workload_manifest: document.map(|v| v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_typed_authenticated_refusal_becomes_busy_and_is_not_auto_replayed() {
        let error = anyhow::Error::new(axiom_pool_protocol::frames::Rejection {
            code: "WORKER_BUSY".into(),
            retry_after_secs: 2,
        });
        let busy = exchange_error(&error, false, false);
        assert_eq!(busy.code(), crate::error::SecureErrorCode::RateLimited);
        assert!(!busy.retryable());
        assert!(busy.retry_at().is_some());
        let untrusted = exchange_error(&anyhow::anyhow!("WORKER_BUSY"), false, false);
        assert_eq!(
            untrusted.code(),
            crate::error::SecureErrorCode::AttestationRejected
        );
        let cancelled = exchange_error(&error, true, false);
        assert_eq!(cancelled.code(), crate::error::SecureErrorCode::Cancelled);
    }

    #[test]
    fn transport_backpressure_never_claims_the_request_was_not_dispatched() {
        let error = anyhow::Error::new(axiom_pool_client::direct::CapacityUnavailable::Exchange);
        let mapped = exchange_error(&error, false, false);
        assert_eq!(mapped.code(), crate::error::SecureErrorCode::RateLimited);
        assert!(!mapped.retryable());
        assert!(mapped.safe_detail().contains("did not complete"));
        assert!(!mapped.safe_detail().contains("not started"));
    }

    fn installed() -> SecureClientConfig {
        let fixture_root = std::env::temp_dir();
        let mut config = SecureClientConfig::new("https://127.0.0.1:9443").unwrap();
        config.direct = Some(
            serde_json::from_value(serde_json::json!({
                "origin":"https://worker.example:8443", "connect_ip":null,
                "model_manifest":"aa".repeat(32), "issuer_key_file":fixture_root.join("axiom-test-operator.pem"),
                "verifier":{"binary":fixture_root.join("axiom-test-verifier"), "binary_sha256":"bb".repeat(32),
                    "publisher_key":"cc".repeat(32), "minimum_policy_sequence":10}
            }))
            .unwrap(),
        );
        config
    }

    #[test]
    fn installed_direct_contract_rejects_catalog_substitution() {
        let config = installed();
        let provider = provider(&config, &Arc::new(ApiCredential::new("synthetic")))
            .unwrap()
            .unwrap();
        let model = models(&config).remove(0);
        assert!(provider.supports(&model).is_ok());
        assert!(model.supports_streaming && !model.supports_tools && !model.supports_images);
        for altered in [
            ModelInfo {
                supports_images: true,
                ..model.clone()
            },
            ModelInfo {
                provider_base_url: "https://attacker.example".into(),
                ..model.clone()
            },
            ModelInfo {
                upstream_model: "dd".repeat(32),
                ..model.clone()
            },
            ModelInfo {
                attestation_protocol: "unqualified-gpu-v1".into(),
                ..model.clone()
            },
            ModelInfo {
                e2ee_encryption_version: 2,
                ..model.clone()
            },
            ModelInfo {
                max_output_tokens: 8192,
                ..model.clone()
            },
        ] {
            assert!(provider.supports(&altered).is_err());
        }
    }

    #[test]
    fn vision_requires_local_installation_and_never_advertises_tools_or_files() {
        let mut config = installed();
        assert!(!models(&config)[0].supports_images);
        config.direct.as_mut().unwrap().vision = true;
        let model = models(&config).remove(0);
        assert!(model.supports_images);
        assert!(!model.supports_tools && !model.supports_parallel_tools);
        assert!(model.file_mime_types.is_empty());
        assert!(
            provider(&config, &Arc::new(ApiCredential::new("synthetic")))
                .unwrap()
                .unwrap()
                .supports(&model)
                .is_ok()
        );
    }

    #[test]
    fn tool_capability_requires_the_installed_contract() {
        let mut config = installed();
        let legacy = models(&config).remove(0);
        config.direct.as_mut().unwrap().tools = true;
        let model = models(&config).remove(0);
        assert!(model.supports_tools);
        let configured = provider(&config, &Arc::new(ApiCredential::new("synthetic")))
            .unwrap()
            .unwrap();
        assert!(configured.supports(&model).is_ok());
        assert!(configured.supports(&legacy).is_err());
    }

    #[test]
    fn direct_image_contract_keeps_five_references_and_full_size_square_default() {
        let runtime = direct_image_runtime();
        runtime.validate().unwrap();
        assert_eq!(runtime.image.max_references, 5);
        assert!(runtime.references_use_default_size());
        let size = runtime
            .dimensions(&runtime.image.default_aspect_ratio)
            .unwrap();
        assert_eq!((size.width, size.height), (1536, 1536));
        assert!(runtime.image.aspect_ratios.contains(&"2:3".into()));
        let config = installed();
        assert!(
            models(&config)
                .iter()
                .all(|model| model.has_tag(axiom_inference::UNCENSORED_MODEL_TAG))
        );
    }

    #[tokio::test]
    async fn image_installation_flags_are_required_before_network_or_key_access() {
        let config = installed();
        let configured = provider(&config, &Arc::new(ApiCredential::new("synthetic")))
            .unwrap()
            .unwrap();
        assert!(
            configured
                .image_capabilities(&TrustPolicy::default(), CancellationToken::new())
                .await
                .unwrap_err()
                .safe_detail()
                .contains("not enabled")
        );
    }

    #[tokio::test]
    async fn image_requests_keep_chat_provider_independent_of_the_image_installation() {
        let config = installed();
        let configured = provider(&config, &Arc::new(ApiCredential::new("synthetic")))
            .unwrap()
            .unwrap();
        for chat_model_id in [MODEL, "tinfoil-deepseek-v4-1-flash", "glm-5-3-flash"] {
            let error = configured
                .image_request(
                    &TrustPolicy::default(),
                    axiom_inference::ImageGenerationRequest {
                        chat_model_id,
                        prompt: "A dog in a garden",
                        references: &[],
                        dimensions: None,
                    },
                    false,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(
                error.safe_detail().contains("not enabled"),
                "{chat_model_id} must reach image-installation validation: {}",
                error.safe_detail()
            );
        }
    }

    #[tokio::test]
    async fn image_requests_reject_invalid_chat_identity_before_key_access() {
        let config = installed();
        let configured = provider(&config, &Arc::new(ApiCredential::new("synthetic")))
            .unwrap()
            .unwrap();
        for chat_model_id in ["", " ", "near\nmodel", &"a".repeat(257)] {
            let error = configured
                .image_request(
                    &TrustPolicy::default(),
                    axiom_inference::ImageGenerationRequest {
                        chat_model_id,
                        prompt: "A dog",
                        references: &[],
                        dimensions: None,
                    },
                    false,
                    None,
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(error.safe_detail().contains("valid chat model"));
        }
    }

    #[tokio::test]
    async fn background_renewal_is_cancelled_when_its_account_provider_drops() {
        let mut config = installed();
        config.direct.as_mut().unwrap().tools = true;
        let direct = DirectProvider {
            client: Arc::new(DirectClient::new(config.direct.clone().unwrap()).unwrap()),
            config,
            warm_started: std::sync::atomic::AtomicBool::new(false),
            warm_cancel: CancellationToken::new(),
        };
        let cancellation = direct.warm_cancel.clone();
        direct.keep_warm();
        assert!(
            direct
                .warm_started
                .load(std::sync::atomic::Ordering::Acquire)
        );
        drop(direct);
        assert!(cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn relaxed_policy_is_rejected_before_any_worker_contact() {
        let config = installed();
        let provider = provider(&config, &Arc::new(ApiCredential::new("synthetic")))
            .unwrap()
            .unwrap();
        let model = models(&config).remove(0);
        for policy in [
            TrustPolicy {
                require_nvidia_verified: false,
                ..TrustPolicy::default()
            },
            TrustPolicy {
                require_model_binding: false,
                ..TrustPolicy::default()
            },
            TrustPolicy {
                require_live_tls_binding: false,
                ..TrustPolicy::default()
            },
            TrustPolicy {
                expires_at_unix_seconds: 1,
                ..TrustPolicy::default()
            },
        ] {
            let result = provider
                .establish(&model, &policy, CancellationToken::new())
                .await;
            assert!(result.is_err());
            assert!(
                result
                    .err()
                    .unwrap()
                    .safe_detail()
                    .contains("strict direct attestation policy")
            );
        }
    }
}

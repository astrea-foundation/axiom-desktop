//! Native Chutes channel engine and the separate local development adapter.
//! The native helper verifies Chutes hardware, encrypts before network dispatch,
//! and authenticates a complete reply using a secret inside that ciphertext.
use crate::{
    ApiCredential, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecurityEvidence, SecurityState, TrustPolicy,
    provider::{SecureProvider, VerifiedSession, sealed},
};
use async_trait::async_trait;
#[cfg(test)]
use axiom_inference::ChatRole;
use axiom_inference::{
    AssistantTurn, FinishReason, InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent,
    ReasoningEffort, ResponseFormat, SamplingParameters, ThinkingMode, Usage,
};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::{Mutex, mpsc},
};
use tokio_util::sync::CancellationToken;

const ID: &str = "chutes-preview";
const MODEL: &str = "chutes-preview:qwen3.8-27b";
const PROFILE: &str = "chutes-hardware-development-v1";
const RELEASE_PROFILE: &str = "chutes-tdx-nvidia-release-v1";
const PROTOCOL: &str = "chutes-mlkem768-desktop-v1";
const MAX_PIPE: usize = 40 * 1024 * 1024;
const HOSTED_ID: &str = "chutes";
const HOSTED_MODEL: &str = "chutes:qwen3.8-27b";

/// Bundled Qwen3.8 Uncensored offerings, used for their model identity prompt.
/// Image adapter policy follows the chat model's catalog tag instead.
#[must_use]
pub fn is_uncensored_model(model_id: &str) -> bool {
    matches!(model_id, HOSTED_MODEL | MODEL)
}

/// `uncensored` is the client's decision from the chat model's catalog tags,
/// never an image-tool argument.
fn image_operation_request(request_id: &str, prompt: &str, edit: bool, uncensored: bool) -> Value {
    json!({"op": if edit { "image_edit" } else { "image" },
        "request_id": request_id, "prompt": prompt, "uncensored": uncensored})
}

fn runtime_image_lora(runtime: &axiom_inference::ImageRuntimePolicy, uncensored: bool) -> Value {
    if uncensored && let Some(lora) = &runtime.image.lora {
        json!({"name":lora.name,"sha256":lora.sha256,"strength":lora.strength})
    } else {
        Value::Null
    }
}

#[cfg(test)]
fn expected_image_lora(uncensored: bool) -> Value {
    runtime_image_lora(&axiom_inference::ImageRuntimePolicy::default(), uncensored)
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeConfig {
    #[serde(default)]
    python: Option<PathBuf>,
    #[serde(default)]
    script: Option<PathBuf>,
    #[serde(default)]
    helper: Option<PathBuf>,
    #[serde(default)]
    helper_sha256: Option<String>,
    config: PathBuf,
    #[serde(default)]
    release_authority_fingerprint: Option<String>,
    #[serde(default)]
    script_sha256: Option<String>,
    #[serde(default)]
    maximum_charge_microusd: u64,
}

const IMAGE_HELPER_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Owned by one `SecureClient` (and therefore one fixed credential). Image
/// verification is independent of the selected chat provider. Only the helper
/// and its public verification cache survive a successful request.
#[derive(Default)]
pub(crate) struct ImageSessionCache {
    idle: Arc<Mutex<Option<CachedImageBridge>>>,
}

struct CachedImageBridge {
    bridge: NativeBridge,
    native: NativeConfig,
    config_sha256: [u8; 32],
    idle_since: Instant,
}

impl ImageSessionCache {
    fn retain(&self, cached: CachedImageBridge, slot: &mut Option<CachedImageBridge>) {
        let idle_since = cached.idle_since;
        *slot = Some(cached);
        // A weak reference lets dropping the account's client kill the helper
        // immediately. Idle helpers must not keep credentials alive forever.
        let weak = Arc::downgrade(&self.idle);
        let deadline = tokio::time::Instant::now() + IMAGE_HELPER_IDLE_TIMEOUT;
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(cache) = weak.upgrade() {
                let mut slot = cache.lock().await;
                if slot
                    .as_ref()
                    .is_some_and(|cached| cached.idle_since == idle_since)
                {
                    slot.take();
                }
            }
        });
    }
}

#[cfg(debug_assertions)]
pub(crate) fn installed(config: &SecureClientConfig) -> Result<Option<NativeConfig>> {
    let Some(path) = std::env::var_os("AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG") else {
        return Ok(None);
    };
    if config.relay_base_url.as_str().trim_end_matches('/') != "https://127.0.0.1:9443" {
        return Err(SecureClientError::configuration(
            "Chutes development adapter requires the isolated local API",
        ));
    }
    let raw = std::fs::read(path).map_err(|_| {
        SecureClientError::configuration("local Chutes adapter configuration unavailable")
    })?;
    if raw.len() > 16 * 1024 {
        return Err(SecureClientError::configuration(
            "local adapter configuration too large",
        ));
    }
    let value: NativeConfig = serde_json::from_slice(&raw).map_err(|_| {
        SecureClientError::configuration("invalid local Chutes adapter configuration")
    })?;
    if value.helper.is_some()
        || value.helper_sha256.is_some()
        || [
            value.python.as_deref(),
            value.script.as_deref(),
            Some(value.config.as_path()),
        ]
        .iter()
        .any(|path| path.is_none_or(|path| !path.is_absolute() || !path.is_file()))
    {
        return Err(SecureClientError::configuration(
            "local adapter requires absolute existing native paths",
        ));
    }
    if value
        .release_authority_fingerprint
        .as_ref()
        .is_some_and(|pin| {
            pin.len() != 64
                || !pin
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    {
        return Err(SecureClientError::configuration(
            "invalid installed release authority",
        ));
    }
    Ok(Some(value))
}

#[cfg(debug_assertions)]
pub(crate) fn models(config: &SecureClientConfig) -> Vec<ModelInfo> {
    let Ok(Some(native)) = installed(config) else {
        return vec![];
    };
    vec![ModelInfo {
        id: MODEL.into(),
        label: "Qwen3.8 Uncensored".into(),
        short_label: "Qwen3.8 Uncensored".into(),
        provider_id: ID.into(),
        provider_label: "Chutes local staging".into(),
        tags: vec![axiom_inference::ModelTag {
            id: axiom_inference::UNCENSORED_MODEL_TAG.into(),
            label: "Uncensored".into(),
            tone: axiom_inference::ModelTagTone::Pink,
            position: 50,
        }],
        upstream_model: "Qwen3.8-27B-Uncensored".into(),
        provider_base_url: "https://api.chutes.ai".into(),
        e2ee_protocol: PROTOCOL.into(),
        e2ee_encryption_version: 1,
        attestation_protocol: if native.release_authority_fingerprint.is_some() {
            RELEASE_PROFILE
        } else {
            PROFILE
        }
        .into(),
        context_window_tokens: 32768,
        max_output_tokens: 4096,
        supports_streaming: true,
        supports_tools: true,
        supported_reasoning_efforts: vec![ReasoningEffort::Medium],
        supported_thinking_modes: vec![ThinkingMode::ProviderDefault, ThinkingMode::Enabled],
        input_price_microusd_per_million_tokens: None,
        output_price_microusd_per_million_tokens: None,
        ..ModelInfo::default()
    }]
}

#[cfg(debug_assertions)]
pub(crate) fn provider(config: &SecureClientConfig) -> Result<Option<Arc<dyn SecureProvider>>> {
    Ok(installed(config)?.map(|native| {
        Arc::new(PreviewProvider {
            native,
            hosted: None,
        }) as Arc<dyn SecureProvider>
    }))
}

pub(crate) fn hosted_provider(
    config: &SecureClientConfig,
    credential: Arc<ApiCredential>,
) -> Result<Option<Arc<dyn SecureProvider>>> {
    Ok(hosted_native()?.map(|native| {
        Arc::new(PreviewProvider {
            native,
            hosted: Some((config.clone(), credential)),
        }) as Arc<dyn SecureProvider>
    }))
}

fn hosted_native() -> Result<Option<NativeConfig>> {
    let path = if let Some(path) = std::env::var_os("AXIOM_CHUTES_CLIENT_CONFIG_FILE") {
        PathBuf::from(path)
    } else {
        let executable = std::env::current_exe().map_err(|_| {
            SecureClientError::configuration("native executable location unavailable")
        })?;
        let Some(path) = bundled_native_config(&executable)? else {
            return Ok(None);
        };
        path
    };
    load_hosted_native(&path).map(Some)
}

fn bundled_native_config(executable: &std::path::Path) -> Result<Option<PathBuf>> {
    let Some(directory) = executable.parent().and_then(|directory| directory.parent()) else {
        return Ok(None);
    };
    let path = directory.join("chutes/native.json");
    if !path.exists() {
        return Ok(None);
    }
    let identity = std::fs::read(directory.join("axiom-install.json")).map_err(|_| {
        SecureClientError::configuration("bundled Chutes installation identity unavailable")
    })?;
    let identity: Value = serde_json::from_slice(&identity).map_err(|_| {
        SecureClientError::configuration("invalid bundled Chutes installation identity")
    })?;
    if identity["schemaVersion"] != 1
        || !matches!(identity["product"].as_str(), Some("cli" | "desktop"))
    {
        return Err(SecureClientError::configuration(
            "unrecognized bundled Chutes installation",
        ));
    }
    Ok(Some(path))
}

#[cfg(test)]
#[test]
fn helper_discovery_requires_a_recognized_installation_identity() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("bin/axiomcli");
    assert!(bundled_native_config(&executable).unwrap().is_none());
    std::fs::create_dir(root.path().join("chutes")).unwrap();
    let config = root.path().join("chutes/native.json");
    std::fs::write(&config, "{}").unwrap();
    assert!(bundled_native_config(&executable).is_err());
    for identity in [
        json!({"schemaVersion":2,"product":"cli"}),
        json!({"schemaVersion":1,"product":"other"}),
    ] {
        std::fs::write(root.path().join("axiom-install.json"), identity.to_string()).unwrap();
        assert!(bundled_native_config(&executable).is_err());
    }
    for product in ["cli", "desktop"] {
        std::fs::write(
            root.path().join("axiom-install.json"),
            json!({"schemaVersion":1,"product":product}).to_string(),
        )
        .unwrap();
        assert_eq!(
            bundled_native_config(&executable).unwrap(),
            Some(config.clone())
        );
    }
}

fn load_hosted_native(path: &std::path::Path) -> Result<NativeConfig> {
    let raw = std::fs::read(path)
        .map_err(|_| SecureClientError::configuration("Chutes native installation unavailable"))?;
    if raw.len() > 16 * 1024 {
        return Err(SecureClientError::configuration(
            "Chutes native configuration too large",
        ));
    }
    let mut native: NativeConfig = serde_json::from_slice(&raw)
        .map_err(|_| SecureClientError::configuration("invalid Chutes native installation"))?;
    let directory = path.parent().ok_or_else(|| {
        SecureClientError::configuration("Chutes native installation directory unavailable")
    })?;
    for installed in [&mut native.python, &mut native.script, &mut native.helper]
        .into_iter()
        .flatten()
    {
        resolve_installed_path(installed, directory)?;
    }
    resolve_installed_path(&mut native.config, directory)?;
    let pinned_code = match (
        &native.python,
        &native.script,
        &native.script_sha256,
        &native.helper,
        &native.helper_sha256,
    ) {
        (Some(_), Some(script), Some(pin), None, None) => Some((script, pin)),
        (None, None, None, Some(helper), Some(pin)) => Some((helper, pin)),
        _ => None,
    };
    if native
        .release_authority_fingerprint
        .as_ref()
        .is_none_or(|pin| {
            pin.len() != 64
                || !pin
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        || native.maximum_charge_microusd > 9_007_199_254_740_991
        || pinned_code.is_none_or(|(code, pin)| {
            std::fs::read(code).map_or(true, |raw| hex::encode(Sha256::digest(raw)) != *pin)
        })
    {
        return Err(SecureClientError::configuration(
            "Chutes installed helper, publisher pin or price ceiling differs",
        ));
    }
    Ok(native)
}

fn resolve_installed_path(path: &mut PathBuf, directory: &std::path::Path) -> Result<()> {
    if !path.is_absolute() {
        if path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(SecureClientError::configuration(
                "invalid relative Chutes installation path",
            ));
        }
        let base = directory.canonicalize().map_err(|_| {
            SecureClientError::configuration("Chutes native installation directory unavailable")
        })?;
        let resolved = base.join(&*path).canonicalize().map_err(|_| {
            SecureClientError::configuration("Chutes native installation file unavailable")
        })?;
        if !resolved.starts_with(base) {
            return Err(SecureClientError::configuration(
                "Chutes installation path escapes bundle",
            ));
        }
        *path = resolved;
    }
    if !path.is_file() {
        return Err(SecureClientError::configuration(
            "Chutes native installation file unavailable",
        ));
    }
    Ok(())
}

/// A verified artifact, kept out of model history and scoped to its native tool.
#[derive(Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImage {
    pub request_id: String,
    pub data: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_image_ids: Vec<String>,
    /// Native-selected origin, retained so regeneration keeps the same policy
    /// after the user switches the thread's chat model. Never a tool argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_model_id: Option<String>,
    pub evidence: SecurityEvidence,
    pub usage: axiom_inference::RequestUsage,
}

impl std::fmt::Debug for GeneratedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedImage")
            .field("request_id", &self.request_id)
            .field("data", &"[image bytes omitted]")
            .field("mime_type", &self.mime_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("sha256", &self.sha256)
            .field("source_image_ids", &self.source_image_ids)
            .field("chat_model_id", &self.chat_model_id)
            .field("evidence", &self.evidence)
            .field("usage", &self.usage)
            .finish()
    }
}

pub(crate) async fn generate_image(
    config: &SecureClientConfig,
    credential: Arc<ApiCredential>,
    sessions: &ImageSessionCache,
    chat_model_id: &str,
    prompt: &str,
    uncensored: bool,
    cancel: CancellationToken,
) -> Result<GeneratedImage> {
    image_operation(
        config,
        credential,
        sessions,
        chat_model_id,
        prompt,
        None,
        None,
        uncensored,
        cancel,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "compatibility wrapper around the structured image request"
)]
pub(crate) async fn image_operation(
    config: &SecureClientConfig,
    credential: Arc<ApiCredential>,
    sessions: &ImageSessionCache,
    chat_model_id: &str,
    prompt: &str,
    source: Option<(&str, &axiom_inference::ImageContent)>,
    dimensions: Option<axiom_inference::ImageDimensions>,
    uncensored: bool,
    cancel: CancellationToken,
) -> Result<GeneratedImage> {
    let references = source.into_iter().collect::<Vec<_>>();
    image_operation_with_progress(
        config,
        credential,
        sessions,
        axiom_inference::ImageGenerationRequest {
            chat_model_id,
            prompt,
            references: &references,
            dimensions,
        },
        uncensored,
        None,
        cancel,
    )
    .await
}

pub(crate) async fn image_operation_with_progress(
    config: &SecureClientConfig,
    credential: Arc<ApiCredential>,
    sessions: &ImageSessionCache,
    request: axiom_inference::ImageGenerationRequest<'_>,
    uncensored: bool,
    progress: Option<mpsc::Sender<axiom_inference::ImageGenerationProgress>>,
    cancel: CancellationToken,
) -> Result<GeneratedImage> {
    let native = hosted_native()?.ok_or_else(|| {
        SecureClientError::capability("encrypted image generation is not installed")
    })?;
    image_operation_on_installation(
        config, credential, sessions, native, request, uncensored, progress, cancel,
    )
    .await
}

async fn checkout_image_bridge(
    native: &NativeConfig,
    config: &SecureClientConfig,
    credential: &ApiCredential,
    cached: Option<CachedImageBridge>,
    cancel: &CancellationToken,
) -> Result<(NativeBridge, [u8; 32])> {
    let config_raw = std::fs::read(&native.config)
        .map_err(|_| SecureClientError::configuration("image helper configuration unavailable"))?;
    if config_raw.len() > 64 * 1024 {
        return Err(SecureClientError::configuration(
            "image helper configuration too large",
        ));
    }
    let config_sha256: [u8; 32] = Sha256::digest(config_raw).into();
    let cached = cached.filter(|cached| {
        cached.native == *native
            && cached.config_sha256 == config_sha256
            && cached.idle_since.elapsed() < IMAGE_HELPER_IDLE_TIMEOUT
    });
    let bridge = if let Some(mut cached) = cached
        && matches!(cached.bridge.child.try_wait(), Ok(None))
    {
        cached.bridge
    } else {
        let mut bridge = NativeBridge::spawn(native)?;
        bridge.call(json!({"op":"configure", "relay_origin":config.relay_base_url.as_str().trim_end_matches('/'),
            "access_token":credential.expose(), "maximum_charge_microusd":native.maximum_charge_microusd}),
            cancel, Duration::from_secs(10)).await?;
        bridge
    };
    Ok((bridge, config_sha256))
}

pub(crate) async fn image_capabilities(
    config: &SecureClientConfig,
    credential: Arc<ApiCredential>,
    sessions: &ImageSessionCache,
    cancel: CancellationToken,
) -> Result<axiom_inference::ImageRuntimePolicy> {
    let native = hosted_native()?.ok_or_else(|| {
        SecureClientError::capability("encrypted image generation is not installed")
    })?;
    let mut slot = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(SecureClientError::cancelled()),
        slot = sessions.idle.lock() => slot,
    };
    let (mut bridge, config_sha256) =
        checkout_image_bridge(&native, config, &credential, slot.take(), &cancel).await?;
    let reply = bridge
        .call(
            json!({"op":"capabilities"}),
            &cancel,
            Duration::from_secs(30),
        )
        .await?;
    let release: ReleaseAuthorization =
        serde_json::from_value(reply["release_authorization"].clone()).map_err(|_| {
            SecureClientError::attestation("signed capability authorization missing")
        })?;
    if release.method.as_deref() != Some("ed25519-publisher")
        || release.authority_fingerprint != native.release_authority_fingerprint
        || release.expires_at <= now()
        || release.policy_sequence == 0
    {
        return Err(SecureClientError::attestation(
            "capability release authority differs",
        ));
    }
    let runtime: axiom_inference::ImageRuntimePolicy =
        serde_json::from_value(reply["runtime"].clone()).map_err(|_| {
            SecureClientError::capability(
                "Unsupported runtime policy. Update Axiom before generating.",
            )
        })?;
    runtime.validate().map_err(SecureClientError::capability)?;
    sessions.retain(
        CachedImageBridge {
            bridge,
            native,
            config_sha256,
            idle_since: Instant::now(),
        },
        &mut slot,
    );
    Ok(runtime)
}

#[expect(
    clippy::too_many_arguments,
    reason = "installation-scoped image request with the client's tag-derived adapter decision"
)]
async fn image_operation_on_installation(
    config: &SecureClientConfig,
    credential: Arc<ApiCredential>,
    sessions: &ImageSessionCache,
    native: NativeConfig,
    request: axiom_inference::ImageGenerationRequest<'_>,
    uncensored: bool,
    progress: Option<mpsc::Sender<axiom_inference::ImageGenerationProgress>>,
    cancel: CancellationToken,
) -> Result<GeneratedImage> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let axiom_inference::ImageGenerationRequest {
        chat_model_id,
        prompt,
        references,
        dimensions,
    } = request;
    if prompt.trim().is_empty() || prompt.len() > 8192 {
        return Err(SecureClientError::capability(
            "image prompt must contain 1–8192 bytes",
        ));
    }
    if let Some(size) = dimensions {
        size.validate().map_err(|_| {
            SecureClientError::capability(
                "image dimensions must fit the 32-pixel grid and 1536 squared pixel area",
            )
        })?;
    }
    if references.len() > 5 {
        return Err(SecureClientError::capability(
            "at most five image references are supported",
        ));
    }
    let mut source_values = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    let mut total = 0;
    for &(id, image) in references {
        if id.len() != 36
            || !id.starts_with("img_")
            || !id[4..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !identities.insert(id)
        {
            return Err(SecureClientError::capability(
                "invalid or duplicate local image ID",
            ));
        }
        image
            .validate_bounded(axiom_inference::MAX_IMAGE_BYTES)
            .map_err(|_| SecureClientError::capability("normalized image reference required"))?;
        let raw = STANDARD
            .decode(&image.data)
            .map_err(|_| SecureClientError::capability("invalid image bytes"))?;
        total += raw.len();
        if total > 25 * 1024 * 1024 {
            return Err(SecureClientError::capability(
                "aggregate image reference byte limit",
            ));
        }
        source_values.push(json!({"id":id,"mime_type":image.mime_type,"data":image.data,"sha256":hex::encode(Sha256::digest(raw))}));
    }
    let source_bindings = source_values
        .iter()
        .map(|value| json!({"id": value["id"], "sha256": value["sha256"]}))
        .collect::<Vec<_>>();
    let model = ModelInfo {
        id: if references.is_empty() {
            "chutes:qwen-image-2.1-turbo"
        } else {
            "chutes:qwen-image-2.1-turbo-edit"
        }
        .into(),
        provider_id: HOSTED_ID.into(),
        attestation_protocol: RELEASE_PROFILE.into(),
        e2ee_protocol: PROTOCOL.into(),
        e2ee_encryption_version: 1,
        ..ModelInfo::default()
    };
    // Serialize this credential's image operations. Check the helper out before
    // awaiting any pipe operation: cancellation, task abort, EOF, malformed
    // output and failed validation must drop it with no replay or unread frames.
    let mut slot = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(SecureClientError::cancelled()),
        slot = sessions.idle.lock() => slot,
    };
    let cached = slot.take();
    let (mut bridge, config_sha256) =
        checkout_image_bridge(&native, config, &credential, cached, &cancel).await?;
    // The helper may reuse its still-fresh Chutes evidence. Rust accepts it for
    // this image model before dispatch, irrespective of the chat provider.
    let report = bridge
        .call(json!({"op":"attest"}), &cancel, Duration::from_secs(120))
        .await?;
    let hardware: Hardware = serde_json::from_value(report["evidence"].clone())
        .map_err(|_| SecureClientError::attestation("image hardware report missing"))?;
    let evidence = hardware
        .evidence_with_authority(&model, native.release_authority_fingerprint.as_deref())?;
    let runtime = hardware.image_runtime()?;
    let dimensions = match (dimensions, references.is_empty()) {
        (None, true) => Some(
            runtime
                .dimensions(&runtime.image.default_aspect_ratio)
                .map_err(SecureClientError::capability)?,
        ),
        (size, _) => size,
    };
    if references.len() > runtime.image.max_references
        || dimensions
            .is_some_and(|d| u64::from(d.width) * u64::from(d.height) > runtime.image.max_pixels)
    {
        return Err(SecureClientError::capability(
            "image exceeds the signed runtime limits",
        ));
    }
    let mut identity = [0_u8; 16];
    OsRng.fill_bytes(&mut identity);
    let request_id = hex::encode(identity);
    let mut operation =
        image_operation_request(&request_id, prompt, !references.is_empty(), uncensored);
    if let Some(digest) = &hardware.runtime_policy_sha256 {
        operation["runtime_policy_sha256"] = json!(digest);
    }
    operation["references"] = json!(source_values);
    if let Some(size) = dimensions {
        operation["width"] = json!(size.width);
        operation["height"] = json!(size.height);
    }
    let reply = if let Some(progress) = progress {
        operation["stream"] = json!(true);
        bridge
            .image_call(operation, &request_id, progress, &cancel)
            .await?
    } else {
        bridge
            .call(operation, &cancel, Duration::from_secs(660))
            .await?
    };
    if reply["response_authenticated"] != true
        || reply["billing_receipt_verified"] != true
        || reply["billing"]["settled"] != true
        || reply["request_id"] != request_id
        || reply["model_id"] != model.id
        || reply["mime_type"] != "image/png"
        || reply["uncensored"] != uncensored
        || reply.get("lora").is_none()
        || reply["lora"] != runtime_image_lora(&runtime, uncensored)
        || reply.get("runtime_policy_sha256").and_then(Value::as_str)
            != hardware.runtime_policy_sha256.as_deref()
        || dimensions
            .is_some_and(|size| reply["width"] != size.width || reply["height"] != size.height)
        || reply["references"] != json!(source_bindings)
        || reply["steps"] != runtime.image.sigmas.len() - 1
        || reply["evidence"]["key_sha256"] != evidence.model_key_fingerprint
        || reply["evidence"]["instance_id"] != hardware.instance_id
    {
        return Err(SecureClientError::attestation(
            "image completion, worker or usage differs",
        ));
    }
    let width = reply["width"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (256..=8192).contains(n) && n % 32 == 0)
        .ok_or_else(|| SecureClientError::attestation("invalid image width"))?;
    let height = reply["height"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (256..=8192).contains(n) && n % 32 == 0)
        .ok_or_else(|| SecureClientError::attestation("invalid image height"))?;
    if u64::from(width) * u64::from(height) > axiom_inference::MAX_GENERATED_IMAGE_PIXELS {
        return Err(SecureClientError::attestation("image pixel limit exceeded"));
    }
    let data = reply["image_base64"]
        .as_str()
        .filter(|v| v.len() <= 14_000_000)
        .ok_or_else(|| SecureClientError::attestation("bounded image bytes required"))?;
    let raw = STANDARD
        .decode(data)
        .map_err(|_| SecureClientError::attestation("invalid image encoding"))?;
    if raw.len() < 24
        || raw.len() > axiom_inference::MAX_GENERATED_IMAGE_BYTES
        || raw[..16] != *b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"
        || raw[16..20] != width.to_be_bytes()
        || raw[20..24] != height.to_be_bytes()
        || STANDARD.encode(&raw) != data
        || reply["image_sha256"] != hex::encode(Sha256::digest(&raw))
    {
        return Err(SecureClientError::attestation(
            "image bytes or dimensions differ",
        ));
    }
    let mut usage: axiom_inference::RequestUsage =
        serde_json::from_value(reply["accounting"].clone())
            .map_err(|_| SecureClientError::attestation("image usage missing"))?;
    if usage.request_id != request_id
        || usage.provider_id != HOSTED_ID
        || usage.model_id != model.id
        || !usage.settled
        || usage.response_verified
        || !usage.validate_counters()
        || usage.state != axiom_inference::InvocationState::Completed
        || usage.completeness != axiom_inference::UsageCompleteness::Final
        || usage
            .cost_microusd
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok())
            .is_none_or(|cost| cost > native.maximum_charge_microusd)
        || usage.input_tokens.as_deref() != Some("0")
        || usage.output_tokens.as_deref() != Some("0")
    {
        return Err(SecureClientError::attestation("image accounting differs"));
    }
    usage.response_verified = true;
    sessions.retain(
        CachedImageBridge {
            bridge,
            native,
            config_sha256,
            idle_since: Instant::now(),
        },
        &mut slot,
    );
    Ok(GeneratedImage {
        request_id,
        data: data.into(),
        mime_type: "image/png".into(),
        width,
        height,
        sha256: hex::encode(Sha256::digest(&raw)),
        source_image_ids: references.iter().map(|(id, _)| (*id).into()).collect(),
        chat_model_id: Some(chat_model_id.into()),
        evidence,
        usage,
    })
}

struct PreviewProvider {
    native: NativeConfig,
    hosted: Option<(SecureClientConfig, Arc<ApiCredential>)>,
}
impl sealed::Provider for PreviewProvider {}
#[async_trait]
impl SecureProvider for PreviewProvider {
    fn id(&self) -> &'static str {
        if self.hosted.is_some() { HOSTED_ID } else { ID }
    }
    fn supports(&self, model: &ModelInfo) -> Result<()> {
        if model.id
            != if self.hosted.is_some() {
                HOSTED_MODEL
            } else {
                MODEL
            }
            || model.provider_id != self.id()
            || model.e2ee_protocol != PROTOCOL
            || model.e2ee_encryption_version != 1
            || model.attestation_protocol
                != if self.native.release_authority_fingerprint.is_some() {
                    RELEASE_PROFILE
                } else {
                    PROFILE
                }
            || !matches!(
                model.provider_base_url.as_str(),
                "https://api.chutes.ai" | "https://api.chutes.ai/"
            )
            || model.upstream_model != "Qwen3.8-27B-Uncensored"
            || (self.hosted.is_some()
                && (model.context_window_tokens != 32768 || model.max_output_tokens != 4096))
        {
            return Err(SecureClientError::capability(
                "unconfigured development model",
            ));
        }
        Ok(())
    }
    fn supports_contract(&self, protocol: &str, version: u16, profile: &str) -> bool {
        self.hosted.is_some() && protocol == PROTOCOL && version == 1 && profile == RELEASE_PROFILE
    }
    fn populate_capabilities(&self, model: &mut ModelInfo) {
        model.supports_streaming = true;
        model.supports_tools = true;
        model.supports_parallel_tools = false;
        model.supports_images = true;
        model.file_mime_types.clear();
        model.supported_reasoning_efforts = vec![ReasoningEffort::Medium];
        model.supported_thinking_modes = vec![ThinkingMode::ProviderDefault, ThinkingMode::Enabled];
    }
    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancel: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.supports(model)?;
        if !policy.is_valid_at(now()) {
            return Err(SecureClientError::attestation("trust policy expired"));
        }
        let mut bridge = NativeBridge::spawn(&self.native)?;
        if let Some((config, credential)) = &self.hosted {
            bridge.call(json!({"op":"configure", "relay_origin":config.relay_base_url.as_str().trim_end_matches('/'),
                "access_token":credential.expose(), "maximum_charge_microusd":self.native.maximum_charge_microusd}),
                &cancel, Duration::from_secs(10)).await?;
        }
        let reply = bridge
            .call(json!({"op":"attest"}), &cancel, Duration::from_secs(120))
            .await?;
        let hardware: Hardware = serde_json::from_value(reply["evidence"].clone())
            .map_err(|_| SecureClientError::attestation("invalid local hardware report"))?;
        let evidence = hardware
            .evidence_with_authority(model, self.native.release_authority_fingerprint.as_deref())?;
        Ok(Box::new(Session {
            bridge,
            model: model.clone(),
            evidence,
            release_authority_fingerprint: self.native.release_authority_fingerprint.clone(),
            hosted: self.hosted.is_some(),
        }))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hardware {
    instance_id: String,
    expires_at: u64,
    verified_at: u64,
    quote_sha256: String,
    key_sha256: String,
    #[serde(rename = "hardware_verified")]
    verified: bool,
    workload_binding_verified: bool,
    #[serde(default)]
    release_authorization: Option<ReleaseAuthorization>,
    #[serde(default)]
    runtime: Option<axiom_inference::ImageRuntimePolicy>,
    #[serde(default)]
    runtime_policy_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseAuthorization {
    repository: String,
    #[serde(default)]
    commit: String,
    manifest_sha256: String,
    policy_sequence: u64,
    expires_at: u64,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    authority_fingerprint: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    release_id: Option<String>,
}
impl Hardware {
    fn image_runtime(&self) -> Result<axiom_inference::ImageRuntimePolicy> {
        if self.runtime.is_some() != self.runtime_policy_sha256.is_some()
            || self.runtime_policy_sha256.as_ref().is_some_and(|v| {
                v.len() != 64
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err(SecureClientError::attestation(
                "runtime policy binding missing",
            ));
        }
        let runtime = self.runtime.clone().unwrap_or_default();
        runtime.validate().map_err(SecureClientError::capability)?;
        Ok(runtime)
    }
    #[cfg(test)]
    fn evidence(&self, model: &ModelInfo) -> Result<SecurityEvidence> {
        self.evidence_with_authority(model, None)
    }
    fn evidence_with_authority(
        &self,
        model: &ModelInfo,
        authority: Option<&str>,
    ) -> Result<SecurityEvidence> {
        let valid_hash =
            |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
        if !self.verified
            || self.workload_binding_verified
            || self.expires_at <= now()
            || self.expires_at > now() + 125
            || self.verified_at > now() + 5
            || !valid_hash(&self.quote_sha256)
            || !valid_hash(&self.key_sha256)
        {
            return Err(SecureClientError::attestation(
                "fresh hardware evidence required for preview",
            ));
        }
        let authorized = if model.attestation_protocol == RELEASE_PROFILE {
            let release = self.release_authorization.as_ref().ok_or_else(|| {
                SecureClientError::attestation("signed release authorization required")
            })?;
            if authority.is_none()
                || release.method.as_deref() != Some("ed25519-publisher")
                || release.authority_fingerprint.as_deref() != authority
                || !matches!(release.channel.as_deref(), Some("staging" | "production"))
                || (model.provider_id != HOSTED_ID && release.channel.as_deref() != Some("staging"))
                || release.release_id.as_ref().is_none_or(|id| {
                    id.is_empty()
                        || id.len() > 128
                        || !id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                })
            {
                return Err(SecureClientError::attestation(
                    "release does not match the installed staging authority",
                ));
            }
            true
        } else {
            if authority.is_some()
                || self
                    .release_authorization
                    .as_ref()
                    .is_some_and(|release| release.method.is_some())
            {
                return Err(SecureClientError::attestation(
                    "release profile downgrade rejected",
                ));
            }
            false
        };
        let mut evidence = SecurityEvidence {
            state: if authorized { SecurityState::Verified } else { SecurityState::Degraded }, provider_id: model.provider_id.clone(), model_id: model.id.clone(),
            attestation_protocol: model.attestation_protocol.clone(), e2ee_protocol: PROTOCOL.into(), e2ee_encryption_version: 1,
            trust_policy_version: if authorized { "signed-publisher-release-authority" } else { "local-development-hardware-only" }.into(),
            verified_at_unix_seconds: self.verified_at, attestation_generation: None,
            hard_expires_at_unix_seconds: Some(self.expires_at), model_key_fingerprint: self.key_sha256.clone(),
            tls_spki_fingerprint: None,
            checks: [
                ("intel_tdx", "Intel TDX", true, "UpToDate"),
                ("nvidia_gpu", "NVIDIA GPU", true, "verified"),
                ("key_binding", "Chutes encryption key and fresh challenge", true, "quote-bound"),
            ].into_iter().map(|(id, label, passed, status)| EvidenceCheck {
                id: id.into(), label: label.into(), status: status.into(), passed,
            }).collect(),
            provider_claims: vec![
                EvidenceClaim { name: "instance_id".into(), value: self.instance_id.clone() },
                EvidenceClaim { name: "quote_sha256".into(), value: self.quote_sha256.clone() },
                EvidenceClaim { name: "preview_notice".into(), value: "Hardware checked locally. Application provenance and model-file binding remain unqualified.".into() },
            ], workload_manifest: None,
        };
        if authorized {
            evidence
                .provider_claims
                .retain(|claim| claim.name != "preview_notice");
            evidence.provider_claims.extend([
                EvidenceClaim { name: "verification_scope".into(), value: "TEE hardware, encryption key and publisher release authorization".into() },
                EvidenceClaim { name: "execution_provenance".into(), value: "Not independently established by the Chutes evidence; release authorization is a separate claim.".into() },
            ]);
        } else {
            evidence.checks.push(EvidenceCheck {
                id: "workload_provenance".into(),
                label: "Application provenance".into(),
                passed: false,
                status: "unqualified development preview".into(),
            });
        }
        if let Some(release) = &self.release_authorization {
            let repository = release.repository.split('/').collect::<Vec<_>>();
            if repository.len() != 2
                || repository.iter().any(|part| {
                    part.is_empty()
                        || part.len() > 100
                        || !part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                })
                || (!authorized
                    && (release.commit.len() != 40
                        || !release.commit.bytes().all(|b| b.is_ascii_hexdigit())))
                || !valid_hash(&release.manifest_sha256)
                || release.policy_sequence == 0
                || release.expires_at < self.expires_at
                || release.expires_at > now() + 7 * 86400
            {
                return Err(SecureClientError::attestation(
                    "invalid repository release authorization",
                ));
            }
            evidence.checks.push(EvidenceCheck {
                id: "release_authorization".into(), label: "Repository release authorization".into(),
                passed: true, status: if authorized { "Pinned publisher signature, current release policy and rollback protection verified" } else { "GitHub/Sigstore signature and current release policy checked; execution binding remains unqualified" }.into(),
            });
            for (name, value) in [
                ("github_repository", release.repository.clone()),
                ("release_commit", release.commit.clone()),
                ("release_manifest_sha256", release.manifest_sha256.clone()),
                (
                    "release_policy_sequence",
                    release.policy_sequence.to_string(),
                ),
            ] {
                evidence.provider_claims.push(EvidenceClaim {
                    name: name.into(),
                    value,
                });
            }
            if authorized {
                for (name, value) in [
                    (
                        "release_authority_fingerprint",
                        release.authority_fingerprint.clone().unwrap_or_default(),
                    ),
                    (
                        "release_channel",
                        release.channel.clone().unwrap_or_default(),
                    ),
                    ("release_id", release.release_id.clone().unwrap_or_default()),
                ] {
                    evidence.provider_claims.push(EvidenceClaim {
                        name: name.into(),
                        value,
                    });
                }
            }
        }
        Ok(evidence)
    }
}

struct NativeBridge {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl NativeBridge {
    fn spawn(config: &NativeConfig) -> Result<Self> {
        let mut command = match (&config.helper, &config.python, &config.script) {
            (Some(helper), None, None) => tokio::process::Command::new(helper),
            (None, Some(python), Some(script)) => {
                let mut command = tokio::process::Command::new(python);
                command.arg("-u").arg(script);
                command
            }
            _ => {
                return Err(SecureClientError::configuration(
                    "ambiguous native helper installation",
                ));
            }
        };
        command.env_clear();
        #[cfg(not(windows))]
        command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        #[cfg(windows)]
        {
            for key in [
                "SystemRoot",
                "WINDIR",
                "USERPROFILE",
                "LOCALAPPDATA",
                "TEMP",
                "TMP",
            ] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            if let Some(root) = std::env::var_os("SystemRoot") {
                command.env("PATH", PathBuf::from(root).join("System32"));
            }
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .arg("--config")
            .arg(&config.config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                SecureClientError::configuration("could not launch local encryption helper")
            })?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| SecureClientError::configuration("native input pipe unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| SecureClientError::configuration("native output pipe unavailable"))?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output),
        })
    }
    async fn send(&mut self, value: &Value) -> Result<()> {
        let mut raw = serde_json::to_vec(value)
            .map_err(|_| SecureClientError::capability("invalid native request"))?;
        if raw.len() > MAX_PIPE {
            return Err(SecureClientError::capability("native request too large"));
        }
        raw.push(b'\n');
        self.input
            .write_all(&raw)
            .await
            .map_err(|_| SecureClientError::session("native helper unavailable"))?;
        self.input
            .flush()
            .await
            .map_err(|_| SecureClientError::session("native helper unavailable"))
    }
    async fn receive(&mut self) -> Result<Value> {
        let mut line = Vec::new();
        loop {
            let available = self
                .output
                .fill_buf()
                .await
                .map_err(|_| SecureClientError::session("native helper unavailable"))?;
            if available.is_empty() {
                return Err(SecureClientError::attestation(
                    "native stream ended without completion",
                ));
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |i| i + 1);
            if line.len() + count > MAX_PIPE {
                return Err(SecureClientError::attestation("native reply too large"));
            }
            let ended = available[count - 1] == b'\n';
            line.extend_from_slice(&available[..count]);
            self.output.consume(count);
            if ended {
                break;
            }
        }
        let reply: Value = serde_json::from_slice(&line)
            .map_err(|_| SecureClientError::attestation("invalid native reply"))?;
        if reply["ok"] != true {
            return Err(match reply["error"].as_str() {
                Some("InsufficientCredit") => SecureClientError::new(
                    axiom_inference::ProviderFailureKind::InsufficientCredit,
                    crate::INSUFFICIENT_CREDIT_MESSAGE,
                ),
                Some("UnresolvedRequests") => SecureClientError::new(
                    axiom_inference::ProviderFailureKind::InvalidRequest,
                    "Earlier Chutes requests are still unresolved. They expire after 15 minutes; no new request was sent to the model.",
                ),
                Some("RequestAdmissionRejected") => SecureClientError::new(
                    axiom_inference::ProviderFailureKind::InvalidRequest,
                    "Chutes could not reserve this request. Try again or check your per-request price limit; no new request was sent to the model.",
                ),
                Some("RuntimeUpdateRequired") => SecureClientError::capability(
                    "This backend requires a newer version of Axiom. Update Axiom before generating; no request was sent to the model.",
                ),
                _ => SecureClientError::attestation(
                    "Chutes hardware or encrypted exchange failed; request was not replayed",
                ),
            });
        }
        Ok(reply)
    }
    async fn call(
        &mut self,
        value: Value,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> Result<Value> {
        let operation = async {
            self.send(&value).await?;
            self.receive().await
        };
        tokio::select! {
            () = cancel.cancelled() => Err(SecureClientError::cancelled()),
            reply = tokio::time::timeout(timeout, operation) => reply.map_err(|_| SecureClientError::session("Chutes preview timed out; request was not replayed"))?,
        }
    }

    async fn image_call(
        &mut self,
        value: Value,
        request_id: &str,
        progress: mpsc::Sender<axiom_inference::ImageGenerationProgress>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let operation = async {
            self.send(&value).await?;
            let mut stream = NativeImageStream::default();
            loop {
                let reply = self.receive().await?;
                if let Some(update) = stream.accept(&reply, request_id)? {
                    progress
                        .send(update)
                        .await
                        .map_err(|_| SecureClientError::cancelled())?;
                } else {
                    return Ok(reply);
                }
            }
        };
        tokio::select! {
            () = cancel.cancelled() => Err(SecureClientError::cancelled()),
            reply = tokio::time::timeout(Duration::from_secs(660), operation) => {
                reply.map_err(|_| SecureClientError::session("Image generation timed out; request was not replayed"))?
            }
        }
    }
}

#[derive(Default)]
struct NativeImageStream {
    next: u64,
    last: Option<axiom_inference::ImageGenerationProgress>,
    finished: bool,
}

impl NativeImageStream {
    fn accept(
        &mut self,
        reply: &Value,
        request_id: &str,
    ) -> Result<Option<axiom_inference::ImageGenerationProgress>> {
        use axiom_inference::{ImageGenerationProgress, ImageGenerationStage};
        if self.finished
            || self.next >= 2048
            || reply["request_id"] != request_id
            || reply["sequence"].as_u64() != Some(self.next)
        {
            return Err(SecureClientError::attestation(
                "Image progress order or request differs",
            ));
        }
        let result = match reply["event"].as_str() {
            Some("image_progress") => {
                let update: ImageGenerationProgress =
                    serde_json::from_value(reply["progress"].clone())
                        .map_err(|_| SecureClientError::attestation("Invalid image progress"))?;
                if !update.valid_after(self.last)
                    || self.last.is_none()
                        && update
                            != (ImageGenerationProgress {
                                stage: ImageGenerationStage::Queued,
                                percent: 0,
                            })
                {
                    return Err(SecureClientError::attestation(
                        "Invalid or regressing image progress",
                    ));
                }
                self.last = Some(update);
                Some(update)
            }
            Some("image_complete")
                if self.last
                    == Some(ImageGenerationProgress {
                        stage: ImageGenerationStage::Finishing,
                        percent: 99,
                    }) =>
            {
                self.finished = true;
                None
            }
            _ => {
                return Err(SecureClientError::attestation(
                    "Image progress ended without completion",
                ));
            }
        };
        self.next += 1;
        Ok(result)
    }
}

enum StreamItem {
    Start,
    Delta(ProviderEvent),
    Complete(InferenceResponse),
}
#[derive(Default)]
struct NativeStream {
    next: u64,
    started: bool,
    finished: bool,
    bytes: usize,
    text: String,
    reasoning: String,
    authority_fingerprint: Option<String>,
    hosted: bool,
}
impl NativeStream {
    fn accept(
        &mut self,
        reply: &Value,
        model: &ModelInfo,
        expected_key: &str,
    ) -> Result<StreamItem> {
        if self.finished || self.next >= 16384 || reply["sequence"].as_u64() != Some(self.next) {
            return Err(SecureClientError::attestation(
                "native stream order mismatch",
            ));
        }
        let event = reply["event"].as_str().unwrap_or_default();
        if matches!(event, "start" | "complete") {
            let hardware: Hardware = serde_json::from_value(reply["evidence"].clone())
                .map_err(|_| SecureClientError::attestation("invalid stream hardware report"))?;
            if !hardware.verified
                || hardware.workload_binding_verified
                || hardware.key_sha256 != expected_key
            {
                return Err(SecureClientError::attestation("preview worker changed"));
            }
            if event == "start" {
                hardware.evidence_with_authority(model, self.authority_fingerprint.as_deref())?;
            }
        }
        let item = match event {
            "start" if !self.started && self.next == 0 => {
                self.started = true;
                StreamItem::Start
            }
            "text" | "reasoning" if self.started => {
                let text = reply["text"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| SecureClientError::attestation("invalid stream text"))?;
                self.bytes = self.bytes.saturating_add(text.len());
                if text.len() > 128 * 1024 || self.bytes > 2 * 1024 * 1024 {
                    return Err(SecureClientError::attestation("native stream text limit"));
                }
                if event == "text" {
                    self.text.push_str(text);
                    StreamItem::Delta(ProviderEvent::TextDelta(text.to_owned()))
                } else {
                    self.reasoning.push_str(text);
                    StreamItem::Delta(ProviderEvent::ReasoningDelta(text.to_owned()))
                }
            }
            "complete" if self.started => {
                let response = self.complete(reply)?;
                self.finished = true;
                StreamItem::Complete(response)
            }
            _ => {
                return Err(SecureClientError::attestation(
                    "invalid native stream event",
                ));
            }
        };
        self.next += 1;
        Ok(item)
    }
    fn complete(&self, reply: &Value) -> Result<InferenceResponse> {
        if reply["response_authenticated"] != true
            || (self.hosted
                && (reply["billing_receipt_verified"] != true
                    || reply["billing"]["settled"] != true))
            || reply["text"].as_str() != Some(self.text.as_str())
            || reply["reasoning"].as_str() != Some(self.reasoning.as_str())
        {
            return Err(SecureClientError::attestation(
                "stream completion or transcript mismatch",
            ));
        }
        let tokens = |name| {
            reply[name]
                .as_u64()
                .filter(|v| *v <= 65536)
                .ok_or_else(|| SecureClientError::attestation("invalid authenticated stream usage"))
        };
        let input_tokens = tokens("input_tokens")?;
        let output_tokens = tokens("output_tokens")?;
        let finish = match reply["finish_reason"].as_str() {
            Some("stop") => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            Some("tool_calls") => FinishReason::ToolCalls,
            _ => {
                return Err(SecureClientError::attestation(
                    "invalid authenticated stream finish",
                ));
            }
        };
        let reasoning = if self.reasoning.is_empty() {
            None
        } else {
            Some(self.reasoning.clone())
        };
        let tool_calls: Vec<axiom_inference::ToolCall> = if reply.get("tool_calls").is_some() {
            serde_json::from_value(reply["tool_calls"].clone())
                .map_err(|_| SecureClientError::attestation("invalid authenticated tools"))?
        } else {
            Vec::new()
        };
        if (finish == FinishReason::ToolCalls) == tool_calls.is_empty() || tool_calls.len() > 32 {
            return Err(SecureClientError::attestation("tool completion differs"));
        }
        Ok(InferenceResponse {
            assistant: AssistantTurn {
                text: self.text.clone(),
                reasoning: reasoning.clone(),
                tool_calls,
            },
            reasoning,
            refusal: None,
            usage: Usage {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
            },
            finish_reason: Some(finish),
        })
    }
}

struct Session {
    bridge: NativeBridge,
    model: ModelInfo,
    evidence: SecurityEvidence,
    release_authority_fingerprint: Option<String>,
    hosted: bool,
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
        if request.model != self.model.id
            || request.response_format != ResponseFormat::Text
            || request.sampling != SamplingParameters::default()
        {
            return Err(SecureClientError::capability(
                "local preview supports text chat with medium reasoning",
            ));
        }
        let mut messages = Vec::new();
        let request_id = request.request_id.clone().unwrap_or_else(|| {
            let mut bytes = [0_u8; 16];
            OsRng.fill_bytes(&mut bytes);
            hex::encode(bytes)
        });
        if request_id.len() != 32
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(SecureClientError::capability(
                "canonical native invocation ID required",
            ));
        }
        let mut image_count = 0;
        for message in request.messages {
            if !message.files.is_empty() {
                return Err(SecureClientError::capability(
                    "this model does not accept document files",
                ));
            }
            let mut serialized = json!({"role": message.role, "content": message.content});
            if !message.images.is_empty() {
                image_count += message.images.len();
                if message.role != axiom_inference::ChatRole::User || image_count > 10 {
                    return Err(SecureClientError::capability(
                        "at most ten private user images are supported in the model context",
                    ));
                }
                let mut blocks = vec![json!({"type":"text","text":message.content})];
                for image in &message.images {
                    image.validate().map_err(|_| {
                        SecureClientError::capability("invalid private image input")
                    })?;
                    blocks.push(json!({"type":"image_url","image_url":{"url":image.data_url()}}));
                }
                serialized["content"] = json!(blocks);
            }
            if !message.tool_calls.is_empty() {
                serialized["tool_calls"] = json!(message.tool_calls);
            }
            if let Some(id) = message.tool_call_id {
                serialized["tool_call_id"] = json!(id);
            }
            messages.push(serialized);
        }
        let operation = async {
            let mut command = json!({"op":"stream", "messages":messages,
                "max_tokens":request.max_output_tokens.unwrap_or(4096).min(4096)});
            if !request.tools.is_empty() {
                command["tools"] = json!(request.tools);
            }
            if self.hosted {
                command["request_id"] = json!(request_id);
            }
            self.bridge.send(&command).await?;
            let mut stream = NativeStream {
                authority_fingerprint: self.release_authority_fingerprint.clone(),
                hosted: self.hosted,
                ..NativeStream::default()
            };
            loop {
                let reply = self.bridge.receive().await?;
                match stream.accept(&reply, &self.model, &self.evidence.model_key_fingerprint)? {
                    StreamItem::Start => {
                        let hardware: Hardware = serde_json::from_value(reply["evidence"].clone())
                            .map_err(|_| {
                                SecureClientError::attestation("invalid stream hardware report")
                            })?;
                        self.evidence = hardware.evidence_with_authority(
                            &self.model,
                            self.release_authority_fingerprint.as_deref(),
                        )?;
                        if let Some(events) = &events {
                            events
                                .send(ProviderEvent::SecurityEvidence(Box::new(
                                    self.evidence.clone(),
                                )))
                                .await
                                .map_err(|_| SecureClientError::cancelled())?;
                        }
                    }
                    StreamItem::Delta(event) => {
                        if let Some(events) = &events {
                            events
                                .send(event)
                                .await
                                .map_err(|_| SecureClientError::cancelled())?;
                        }
                    }
                    StreamItem::Complete(response) => {
                        if self.hosted {
                            let record: axiom_inference::RequestUsage = serde_json::from_value(
                                reply["accounting"].clone(),
                            )
                            .map_err(|_| {
                                SecureClientError::attestation("signed usage accounting is missing")
                            })?;
                            if record.provider_id != HOSTED_ID
                                || record.model_id != self.model.id
                                || !record.validate_counters()
                                || record.request_id != request_id
                                || !record.request_id.bytes().all(|b| b.is_ascii_hexdigit())
                                || !record.settled
                                || record.response_verified
                                || record.state != axiom_inference::InvocationState::Completed
                                || record.completeness != axiom_inference::UsageCompleteness::Final
                                || record
                                    .input_tokens
                                    .as_deref()
                                    .and_then(|s| s.parse::<u64>().ok())
                                    != Some(response.usage.input_tokens)
                                || record
                                    .output_tokens
                                    .as_deref()
                                    .and_then(|s| s.parse::<u64>().ok())
                                    != Some(response.usage.output_tokens)
                            {
                                return Err(SecureClientError::attestation(
                                    "signed usage accounting differs from the authenticated response",
                                ));
                            }
                            if let Some(events) = &events {
                                events
                                    .send(ProviderEvent::Accounting(Box::new(record)))
                                    .await
                                    .map_err(|_| SecureClientError::cancelled())?;
                            }
                        }
                        break Ok(response);
                    }
                }
            }
        };
        let response: InferenceResponse = tokio::select! {
            () = cancel.cancelled() => return Err(SecureClientError::cancelled()),
            result = tokio::time::timeout(Duration::from_secs(780), operation) => {
                result.map_err(|_| SecureClientError::session("Chutes stream timed out; request was not replayed"))??
            },
        };
        if let Some(events) = events {
            events
                .send(ProviderEvent::Usage {
                    input_tokens: response.usage.input_tokens,
                    output_tokens: response.usage.output_tokens,
                })
                .await
                .map_err(|_| SecureClientError::cancelled())?;
            events
                .send(ProviderEvent::ResponseVerified)
                .await
                .map_err(|_| SecureClientError::cancelled())?;
        }
        Ok(response)
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
#[path = "chutes_preview_image_tests.rs"]
mod image_cache_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_progress_requires_ordered_request_bound_updates_and_a_finishing_state() {
        let id = "12".repeat(16);
        let frame = |sequence, stage, percent| {
            json!({"ok":true,"request_id":id,"sequence":sequence,
            "event":"image_progress","progress":{"stage":stage,"percent":percent}})
        };
        let mut stream = NativeImageStream::default();
        for update in [
            frame(0, "queued", 0),
            frame(1, "generating", 38),
            frame(2, "finishing", 99),
        ] {
            assert!(stream.accept(&update, &id).unwrap().is_some());
        }
        let complete = json!({"ok":true,"request_id":id,"sequence":3,"event":"image_complete"});
        assert!(stream.accept(&complete, &id).unwrap().is_none());
        assert!(stream.accept(&complete, &id).is_err());
        for invalid in [
            frame(1, "queued", 0),
            frame(0, "generating", 38),
            frame(0, "queued", 100),
            json!({"ok":true,"request_id":"other","sequence":0,"event":"image_progress","progress":{"stage":"queued","percent":0}}),
            complete,
        ] {
            assert!(NativeImageStream::default().accept(&invalid, &id).is_err());
        }
        let mut stream = NativeImageStream::default();
        stream.accept(&frame(0, "queued", 0), &id).unwrap();
        stream.accept(&frame(1, "generating", 38), &id).unwrap();
        assert!(stream.accept(&frame(2, "generating", 25), &id).is_err());
    }

    #[test]
    fn standalone_installation_survives_relocation_and_rejects_changed_or_ambiguous_code() {
        let parent = tempfile::tempdir().unwrap();
        let original = parent.path().join("original");
        std::fs::create_dir_all(original.join("runtime")).unwrap();
        let helper = b"standalone native helper fixture";
        std::fs::write(original.join("runtime/helper"), helper).unwrap();
        std::fs::write(original.join("bridge.json"), b"{}").unwrap();
        let value = json!({"helper": "runtime/helper", "helper_sha256": hex::encode(Sha256::digest(helper)),
            "config": "bridge.json", "release_authority_fingerprint": "ab".repeat(32),
            "maximum_charge_microusd": 100_000});
        std::fs::write(original.join("native.json"), value.to_string()).unwrap();
        let moved = parent.path().join("moved app resources");
        std::fs::rename(&original, &moved).unwrap();
        let loaded = load_hosted_native(&moved.join("native.json")).unwrap();
        assert_eq!(
            loaded.helper,
            Some(moved.join("runtime/helper").canonicalize().unwrap())
        );
        assert!(loaded.python.is_none());
        assert!(loaded.script.is_none());
        assert_eq!(loaded.maximum_charge_microusd, 100_000);
        let mut ambiguous = value.clone();
        ambiguous["python"] = json!("runtime/helper");
        ambiguous["script"] = json!("runtime/helper");
        std::fs::write(moved.join("native.json"), ambiguous.to_string()).unwrap();
        assert!(load_hosted_native(&moved.join("native.json")).is_err());
        std::fs::write(moved.join("native.json"), value.to_string()).unwrap();
        std::fs::write(moved.join("runtime/helper"), b"changed native helper").unwrap();
        assert!(load_hosted_native(&moved.join("native.json")).is_err());
    }

    #[test]
    fn installed_relative_paths_cannot_leave_the_bundle() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("bundle");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(parent.path().join("outside"), b"outside").unwrap();
        assert!(resolve_installed_path(&mut PathBuf::from("../outside"), &directory).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(parent.path().join("outside"), directory.join("escape"))
                .unwrap();
            assert!(resolve_installed_path(&mut PathBuf::from("escape"), &directory).is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires AXIOM_CHUTES_BUNDLE_TEST_CONFIG_FILE pointing to a built native payload"]
    async fn frozen_helper_accepts_the_real_native_stdio_handshake_without_network() {
        let path = std::env::var_os("AXIOM_CHUTES_BUNDLE_TEST_CONFIG_FILE")
            .expect("built native config required");
        let native = load_hosted_native(&PathBuf::from(path)).unwrap();
        assert!(native.helper.is_some());
        let mut bridge = NativeBridge::spawn(&native).unwrap();
        bridge
            .send(
                &json!({"op":"configure", "relay_origin":"https://api-staging.axiom.stream",
            "access_token":"axa_synthetic_no_network", "maximum_charge_microusd":100_000}),
            )
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(15), bridge.receive())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, json!({"ok":true}));
        bridge
            .send(&json!({"op":"unsupported_synthetic_no_network"}))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), bridge.receive())
            .await
            .unwrap();
        assert!(
            reply.is_err(),
            "unsupported operations must fail closed in the native client"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_admission_errors_are_distinct_and_never_expose_helper_error_text() {
        use std::os::unix::fs::PermissionsExt;
        for (error_class, admission) in [
            ("InsufficientCredit", true),
            ("UnresolvedRequests", true),
            ("RequestAdmissionRejected", true),
            ("RuntimeUpdateRequired", false),
            ("ValueError", false),
            ("private_provider_error", false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let helper = directory.path().join("helper");
            let reply =
                json!({"ok":false,"error":error_class,"message":"private prompt or credential"});
            std::fs::write(&helper, format!("#!/bin/sh\nprintf '%s\\n' '{reply}'\n")).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
            let native = NativeConfig {
                python: None,
                script: None,
                helper: Some(helper),
                helper_sha256: None,
                config: directory.path().join("public.json"),
                release_authority_fingerprint: None,
                script_sha256: None,
                maximum_charge_microusd: 100_000,
            };
            let mut bridge = NativeBridge::spawn(&native).unwrap();
            let error = tokio::time::timeout(Duration::from_secs(2), bridge.receive())
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(
                error.kind(),
                if error_class == "InsufficientCredit" {
                    axiom_inference::ProviderFailureKind::InsufficientCredit
                } else if error_class == "RuntimeUpdateRequired" {
                    axiom_inference::ProviderFailureKind::CapabilityMismatch
                } else if admission {
                    axiom_inference::ProviderFailureKind::InvalidRequest
                } else {
                    axiom_inference::ProviderFailureKind::AttestationRejected
                }
            );
            assert!(!error.safe_detail().contains("private"));
            assert!(!error.retryable());
            if error_class == "InsufficientCredit" {
                assert_eq!(error.code(), crate::SecureErrorCode::InsufficientCredit);
                assert_eq!(error.safe_detail(), crate::INSUFFICIENT_CREDIT_MESSAGE);
            } else {
                assert!(!error.safe_detail().contains("Top up"));
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn standalone_helper_receives_only_config_arguments_and_private_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("helper");
        std::fs::write(&helper, "#!/bin/sh\n[ \"$1\" = --config ] || exit 1\n[ \"$#\" = 2 ] || exit 1\nIFS= read -r request || exit 1\nprintf '%s\\n' '{\"ok\":true}'\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let native = NativeConfig {
            python: None,
            script: None,
            helper: Some(helper),
            helper_sha256: None,
            config: directory.path().join("public.json"),
            release_authority_fingerprint: None,
            script_sha256: None,
            maximum_charge_microusd: 100_000,
        };
        let mut bridge = NativeBridge::spawn(&native).unwrap();
        bridge
            .send(&json!({"op":"configure","access_token":"synthetic-pipe-only"}))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(2), bridge.receive())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, json!({"ok":true}));
    }

    #[test]
    fn image_generation_selects_the_fixed_lora_only_from_the_client_tag_decision() {
        for edit in [false, true] {
            for uncensored in [false, true] {
                let request = image_operation_request("test", "A blue mug", edit, uncensored);
                assert_eq!(request["op"], if edit { "image_edit" } else { "image" });
                assert_eq!(request["uncensored"], uncensored);
                if uncensored {
                    assert_eq!(expected_image_lora(true)["strength"], 0.9);
                } else {
                    assert_eq!(expected_image_lora(false), Value::Null);
                }
                assert!(request.get("lora").is_none());
                assert!(request.get("strength").is_none());
            }
        }
        // The bundled offerings keep their Qwen identity prompt by exact ID.
        for (model, expected) in [
            (HOSTED_MODEL, true),
            (MODEL, true),
            ("near:qwen3.8-27b", false),
            ("chutes:qwen3.8-27b-extra", false),
            ("uncensored", false),
            ("", false),
        ] {
            assert_eq!(is_uncensored_model(model), expected);
        }
    }

    #[test]
    fn hosted_contract_is_distinct_from_preview_and_rejects_catalog_substitution() {
        let provider = PreviewProvider {
            native: NativeConfig {
                python: Some(PathBuf::from("/installed/python")),
                script: Some(PathBuf::from("/installed/bridge.py")),
                helper: None,
                helper_sha256: None,
                config: PathBuf::from("/installed/public.json"),
                release_authority_fingerprint: Some("ab".repeat(32)),
                script_sha256: Some("cd".repeat(32)),
                maximum_charge_microusd: 0,
            },
            hosted: Some((
                SecureClientConfig::new("https://relay.example").unwrap(),
                Arc::new(ApiCredential::new("axa_synthetic_test")),
            )),
        };
        let selected = ModelInfo {
            id: HOSTED_MODEL.into(),
            provider_id: HOSTED_ID.into(),
            e2ee_protocol: PROTOCOL.into(),
            e2ee_encryption_version: 1,
            attestation_protocol: RELEASE_PROFILE.into(),
            provider_base_url: "https://api.chutes.ai/".into(),
            upstream_model: "Qwen3.8-27B-Uncensored".into(),
            context_window_tokens: 32768,
            max_output_tokens: 4096,
            ..ModelInfo::default()
        };
        assert!(provider.supports(&selected).is_ok());
        assert!(provider.supports_contract(PROTOCOL, 1, RELEASE_PROFILE));
        assert!(!provider.supports_contract(PROTOCOL, 1, PROFILE));
        for change in 0..5 {
            let mut changed = selected.clone();
            match change {
                0 => changed.provider_base_url = "https://other.example".into(),
                1 => changed.id = MODEL.into(),
                2 => changed.attestation_protocol = PROFILE.into(),
                3 => changed.max_output_tokens = 8192,
                _ => changed.e2ee_encryption_version = 2,
            }
            assert!(provider.supports(&changed).is_err());
        }
    }

    #[test]
    fn hosted_completion_requires_verified_usage_and_acknowledged_settlement() {
        let stream = NativeStream {
            hosted: true,
            text: "answer".into(),
            reasoning: "thinking".into(),
            ..NativeStream::default()
        };
        let mut reply = terminal();
        assert!(stream.complete(&reply).is_err());
        reply["billing_receipt_verified"] = json!(true);
        assert!(stream.complete(&reply).is_err());
        reply["billing"] = json!({"settled":true});
        assert!(stream.complete(&reply).is_ok());
        reply["response_authenticated"] = json!(false);
        assert!(stream.complete(&reply).is_err());
    }

    fn model() -> ModelInfo {
        ModelInfo {
            id: MODEL.into(),
            ..ModelInfo::default()
        }
    }
    fn evidence() -> Value {
        json!({"instance_id":"local-test", "expires_at":now()+120, "verified_at":now(),
            "quote_sha256":"ab".repeat(32), "key_sha256":"cd".repeat(32),
            "hardware_verified":true, "workload_binding_verified":false})
    }
    fn start() -> Value {
        json!({"ok":true,"event":"start","sequence":0,"evidence":evidence()})
    }
    fn terminal() -> Value {
        json!({"ok":true,"event":"complete","sequence":3,"evidence":evidence(),
            "text":"answer", "reasoning":"thinking", "input_tokens":10, "output_tokens":20,
            "response_authenticated":true, "finish_reason":"stop"})
    }
    fn deltas(stream: &mut NativeStream) {
        let key = "cd".repeat(32);
        let item = stream
            .accept(
                &json!({"event":"reasoning","sequence":1,"text":"thinking"}),
                &model(),
                &key,
            )
            .unwrap();
        assert!(matches!(
            item,
            StreamItem::Delta(ProviderEvent::ReasoningDelta(_))
        ));
        let item = stream
            .accept(
                &json!({"event":"text","sequence":2,"text":"answer"}),
                &model(),
                &key,
            )
            .unwrap();
        assert!(matches!(
            item,
            StreamItem::Delta(ProviderEvent::TextDelta(_))
        ));
    }
    #[test]
    fn native_deltas_are_released_before_terminal_and_not_repeated_at_completion() {
        let mut stream = NativeStream::default();
        let key = "cd".repeat(32);
        assert!(matches!(
            stream.accept(&start(), &model(), &key).unwrap(),
            StreamItem::Start
        ));
        deltas(&mut stream);
        assert!(!stream.finished);
        match stream.accept(&terminal(), &model(), &key).unwrap() {
            StreamItem::Complete(response) => {
                assert_eq!(response.assistant.text, "answer");
                assert_eq!(response.usage.total_tokens, 30);
            }
            _ => panic!("expected authenticated completion"),
        }
        assert!(stream.accept(&terminal(), &model(), &key).is_err());
    }
    #[test]
    fn native_completion_rejects_worker_changes_reordering_and_transcript_substitution() {
        let key = "cd".repeat(32);
        let mut before_start = NativeStream::default();
        let mut first = terminal();
        first["sequence"] = json!(0);
        assert!(before_start.accept(&first, &model(), &key).is_err());
        for broken in 0..5 {
            let mut stream = NativeStream::default();
            stream.accept(&start(), &model(), &key).unwrap();
            deltas(&mut stream);
            let mut finish = terminal();
            match broken {
                0 => finish["text"] = json!("another user's response"),
                1 => finish["response_authenticated"] = json!(false),
                2 => finish["sequence"] = json!(4),
                3 => finish["evidence"]["key_sha256"] = json!("ef".repeat(32)),
                _ => finish["finish_reason"] = Value::Null,
            }
            assert!(stream.accept(&finish, &model(), &key).is_err());
            assert!(!stream.finished);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn actual_native_pipe_streams_before_completion_and_cancels_without_success() {
        use axiom_inference::ChatMessage;
        for cancel_after_delta in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let script = directory.path().join("stream.sh");
            let release = directory.path().join("release");
            let delta = json!({"ok":true,"event":"text","sequence":1,"text":"answer"});
            let mut end = terminal();
            end["sequence"] = json!(2);
            end["reasoning"] = json!("");
            std::fs::write(&script, format!(
                "IFS= read -r request || exit 1\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{}'\nwhile [ ! -f \"$2\" ]; do sleep 0.01; done\nprintf '%s\\n' '{}'\n",
                start(), delta, end,
            )).unwrap();
            let config = NativeConfig {
                python: Some(PathBuf::from("/bin/sh")),
                script: Some(script),
                helper: None,
                helper_sha256: None,
                config: release.clone(),
                release_authority_fingerprint: None,
                script_sha256: None,
                maximum_charge_microusd: 0,
            };
            let hardware: Hardware = serde_json::from_value(evidence()).unwrap();
            let model = model();
            let mut session = Session {
                bridge: NativeBridge::spawn(&config).unwrap(),
                evidence: hardware.evidence(&model).unwrap(),
                model,
                release_authority_fingerprint: None,
                hosted: false,
            };
            let request = InferenceRequest::streaming(
                MODEL,
                vec![ChatMessage::text(ChatRole::User, "fixed pipe fixture")],
                vec![],
            );
            let cancel = CancellationToken::new();
            let token = cancel.clone();
            let (send, mut receive) = mpsc::channel(8);
            let inference = tokio::spawn(async move { session.stream(request, send, token).await });
            let first = tokio::time::timeout(Duration::from_secs(2), receive.recv())
                .await
                .unwrap()
                .unwrap();
            let ProviderEvent::SecurityEvidence(report) = first else {
                panic!("the actual session report must precede display deltas");
            };
            assert_eq!(report.model_key_fingerprint, "cd".repeat(32));
            assert_eq!(report.state, SecurityState::Degraded);
            assert!(
                report
                    .checks
                    .iter()
                    .any(|check| check.id == "workload_provenance" && !check.passed)
            );
            let first = tokio::time::timeout(Duration::from_secs(2), receive.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(first, ProviderEvent::TextDelta(ref text) if text == "answer"));
            assert!(
                !inference.is_finished(),
                "native provider buffered until completion"
            );
            if cancel_after_delta {
                cancel.cancel();
                assert!(
                    tokio::time::timeout(Duration::from_secs(2), inference)
                        .await
                        .unwrap()
                        .unwrap()
                        .is_err()
                );
                while let Some(event) = receive.recv().await {
                    assert!(!matches!(event, ProviderEvent::ResponseVerified));
                }
            } else {
                std::fs::write(release, b"continue").unwrap();
                let response = tokio::time::timeout(Duration::from_secs(2), inference)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(response.assistant.text, "answer");
                let mut verified = 0;
                while let Some(event) = receive.recv().await {
                    if matches!(event, ProviderEvent::ResponseVerified) {
                        verified += 1;
                    }
                    assert!(
                        !matches!(event, ProviderEvent::TextDelta(_)),
                        "completion repeated the streamed answer"
                    );
                }
                assert_eq!(verified, 1);
            }
        }
    }

    #[test]
    fn authorized_repository_release_does_not_establish_execution_provenance() {
        let mut raw = evidence();
        raw["release_authorization"] = json!({
            "repository":"example/inference", "commit":"12".repeat(20),
            "manifest_sha256":"34".repeat(32), "policy_sequence":42, "expires_at":now()+172_800,
        });
        let hardware: Hardware = serde_json::from_value(raw.clone()).unwrap();
        let report = hardware.evidence(&model()).unwrap();
        assert_eq!(report.state, SecurityState::Degraded);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "release_authorization" && check.passed)
        );
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "workload_provenance" && !check.passed)
        );
        for (name, value) in [
            ("repository", json!("https://evil.example")),
            ("commit", json!("invalid")),
            ("manifest_sha256", json!("invalid")),
            ("policy_sequence", json!(0)),
            ("expires_at", json!(now() - 1)),
        ] {
            let mut broken = raw.clone();
            broken["release_authorization"][name] = value;
            let hardware: Hardware = serde_json::from_value(broken).unwrap();
            assert!(hardware.evidence(&model()).is_err());
        }
    }

    #[test]
    fn signed_staging_authority_verifies_hardware_without_claiming_execution_provenance() {
        let mut raw = evidence();
        let pin = "56".repeat(32);
        raw["release_authorization"] = json!({
            "repository":"astrea-foundation/axiom-inference", "manifest_sha256":"34".repeat(32),
            "policy_sequence":42, "expires_at":now()+3600, "method":"ed25519-publisher",
            "authority_fingerprint":pin, "channel":"staging", "release_id":"staging-two"
        });
        let mut selected = model();
        selected.attestation_protocol = RELEASE_PROFILE.into();
        let hardware: Hardware = serde_json::from_value(raw.clone()).unwrap();
        let report = hardware
            .evidence_with_authority(&selected, Some(&pin))
            .unwrap();
        assert_eq!(report.state, SecurityState::Verified);
        assert!(report.checks.iter().all(|check| check.passed));
        assert!(
            report
                .provider_claims
                .iter()
                .any(|claim| claim.name == "execution_provenance"
                    && claim.value.starts_with("Not independently established"))
        );
        assert!(hardware.evidence_with_authority(&selected, None).is_err());
        assert!(
            hardware
                .evidence_with_authority(&selected, Some(&"ab".repeat(32)))
                .is_err()
        );
        assert!(
            hardware
                .evidence_with_authority(&model(), Some(&pin))
                .is_err()
        );
        for (field, value) in [
            ("method", json!("self-signed")),
            ("channel", json!("production")),
            ("expires_at", json!(now() - 1)),
            ("manifest_sha256", json!("bad")),
        ] {
            let mut changed = raw.clone();
            changed["release_authorization"][field] = value;
            let hardware: Hardware = serde_json::from_value(changed).unwrap();
            assert!(
                hardware
                    .evidence_with_authority(&selected, Some(&pin))
                    .is_err()
            );
        }
        raw["workload_binding_verified"] = json!(true);
        let hardware: Hardware = serde_json::from_value(raw).unwrap();
        assert!(
            hardware
                .evidence_with_authority(&selected, Some(&pin))
                .is_err()
        );
    }
}

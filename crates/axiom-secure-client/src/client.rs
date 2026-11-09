use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axiom_inference::ModelInfo;
use fs2::FileExt as _;
use rand_core::{OsRng, RngCore as _};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, ProviderRegistry, Result, SecureClientConfig, TrustPolicy, VerifiedSession,
    catalog,
};

pub struct SecureClient {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    providers: ProviderRegistry,
    image_sessions: crate::providers::chutes_preview::ImageSessionCache,
    catalog_cache: RwLock<Option<CachedCatalog>>,
    trust_policy: RwLock<CachedTrustPolicy>,
}

struct CachedCatalog {
    fetched_at: Instant,
    models: Vec<ModelInfo>,
}

struct CachedTrustPolicy {
    policy: TrustPolicy,
    refreshed_at: Option<Instant>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedTrustPolicy {
    highest_sequence: u64,
    signed_policy: String,
}

impl SecureClient {
    fn direct_image_provider(&self) -> Result<std::sync::Arc<dyn crate::provider::SecureProvider>> {
        self.providers
            .by_id(crate::providers::direct::ID)
            .ok_or_else(|| {
                crate::SecureClientError::capability("direct image provider unavailable")
            })
    }

    pub async fn image_capabilities(
        &self,
        cancellation: CancellationToken,
    ) -> Result<axiom_inference::ImageRuntimePolicy> {
        if self.config.direct.is_some() {
            let policy = self.trust_policy(cancellation.clone()).await?;
            return self
                .direct_image_provider()?
                .image_capabilities(&policy, cancellation)
                .await;
        }
        crate::providers::chutes_preview::image_capabilities(
            &self.config,
            Arc::clone(&self.credential),
            &self.image_sessions,
            cancellation,
        )
        .await
    }
    pub fn new(config: SecureClientConfig, credential: ApiCredential) -> Result<Self> {
        if credential.is_empty() {
            return Err(crate::SecureClientError::configuration(
                "Axiom credential must not be empty",
            ));
        }
        let credential = Arc::new(credential);
        let providers = ProviderRegistry::production(config.clone(), Arc::clone(&credential))?;
        let trust_policy = load_initial_policy(&config)?;
        Ok(Self {
            config,
            credential,
            providers,
            image_sessions: crate::providers::chutes_preview::ImageSessionCache::default(),
            catalog_cache: RwLock::new(None),
            trust_policy: RwLock::new(CachedTrustPolicy {
                policy: trust_policy,
                refreshed_at: None,
            }),
        })
    }

    pub async fn request_accounting(
        &self,
        ids: &[String],
        cancellation: CancellationToken,
    ) -> Result<Vec<axiom_inference::RequestUsage>> {
        crate::relay::client::RelayClient::new(&self.config, &self.credential)
            .accounting(ids, &cancellation)
            .await
    }

    /// Whether the chat model's catalog tags select the uncensored image
    /// adapter. Uses the accepted catalog, refreshing it when absent or stale;
    /// an unknown chat model uses the standard adapter.
    async fn uncensored_image_policy(
        &self,
        chat_model_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<bool> {
        let tagged = |models: &[ModelInfo]| {
            models
                .iter()
                .find(|model| model.id == chat_model_id)
                .is_some_and(|model| model.has_tag(axiom_inference::UNCENSORED_MODEL_TAG))
        };
        {
            let cache = self.catalog_cache.read().await;
            if let Some(cached) = cache.as_ref()
                && cached.fetched_at.elapsed() <= self.config.catalog_max_stale
            {
                return Ok(tagged(&cached.models));
            }
        }
        Ok(tagged(&self.models(cancellation.clone()).await?))
    }

    pub async fn generate_image(
        &self,
        chat_model_id: &str,
        prompt: &str,
        cancellation: CancellationToken,
    ) -> Result<crate::GeneratedImage> {
        if self.config.direct.is_some() {
            return self
                .image_request(chat_model_id, prompt, None, None, cancellation)
                .await;
        }
        let uncensored = self
            .uncensored_image_policy(chat_model_id, &cancellation)
            .await?;
        crate::providers::chutes_preview::generate_image(
            &self.config,
            Arc::clone(&self.credential),
            &self.image_sessions,
            chat_model_id,
            prompt,
            uncensored,
            cancellation,
        )
        .await
    }

    pub async fn image_request(
        &self,
        chat_model_id: &str,
        prompt: &str,
        source: Option<(&str, &axiom_inference::ImageContent)>,
        dimensions: Option<axiom_inference::ImageDimensions>,
        cancellation: CancellationToken,
    ) -> Result<crate::GeneratedImage> {
        if self.config.direct.is_some() {
            let references = source.into_iter().collect::<Vec<_>>();
            let (progress, receiver) = tokio::sync::mpsc::channel(1);
            // A temporary tuple can keep its unread receiver alive across await,
            // blocking the second update. This API has no progress consumer.
            drop(receiver);
            return self
                .image_request_with_progress(
                    axiom_inference::ImageGenerationRequest {
                        chat_model_id,
                        prompt,
                        references: &references,
                        dimensions,
                    },
                    progress,
                    cancellation,
                )
                .await;
        }
        let uncensored = self
            .uncensored_image_policy(chat_model_id, &cancellation)
            .await?;
        crate::providers::chutes_preview::image_operation(
            &self.config,
            Arc::clone(&self.credential),
            &self.image_sessions,
            chat_model_id,
            prompt,
            source,
            dimensions,
            uncensored,
            cancellation,
        )
        .await
    }

    pub async fn image_request_with_progress(
        &self,
        request: axiom_inference::ImageGenerationRequest<'_>,
        progress: tokio::sync::mpsc::Sender<axiom_inference::ImageGenerationProgress>,
        cancellation: CancellationToken,
    ) -> Result<crate::GeneratedImage> {
        let uncensored = self
            .uncensored_image_policy(request.chat_model_id, &cancellation)
            .await?;
        if self.config.direct.is_some() {
            let policy = self.trust_policy(cancellation.clone()).await?;
            return self
                .direct_image_provider()?
                .image_request(&policy, request, uncensored, Some(progress), cancellation)
                .await;
        }
        crate::providers::chutes_preview::image_operation_with_progress(
            &self.config,
            Arc::clone(&self.credential),
            &self.image_sessions,
            request,
            uncensored,
            Some(progress),
            cancellation,
        )
        .await
    }

    pub async fn edit_image(
        &self,
        chat_model_id: &str,
        image_id: &str,
        prompt: &str,
        source: &axiom_inference::ImageContent,
        cancellation: CancellationToken,
    ) -> Result<crate::GeneratedImage> {
        if self.config.direct.is_some() {
            return self
                .image_request(
                    chat_model_id,
                    prompt,
                    Some((image_id, source)),
                    None,
                    cancellation,
                )
                .await;
        }
        let uncensored = self
            .uncensored_image_policy(chat_model_id, &cancellation)
            .await?;
        crate::providers::chutes_preview::image_operation(
            &self.config,
            Arc::clone(&self.credential),
            &self.image_sessions,
            chat_model_id,
            prompt,
            Some((image_id, source)),
            None,
            uncensored,
            cancellation,
        )
        .await
    }

    pub async fn models(&self, cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        // Installed AstreaAI pins add an offering to the shared catalog; they
        // must not suppress discovery of the other registered providers.
        let result = catalog::fetch_models(
            &self.config,
            &self.credential,
            &self.providers,
            &cancellation,
        )
        .await;
        let mut configured_pool = crate::providers::pool::models(&self.config);
        configured_pool.extend(crate::providers::direct::models(&self.config));
        #[cfg(debug_assertions)]
        let configured_pool = {
            let preview = crate::providers::chutes_preview::models(&self.config);
            if preview.is_empty() {
                configured_pool
            } else {
                preview
            }
        };
        let result = match result {
            Ok(mut models) => {
                for model in configured_pool {
                    if !models.iter().any(|item| item.id == model.id) {
                        models.push(model);
                    }
                }
                Ok(models)
            }
            // Locally pinned discovery remains useful when no other provider
            // is configured. Inference still requires fresh worker admission.
            Err(error)
                if !configured_pool.is_empty()
                    && error.kind() != axiom_inference::ProviderFailureKind::Cancelled =>
            {
                Ok(configured_pool)
            }
            Err(error) => Err(error),
        };
        self.accept_catalog(result).await
    }

    async fn accept_catalog(&self, result: Result<Vec<ModelInfo>>) -> Result<Vec<ModelInfo>> {
        match result {
            Ok(models) => {
                *self.catalog_cache.write().await = Some(CachedCatalog {
                    fetched_at: Instant::now(),
                    models: models.clone(),
                });
                Ok(models)
            }
            Err(error) => {
                if error.kind() != axiom_inference::ProviderFailureKind::Transient {
                    // Do not resurrect withdrawn or rejected metadata on a later outage.
                    *self.catalog_cache.write().await = None;
                    return Err(error);
                }
                let cache = self.catalog_cache.read().await;
                if let Some(cached) = cache.as_ref()
                    && cached.fetched_at.elapsed() <= self.config.catalog_max_stale
                {
                    tracing::warn!(
                        detail = error.safe_detail(),
                        "using last-known-good model catalog after refresh failure"
                    );
                    return Ok(cached.models.clone());
                }
                Err(error)
            }
        }
    }

    pub async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.providers.establish(model, policy, cancellation).await
    }

    /// Resolve the newest valid signed trust policy without making policy
    /// availability a single point of failure. A valid local policy remains
    /// usable after a refresh failure. Invalid candidates cannot replace it,
    /// sequence upgrades must persist, and expired policies cannot be used.
    pub async fn trust_policy(&self, cancellation: CancellationToken) -> Result<TrustPolicy> {
        let now = now_unix();
        {
            let cached = self.trust_policy.read().await;
            if cached.policy.is_valid_at(now)
                && cached.refreshed_at.is_some_and(|refreshed| {
                    refreshed.elapsed() < self.config.trust_policy_refresh_interval
                })
            {
                return Ok(cached.policy.clone());
            }
        }

        let wire = crate::relay::client::RelayClient::new(&self.config, &self.credential)
            .signed_trust_policy(&cancellation)
            .await;
        let refresh = async {
            let bytes = wire?;
            let signed_policy = String::from_utf8(bytes).map_err(|_| {
                crate::SecureClientError::configuration("trust policy encoding is invalid")
            })?;
            let current = self.trust_policy.read().await.policy.clone();
            let candidate = TrustPolicy::from_signed_json(&signed_policy, current.sequence, now)?;
            if candidate.sequence == current.sequence && candidate != current {
                return Err(crate::SecureClientError::configuration(
                    "trust policy sequence equivocation was rejected",
                ));
            }
            if candidate.sequence > current.sequence
                && let Some(path) = &self.config.trust_policy_cache_path
            {
                persist_policy(
                    path,
                    &signed_policy,
                    &candidate,
                    self.config.limits.trust_policy_bytes,
                )?;
            }
            let mut cached = self.trust_policy.write().await;
            cached.policy = candidate.clone();
            cached.refreshed_at = Some(Instant::now());
            Ok(candidate)
        }
        .await;

        match refresh {
            Ok(policy) => Ok(policy),
            Err(error) if !cancellation.is_cancelled() => {
                let mut cached = self.trust_policy.write().await;
                if cached.policy.is_valid_at(now) {
                    tracing::warn!(
                        detail = error.safe_detail(),
                        sequence = cached.policy.sequence,
                        "using last-known-good signed trust policy after refresh failure"
                    );
                    cached.refreshed_at = Some(Instant::now());
                    return Ok(cached.policy.clone());
                }
                Err(error)
            }
            Err(_) => Err(crate::SecureClientError::cancelled()),
        }
    }

    pub async fn invalidate(&self, model: &ModelInfo) -> Result<()> {
        self.providers.invalidate(model).await
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn load_initial_policy(config: &SecureClientConfig) -> Result<TrustPolicy> {
    let bundled = TrustPolicy::production()?;
    let Some(path) = &config.trust_policy_cache_path else {
        return Ok(bundled);
    };
    let persisted = match read_persisted(path, config.limits.trust_policy_bytes) {
        Ok(Some(persisted)) => persisted,
        Ok(None) => return Ok(bundled),
        Err(error) => return Err(error),
    };
    if persisted.highest_sequence < bundled.sequence {
        return Ok(bundled);
    }
    let cached = TrustPolicy::from_signed_json_for_rollback(
        &persisted.signed_policy,
        persisted.highest_sequence.max(bundled.sequence),
        now_unix(),
    )?;
    if cached.sequence != persisted.highest_sequence {
        return Err(crate::SecureClientError::configuration(
            "trust policy rollback journal is inconsistent",
        ));
    }
    if cached.sequence == bundled.sequence && cached != bundled {
        return Err(crate::SecureClientError::configuration(
            "trust policy sequence equivocation was rejected",
        ));
    }
    Ok(cached)
}

fn read_persisted(path: &Path, limit: usize) -> Result<Option<PersistedTrustPolicy>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(crate::SecureClientError::configuration(
                "trust policy rollback journal is unavailable",
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(crate::SecureClientError::configuration(
            "trust policy rollback journal is invalid",
        ));
    }
    let bytes = fs::read(path).map_err(|_| {
        crate::SecureClientError::configuration("trust policy rollback journal is unavailable")
    })?;
    let persisted = serde_json::from_slice(&bytes).map_err(|_| {
        crate::SecureClientError::configuration("trust policy rollback journal is invalid")
    })?;
    Ok(Some(persisted))
}

fn persist_policy(
    path: &Path,
    signed_policy: &str,
    policy: &TrustPolicy,
    limit: usize,
) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        crate::SecureClientError::configuration("trust policy cache path is invalid")
    })?;
    fs::create_dir_all(parent).map_err(|_| {
        crate::SecureClientError::configuration("trust policy cache directory is unavailable")
    })?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| {
        crate::SecureClientError::configuration("trust policy cache directory is unavailable")
    })?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(crate::SecureClientError::configuration(
            "trust policy cache directory is invalid",
        ));
    }

    let lock_path = path.with_extension("lock");
    reject_symlink_or_nonfile(&lock_path)?;
    let mut lock_options = OpenOptions::new();
    lock_options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        lock_options.mode(0o600);
    }
    let lock = lock_options.open(&lock_path).map_err(|_| {
        crate::SecureClientError::configuration("trust policy rollback lock is unavailable")
    })?;
    lock.lock_exclusive().map_err(|_| {
        crate::SecureClientError::configuration("trust policy rollback lock is unavailable")
    })?;

    if let Some(existing) = read_persisted(path, limit)?
        && existing.highest_sequence > policy.sequence
    {
        return Err(crate::SecureClientError::configuration(
            "trust policy rollback was rejected",
        ));
    }
    let encoded = serde_json::to_vec(&PersistedTrustPolicy {
        highest_sequence: policy.sequence,
        signed_policy: signed_policy.to_owned(),
    })
    .map_err(|_| crate::SecureClientError::configuration("trust policy cache is invalid"))?;
    if encoded.len() > limit {
        return Err(crate::SecureClientError::configuration(
            "trust policy cache exceeds its configured bound",
        ));
    }
    reject_symlink_or_nonfile(path)?;
    let mut random = [0_u8; 8];
    OsRng.fill_bytes(&mut random);
    let temporary = path.with_extension(format!("tmp-{}", hex::encode(random)));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let write_result = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(crate::SecureClientError::configuration(
            "trust policy rollback journal could not be committed",
        ));
    }
    Ok(())
}

fn reject_symlink_or_nonfile(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            crate::SecureClientError::configuration("trust policy cache path is invalid"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(crate::SecureClientError::configuration(
            "trust policy cache path is unavailable",
        )),
    }
}

impl std::fmt::Debug for SecureClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecureClient")
            .field("relay_base_url", &self.config.relay_base_url)
            .field("credential", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed_direct_config(relay: &str) -> SecureClientConfig {
        let mut config = SecureClientConfig::new("https://relay.example").unwrap();
        config.relay_base_url = url::Url::parse(relay).unwrap();
        config.endpoint_policy.require_https = false;
        config.endpoint_policy.reject_local_addresses = false;
        config.request_timeout = std::time::Duration::from_secs(2);
        config.direct = Some(
            serde_json::from_value(serde_json::json!({
                "origin":"https://worker.example:8443", "connect_ip":null,
                "model_manifest":"aa".repeat(32), "issuer_key_file":"/private/operator.pem",
                "verifier":{"binary":"/installed/verifier", "binary_sha256":"bb".repeat(32),
                    "publisher_key":"cc".repeat(32), "minimum_policy_sequence":20}
            }))
            .unwrap(),
        );
        config
    }

    struct ImageProgressFixture;
    impl crate::provider::sealed::Provider for ImageProgressFixture {}
    #[async_trait::async_trait]
    impl crate::provider::SecureProvider for ImageProgressFixture {
        fn id(&self) -> &'static str {
            crate::providers::direct::ID
        }
        fn supports(&self, _: &ModelInfo) -> Result<()> {
            Ok(())
        }
        fn supports_contract(&self, _: &str, _: u16, _: &str) -> bool {
            true
        }
        fn populate_capabilities(&self, _: &mut ModelInfo) {}
        async fn establish(
            &self,
            _: &ModelInfo,
            _: &TrustPolicy,
            _: CancellationToken,
        ) -> Result<Box<dyn VerifiedSession>> {
            Err(crate::SecureClientError::capability("image fixture only"))
        }
        async fn image_request(
            &self,
            _: &TrustPolicy,
            _: axiom_inference::ImageGenerationRequest<'_>,
            _: bool,
            progress: Option<tokio::sync::mpsc::Sender<axiom_inference::ImageGenerationProgress>>,
            _: CancellationToken,
        ) -> Result<crate::GeneratedImage> {
            if let Some(progress) = progress {
                for percent in [0, 1, 99] {
                    let _ = progress
                        .send(axiom_inference::ImageGenerationProgress {
                            stage: match percent {
                                0 => axiom_inference::ImageGenerationStage::Queued,
                                99 => axiom_inference::ImageGenerationStage::Finishing,
                                _ => axiom_inference::ImageGenerationStage::Generating,
                            },
                            percent,
                        })
                        .await;
                }
            }
            Err(crate::SecureClientError::capability(
                "image fixture completed",
            ))
        }
    }

    #[tokio::test]
    async fn image_adapter_policy_follows_the_chat_models_uncensored_catalog_tag() {
        let config = installed_direct_config("https://relay.example");
        let client = SecureClient::new(config, ApiCredential::new("synthetic")).unwrap();
        let tag = |id: &str| axiom_inference::ModelTag {
            id: id.into(),
            label: id.into(),
            tone: axiom_inference::ModelTagTone::default(),
            position: 0,
        };
        *client.catalog_cache.write().await = Some(CachedCatalog {
            fetched_at: Instant::now(),
            models: vec![
                ModelInfo {
                    id: "deepseek-v4-1-flash-1017-abliterated".into(),
                    tags: vec![tag("cheap"), tag("uncensored")],
                    ..ModelInfo::default()
                },
                ModelInfo {
                    id: "tinfoil-deepseek-v4-1-flash".into(),
                    tags: vec![tag("cheap")],
                    ..ModelInfo::default()
                },
            ],
        });
        let cancel = CancellationToken::new();
        for (chat_model, expected) in [
            ("deepseek-v4-1-flash-1017-abliterated", true),
            ("tinfoil-deepseek-v4-1-flash", false),
            ("withdrawn-model", false),
        ] {
            assert_eq!(
                client
                    .uncensored_image_policy(chat_model, &cancel)
                    .await
                    .unwrap(),
                expected,
                "{chat_model}"
            );
        }
    }

    #[tokio::test]
    async fn image_request_without_progress_cannot_stall_on_unconsumed_updates() {
        let config = installed_direct_config("https://relay.example");
        let mut client = SecureClient::new(config, ApiCredential::new("synthetic")).unwrap();
        client.providers = ProviderRegistry::new([
            Arc::new(ImageProgressFixture) as Arc<dyn crate::provider::SecureProvider>
        ])
        .unwrap();
        client.trust_policy.write().await.refreshed_at = Some(Instant::now());
        *client.catalog_cache.write().await = Some(CachedCatalog {
            fetched_at: Instant::now(),
            models: vec![ModelInfo {
                id: "tinfoil-model".into(),
                ..ModelInfo::default()
            }],
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            client.image_request(
                "tinfoil-model",
                "A dog",
                None,
                None,
                CancellationToken::new(),
            ),
        )
        .await
        .expect("image API must complete without a progress consumer");
        assert_eq!(result.unwrap_err().safe_detail(), "image fixture completed");
    }

    async fn catalog_server(
        status: &str,
        body: serde_json::Value,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0 && request.len() + count <= 16 * 1024);
                request.extend_from_slice(&buffer[..count]);
            }
            assert!(request.starts_with(b"GET /api/v1/relay/models HTTP/1.1\r\n"));
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        (relay, server)
    }

    #[tokio::test]
    async fn installed_astreaai_keeps_near_and_tinfoil_in_the_catalog() {
        let mut near = serde_json::json!({
            "relay_contract_version":2, "id":"near-model", "label":"NEAR model",
            "short_label":"NEAR model", "model":"model/upstream",
            "base_url":"https://worker.near.ai/v1", "provider":"near",
            "provider_label":"NEAR AI", "e2ee_protocol":"near-v3",
            "e2ee_encryption_version":2, "attestation_protocol":"near-tdx-nvidia-v2",
            "context_window_tokens":32768, "max_output_tokens":4096,
            "input_price_microusd_per_million_tokens":250_000,
            "output_price_microusd_per_million_tokens":1_500_000
        });
        let mut tinfoil = near.clone();
        tinfoil["id"] = "tinfoil-model".into();
        tinfoil["label"] = "Tinfoil model".into();
        tinfoil["short_label"] = "Tinfoil model".into();
        tinfoil["provider"] = "tinfoil".into();
        tinfoil["provider_label"] = "Tinfoil".into();
        tinfoil["model"] = "gpt-oss-120b".into();
        tinfoil["base_url"] = "https://inference.tinfoil.sh/v1".into();
        tinfoil["e2ee_protocol"] = "tinfoil-ehbp-v1".into();
        tinfoil["e2ee_encryption_version"] = 1.into();
        tinfoil["attestation_protocol"] = "tinfoil-snp-sigstore-v1".into();
        near["supports_tools"] = true.into();
        let (relay, server) = catalog_server("200 OK", serde_json::json!([near, tinfoil])).await;
        let client =
            SecureClient::new(installed_direct_config(&relay), ApiCredential::new("test")).unwrap();
        let models = client.models(CancellationToken::new()).await.unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.provider_id.as_str())
                .collect::<Vec<_>>(),
            ["near", "tinfoil", "axiom-direct"]
        );
        for model in &models {
            client.providers.resolve(model).unwrap();
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn installed_astreaai_survives_catalog_outages_but_respects_cancellation() {
        let (relay, server) =
            catalog_server("503 Service Unavailable", serde_json::json!({})).await;
        let client =
            SecureClient::new(installed_direct_config(&relay), ApiCredential::new("test")).unwrap();
        let models = client.models(CancellationToken::new()).await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider_id, "axiom-direct");
        server.await.unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            client.models(cancelled).await.unwrap_err().kind(),
            axiom_inference::ProviderFailureKind::Cancelled
        );
    }

    #[tokio::test]
    async fn rejected_or_withdrawn_catalogs_cannot_return_after_a_later_outage() {
        let client = SecureClient::new(
            SecureClientConfig::new("https://relay.example").unwrap(),
            ApiCredential::new("test"),
        )
        .unwrap();
        let transient = || {
            crate::SecureClientError::new(axiom_inference::ProviderFailureKind::Transient, "outage")
        };
        assert!(client.accept_catalog(Err(transient())).await.is_err());
        let model = ModelInfo {
            id: "new-offering".into(),
            ..ModelInfo::default()
        };
        client
            .accept_catalog(Ok(vec![model.clone()]))
            .await
            .unwrap();
        assert_eq!(
            client.accept_catalog(Err(transient())).await.unwrap()[0].id,
            model.id
        );
        assert!(
            client
                .accept_catalog(Err(crate::SecureClientError::catalog("no usable models")))
                .await
                .is_err()
        );
        assert!(client.accept_catalog(Err(transient())).await.is_err());
        client.accept_catalog(Ok(vec![model])).await.unwrap();
        client
            .catalog_cache
            .write()
            .await
            .as_mut()
            .unwrap()
            .fetched_at = Instant::now()
            .checked_sub(std::time::Duration::from_secs(1801))
            .unwrap();
        assert!(client.accept_catalog(Err(transient())).await.is_err());
    }

    #[test]
    fn client_rejects_empty_credentials_and_redacts_nonempty_ones() {
        let config = SecureClientConfig::new("https://api.axiom.stream").unwrap();
        assert!(SecureClient::new(config.clone(), ApiCredential::new("")).is_err());
        let client = SecureClient::new(config, ApiCredential::new("axm_not_for_logs")).unwrap();
        assert!(!format!("{client:?}").contains("axm_not_for_logs"));
    }

    #[test]
    fn bundled_policy_replaces_an_older_journal_without_loading_old_rules() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trust-policy.json");
        let mut config = SecureClientConfig::new("https://api.axiom.stream").unwrap();
        config.trust_policy_cache_path = Some(path.clone());
        let policy = TrustPolicy::production().unwrap();
        let old = PersistedTrustPolicy {
            highest_sequence: policy.sequence - 1,
            signed_policy: "superseded policy format".into(),
        };
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(load_initial_policy(&config).unwrap(), policy);
        persist_policy(
            &path,
            crate::security::bundled_signed_policy(),
            &policy,
            config.limits.trust_policy_bytes,
        )
        .unwrap();
        assert_eq!(load_initial_policy(&config).unwrap(), policy);
        assert_eq!(
            read_persisted(&path, config.limits.trust_policy_bytes)
                .unwrap()
                .unwrap()
                .highest_sequence,
            policy.sequence
        );
    }

    #[test]
    fn trust_policy_journal_is_atomic_bounded_and_rollback_protected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trust-policy.json");
        let mut config = SecureClientConfig::new("https://api.axiom.stream").unwrap();
        config.trust_policy_cache_path = Some(path.clone());
        let policy = TrustPolicy::production().unwrap();
        persist_policy(
            &path,
            crate::security::bundled_signed_policy(),
            &policy,
            config.limits.trust_policy_bytes,
        )
        .unwrap();
        assert_eq!(load_initial_policy(&config).unwrap(), policy);

        let impossible_rollback = PersistedTrustPolicy {
            highest_sequence: policy.sequence + 1,
            signed_policy: crate::security::bundled_signed_policy().into(),
        };
        fs::write(&path, serde_json::to_vec(&impossible_rollback).unwrap()).unwrap();
        assert!(load_initial_policy(&config).is_err());
    }
}

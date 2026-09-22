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

    pub async fn models(&self, cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        let result = catalog::fetch_models(
            &self.config,
            &self.credential,
            &self.providers,
            &cancellation,
        )
        .await;
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

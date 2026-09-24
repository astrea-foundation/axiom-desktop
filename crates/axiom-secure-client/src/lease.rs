use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axiom_inference::ModelInfo;
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::{Result, SecureClientError, TrustPolicy, relay::dto::ProviderKeyLease};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttestationBinding {
    pub(crate) generation: u64,
    pub(crate) model_key_fingerprint: String,
    pub(crate) keyset_digest: Option<String>,
    pub(crate) hard_expires_at_unix_seconds: u64,
}

struct CachedMaterial<T> {
    model: ModelInfo,
    policy: TrustPolicy,
    expires_at_unix_seconds: u64,
    material: T,
}

pub(crate) struct VerifiedMaterialCache<T> {
    entries: RwLock<HashMap<String, CachedMaterial<T>>>,
    model_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl<T> Default for VerifiedMaterialCache<T> {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            model_locks: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Clone> VerifiedMaterialCache<T> {
    pub(crate) async fn get(&self, model: &ModelInfo, policy: &TrustPolicy) -> Option<T> {
        let now = now_unix();
        let entries = self.entries.read().await;
        entries.get(&model.id).and_then(|cached| {
            (cached.expires_at_unix_seconds > now
                && cached.model == *model
                && cached.policy == *policy)
                .then(|| cached.material.clone())
        })
    }

    pub(crate) async fn model_lock(&self, model_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.model_locks.lock().await;
            Arc::clone(
                locks
                    .entry(model_id.to_owned())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        lock.lock_owned().await
    }

    pub(crate) async fn insert(
        &self,
        model: ModelInfo,
        policy: TrustPolicy,
        material: T,
        provider_hard_expiry: u64,
        maximum_ttl: Duration,
    ) {
        let expires_at_unix_seconds =
            provider_hard_expiry.min(now_unix().saturating_add(maximum_ttl.as_secs()));
        self.entries.write().await.insert(
            model.id.clone(),
            CachedMaterial {
                model,
                policy,
                expires_at_unix_seconds,
                material,
            },
        );
    }

    pub(crate) async fn invalidate(&self, model_id: &str) {
        self.entries.write().await.remove(model_id);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

impl AttestationBinding {
    pub(crate) fn verify(
        wire: ProviderKeyLease,
        model: &ModelInfo,
        locally_verified_model_key: &str,
    ) -> Result<Self> {
        let local_key = normalize_key(locally_verified_model_key)?;
        let relay_key = normalize_key(&wire.model_public_key)?;
        let fingerprint = key_fingerprint(&local_key)?;
        if wire.model_id != model.id
            || wire.model != model.upstream_model
            || wire.provider != model.provider_id
            || wire.attestation_protocol != model.attestation_protocol
            || wire.e2ee_protocol != model.e2ee_protocol
            || wire.encryption_version != model.e2ee_encryption_version
            || wire.base_url.trim_end_matches('/') != model.provider_base_url.trim_end_matches('/')
            || !wire.verified
            || relay_key != local_key
            || wire.model_key_fingerprint != fingerprint
            || wire.attestation_generation == 0
        {
            return Err(SecureClientError::attestation(
                "backend attestation lease does not match local provider evidence",
            ));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        if wire.hard_expires_at_unix_seconds <= now {
            return Err(SecureClientError::with_code(
                axiom_inference::ProviderFailureKind::AttestationUnavailable,
                crate::SecureErrorCode::AttestationExpired,
                "backend attestation lease is expired",
                true,
            ));
        }
        Ok(Self {
            generation: wire.attestation_generation,
            model_key_fingerprint: fingerprint,
            keyset_digest: wire.keyset_digest,
            hard_expires_at_unix_seconds: wire.hard_expires_at_unix_seconds,
        })
    }
}

fn normalize_key(value: &str) -> Result<String> {
    let value = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SecureClientError::attestation(
            "attested model key is invalid",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

pub(crate) fn key_fingerprint(key: &str) -> Result<String> {
    let normalized_key = normalize_key(key)?;
    let bytes = hex::decode(normalized_key)
        .map_err(|_| SecureClientError::attestation("attested model key is invalid"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelInfo {
        ModelInfo {
            id: "model-id".into(),
            provider_id: "near".into(),
            upstream_model: "upstream/model".into(),
            provider_base_url: "https://model.example/v1".into(),
            e2ee_protocol: "near-v2".into(),
            e2ee_encryption_version: 2,
            attestation_protocol: "near-tdx-nvidia-v1".into(),
            ..ModelInfo::default()
        }
    }

    fn wire(key: &str) -> ProviderKeyLease {
        ProviderKeyLease {
            model_id: "model-id".into(),
            model: "upstream/model".into(),
            base_url: "https://model.example/v1".into(),
            provider: "near".into(),
            attestation_protocol: "near-tdx-nvidia-v1".into(),
            encryption_version: 2,
            e2ee_protocol: "near-v2".into(),
            model_public_key: key.into(),
            verified: true,
            attestation_generation: 4,
            model_key_fingerprint: key_fingerprint(key).unwrap(),
            keyset_digest: None,
            response_signing_address: None,
            response_signing_key_fingerprint: None,
            hard_expires_at_unix_seconds: u64::MAX,
        }
    }

    #[test]
    fn binding_requires_exact_local_key_generation_and_contract() {
        let key = "12".repeat(32);
        let binding = AttestationBinding::verify(wire(&key), &model(), &key).unwrap();
        assert_eq!(binding.generation, 4);

        let mut wrong = wire(&key);
        wrong.attestation_generation = 0;
        assert!(AttestationBinding::verify(wrong, &model(), &key).is_err());

        let other_key = "34".repeat(32);
        assert!(AttestationBinding::verify(wire(&key), &model(), &other_key).is_err());
    }

    #[tokio::test]
    async fn verified_material_cache_obeys_hard_expiry_policy_and_invalidation() {
        let cache = VerifiedMaterialCache::default();
        let model = model();
        let policy = TrustPolicy::default();

        cache
            .insert(
                model.clone(),
                policy.clone(),
                "expired",
                now_unix(),
                Duration::from_secs(60),
            )
            .await;
        assert_eq!(cache.get(&model, &policy).await, None);

        cache
            .insert(
                model.clone(),
                policy.clone(),
                "current",
                now_unix().saturating_add(60),
                Duration::from_secs(60),
            )
            .await;
        assert_eq!(cache.get(&model, &policy).await, Some("current"));

        let mut changed_policy = policy.clone();
        changed_policy.sequence += 1;
        assert_eq!(cache.get(&model, &changed_policy).await, None);

        cache.invalidate(&model.id).await;
        assert_eq!(cache.get(&model, &policy).await, None);
    }
}

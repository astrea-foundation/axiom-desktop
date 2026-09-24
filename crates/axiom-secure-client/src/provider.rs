use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use axiom_inference::{InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{ApiCredential, Result, SecureClientError, SecurityEvidence, TrustPolicy};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderContract {
    pub(crate) e2ee_protocol: String,
    pub(crate) e2ee_encryption_version: u16,
    pub(crate) attestation_protocol: String,
}

pub(crate) mod sealed {
    pub trait Provider {}
    pub trait Session {}
}

#[async_trait]
pub trait VerifiedSession: sealed::Session + Send {
    fn evidence(&self) -> &SecurityEvidence;

    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse>;

    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse>;
}

#[async_trait]
pub trait SecureProvider: sealed::Provider + Send + Sync {
    fn id(&self) -> &'static str;

    fn supports(&self, model: &ModelInfo) -> Result<()>;

    /// Whether this installed provider engine understands the complete
    /// cryptographic contract. Server advertisements can offer rollout
    /// choices, but cannot add cryptographic protocols the driver does not implement.
    fn supports_contract(
        &self,
        e2ee_protocol: &str,
        e2ee_encryption_version: u16,
        attestation_protocol: &str,
    ) -> bool;

    fn populate_capabilities(&self, model: &mut ModelInfo);

    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>>;

    /// Drop public verified material after a typed rotation/protocol failure.
    /// Request private keys and nonces are never cached by provider engines.
    async fn invalidate(&self, _model_id: &str) {}
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<&'static str, Arc<dyn SecureProvider>>,
}

impl ProviderRegistry {
    pub fn production(
        config: crate::SecureClientConfig,
        credential: Arc<ApiCredential>,
    ) -> Result<Self> {
        #[cfg(not(feature = "test-fixture"))]
        {
            let near = Arc::new(crate::providers::near::NearProvider::new(
                config.clone(),
                Arc::clone(&credential),
            )) as Arc<dyn SecureProvider>;
            let tinfoil = Arc::new(crate::providers::tinfoil::TinfoilProvider::new(
                config, credential,
            )) as Arc<dyn SecureProvider>;
            Self::new([near, tinfoil])
        }
        #[cfg(feature = "test-fixture")]
        {
            let near = Arc::new(crate::providers::near::NearProvider::new(
                config.clone(),
                Arc::clone(&credential),
            )) as Arc<dyn SecureProvider>;
            let tinfoil = Arc::new(crate::providers::tinfoil::TinfoilProvider::new(
                config.clone(),
                Arc::clone(&credential),
            )) as Arc<dyn SecureProvider>;
            let mut providers = vec![near, tinfoil];
            if config.uses_test_fixture() {
                providers.push(Arc::new(crate::providers::fixture::FixtureProvider::new(
                    config, credential,
                )) as Arc<dyn SecureProvider>);
            }
            Self::new(providers)
        }
    }

    pub fn new(providers: impl IntoIterator<Item = Arc<dyn SecureProvider>>) -> Result<Self> {
        let mut registry = Self::default();
        for provider in providers {
            let id = provider.id();
            if id.trim().is_empty() || registry.providers.insert(id, provider).is_some() {
                return Err(SecureClientError::configuration(
                    "provider IDs must be non-empty and unique",
                ));
            }
        }
        Ok(registry)
    }

    pub fn resolve(&self, model: &ModelInfo) -> Result<Arc<dyn SecureProvider>> {
        let provider = self
            .providers
            .get(model.provider_id.as_str())
            .cloned()
            .ok_or_else(|| SecureClientError::capability("model provider is not supported"))?;
        provider.supports(model)?;
        Ok(provider)
    }

    #[must_use]
    pub(crate) fn contains(&self, provider_id: &str) -> bool {
        self.providers.contains_key(provider_id)
    }

    pub(crate) fn validate_model(&self, model: &mut ModelInfo) -> Result<()> {
        let provider = self.resolve(model)?;
        provider.populate_capabilities(model);
        Ok(())
    }

    pub(crate) fn select_contract(
        &self,
        provider_id: &str,
        contracts: &[ProviderContract],
    ) -> Option<ProviderContract> {
        let provider = self.providers.get(provider_id)?;
        contracts
            .iter()
            .find(|contract| {
                provider.supports_contract(
                    &contract.e2ee_protocol,
                    contract.e2ee_encryption_version,
                    &contract.attestation_protocol,
                )
            })
            .cloned()
    }

    pub async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        if cancellation.is_cancelled() {
            return Err(SecureClientError::cancelled());
        }
        let provider = self.resolve(model)?;
        provider.establish(model, policy, cancellation).await
    }

    pub async fn invalidate(&self, model: &ModelInfo) -> Result<()> {
        let provider = self.resolve(model)?;
        provider.invalidate(&model.id).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderContract, ProviderRegistry, SecureProvider, VerifiedSession};
    use crate::{Result, SecureClientError, SecurityEvidence, TrustPolicy};
    use async_trait::async_trait;
    use axiom_inference::{InferenceRequest, InferenceResponse, ModelInfo, ProviderEvent};
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    struct MechanicalProvider;

    impl super::sealed::Provider for MechanicalProvider {}

    #[async_trait]
    impl SecureProvider for MechanicalProvider {
        fn id(&self) -> &'static str {
            "mechanical-test"
        }

        fn supports(&self, model: &ModelInfo) -> Result<()> {
            if model.e2ee_protocol == "test-v1" && model.e2ee_encryption_version == 7 {
                Ok(())
            } else {
                Err(SecureClientError::capability(
                    "provider contract does not match model",
                ))
            }
        }

        fn supports_contract(
            &self,
            e2ee_protocol: &str,
            e2ee_encryption_version: u16,
            attestation_protocol: &str,
        ) -> bool {
            e2ee_protocol == "test-v1"
                && e2ee_encryption_version == 7
                && attestation_protocol == "test-attestation-v1"
        }

        fn populate_capabilities(&self, model: &mut ModelInfo) {
            model.supports_streaming = true;
        }

        async fn establish(
            &self,
            _model: &ModelInfo,
            _policy: &TrustPolicy,
            _cancellation: CancellationToken,
        ) -> Result<Box<dyn VerifiedSession>> {
            Err(SecureClientError::new(
                axiom_inference::ProviderFailureKind::SessionEstablishment,
                "test provider has no production session",
            ))
        }
    }

    // The trait is intentionally exercised without a constructible production
    // session. Only provider modules can return their private session types.
    #[allow(dead_code)]
    struct NeverConstructedSession {
        evidence: SecurityEvidence,
    }

    impl super::sealed::Session for NeverConstructedSession {}

    #[async_trait]
    impl VerifiedSession for NeverConstructedSession {
        fn evidence(&self) -> &SecurityEvidence {
            &self.evidence
        }

        async fn complete(
            &mut self,
            _request: InferenceRequest,
            _cancellation: CancellationToken,
        ) -> Result<InferenceResponse> {
            unreachable!()
        }

        async fn stream(
            &mut self,
            _request: InferenceRequest,
            _events: mpsc::Sender<ProviderEvent>,
            _cancellation: CancellationToken,
        ) -> Result<InferenceResponse> {
            unreachable!()
        }
    }

    fn model(protocol: &str, version: u16) -> ModelInfo {
        ModelInfo {
            id: "future-model".into(),
            provider_id: "mechanical-test".into(),
            e2ee_protocol: protocol.into(),
            e2ee_encryption_version: version,
            attestation_protocol: "test-attestation-v1".into(),
            ..ModelInfo::default()
        }
    }

    #[tokio::test]
    async fn registry_rejects_duplicates_mismatches_and_precancelled_establishment() {
        assert!(
            ProviderRegistry::new([
                Arc::new(MechanicalProvider) as Arc<dyn SecureProvider>,
                Arc::new(MechanicalProvider) as Arc<dyn SecureProvider>,
            ])
            .is_err()
        );

        let registry =
            ProviderRegistry::new([Arc::new(MechanicalProvider) as Arc<dyn SecureProvider>])
                .unwrap();
        assert!(registry.resolve(&model("wrong", 7)).is_err());
        assert!(registry.resolve(&model("test-v1", 7)).is_ok());

        let selected = registry
            .select_contract(
                "mechanical-test",
                &[
                    ProviderContract {
                        e2ee_protocol: "test-v2".into(),
                        e2ee_encryption_version: 8,
                        attestation_protocol: "test-attestation-v2".into(),
                    },
                    ProviderContract {
                        e2ee_protocol: "test-v1".into(),
                        e2ee_encryption_version: 7,
                        attestation_protocol: "test-attestation-v1".into(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(selected.e2ee_protocol, "test-v1");

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = registry
            .establish(&model("test-v1", 7), &TrustPolicy::default(), cancellation)
            .await;
        let Err(error) = result else {
            panic!("pre-cancelled establishment unexpectedly returned a session");
        };
        assert_eq!(
            error.kind(),
            axiom_inference::ProviderFailureKind::Cancelled
        );
    }
}

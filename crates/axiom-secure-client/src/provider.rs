use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use axiom_inference::{AssistantTurn, InferenceRequest, ModelInfo, ProviderEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{Result, SecureClientError, SecurityEvidence, TrustPolicy};

mod sealed {
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
    ) -> Result<AssistantTurn>;

    async fn stream(
        &mut self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn>;
}

#[async_trait]
pub trait SecureProvider: sealed::Provider + Send + Sync {
    fn id(&self) -> &'static str;

    fn supports(&self, model: &ModelInfo) -> Result<()>;

    async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<&'static str, Arc<dyn SecureProvider>>,
}

impl ProviderRegistry {
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
}

#[cfg(test)]
mod tests {
    use super::{ProviderRegistry, SecureProvider, VerifiedSession};
    use crate::{Result, SecureClientError, SecurityEvidence, TrustPolicy};
    use async_trait::async_trait;
    use axiom_inference::{AssistantTurn, InferenceRequest, ModelInfo, ProviderEvent};
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
        ) -> Result<AssistantTurn> {
            unreachable!()
        }

        async fn stream(
            &mut self,
            _request: InferenceRequest,
            _events: mpsc::Sender<ProviderEvent>,
            _cancellation: CancellationToken,
        ) -> Result<AssistantTurn> {
            unreachable!()
        }
    }

    fn model(protocol: &str, version: u16) -> ModelInfo {
        ModelInfo {
            id: "future-model".into(),
            provider_id: "mechanical-test".into(),
            e2ee_protocol: protocol.into(),
            e2ee_encryption_version: version,
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

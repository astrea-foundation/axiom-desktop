use std::sync::Arc;

use axiom_inference::ModelInfo;
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, ProviderRegistry, Result, SecureClientConfig, TrustPolicy, VerifiedSession,
    catalog,
};

pub struct SecureClient {
    config: SecureClientConfig,
    credential: Arc<ApiCredential>,
    providers: ProviderRegistry,
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
        Ok(Self {
            config,
            credential,
            providers,
        })
    }

    pub async fn models(&self, cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        catalog::fetch_models(
            &self.config,
            &self.credential,
            &self.providers,
            &cancellation,
        )
        .await
    }

    pub async fn establish(
        &self,
        model: &ModelInfo,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>> {
        self.providers.establish(model, policy, cancellation).await
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

    #[test]
    fn client_rejects_empty_credentials_and_redacts_nonempty_ones() {
        let config = SecureClientConfig::new("https://api.axiom.stream").unwrap();
        assert!(SecureClient::new(config.clone(), ApiCredential::new("")).is_err());
        let client = SecureClient::new(config, ApiCredential::new("axm_not_for_logs")).unwrap();
        assert!(!format!("{client:?}").contains("axm_not_for_logs"));
    }
}

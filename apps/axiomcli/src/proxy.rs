//! Desktop's account-bound host for the existing secure loopback proxy.
use crate::{
    auth::{AuthManager, CredentialSource, ValidationStatus},
    paths::AxiomPaths,
    provider::SecureAxiomProvider,
};
use axiom_inference::ProviderFailureKind;
use axiom_proxy::{Arguments, ClientSource};
use axiom_secure_client::{SecureClient, SecureClientError};
use std::io::Read as _;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

struct NativeClient {
    auth: AuthManager,
    provider: SecureAxiomProvider,
    account_id: String,
    shutdown: CancellationToken,
}

impl ClientSource for NativeClient {
    fn client(
        &self,
        cancellation: CancellationToken,
    ) -> futures::future::BoxFuture<'_, axiom_secure_client::Result<Arc<SecureClient>>> {
        Box::pin(async move {
            // Managed authentication checks the shared keyring and refreshes
            // before inference. No retry is made after ciphertext is sent.
            let client = self.provider.client(&cancellation).await.map_err(|_| {
                self.shutdown.cancel();
                SecureClientError::new(
                    ProviderFailureKind::LocalAuthentication,
                    "Sign in and restart the proxy",
                )
            })?;
            if self.auth.credential_source() != CredentialSource::SystemKeyring
                || self.auth.active_account_id().as_deref() != Some(&self.account_id)
            {
                self.shutdown.cancel();
                return Err(SecureClientError::new(
                    ProviderFailureKind::LocalAuthentication,
                    "The proxy account changed",
                ));
            }
            Ok(client)
        })
    }
}

pub async fn run(arguments: Arguments, account_id: String) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var_os("AXIOM_API_KEY").is_none(),
        "Desktop proxy requires a native account session"
    );
    let paths = AxiomPaths::discover()?;
    paths.prepare()?;
    let timeout = Duration::from_secs(arguments.request_timeout_secs);
    let auth =
        AuthManager::new_with_paths_async(&arguments.axiom_base_url, timeout, &paths).await?;
    anyhow::ensure!(
        matches!(auth.validate().await, ValidationStatus::Valid(ref status) if status.account.id == account_id),
        "Sign in to the selected account before starting the proxy"
    );
    let shutdown = CancellationToken::new();
    let source = Arc::new(NativeClient {
        provider: SecureAxiomProvider::with_auth(&arguments.axiom_base_url, auth.clone(), timeout)?,
        auth,
        account_id,
        shutdown: shutdown.clone(),
    });
    source.client(shutdown.child_token()).await?;
    let token = std::env::var("AXIOM_PROXY_TOKEN")
        .map_err(|_| anyhow::anyhow!("A local proxy token is required"))?;
    // The supervisor owns stdin. EOF also stops the listener after an Electron
    // crash, on all operating systems, even without a delivered process signal.
    let parent_shutdown = shutdown.clone();
    std::thread::spawn(move || {
        let mut byte = [0_u8; 1];
        let _ = std::io::stdin().read(&mut byte);
        parent_shutdown.cancel();
    });
    let result = axiom_proxy::serve(arguments, &token, source, shutdown).await;
    result.map_err(|error| anyhow::anyhow!(error.to_string()))
}

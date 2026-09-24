use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
#[cfg(test)]
use futures::StreamExt as _;
use secrecy::{ExposeSecret as _, SecretString};
#[cfg(test)]
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use axiom_secure_client::{ApiCredential, SecureClient, SecureClientConfig, SecurityEvidence};

use crate::{AxiomError, Result, auth::AuthManager, paths::AxiomPaths};
pub use axiom_inference::{
    AssistantTurn, ChatMessage, ChatRole, FunctionCall, FunctionDefinition, InferenceRequest,
    ModelInfo, ProviderEvent, ProviderFailureKind, ProviderSecurityState, ToolCall, ToolDefinition,
};

/// Result of a provider security preflight. Evidence is present only for a
/// genuinely attested transport; development and future non-attested
/// providers must not manufacture it.
#[derive(Clone, Debug)]
pub struct ProviderSecurityVerification {
    pub state: ProviderSecurityState,
    pub evidence: Option<SecurityEvidence>,
}

#[async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn accept_outdated_tee(
        &self,
        _model: &str,
        _cancellation: CancellationToken,
    ) -> Result<ProviderSecurityVerification> {
        Err(AxiomError::Provider(
            "this provider does not support outdated-TEE consent".into(),
        ))
    }
    /// Establish and verify the selected model's security session without
    /// performing inference. This explicitly refreshes attestation.
    async fn verify_security(
        &self,
        _model: &str,
        _cancellation: CancellationToken,
    ) -> Result<ProviderSecurityVerification> {
        Err(AxiomError::Provider(
            "provider does not support security preflight".into(),
        ))
    }

    /// Reuse fresh verified material, or join/perform verification if needed.
    async fn prewarm_security(
        &self,
        model: &str,
        cancellation: CancellationToken,
    ) -> Result<ProviderSecurityVerification> {
        self.verify_security(model, cancellation).await
    }

    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn>;

    async fn request_accounting(
        &self,
        _ids: &[String],
        _cancellation: CancellationToken,
    ) -> Result<Vec<axiom_inference::RequestUsage>> {
        Ok(Vec::new())
    }

    async fn models(&self, _cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        Err(AxiomError::Provider(
            "provider does not support model discovery".into(),
        ))
    }
}

/// Production provider: model discovery, attestation, and E2EE all happen in
/// this process. The local tool runtime remains a separate policy boundary.
pub struct SecureAxiomProvider {
    config: SecureClientConfig,
    credential: ProviderCredential,
    client_cache: Mutex<Option<CachedSecureClient>>,
}

struct CachedSecureClient {
    credential_fingerprint: [u8; 32],
    client: Arc<SecureClient>,
}

enum ProviderCredential {
    Fixed(SecretString),
    Managed(AuthManager),
}

impl std::fmt::Debug for ProviderCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed(_) => formatter.write_str("Fixed([REDACTED])"),
            Self::Managed(manager) => formatter.debug_tuple("Managed").field(manager).finish(),
        }
    }
}

impl SecureAxiomProvider {
    pub fn new(relay_base_url: &str, api_key: String, timeout: Duration) -> Result<Self> {
        let mut config = SecureClientConfig::new(relay_base_url)
            .map_err(|error| AxiomError::Config(error.safe_detail().to_owned()))?;
        config.request_timeout = timeout;
        config.trust_policy_cache_path = Some(AxiomPaths::discover()?.trust_policy_cache_path());
        Ok(Self {
            config,
            credential: ProviderCredential::Fixed(SecretString::from(api_key)),
            client_cache: Mutex::new(None),
        })
    }

    pub fn with_auth(relay_base_url: &str, auth: AuthManager, timeout: Duration) -> Result<Self> {
        let mut config = SecureClientConfig::new(relay_base_url)
            .map_err(|error| AxiomError::Config(error.safe_detail().to_owned()))?;
        config.request_timeout = timeout;
        config.trust_policy_cache_path = Some(AxiomPaths::discover()?.trust_policy_cache_path());
        Ok(Self {
            config,
            credential: ProviderCredential::Managed(auth),
            client_cache: Mutex::new(None),
        })
    }

    pub fn from_environment(relay_base_url: &str, timeout: Duration) -> Result<Self> {
        let api_key = std::env::var("AXIOM_API_KEY").map_err(|_| {
            AxiomError::Config("AXIOM_API_KEY is required for secure inference".to_owned())
        })?;
        Self::new(relay_base_url, api_key, timeout)
    }

    pub(crate) async fn client(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Arc<SecureClient>> {
        let value = match &self.credential {
            ProviderCredential::Fixed(value) => value.expose_secret().to_owned(),
            ProviderCredential::Managed(manager) => manager
                .access_token_for_secure_operation(cancellation)
                .await?
                .expose_for_authorization()
                .to_owned(),
        };
        let fingerprint: [u8; 32] = Sha256::digest(value.as_bytes()).into();
        let mut cache = self
            .client_cache
            .lock()
            .map_err(|_| AxiomError::Provider("secure client cache is unavailable".into()))?;
        if let Some(cached) = cache.as_ref()
            && cached.credential_fingerprint == fingerprint
        {
            return Ok(Arc::clone(&cached.client));
        }
        let client = Arc::new(
            SecureClient::new(self.config.clone(), ApiCredential::new(value))
                .map_err(map_secure_error)?,
        );
        *cache = Some(CachedSecureClient {
            credential_fingerprint: fingerprint,
            client: Arc::clone(&client),
        });
        Ok(client)
    }

    async fn establish_verified(
        &self,
        model_id: &str,
        cancellation: CancellationToken,
        refresh_attestation: bool,
    ) -> Result<(
        Arc<SecureClient>,
        ModelInfo,
        Box<dyn axiom_secure_client::VerifiedSession>,
    )> {
        // Authentication is resolved before discovery or attestation begins.
        // No authorization retry exists after provider ciphertext might have
        // been dispatched, so a lost response can never replay an inference.
        let client = self.client(&cancellation).await?;
        let models = client
            .models(cancellation.clone())
            .await
            .map_err(map_secure_error)?;
        let model = models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| {
                AxiomError::Provider("selected secure model is unavailable".to_owned())
            })?;
        let trust_policy = client
            .trust_policy(cancellation.clone())
            .await
            .map_err(map_secure_error)?;
        if refresh_attestation {
            // An explicit verification must obtain a new local attestation,
            // not merely return the reusable lease used by ordinary inference.
            client.invalidate(model).await.map_err(map_secure_error)?;
        }
        let session = client
            .establish(model, &trust_policy, cancellation)
            .await
            .map_err(map_secure_error)?;
        Ok((client, model.clone(), session))
    }
}

impl std::fmt::Debug for SecureAxiomProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecureAxiomProvider")
            .field("config", &self.config)
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl InferenceProvider for SecureAxiomProvider {
    async fn accept_outdated_tee(
        &self,
        model: &str,
        cancellation: CancellationToken,
    ) -> Result<ProviderSecurityVerification> {
        let client = self.client(&cancellation).await?;
        let models = client
            .models(cancellation.clone())
            .await
            .map_err(map_secure_error)?;
        let model_info = models
            .iter()
            .find(|entry| entry.id == model)
            .ok_or_else(|| AxiomError::Provider("selected secure model is unavailable".into()))?;
        if cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        self.config
            .accept_outdated_tee_for_provider(&model_info.provider_id)
            .map_err(map_secure_error)?;
        self.verify_security(model, cancellation).await
    }

    async fn verify_security(
        &self,
        model: &str,
        cancellation: CancellationToken,
    ) -> Result<ProviderSecurityVerification> {
        let (_, _, session) = self.establish_verified(model, cancellation, true).await?;
        Ok(ProviderSecurityVerification {
            state: if session.evidence().state == axiom_secure_client::SecurityState::Degraded {
                ProviderSecurityState::Degraded
            } else {
                ProviderSecurityState::Verified
            },
            evidence: Some(session.evidence().clone()),
        })
    }

    async fn prewarm_security(
        &self,
        model: &str,
        cancellation: CancellationToken,
    ) -> Result<ProviderSecurityVerification> {
        let (_, _, session) = self.establish_verified(model, cancellation, false).await?;
        Ok(ProviderSecurityVerification {
            state: if session.evidence().state == axiom_secure_client::SecurityState::Degraded {
                ProviderSecurityState::Degraded
            } else {
                ProviderSecurityState::Verified
            },
            evidence: Some(session.evidence().clone()),
        })
    }

    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        use axiom_inference::{InvocationState, RequestUsage, UsageCompleteness};
        let mut request = request;
        let request_id = request
            .request_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().simple().to_string())
            .clone();
        let mut accounting = RequestUsage {
            request_id,
            model_id: request.model.clone(),
            started_at_ms: chrono::Utc::now().timestamp_millis().to_string(),
            ..RequestUsage::default()
        };
        let output = events;
        output
            .send(ProviderEvent::Accounting(Box::new(accounting.clone())))
            .await
            .map_err(|_| AxiomError::Cancelled)?;
        let (events, mut incoming) = mpsc::channel(64);
        let initial_accounting = accounting.clone();
        let operation = async {
            send_provider_event(
                &events,
                ProviderEvent::SecurityState(ProviderSecurityState::Verifying),
                &cancellation,
            )
            .await?;
            send_provider_event(
                &events,
                ProviderEvent::Status {
                    connected: true,
                    detail: "Verifying model attestation and secure endpoint binding".to_owned(),
                },
                &cancellation,
            )
            .await?;
            let (client, model, mut session) = match self
                .establish_verified(&request.model, cancellation.clone(), false)
                .await
            {
                Ok(session) => session,
                Err(error) => return secure_stream_failure(&events, &cancellation, error).await,
            };
            send_provider_event(
                &events,
                ProviderEvent::SecurityState(
                    if session.evidence().state == axiom_secure_client::SecurityState::Degraded {
                        ProviderSecurityState::Degraded
                    } else {
                        ProviderSecurityState::Verified
                    },
                ),
                &cancellation,
            )
            .await?;
            let mut established_accounting = initial_accounting.clone();
            established_accounting.provider_id = model.provider_id.clone();
            established_accounting.context_window_tokens = Some(model.context_window_tokens);
            established_accounting.auto_compact_threshold_tokens =
                Some(model.auto_compact_threshold_tokens());
            send_provider_event(
                &events,
                ProviderEvent::Accounting(Box::new(established_accounting)),
                &cancellation,
            )
            .await?;
            let retry_request = request.clone();
            let retry_events = events.clone();
            let response_result = async {
                match session.stream(request, events, cancellation.clone()).await {
                    Ok(response) => Ok(response),
                    Err(error) if error.requires_reattest_and_reencrypt() => {
                        client.invalidate(&model).await.map_err(map_secure_error)?;
                        send_provider_event(
                            &retry_events,
                            ProviderEvent::SecurityState(ProviderSecurityState::Verifying),
                            &cancellation,
                        )
                        .await?;
                        send_provider_event(
                            &retry_events,
                            ProviderEvent::Status {
                                connected: true,
                                detail: "Provider key rotated; re-verifying and re-encrypting once"
                                    .to_owned(),
                            },
                            &cancellation,
                        )
                        .await?;
                        let (_, _, mut refreshed) = self
                            .establish_verified(&retry_request.model, cancellation.clone(), false)
                            .await?;
                        send_provider_event(
                            &retry_events,
                            ProviderEvent::SecurityState(
                                if session.evidence().state
                                    == axiom_secure_client::SecurityState::Degraded
                                {
                                    ProviderSecurityState::Degraded
                                } else {
                                    ProviderSecurityState::Verified
                                },
                            ),
                            &cancellation,
                        )
                        .await?;
                        refreshed
                            .stream(retry_request, retry_events.clone(), cancellation.clone())
                            .await
                            .map_err(map_secure_error)
                    }
                    Err(error) => Err(map_secure_error(error)),
                }
            }
            .await;
            let response = match response_result {
                Ok(response) => response,
                Err(error) => {
                    return secure_stream_failure(&retry_events, &cancellation, error).await;
                }
            };
            send_provider_event(
                &retry_events,
                ProviderEvent::ResponseVerified,
                &cancellation,
            )
            .await?;
            let mut assistant = response.assistant;
            assistant.reasoning = response.reasoning;
            Ok(assistant)
        };
        tokio::pin!(operation);
        let mut result = loop {
            tokio::select! {
                result = &mut operation => break result,
                event = incoming.recv() => if let Some(event) = event {
                    update_request_accounting(&mut accounting, &event)?;
                    output.send(event).await.map_err(|_| AxiomError::Cancelled)?;
                }
            }
        };
        while let Ok(event) = incoming.try_recv() {
            update_request_accounting(&mut accounting, &event)?;
            output
                .send(event)
                .await
                .map_err(|_| AxiomError::Cancelled)?;
        }
        accounting.finished_at_ms = Some(chrono::Utc::now().timestamp_millis().to_string());
        accounting.state = match &result {
            Ok(_) => InvocationState::Completed,
            Err(AxiomError::Cancelled) => InvocationState::Cancelled,
            Err(_) => InvocationState::Failed,
        };
        if result.is_err() {
            accounting.response_verified = false;
            if accounting.completeness == UsageCompleteness::Live {
                accounting.completeness = UsageCompleteness::Partial;
            }
            accounting.error_code = Some(match &result {
                Err(AxiomError::SecureProvider { code, .. }) => (*code).to_owned(),
                Err(AxiomError::Cancelled) => "CANCELLED".to_owned(),
                _ => "PROVIDER_REQUEST_FAILED".to_owned(),
            });
        } else if !accounting.response_verified {
            result = Err(AxiomError::Provider(
                "Response authentication was not verified".into(),
            ));
            accounting.state = InvocationState::Failed;
        }
        output
            .send(ProviderEvent::Accounting(Box::new(accounting)))
            .await
            .map_err(|_| AxiomError::Cancelled)?;
        result
    }

    async fn request_accounting(
        &self,
        ids: &[String],
        cancellation: CancellationToken,
    ) -> Result<Vec<axiom_inference::RequestUsage>> {
        self.client(&cancellation)
            .await?
            .request_accounting(ids, cancellation)
            .await
            .map_err(map_secure_error)
    }

    async fn models(&self, cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        self.client(&cancellation)
            .await?
            .models(cancellation)
            .await
            .map_err(map_secure_error)
    }
}

fn update_request_accounting(
    record: &mut axiom_inference::RequestUsage,
    event: &ProviderEvent,
) -> Result<()> {
    match event {
        ProviderEvent::Accounting(snapshot) => {
            if snapshot.request_id != record.request_id
                || snapshot.model_id != record.model_id
                || !snapshot.validate_counters()
            {
                return Err(AxiomError::Provider(
                    "Request accounting identity mismatch".into(),
                ));
            }
            let verified = record.response_verified;
            let context_window = record.context_window_tokens;
            let compact_at = record.auto_compact_threshold_tokens;
            *record = *snapshot.clone();
            record.response_verified = verified;
            record.context_window_tokens = record.context_window_tokens.or(context_window);
            record.auto_compact_threshold_tokens =
                record.auto_compact_threshold_tokens.or(compact_at);
        }
        ProviderEvent::ResponseVerified => record.response_verified = true,
        ProviderEvent::Finished(reason) => {
            record.finish_reason = Some(format!("{reason:?}").to_lowercase());
        }
        _ => {}
    }
    Ok(())
}

async fn send_provider_event(
    events: &mpsc::Sender<ProviderEvent>,
    event: ProviderEvent,
    cancellation: &CancellationToken,
) -> Result<()> {
    tokio::select! {
        () = cancellation.cancelled() => Err(AxiomError::Cancelled),
        result = events.send(event) => result.map_err(|_| AxiomError::Cancelled),
    }
}

async fn secure_stream_failure<T>(
    events: &mpsc::Sender<ProviderEvent>,
    cancellation: &CancellationToken,
    error: AxiomError,
) -> Result<T> {
    if matches!(error, AxiomError::Cancelled) || cancellation.is_cancelled() {
        return Err(AxiomError::Cancelled);
    }
    // Attestation and completion integrity are separate. An execution failure
    // does not retroactively reject previously verified TEE evidence.
    if matches!(
        &error,
        AxiomError::SecureProvider {
            kind: ProviderFailureKind::AttestationRejected
                | ProviderFailureKind::AttestationUnavailable
                | ProviderFailureKind::SessionEstablishment,
            ..
        }
    ) {
        let _ = send_provider_event(
            events,
            ProviderEvent::SecurityState(
                if matches!(
                    &error,
                    AxiomError::SecureProvider {
                        code: "PROVIDER_TDX_OUT_OF_DATE",
                        ..
                    }
                ) {
                    ProviderSecurityState::Outdated
                } else {
                    ProviderSecurityState::Failed
                },
            ),
            &CancellationToken::new(),
        )
        .await;
    }
    Err(error)
}

#[allow(clippy::needless_pass_by_value)]
fn map_secure_error(error: axiom_secure_client::SecureClientError) -> AxiomError {
    match error.kind() {
        ProviderFailureKind::Cancelled => AxiomError::Cancelled,
        ProviderFailureKind::InsufficientCredit => AxiomError::Provider(
            "HTTP 402 Payment Required: insufficient credit; visit axiom.stream to top up"
                .to_owned(),
        ),
        kind => AxiomError::SecureProvider {
            kind,
            code: error.code().as_str(),
            message: error.safe_detail(),
        },
    }
}

#[cfg(test)]
// This fake HTTP transport exists only to exercise generic provider failure,
// cancellation, and parser behavior. It is not compiled into AxiomCLI binaries.
mod test_http_provider {
    use std::collections::BTreeMap;

    use crate::audit::redact_text;

    use super::*;

    const MAX_PROVIDER_ERROR_BYTES: usize = 4 * 1024;
    pub(super) const MAX_MODEL_CATALOG_BYTES: usize = 4 * 1024 * 1024;
    pub(super) const MAX_PROVIDER_SSE_EVENT_BYTES: usize = 1024 * 1024;
    const MAX_PROVIDER_STREAM_BYTES: usize = 16 * 1024 * 1024;

    pub struct TestHttpProvider {
        client: reqwest::Client,
        base_url: String,
        api_key: Option<SecretString>,
    }

    impl std::fmt::Debug for TestHttpProvider {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("TestHttpProvider")
                .field("base_url", &self.base_url)
                .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
                .finish_non_exhaustive()
        }
    }

    impl TestHttpProvider {
        pub fn new(
            base_url: impl Into<String>,
            api_key: Option<String>,
            timeout: Duration,
        ) -> Result<Self> {
            let base_url = base_url.into().trim_end_matches('/').to_owned();
            reqwest::Url::parse(&base_url)
                .map_err(|error| AxiomError::Config(format!("invalid provider URL: {error}")))?;
            let client = reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()
                .map_err(|error| AxiomError::Provider(error.to_string()))?;
            Ok(Self {
                client,
                base_url,
                api_key: api_key.map(|value| SecretString::new(value.into_boxed_str())),
            })
        }

        fn endpoint(&self) -> String {
            format!("{}/chat/completions", self.base_url)
        }

        fn models_endpoint(&self) -> String {
            format!("{}/models", self.base_url)
        }

        fn authenticated(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
            if let Some(api_key) = &self.api_key {
                builder.bearer_auth(api_key.expose_secret())
            } else {
                builder
            }
        }

        async fn send_with_retry(
            &self,
            body: &WireRequest<'_>,
            request_id: &str,
            cancellation: &CancellationToken,
        ) -> Result<reqwest::Response> {
            const ATTEMPTS: usize = 3;
            for attempt in 0..ATTEMPTS {
                let builder = self
                    .client
                    .post(self.endpoint())
                    .header("Idempotency-Key", request_id)
                    .header("X-AxiomCLI-Request-ID", request_id)
                    .json(body);
                let response = tokio::select! {
                    () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                    response = self.authenticated(builder).send() => response,
                };
                match response {
                    Ok(response) if response.status().is_success() => return Ok(response),
                    Ok(response) => {
                        let status = response.status();
                        let retry_after = parse_retry_after(response.headers());
                        if is_retryable_status(status) && attempt + 1 < ATTEMPTS {
                            retry_delay(attempt, retry_after, request_id, cancellation).await?;
                            continue;
                        }
                        let detail =
                            read_response_prefix(response, MAX_PROVIDER_ERROR_BYTES, cancellation)
                                .await
                                .map_or_else(
                                    |_| "unreadable response".into(),
                                    |(bytes, truncated)| {
                                        format!(
                                            "{}{}",
                                            String::from_utf8_lossy(&bytes),
                                            if truncated { "…" } else { "" }
                                        )
                                    },
                                );
                        return Err(provider_http_error(status, &detail));
                    }
                    Err(error)
                        if (error.is_connect() || error.is_timeout()) && attempt + 1 < ATTEMPTS =>
                    {
                        retry_delay(attempt, None, request_id, cancellation).await?;
                    }
                    Err(error) => {
                        return Err(AxiomError::Provider(format!(
                            "{}: {}",
                            if error.is_timeout() {
                                "transient timeout"
                            } else if error.is_connect() {
                                "transient connection failure"
                            } else {
                                "request failure"
                            },
                            error
                        )));
                    }
                }
            }
            Err(AxiomError::Provider("retry budget exhausted".into()))
        }
    }

    #[derive(Serialize)]
    struct WireRequest<'a> {
        model: &'a str,
        messages: &'a [ChatMessage],
        stream: bool,
        stream_options: StreamOptions,
        #[serde(skip_serializing_if = "tools_are_empty")]
        tools: &'a [ToolDefinition],
    }

    fn tools_are_empty(tools: &&[ToolDefinition]) -> bool {
        tools.is_empty()
    }

    #[derive(Serialize)]
    struct StreamOptions {
        include_usage: bool,
    }

    #[derive(Debug, Deserialize)]
    struct ModelsResponse {
        #[serde(default)]
        data: Vec<ModelInfo>,
    }

    #[derive(Debug, Deserialize)]
    struct StreamChunk {
        #[serde(default)]
        choices: Vec<Choice>,
        usage: Option<Usage>,
    }

    #[derive(Debug, Deserialize)]
    struct Choice {
        delta: Delta,
    }

    #[derive(Debug, Default, Deserialize)]
    struct Delta {
        content: Option<String>,
        reasoning_content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCallDelta>,
    }

    #[derive(Debug, Deserialize)]
    struct ToolCallDelta {
        index: usize,
        id: Option<String>,
        function: Option<FunctionDelta>,
    }

    #[derive(Debug, Deserialize)]
    struct FunctionDelta {
        name: Option<String>,
        arguments: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    }

    #[derive(Default)]
    struct PartialToolCall {
        id: String,
        name: String,
        arguments: String,
    }

    #[async_trait]
    impl InferenceProvider for TestHttpProvider {
        async fn verify_security(
            &self,
            _model: &str,
            cancellation: CancellationToken,
        ) -> Result<ProviderSecurityVerification> {
            if cancellation.is_cancelled() {
                return Err(AxiomError::Cancelled);
            }
            Ok(ProviderSecurityVerification {
                state: ProviderSecurityState::UnattestedDevelopment,
                evidence: None,
            })
        }

        async fn stream(
            &self,
            request: InferenceRequest,
            events: mpsc::Sender<ProviderEvent>,
            cancellation: CancellationToken,
        ) -> Result<AssistantTurn> {
            let _ = events
                .send(ProviderEvent::SecurityState(
                    ProviderSecurityState::UnattestedDevelopment,
                ))
                .await;
            let body = WireRequest {
                model: &request.model,
                messages: &request.messages,
                stream: true,
                stream_options: StreamOptions {
                    include_usage: true,
                },
                tools: &request.tools,
            };
            let request_id = uuid::Uuid::new_v4().to_string();
            let response = self
                .send_with_retry(&body, &request_id, &cancellation)
                .await?;
            events
                .send(ProviderEvent::Status {
                    connected: true,
                    detail: "Axiom development inference connected · UNATTESTED".into(),
                })
                .await
                .map_err(|_| AxiomError::Cancelled)?;

            let mut stream = response.bytes_stream();
            let mut text = String::new();
            let mut partial_calls: BTreeMap<usize, PartialToolCall> = BTreeMap::new();
            let mut saw_done = false;
            let mut stream_bytes = 0_usize;
            let mut event_buffer = Vec::new();
            loop {
                let next = tokio::select! {
                    () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                    next = stream.next() => next,
                };
                let Some(event) = next else { break };
                let bytes = event.map_err(|error| AxiomError::Provider(error.to_string()))?;
                stream_bytes = stream_bytes.saturating_add(bytes.len());
                if stream_bytes > MAX_PROVIDER_STREAM_BYTES {
                    return Err(AxiomError::Provider(
                        "provider stream exceeded the 16 MiB turn limit".into(),
                    ));
                }
                event_buffer.extend_from_slice(&bytes);
                while let Some((boundary, delimiter_bytes)) = sse_boundary(&event_buffer) {
                    if boundary > MAX_PROVIDER_SSE_EVENT_BYTES {
                        return Err(AxiomError::Provider(
                            "provider SSE event exceeded 1 MiB".into(),
                        ));
                    }
                    let remainder =
                        event_buffer.split_off(boundary.saturating_add(delimiter_bytes));
                    let raw_event = std::mem::replace(&mut event_buffer, remainder);
                    let raw_event = &raw_event[..boundary];
                    if let Some(data) = sse_data(raw_event)?
                        && process_stream_data(&data, &mut text, &mut partial_calls, &events)
                            .await?
                    {
                        saw_done = true;
                        break;
                    }
                }
                if saw_done {
                    break;
                }
                if event_buffer.len() > MAX_PROVIDER_SSE_EVENT_BYTES {
                    return Err(AxiomError::Provider(
                        "provider SSE event exceeded 1 MiB".into(),
                    ));
                }
            }

            if !saw_done {
                return Err(AxiomError::Provider(
                    "provider stream ended before [DONE]".into(),
                ));
            }

            let tool_calls = partial_calls
                .into_values()
                .map(|call| {
                    if call.id.is_empty() || call.name.is_empty() {
                        return Err(AxiomError::Provider(
                            "provider returned an incomplete tool call".into(),
                        ));
                    }
                    Ok(ToolCall {
                        id: call.id,
                        kind: "function".into(),
                        function: FunctionCall {
                            name: call.name,
                            arguments: call.arguments,
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(AssistantTurn {
                reasoning: None,
                text,
                tool_calls,
            })
        }

        async fn models(&self, cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
            let request = self.authenticated(self.client.get(self.models_endpoint()));
            let response = tokio::select! {
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                response = request.send() => response.map_err(|error| AxiomError::Provider(error.to_string()))?,
            };
            if !response.status().is_success() {
                let status = response.status();
                let detail =
                    read_response_prefix(response, MAX_PROVIDER_ERROR_BYTES, &cancellation)
                        .await
                        .map_or_else(
                            |_| "unreadable response".into(),
                            |(bytes, truncated)| {
                                format!(
                                    "{}{}",
                                    String::from_utf8_lossy(&bytes),
                                    if truncated { "…" } else { "" }
                                )
                            },
                        );
                return Err(provider_http_error(status, &detail));
            }
            let (payload, truncated) =
                read_response_prefix(response, MAX_MODEL_CATALOG_BYTES, &cancellation).await?;
            if truncated {
                return Err(AxiomError::Provider(format!(
                    "provider model catalog exceeded {MAX_MODEL_CATALOG_BYTES} bytes"
                )));
            }
            let models = serde_json::from_slice::<ModelsResponse>(&payload)
                .map_err(|error| AxiomError::Provider(format!("invalid model catalog: {error}")))?
                .data;
            if models.len() > 10_000 {
                return Err(AxiomError::Provider(
                    "provider model catalog exceeded 10,000 entries".into(),
                ));
            }
            Ok(models)
        }
    }

    async fn read_response_prefix(
        response: reqwest::Response,
        maximum: usize,
        cancellation: &CancellationToken,
    ) -> Result<(Vec<u8>, bool)> {
        if response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
        {
            return Ok((Vec::new(), true));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = tokio::select! {
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            chunk = stream.next() => chunk,
        } {
            let chunk = chunk.map_err(|error| AxiomError::Provider(error.to_string()))?;
            let remaining = maximum.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if chunk.len() > remaining {
                return Ok((bytes, true));
            }
        }
        Ok((bytes, false))
    }

    fn sse_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
        let lf = bytes.windows(2).position(|window| window == b"\n\n");
        let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
        match (lf, crlf) {
            (Some(left), Some(right)) if left <= right => Some((left, 2)),
            (Some(_) | None, Some(right)) => Some((right, 4)),
            (Some(left), None) => Some((left, 2)),
            (None, None) => None,
        }
    }

    fn sse_data(raw: &[u8]) -> Result<Option<String>> {
        let raw = std::str::from_utf8(raw)
            .map_err(|_| AxiomError::Provider("provider SSE event was not valid UTF-8".into()))?;
        let mut data = Vec::new();
        for line in raw.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("data:") {
                data.push(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        Ok((!data.is_empty()).then(|| data.join("\n")))
    }

    async fn process_stream_data(
        data: &str,
        text: &mut String,
        partial_calls: &mut BTreeMap<usize, PartialToolCall>,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<bool> {
        if data.trim() == "[DONE]" {
            return Ok(true);
        }
        let chunk: StreamChunk = serde_json::from_str(data)
            .map_err(|error| AxiomError::Provider(format!("invalid SSE chunk: {error}")))?;
        for choice in chunk.choices {
            if let Some(delta) = choice.delta.content {
                text.push_str(&delta);
                events
                    .send(ProviderEvent::TextDelta(delta))
                    .await
                    .map_err(|_| AxiomError::Cancelled)?;
            }
            if let Some(delta) = choice.delta.reasoning_content {
                events
                    .send(ProviderEvent::ReasoningDelta(delta))
                    .await
                    .map_err(|_| AxiomError::Cancelled)?;
            }
            for delta in choice.delta.tool_calls {
                if delta.index >= 1024 {
                    return Err(AxiomError::Provider(
                        "provider returned more than 1,024 indexed tool calls".into(),
                    ));
                }
                let call = partial_calls.entry(delta.index).or_default();
                if let Some(id) = delta.id {
                    call.id = id;
                }
                if let Some(function) = delta.function {
                    if let Some(name) = function.name {
                        call.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments {
                        call.arguments.push_str(&arguments);
                    }
                }
            }
        }
        if let Some(usage) = chunk.usage {
            events
                .send(ProviderEvent::Usage {
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                })
                .await
                .map_err(|_| AxiomError::Cancelled)?;
        }
        Ok(false)
    }

    fn is_retryable_status(status: reqwest::StatusCode) -> bool {
        matches!(status.as_u16(), 429 | 502 | 503 | 504)
    }

    #[must_use]
    pub fn classify_provider_status(status: reqwest::StatusCode) -> ProviderFailureKind {
        match status.as_u16() {
            401 | 403 => ProviderFailureKind::Authentication,
            429 => ProviderFailureKind::RateLimited,
            502..=504 => ProviderFailureKind::Transient,
            400..=499 => ProviderFailureKind::InvalidRequest,
            _ => ProviderFailureKind::InvalidResponse,
        }
    }

    fn provider_http_error(status: reqwest::StatusCode, detail: &str) -> AxiomError {
        AxiomError::Provider(format!(
            "{:?} HTTP {status}: {}",
            classify_provider_status(status),
            truncate(&redact_text(detail), 4096)
        ))
    }

    fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
        headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|seconds| Duration::from_secs(seconds.min(30)))
    }

    async fn retry_delay(
        attempt: usize,
        retry_after: Option<Duration>,
        request_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let jitter = request_id
            .bytes()
            .fold(0_u64, |total, byte| total.wrapping_add(u64::from(byte)))
            % 83;
        let exponential = 100_u64.saturating_mul(1_u64 << attempt.min(8));
        let delay = retry_after.unwrap_or_else(|| Duration::from_millis(exponential + jitter));
        tokio::select! {
            () = cancellation.cancelled() => Err(AxiomError::Cancelled),
            () = tokio::time::sleep(delay) => Ok(()),
        }
    }

    pub(super) fn truncate(input: &str, max_bytes: usize) -> String {
        if input.len() <= max_bytes {
            return input.into();
        }
        let mut end = max_bytes;
        while !input.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &input[..end])
    }
}

#[cfg(test)]
use test_http_provider::*;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, StatusCode, header},
        response::{IntoResponse as _, Response},
        routing::{get, post},
    };
    use serde_json::json;

    fn request(model: &str) -> InferenceRequest {
        InferenceRequest::streaming(
            model,
            vec![ChatMessage::text(ChatRole::User, "hello")],
            Vec::new(),
        )
    }

    #[test]
    fn provider_debug_redacts_key() {
        let provider = TestHttpProvider::new(
            "http://127.0.0.1:1/v1",
            Some("axm_secret".into()),
            Duration::from_secs(1),
        )
        .expect("provider");
        let output = format!("{provider:?}");
        assert!(!output.contains("axm_secret"));
        assert!(output.contains("REDACTED"));
    }

    #[test]
    fn truncation_preserves_utf8() {
        assert_eq!(truncate("hello", 5), "hello");
        assert!(truncate("ab🌸cd", 4).ends_with('…'));
    }

    #[tokio::test]
    async fn test_adapter_security_preflight_is_explicitly_unattested() {
        let provider = TestHttpProvider::new(
            "http://127.0.0.1:1/v1",
            Some("axm_secret".into()),
            Duration::from_secs(1),
        )
        .expect("provider");
        assert_eq!(
            provider
                .verify_security("test-model", CancellationToken::new())
                .await
                .expect("preflight")
                .state,
            ProviderSecurityState::UnattestedDevelopment
        );
    }

    #[tokio::test]
    async fn streams_from_a_real_local_http_service() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    concat!(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}],",
                        "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n",
                        "data: [DONE]\n\n"
                    ),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("test service");
        });

        let provider =
            TestHttpProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        let (tx, mut rx) = mpsc::channel(64);
        let turn = provider
            .stream(request("test"), tx, CancellationToken::new())
            .await
            .expect("stream");
        assert_eq!(turn.text, "hello world");
        assert!(matches!(
            rx.recv().await,
            Some(ProviderEvent::SecurityState(
                ProviderSecurityState::UnattestedDevelopment
            ))
        ));
        assert!(matches!(
            rx.recv().await,
            Some(ProviderEvent::Status {
                connected: true,
                ..
            })
        ));
        assert_eq!(
            rx.recv().await,
            Some(ProviderEvent::TextDelta("hello ".into()))
        );
        assert_eq!(
            rx.recv().await,
            Some(ProviderEvent::TextDelta("world".into()))
        );
        assert_eq!(
            rx.recv().await,
            Some(ProviderEvent::Usage {
                input_tokens: 3,
                output_tokens: 2
            })
        );
        server.abort();
    }

    #[tokio::test]
    async fn real_http_stream_that_closes_early_is_rejected() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("test service");
        });
        let provider =
            TestHttpProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        let (tx, _rx) = mpsc::channel(64);
        let error = provider
            .stream(request("test"), tx, CancellationToken::new())
            .await
            .expect_err("truncated stream");
        assert!(error.to_string().contains("before [DONE]"));
        server.abort();
    }

    #[tokio::test]
    async fn raw_sse_event_is_bounded_before_json_parsing() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                let mut body = b"data: ".to_vec();
                body.extend(std::iter::repeat_n(b'x', MAX_PROVIDER_SSE_EVENT_BYTES + 1));
                body.extend_from_slice(b"\n\n");
                ([(header::CONTENT_TYPE, "text/event-stream")], body)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let provider =
            TestHttpProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        let (tx, _rx) = mpsc::channel(64);
        let error = provider
            .stream(request("test"), tx, CancellationToken::new())
            .await
            .expect_err("oversized event");
        assert!(error.to_string().contains("SSE event exceeded"));
        server.abort();
    }

    #[derive(Default)]
    struct RetryState {
        attempts: AtomicUsize,
        idempotency_keys: StdMutex<Vec<String>>,
    }

    async fn retry_handler(State(state): State<Arc<RetryState>>, headers: HeaderMap) -> Response {
        state.idempotency_keys.lock().expect("keys").push(
            headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        if state.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "0")],
                "rate limited",
            )
                .into_response();
        }
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"choices\":[{\"delta\":{\"content\":\"retried once\"}}]}\n\ndata: [DONE]\n\n",
        )
            .into_response()
    }

    #[tokio::test]
    async fn rate_limit_retry_uses_one_stable_idempotency_key() {
        let state = Arc::new(RetryState::default());
        let app = Router::new()
            .route("/v1/chat/completions", post(retry_handler))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let provider =
            TestHttpProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        let (tx, mut rx) = mpsc::channel(64);
        let turn = provider
            .stream(request("test"), tx, CancellationToken::new())
            .await
            .expect("retry succeeds");
        assert_eq!(turn.text, "retried once");
        let keys = state.idempotency_keys.lock().expect("keys");
        assert_eq!(keys.len(), 2);
        assert!(!keys[0].is_empty());
        assert_eq!(keys[0], keys[1]);
        let mut text_events = 0;
        while let Ok(event) = rx.try_recv() {
            text_events += usize::from(matches!(event, ProviderEvent::TextDelta(_)));
        }
        assert_eq!(text_events, 1, "the user-visible stream was not duplicated");
        server.abort();
    }

    #[tokio::test]
    async fn authentication_failure_is_classified_redacted_and_not_retried() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::UNAUTHORIZED, "bad axm_123456789abcdef")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let provider = TestHttpProvider::new(
            format!("http://{address}/v1"),
            Some("axm_anothersecretvalue".into()),
            Duration::from_secs(5),
        )
        .expect("provider");
        let (tx, _) = mpsc::channel(64);
        let error = provider
            .stream(request("test"), tx, CancellationToken::new())
            .await
            .expect_err("authentication failure");
        let message = error.to_string();
        assert!(message.contains("Authentication"), "{message}");
        assert!(!message.contains("axm_"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn model_discovery_is_real_http_and_bounded() {
        let app = Router::new().route(
            "/v1/models",
            get(|| async { axum::Json(json!({"data":[{"id":"cherry-1"},{"id":"cherry-2"}]})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let provider =
            TestHttpProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        assert_eq!(
            provider
                .models(CancellationToken::new())
                .await
                .expect("models"),
            vec![
                ModelInfo {
                    id: "cherry-1".into(),
                    ..ModelInfo::default()
                },
                ModelInfo {
                    id: "cherry-2".into(),
                    ..ModelInfo::default()
                }
            ]
        );
        server.abort();
    }

    #[tokio::test]
    async fn raw_model_catalog_is_bounded_before_json_parsing() {
        let app = Router::new().route(
            "/v1/models",
            get(|| async { vec![b'x'; MAX_MODEL_CATALOG_BYTES + 1] }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let provider =
            TestHttpProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        let error = provider
            .models(CancellationToken::new())
            .await
            .expect_err("oversized catalog");
        assert!(error.to_string().contains("catalog exceeded"));
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_closes_an_inflight_provider_request() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                "unreachable"
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let provider = TestHttpProvider::new(
            format!("http://{address}/v1"),
            None,
            Duration::from_secs(60),
        )
        .expect("provider");
        let token = CancellationToken::new();
        let cancel = token.clone();
        let (tx, _) = mpsc::channel(64);
        let task = tokio::spawn(async move { provider.stream(request("test"), tx, token).await });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(matches!(
            task.await.expect("join"),
            Err(AxiomError::Cancelled)
        ));
        server.abort();
    }

    #[test]
    fn secure_provider_redacts_credentials_and_preserves_credit_classification() {
        let provider = SecureAxiomProvider::new(
            "https://api.axiom.stream",
            "axm_secure_test_value".to_owned(),
            Duration::from_secs(30),
        )
        .expect("secure provider");
        assert!(!format!("{provider:?}").contains("axm_secure_test_value"));

        let error = map_secure_error(axiom_secure_client::SecureClientError::new(
            ProviderFailureKind::InsufficientCredit,
            "Axiom credit is exhausted",
        ));
        let rendered = error.to_string();
        assert!(rendered.contains("402 Payment Required"));
        assert!(rendered.contains("insufficient credit"));
    }

    #[tokio::test]
    async fn terminal_receipt_failure_does_not_revoke_preflight_attestation() {
        let (events, mut received) = mpsc::channel(8);
        events
            .send(ProviderEvent::SecurityState(
                ProviderSecurityState::Verified,
            ))
            .await
            .expect("verified preflight");
        let error = secure_stream_failure::<()>(
            &events,
            &CancellationToken::new(),
            AxiomError::Provider("missing signed terminal receipt".into()),
        )
        .await
        .expect_err("missing receipt fails closed");
        assert!(
            error
                .to_string()
                .contains("missing signed terminal receipt")
        );
        drop(events);
        let mut observed = Vec::new();
        while let Some(event) = received.recv().await {
            observed.push(event);
        }
        assert_eq!(
            observed,
            vec![ProviderEvent::SecurityState(
                ProviderSecurityState::Verified
            ),]
        );
        assert!(!observed.contains(&ProviderEvent::ResponseVerified));
    }

    #[tokio::test]
    async fn secure_stream_cancellation_does_not_masquerade_as_verification_failure() {
        let (events, mut received) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            secure_stream_failure::<()>(
                &events,
                &cancellation,
                AxiomError::Provider("request ended during cancellation".into()),
            )
            .await,
            Err(AxiomError::Cancelled)
        ));
        drop(events);
        assert!(received.recv().await.is_none());
    }

    /// Executes one low-cost turn through the configured real Axiom-compatible
    /// development endpoint. CI never supplies credentials; release review must
    /// invoke this test explicitly with its three `AXIOM_LIVE_*` variables.
    #[tokio::test]
    #[ignore = "requires AXIOM_API_KEY, AXIOM_LIVE_BASE_URL, and AXIOM_LIVE_MODEL"]
    async fn live_axiom_text_turn_does_not_expose_credentials() {
        let base_url = std::env::var("AXIOM_LIVE_BASE_URL").expect("AXIOM_LIVE_BASE_URL");
        let model = std::env::var("AXIOM_LIVE_MODEL").expect("AXIOM_LIVE_MODEL");
        let key = std::env::var("AXIOM_API_KEY").expect("AXIOM_API_KEY");
        let provider =
            TestHttpProvider::new(base_url, Some(key), Duration::from_secs(45)).expect("provider");
        let (tx, _rx) = mpsc::channel(64);
        let turn = provider
            .stream(
                InferenceRequest::streaming(
                    model,
                    vec![ChatMessage::text(
                        ChatRole::User,
                        "Reply with the single word cherry.",
                    )],
                    Vec::new(),
                ),
                tx,
                CancellationToken::new(),
            )
            .await
            .expect("live Axiom turn");
        assert!(!turn.text.trim().is_empty());
    }
}

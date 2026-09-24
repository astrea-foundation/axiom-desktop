use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use axiom_inference::{FinishReason, ProviderEvent, ProviderFailureKind};
use axiom_openai_compat::{
    ChatCompletionRequest, CompatError, CompatMode, done_sse, encode_sse, error_envelope,
    model_list, stream_event, stream_start,
};
use axiom_secure_client::{ApiCredential, SecureClient, SecureClientConfig};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, ValueEnum};
use futures_util::stream;
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

const MAX_LOCAL_BODY_BYTES: usize = 64 * 1024 * 1024;
const STREAM_CHANNEL_CAPACITY: usize = 32;
/// Names the parameters a request carried that were not applied. Clients that
/// ignore the header are unaffected; the value is always a sanitized ASCII list.
const IGNORED_PARAMETERS_HEADER: &str = "x-axiom-ignored-parameters";

#[derive(Debug, Parser)]
#[command(name = "axiom-proxy", version, about = "Axiom secure loopback proxy")]
pub struct Arguments {
    #[arg(long, env = "AXIOM_PROXY_BIND", default_value = "127.0.0.1:8484")]
    pub bind: SocketAddr,
    #[arg(
        long,
        env = "AXIOM_BASE_URL",
        default_value = "https://api.axiom.stream"
    )]
    pub axiom_base_url: String,
    #[arg(long, env = "AXIOM_PROXY_MAX_CONCURRENCY", default_value_t = 8)]
    pub max_concurrency: usize,
    #[arg(long, env = "AXIOM_PROXY_REQUEST_TIMEOUT_SECS", default_value_t = 120)]
    pub request_timeout_secs: u64,
    /// How to treat request parameters the Axiom relay cannot honor.
    #[arg(long, env = "AXIOM_PROXY_COMPAT", default_value = "lenient")]
    pub compat: CompatArgument,
}

/// Command-line spelling of [`CompatMode`].
///
/// The translation crate stays free of `clap` and of environment access; this
/// composition root owns both.
#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "lower")]
pub enum CompatArgument {
    /// Drop parameters that cannot be applied so an application the operator
    /// cannot modify still receives an answer. Dropped parameters that could
    /// have changed the answer are reported on the response header and to the
    /// supervisor log.
    Lenient,
    /// Refuse any request carrying a parameter that cannot be applied.
    Strict,
}

impl From<CompatArgument> for CompatMode {
    fn from(value: CompatArgument) -> Self {
        match value {
            CompatArgument::Lenient => Self::Lenient,
            CompatArgument::Strict => Self::Strict,
        }
    }
}

struct AppState {
    client: Arc<dyn ClientSource>,
    local_token_hash: [u8; 32],
    concurrency: Arc<Semaphore>,
    shutdown: CancellationToken,
    compat: CompatMode,
}

#[derive(Clone, Copy, Debug)]
struct ApiError {
    status: StatusCode,
    message: &'static str,
    kind: &'static str,
    code: &'static str,
}

impl ApiError {
    const fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "invalid local proxy credential",
            kind: "authentication_error",
            code: "invalid_api_key",
        }
    }

    const fn busy() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "local proxy concurrency limit reached",
            kind: "rate_limit_error",
            code: "proxy_busy",
        }
    }

    const fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
            kind: "invalid_request_error",
            code: "invalid_request",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(error_envelope(self.message, self.kind, self.code)),
        )
            .into_response()
    }
}

#[derive(Serialize)]
struct Health<'a> {
    status: &'a str,
    security: &'a str,
}

pub async fn run_standalone() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    validate_arguments(&arguments)?;
    let axiom_key = std::env::var("AXIOM_API_KEY")
        .map_err(|_| "AXIOM_API_KEY is required and may only be supplied by environment")?;
    let local_token = std::env::var("AXIOM_PROXY_TOKEN")
        .map_err(|_| "AXIOM_PROXY_TOKEN is required and may only be supplied by environment")?;
    validate_local_token(&local_token)?;

    #[cfg(feature = "test-fixture")]
    let mut secure_config = if std::env::var("AXIOM_TEST_FIXTURE").as_deref() == Ok("1") {
        SecureClientConfig::new_test_fixture(&arguments.axiom_base_url)?
    } else {
        SecureClientConfig::new(&arguments.axiom_base_url)?
    };
    #[cfg(not(feature = "test-fixture"))]
    let mut secure_config = SecureClientConfig::new(&arguments.axiom_base_url)?;
    secure_config.request_timeout = Duration::from_secs(arguments.request_timeout_secs);
    let client = Arc::new(SecureClient::new(
        secure_config,
        ApiCredential::new(axiom_key),
    )?);
    serve(
        arguments,
        &local_token,
        Arc::new(FixedClient(client)),
        CancellationToken::new(),
    )
    .await
}

/// Resolves credentials locally before each request; never sends message data
/// through the credential source. Native hosts can refresh account sessions.
pub trait ClientSource: Send + Sync {
    fn client(
        &self,
        cancellation: CancellationToken,
    ) -> futures_util::future::BoxFuture<'_, axiom_secure_client::Result<Arc<SecureClient>>>;
}

struct FixedClient(Arc<SecureClient>);
impl ClientSource for FixedClient {
    fn client(
        &self,
        _cancellation: CancellationToken,
    ) -> futures_util::future::BoxFuture<'_, axiom_secure_client::Result<Arc<SecureClient>>> {
        Box::pin(async { Ok(Arc::clone(&self.0)) })
    }
}

/// Serves the same attested E2EE implementation for standalone and native hosts.
pub async fn serve(
    arguments: Arguments,
    local_token: &str,
    client: Arc<dyn ClientSource>,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_arguments(&arguments)?;
    validate_local_token(local_token)?;
    let state = Arc::new(AppState {
        client,
        local_token_hash: token_hash(local_token),
        concurrency: Arc::new(Semaphore::new(arguments.max_concurrency)),
        shutdown,
        compat: arguments.compat.into(),
    });
    let app = router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(arguments.bind).await?;
    let address = listener.local_addr()?;
    println!(
        "{}",
        serde_json::json!({
            "event":"ready",
            "address":address.to_string(),
            "openai_base_url":format!("http://{address}/v1"),
            "security":"attestation_per_request"
        })
    );
    eprintln!("axiom-proxy listening on loopback {address}");

    let shutdown = state.shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => shutdown.cancel(),
                () = shutdown.cancelled() => {},
            }
        })
        .await?;
    Ok(())
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(DefaultBodyLimit::max(MAX_LOCAL_BODY_BYTES))
        .with_state(state)
}

async fn health() -> Json<Health<'static>> {
    Json(Health {
        status: "alive",
        security: "not_evaluated",
    })
}

async fn ready() -> Json<Health<'static>> {
    Json(Health {
        status: "ready",
        security: "attestation_per_request",
    })
}

async fn models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorize(&state, &headers)?;
    let _permit = state
        .concurrency
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy())?;
    let client = state
        .client
        .client(state.shutdown.child_token())
        .await
        .map_err(map_secure_error)?;
    let models = client
        .models(state.shutdown.child_token())
        .await
        .map_err(map_secure_error)?;
    Ok(Json(model_list(&models)).into_response())
}

async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    payload: std::result::Result<Json<ChatCompletionRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorize(&state, &headers)?;
    let Json(wire) = payload.map_err(|_| ApiError::invalid_request("request JSON is invalid"))?;
    let include_usage = wire.include_stream_usage();
    let is_stream = wire.stream;
    let translated = match wire.into_domain(state.compat) {
        Ok(translated) => translated,
        Err(error) => return Ok(translation_error_response(&error)),
    };
    let ignored_parameters = translated.ignored_parameters;
    let domain = translated.request;
    let model_id = domain.model.clone();
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let created = unix_seconds();
    let permit = state
        .concurrency
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy())?;
    if !ignored_parameters.is_empty() {
        eprintln!(
            "{}",
            serde_json::json!({
                "event":"request_parameters_ignored",
                "request_id":id,
                "model_id":model_id,
                "parameters":ignored_parameters,
            })
        );
    }
    let cancellation = state.shutdown.child_token();
    let client = state
        .client
        .client(cancellation.clone())
        .await
        .map_err(map_secure_error)?;
    let models = client
        .models(cancellation.clone())
        .await
        .map_err(map_secure_error)?;
    let model = models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or(ApiError {
            status: StatusCode::NOT_FOUND,
            message: "selected model is unavailable",
            kind: "invalid_request_error",
            code: "model_not_found",
        })?;
    let trust_policy = client
        .trust_policy(cancellation.clone())
        .await
        .map_err(map_secure_error)?;
    let mut session = client
        .establish(model, &trust_policy, cancellation.clone())
        .await
        .map_err(|error| {
            log_security_failure(&id, &model_id, error.kind());
            map_secure_error(error)
        })?;
    log_security_evidence(&id, session.evidence());

    if !is_stream {
        let response = session
            .complete(domain, cancellation)
            .await
            .map_err(|error| {
                log_request_failure(&id, &error);
                map_secure_error(error)
            })?;
        log_terminal(&id, "completed", response.usage.total_tokens);
        drop(permit);
        let mut body = Json(axiom_openai_compat::completion(
            id, model_id, created, response,
        ))
        .into_response();
        attach_ignored_parameters(&mut body, &ignored_parameters);
        return Ok(body);
    }

    eprintln!(
        "{}",
        serde_json::json!({
            "event":"request_stream_open",
            "request_id":id,
            "model_id":model_id,
        })
    );
    let (output_tx, output_rx) = mpsc::channel::<Bytes>(STREAM_CHANNEL_CAPACITY);
    // AEAD-authenticated deltas remain provisional until the secure operation
    // succeeds. Couple cancellation to the response body even while a provider
    // is silent, and emit success markers only after terminal verification.
    let response_cancellation = cancellation.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let start = encode_sse(&stream_start(&id, &model_id, created));
        if send_chunk(&output_tx, start, &cancellation).await.is_err() {
            log_terminal(&id, "cancelled", 0);
            return;
        }
        let (event_tx, mut event_rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let inference = session.stream(domain, event_tx, cancellation.clone());
        tokio::pin!(inference);
        let result = loop {
            tokio::select! {
                () = cancellation.cancelled() => break Err(axiom_secure_client::SecureClientError::new(
                    ProviderFailureKind::Cancelled,
                    "operation cancelled",
                )),
                result = &mut inference => break result,
                event = event_rx.recv() => {
                    let Some(event) = event else { continue };
                    if matches!(event, ProviderEvent::Finished(_)) { continue; }
                    if let Some(chunk) = stream_event(&id, &model_id, created, event, include_usage)
                        && send_chunk(&output_tx, encode_sse(&chunk), &cancellation).await.is_err()
                    {
                        log_terminal(&id, "cancelled", 0);
                        return;
                    }
                }
            }
        };
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, ProviderEvent::Finished(_)) {
                continue;
            }
            if let Some(chunk) = stream_event(&id, &model_id, created, event, include_usage)
                && send_chunk(&output_tx, encode_sse(&chunk), &cancellation)
                    .await
                    .is_err()
            {
                log_terminal(&id, "cancelled", 0);
                return;
            }
        }
        match result {
            Ok(response) => {
                let finished = ProviderEvent::Finished(
                    response.finish_reason.clone().unwrap_or(FinishReason::Stop),
                );
                if let Some(chunk) = stream_event(&id, &model_id, created, finished, include_usage)
                    && send_chunk(&output_tx, encode_sse(&chunk), &cancellation)
                        .await
                        .is_err()
                {
                    log_terminal(&id, "cancelled", 0);
                    return;
                }
                log_terminal(&id, "completed", response.usage.total_tokens);
                let _ = send_chunk(&output_tx, Ok(done_sse().to_owned()), &cancellation).await;
            }
            Err(error) => {
                let terminal = if error.kind() == ProviderFailureKind::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                };
                log_terminal(&id, terminal, 0);
                log_request_failure(&id, &error);
                let mapped = map_secure_error(error);
                let envelope = error_envelope(mapped.message, mapped.kind, mapped.code);
                let _ = send_chunk(&output_tx, encode_sse(&envelope), &cancellation).await;
            }
        }
    });

    let body_stream = stream::unfold(
        (output_rx, CancelRequestOnDrop::new(response_cancellation)),
        |(mut receiver, cancellation)| async move {
            receiver
                .recv()
                .await
                .map(|bytes| (Ok::<Bytes, Infallible>(bytes), (receiver, cancellation)))
        },
    );
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body_stream))
        .expect("static response headers are valid");
    attach_ignored_parameters(&mut response, &ignored_parameters);
    Ok(response)
}

/// Records on the response which parameters were dropped.
///
/// A dropped parameter that could have changed the answer must be discoverable
/// by whoever is debugging the answer, so leniency never becomes silence.
fn attach_ignored_parameters(response: &mut Response, parameters: &[String]) {
    if parameters.is_empty() {
        return;
    }
    // Names are sanitized to ASCII alphanumerics, `_`, `-`, and `.` before they
    // reach here, so the joined value is always a valid header value.
    if let Ok(value) = parameters.join(",").parse() {
        response
            .headers_mut()
            .insert(IGNORED_PARAMETERS_HEADER, value);
    }
}

/// Turns a translation failure into a client error.
///
/// A strict-mode rejection also names the offending parameters on the header,
/// because the error message itself is a fixed string.
fn translation_error_response(error: &CompatError) -> Response {
    let mut response = ApiError::invalid_request(error.safe_detail()).into_response();
    attach_ignored_parameters(&mut response, error.parameters());
    response
}

fn log_security_evidence(request_id: &str, evidence: &axiom_secure_client::SecurityEvidence) {
    let checks = evidence
        .checks
        .iter()
        .map(|check| {
            serde_json::json!({
                "id": check.id,
                "status": check.status,
                "passed": check.passed,
            })
        })
        .collect::<Vec<_>>();
    eprintln!(
        "{}",
        serde_json::json!({
            "event":"security_verified",
            "request_id":request_id,
            "model_id":evidence.model_id,
            "provider_id":evidence.provider_id,
            "attestation_protocol":evidence.attestation_protocol,
            "e2ee_protocol":evidence.e2ee_protocol,
            "e2ee_encryption_version":evidence.e2ee_encryption_version,
            "trust_policy_version":evidence.trust_policy_version,
            "verified_at_unix_seconds":evidence.verified_at_unix_seconds,
            "model_key_fingerprint":evidence.model_key_fingerprint,
            "tls_spki_fingerprint":evidence.tls_spki_fingerprint,
            "checks":checks,
        })
    );
}

fn log_security_failure(request_id: &str, model_id: &str, kind: ProviderFailureKind) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event":"security_failed",
            "request_id":request_id,
            "model_id":model_id,
            "failure_kind":format!("{kind:?}"),
        })
    );
}

fn log_terminal(request_id: &str, terminal: &'static str, total_tokens: u64) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event":"request_terminal",
            "request_id":request_id,
            "terminal":terminal,
            "total_tokens":total_tokens,
        })
    );
}

fn log_request_failure(request_id: &str, error: &axiom_secure_client::SecureClientError) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event":"request_failed",
            "request_id":request_id,
            "failure_kind":format!("{:?}", error.kind()),
            "safe_detail":error.safe_detail(),
        })
    );
}

async fn send_chunk(
    sender: &mpsc::Sender<Bytes>,
    encoded: axiom_openai_compat::Result<String>,
    cancellation: &CancellationToken,
) -> Result<(), ()> {
    let bytes = Bytes::from(encoded.map_err(|_| ())?);
    tokio::select! {
        () = cancellation.cancelled() => Err(()),
        result = sender.send(bytes) => {
            if result.is_err() {
                cancellation.cancel();
                Err(())
            } else {
                Ok(())
            }
        }
    }
}

/// Couples an HTTP response body's lifetime to its upstream inference.
///
/// Hyper drops the body stream when the client disconnects. Secure inference
/// may not produce an eligible downstream chunk before terminal verification,
/// so cancellation must not depend on observing a failed channel send.
struct CancelRequestOnDrop(CancellationToken);

impl CancelRequestOnDrop {
    fn new(cancellation: CancellationToken) -> Self {
        Self(cancellation)
    }
}

impl Drop for CancelRequestOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let presented = token_hash(value);
    if bool::from(presented.ct_eq(&state.local_token_hash)) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn token_hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn validate_local_token(value: &str) -> Result<(), &'static str> {
    if value.len() < 32 || value.len() > 512 || value.chars().any(char::is_control) {
        Err("AXIOM_PROXY_TOKEN must contain 32 to 512 non-control bytes")
    } else {
        Ok(())
    }
}

fn validate_arguments(arguments: &Arguments) -> Result<(), &'static str> {
    if !arguments.bind.ip().is_loopback() {
        return Err("axiom-proxy only accepts numeric loopback bind addresses");
    }
    if arguments.max_concurrency == 0 || arguments.max_concurrency > 256 {
        return Err("AXIOM_PROXY_MAX_CONCURRENCY must be between 1 and 256");
    }
    if !(1..=3600).contains(&arguments.request_timeout_secs) {
        return Err("AXIOM_PROXY_REQUEST_TIMEOUT_SECS must be between 1 and 3600");
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn map_secure_error(error: axiom_secure_client::SecureClientError) -> ApiError {
    match error.kind() {
        ProviderFailureKind::Authentication | ProviderFailureKind::LocalAuthentication => {
            ApiError {
                status: StatusCode::BAD_GATEWAY,
                message: "Axiom authentication failed",
                kind: "authentication_error",
                code: "upstream_authentication_failed",
            }
        }
        ProviderFailureKind::InsufficientCredit => ApiError {
            status: StatusCode::PAYMENT_REQUIRED,
            message: "Axiom credit is exhausted; visit axiom.stream to top up",
            kind: "insufficient_quota",
            code: "insufficient_credit",
        },
        ProviderFailureKind::RateLimited => ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "Axiom request was rate limited",
            kind: "rate_limit_error",
            code: "rate_limited",
        },
        ProviderFailureKind::ModelUnavailable => ApiError {
            status: StatusCode::NOT_FOUND,
            message: "selected model is unavailable",
            kind: "invalid_request_error",
            code: "model_not_found",
        },
        ProviderFailureKind::InvalidRequest | ProviderFailureKind::CapabilityMismatch => {
            ApiError::invalid_request("request is not supported by the selected model")
        }
        ProviderFailureKind::Cancelled => ApiError {
            status: StatusCode::REQUEST_TIMEOUT,
            message: "request was cancelled",
            kind: "request_error",
            code: "cancelled",
        },
        ProviderFailureKind::AttestationUnavailable
        | ProviderFailureKind::AttestationRejected
        | ProviderFailureKind::SessionEstablishment
        | ProviderFailureKind::Encryption
        | ProviderFailureKind::Decryption => ApiError {
            status: StatusCode::BAD_GATEWAY,
            message: "secure inference verification failed",
            kind: "security_error",
            code: "secure_inference_failed",
        },
        ProviderFailureKind::Configuration => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "proxy configuration is invalid",
            kind: "server_error",
            code: "configuration_error",
        },
        ProviderFailureKind::Transient | ProviderFailureKind::InvalidResponse => ApiError {
            status: StatusCode::BAD_GATEWAY,
            message: "secure upstream request failed",
            kind: "server_error",
            code: "upstream_failed",
        },
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_bind_and_weak_tokens() {
        let unsafe_arguments = Arguments {
            bind: "0.0.0.0:8484".parse().unwrap(),
            axiom_base_url: "https://api.axiom.stream".to_owned(),
            max_concurrency: 8,
            request_timeout_secs: 120,
            compat: CompatArgument::Lenient,
        };
        assert!(validate_arguments(&unsafe_arguments).is_err());
        assert!(validate_local_token("short").is_err());
        assert!(validate_local_token(&"x".repeat(32)).is_ok());
    }

    #[test]
    fn token_comparison_uses_only_fixed_hashes() {
        let expected = token_hash("correct-local-token-value-123456789");
        let right = token_hash("correct-local-token-value-123456789");
        let wrong = token_hash("incorrect-token-value-1234567890123");
        assert!(bool::from(expected.ct_eq(&right)));
        assert!(!bool::from(expected.ct_eq(&wrong)));
    }

    #[test]
    fn upstream_errors_are_stable_and_do_not_reflect_details() {
        let error = axiom_secure_client::SecureClientError::new(
            ProviderFailureKind::AttestationRejected,
            "do not reflect provider data",
        );
        let mapped = map_secure_error(error);
        assert_eq!(mapped.code, "secure_inference_failed");
        assert!(!mapped.message.contains("provider data"));
    }

    #[test]
    fn dropping_response_body_guard_cancels_silent_upstream_work() {
        let cancellation = CancellationToken::new();
        {
            let _guard = CancelRequestOnDrop::new(cancellation.clone());
            assert!(!cancellation.is_cancelled());
        }
        assert!(cancellation.is_cancelled());
    }
}

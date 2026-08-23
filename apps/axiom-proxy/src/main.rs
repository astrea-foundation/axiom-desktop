use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use axiom_inference::ProviderFailureKind;
use axiom_openai_compat::{
    ChatCompletionRequest, done_sse, encode_sse, error_envelope, model_list, stream_event,
    stream_start,
};
use axiom_secure_client::{ApiCredential, SecureClient, SecureClientConfig, TrustPolicy};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use futures_util::stream;
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

const MAX_LOCAL_BODY_BYTES: usize = 2 * 1024 * 1024;
const STREAM_CHANNEL_CAPACITY: usize = 32;

#[derive(Debug, Parser)]
#[command(name = "axiom-proxy", version, about = "Axiom secure loopback proxy")]
struct Arguments {
    #[arg(long, env = "AXIOM_PROXY_BIND", default_value = "127.0.0.1:8484")]
    bind: SocketAddr,
    #[arg(
        long,
        env = "AXIOM_BASE_URL",
        default_value = "https://api.axiom.stream"
    )]
    axiom_base_url: String,
    #[arg(long, env = "AXIOM_PROXY_MAX_CONCURRENCY", default_value_t = 8)]
    max_concurrency: usize,
    #[arg(long, env = "AXIOM_PROXY_REQUEST_TIMEOUT_SECS", default_value_t = 120)]
    request_timeout_secs: u64,
}

struct AppState {
    client: SecureClient,
    local_token_hash: [u8; 32],
    concurrency: Arc<Semaphore>,
    shutdown: CancellationToken,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let state = Arc::new(AppState {
        client: SecureClient::new(secure_config, ApiCredential::new(axiom_key))?,
        local_token_hash: token_hash(&local_token),
        concurrency: Arc::new(Semaphore::new(arguments.max_concurrency)),
        shutdown: CancellationToken::new(),
    });
    drop(local_token);

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
            let _ = tokio::signal::ctrl_c().await;
            shutdown.cancel();
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
    let models = state
        .client
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
    let domain = wire
        .into_domain()
        .map_err(|error| ApiError::invalid_request(error.safe_detail()))?;
    let model_id = domain.model.clone();
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let created = unix_seconds();
    let permit = state
        .concurrency
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy())?;
    let cancellation = state.shutdown.child_token();
    let models = state
        .client
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
    let mut session = state
        .client
        .establish(model, &TrustPolicy::default(), cancellation.clone())
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
        return Ok(Json(axiom_openai_compat::completion(
            id, model_id, created, response,
        ))
        .into_response());
    }

    let (output_tx, output_rx) = mpsc::channel::<Bytes>(STREAM_CHANNEL_CAPACITY);
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
                let _ = send_chunk(&output_tx, Ok(done_sse().to_owned()), &cancellation).await;
            }
        }
    });

    let body_stream = stream::unfold(output_rx, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|bytes| (Ok::<Bytes, Infallible>(bytes), receiver))
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body_stream))
        .expect("static response headers are valid"))
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
            "approved_workload_identity":evidence.approved_workload_identity,
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
}

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use futures::StreamExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result, audit::redact_text};
pub use axiom_inference::{
    AssistantTurn, ChatMessage, ChatRole, FunctionCall, FunctionDefinition, InferenceRequest,
    ModelInfo, ProviderEvent, ProviderFailureKind, ToolCall, ToolDefinition,
};

const MAX_PROVIDER_ERROR_BYTES: usize = 4 * 1024;
const MAX_MODEL_CATALOG_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROVIDER_SSE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_PROVIDER_STREAM_BYTES: usize = 16 * 1024 * 1024;

#[async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn>;

    async fn models(&self, _cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        Err(AxiomError::Provider(
            "provider does not support model discovery".into(),
        ))
    }
}

pub struct AxiomProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<SecretString>,
}

impl std::fmt::Debug for AxiomProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AxiomProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish_non_exhaustive()
    }
}

impl AxiomProvider {
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

    pub fn from_environment(base_url: impl Into<String>, timeout: Duration) -> Result<Self> {
        Self::new(base_url, std::env::var("AXIOM_API_KEY").ok(), timeout)
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
impl InferenceProvider for AxiomProvider {
    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
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
                let remainder = event_buffer.split_off(boundary.saturating_add(delimiter_bytes));
                let raw_event = std::mem::replace(&mut event_buffer, remainder);
                let raw_event = &raw_event[..boundary];
                if let Some(data) = sse_data(raw_event)?
                    && process_stream_data(&data, &mut text, &mut partial_calls, &events).await?
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
        Ok(AssistantTurn { text, tool_calls })
    }

    async fn models(&self, cancellation: CancellationToken) -> Result<Vec<ModelInfo>> {
        let request = self.authenticated(self.client.get(self.models_endpoint()));
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            response = request.send() => response.map_err(|error| AxiomError::Provider(error.to_string()))?,
        };
        if !response.status().is_success() {
            let status = response.status();
            let detail = read_response_prefix(response, MAX_PROVIDER_ERROR_BYTES, &cancellation)
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

fn truncate(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.into();
    }
    let mut end = max_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &input[..end])
}

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
        let provider = AxiomProvider::new(
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
            AxiomProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
                .expect("provider");
        let (tx, mut rx) = mpsc::channel(64);
        let turn = provider
            .stream(request("test"), tx, CancellationToken::new())
            .await
            .expect("stream");
        assert_eq!(turn.text, "hello world");
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
            AxiomProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
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
            AxiomProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
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
            AxiomProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
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
        let provider = AxiomProvider::new(
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
        assert!(message.contains("Authentication"));
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
            AxiomProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
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
            AxiomProvider::new(format!("http://{address}/v1"), None, Duration::from_secs(5))
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
        let provider = AxiomProvider::new(
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
            AxiomProvider::new(base_url, Some(key), Duration::from_secs(45)).expect("provider");
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

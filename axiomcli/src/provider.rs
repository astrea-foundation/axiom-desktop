use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use eventsource_stream::Eventsource as _;
use futures::StreamExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result, audit::redact_text};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    #[must_use]
    pub fn text(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionDefinition,
}

#[derive(Clone, Debug, Serialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct InferenceRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    Status {
        connected: bool,
        detail: String,
    },
    TextDelta(String),
    ReasoningDelta(String),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssistantTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderFailureKind {
    Authentication,
    RateLimited,
    Transient,
    InvalidRequest,
    InvalidResponse,
}

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
                    let detail = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "unreadable response".into());
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
        let _ = events
            .send(ProviderEvent::Status {
                connected: true,
                detail: "Axiom development inference connected · UNATTESTED".into(),
            })
            .await;

        let mut stream = response.bytes_stream().eventsource();
        let mut text = String::new();
        let mut partial_calls: BTreeMap<usize, PartialToolCall> = BTreeMap::new();
        let mut saw_done = false;
        let mut stream_bytes = 0_usize;
        loop {
            let next = tokio::select! {
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                next = stream.next() => next,
            };
            let Some(event) = next else { break };
            let event = event.map_err(|error| AxiomError::Provider(error.to_string()))?;
            if event.data.trim() == "[DONE]" {
                saw_done = true;
                break;
            }
            if event.data.len() > 1024 * 1024 {
                return Err(AxiomError::Provider(
                    "provider SSE event exceeded 1 MiB".into(),
                ));
            }
            stream_bytes = stream_bytes.saturating_add(event.data.len());
            if stream_bytes > 16 * 1024 * 1024 {
                return Err(AxiomError::Provider(
                    "provider stream exceeded the 16 MiB turn limit".into(),
                ));
            }
            let chunk: StreamChunk = serde_json::from_str(&event.data)
                .map_err(|error| AxiomError::Provider(format!("invalid SSE chunk: {error}")))?;
            for choice in chunk.choices {
                if let Some(delta) = choice.delta.content {
                    text.push_str(&delta);
                    let _ = events.send(ProviderEvent::TextDelta(delta)).await;
                }
                if let Some(delta) = choice.delta.reasoning_content {
                    let _ = events.send(ProviderEvent::ReasoningDelta(delta)).await;
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
                let _ = events
                    .send(ProviderEvent::Usage {
                        input_tokens: usage.prompt_tokens,
                        output_tokens: usage.completion_tokens,
                    })
                    .await;
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
            let detail = response.text().await.unwrap_or_default();
            return Err(provider_http_error(status, &detail));
        }
        let models = response
            .json::<ModelsResponse>()
            .await
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
        InferenceRequest {
            model: model.into(),
            messages: vec![ChatMessage::text(ChatRole::User, "hello")],
            tools: Vec::new(),
        }
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
                    id: "cherry-1".into()
                },
                ModelInfo {
                    id: "cherry-2".into()
                }
            ]
        );
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
                InferenceRequest {
                    model,
                    messages: vec![ChatMessage::text(
                        ChatRole::User,
                        "Reply with the single word cherry.",
                    )],
                    tools: Vec::new(),
                },
                tx,
                CancellationToken::new(),
            )
            .await
            .expect("live Axiom turn");
        assert!(!turn.text.trim().is_empty());
    }
}

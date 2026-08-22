use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use eventsource_stream::Eventsource as _;
use futures::StreamExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result};

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

#[async_trait]
pub trait InferenceProvider: Send + Sync {
    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::UnboundedSender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn>;
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
        events: mpsc::UnboundedSender<ProviderEvent>,
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
        let mut request_builder = self.client.post(self.endpoint()).json(&body);
        if let Some(api_key) = &self.api_key {
            request_builder = request_builder.bearer_auth(api_key.expose_secret());
        }
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            response = request_builder.send() => response.map_err(|error| AxiomError::Provider(error.to_string()))?,
        };
        if !response.status().is_success() {
            let status = response.status();
            let detail = response
                .text()
                .await
                .unwrap_or_else(|_| "unreadable response".into());
            return Err(AxiomError::Provider(format!(
                "HTTP {status}: {}",
                truncate(&detail, 4096)
            )));
        }

        let mut stream = response.bytes_stream().eventsource();
        let mut text = String::new();
        let mut partial_calls: BTreeMap<usize, PartialToolCall> = BTreeMap::new();
        let mut saw_done = false;
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
            let chunk: StreamChunk = serde_json::from_str(&event.data)
                .map_err(|error| AxiomError::Provider(format!("invalid SSE chunk: {error}")))?;
            for choice in chunk.choices {
                if let Some(delta) = choice.delta.content {
                    text.push_str(&delta);
                    let _ = events.send(ProviderEvent::TextDelta(delta));
                }
                if let Some(delta) = choice.delta.reasoning_content {
                    let _ = events.send(ProviderEvent::ReasoningDelta(delta));
                }
                for delta in choice.delta.tool_calls {
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
                let _ = events.send(ProviderEvent::Usage {
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                });
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
    use super::*;
    use axum::{Router, http::header, routing::post};

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
        let (tx, mut rx) = mpsc::unbounded_channel();
        let turn = provider
            .stream(
                InferenceRequest {
                    model: "test".into(),
                    messages: vec![ChatMessage::text(ChatRole::User, "hello")],
                    tools: Vec::new(),
                },
                tx,
                CancellationToken::new(),
            )
            .await
            .expect("stream");
        assert_eq!(turn.text, "hello world");
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
        let (tx, _rx) = mpsc::unbounded_channel();
        let error = provider
            .stream(
                InferenceRequest {
                    model: "test".into(),
                    messages: vec![ChatMessage::text(ChatRole::User, "hello")],
                    tools: Vec::new(),
                },
                tx,
                CancellationToken::new(),
            )
            .await
            .expect_err("truncated stream");
        assert!(error.to_string().contains("before [DONE]"));
        server.abort();
    }
}

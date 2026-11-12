//! Bound remote bodies before parsing; never replay POSTs or follow redirects.
use futures::{StreamExt, stream::BoxStream};
use reqwest::{
    StatusCode,
    header::{HeaderName, HeaderValue},
};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::streamable_http_client::{
        AuthRequiredError, InsufficientScopeError, StreamableHttpClient, StreamableHttpError,
        StreamableHttpPostResponse,
    },
};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc};

const MAX_BODY: usize = 1024 * 1024;
type HttpError = StreamableHttpError<reqwest::Error>;
#[derive(Clone)]
pub(super) struct BoundedHttpClient(pub reqwest::Client);

impl BoundedHttpClient {
    async fn post(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
        max_event: usize,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let mut request = self
            .0
            .post(uri.as_ref())
            .header("accept", "application/json, text/event-stream")
            .headers(headers.into_iter().collect());
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let has_session = session.is_some();
        if let Some(session) = session {
            request = request.header("mcp-session-id", session.as_ref());
        }
        let response = request.json(&message).send().await?;
        let status = response.status();
        let challenge = response
            .headers()
            .get("www-authenticate")
            .and_then(|h| h.to_str().ok());
        if status == StatusCode::UNAUTHORIZED {
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                challenge.unwrap_or("Bearer").to_owned(),
            )));
        }
        if status == StatusCode::FORBIDDEN
            && let Some(challenge) = challenge
        {
            return Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(challenge.to_owned(), None),
            ));
        }
        if has_session && status == StatusCode::NOT_FOUND {
            return Err(StreamableHttpError::SessionExpired);
        }
        if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if status.is_success() && content_type.starts_with("text/event-stream") {
            let limit = max_event.min(MAX_BODY);
            let stream =
                response
                    .bytes_stream()
                    .scan((0usize, false, false, false), move |state, chunk| {
                        if state.3 {
                            return std::future::ready(None);
                        }
                        let result = chunk.map_err(std::io::Error::other).and_then(|chunk| {
                            for byte in &chunk {
                                state.0 += 1;
                                if state.0 > limit {
                                    state.3 = true;
                                    return Err(std::io::Error::other(
                                        "remote MCP SSE event exceeds the size limit",
                                    ));
                                }
                                if state.2 && *byte == b'\n' {
                                    state.2 = false;
                                    continue;
                                }
                                state.2 = *byte == b'\r';
                                if *byte == b'\r' || *byte == b'\n' {
                                    if !state.1 {
                                        state.0 = 0;
                                    }
                                    state.1 = false;
                                } else {
                                    state.1 = true;
                                }
                            }
                            Ok(chunk)
                        });
                        std::future::ready(Some(result))
                    });
            return Ok(StreamableHttpPostResponse::Sse(
                SseStream::from_bytes_stream(stream).boxed(),
                session,
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY as u64)
        {
            return Err(StreamableHttpError::Io(std::io::Error::other(
                "remote MCP response exceeds the size limit",
            )));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if chunk.len() > MAX_BODY - bytes.len() {
                return Err(StreamableHttpError::Io(std::io::Error::other(
                    "remote MCP response exceeds the size limit",
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        if status.is_success()
            && bytes.is_empty()
            && !matches!(message, ClientJsonRpcMessage::Request(_))
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if content_type.starts_with("application/json") {
            let message: ServerJsonRpcMessage = serde_json::from_slice(&bytes)?;
            if status.is_success() || matches!(message, ServerJsonRpcMessage::Error(_)) {
                return Ok(StreamableHttpPostResponse::Json(message, session));
            }
        }
        Err(StreamableHttpError::UnexpectedServerResponse(
            format!("HTTP {status}").into(),
        ))
    }
}

impl StreamableHttpClient for BoundedHttpClient {
    type Error = reqwest::Error;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.post(uri, message, session, token, headers, MAX_BODY)
            .await
    }
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
        limit: usize,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.post(uri, message, session, token, headers, limit)
            .await
    }
    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), HttpError> {
        self.0.delete_session(uri, session, token, headers).await
    }
    async fn get_stream(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last: Option<String>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, HttpError> {
        self.get_stream_with_max_sse_event_size(uri, session, last, token, headers, MAX_BODY)
            .await
    }
    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last: Option<String>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
        limit: usize,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, HttpError> {
        self.0
            .get_stream_with_max_sse_event_size(
                uri,
                session,
                last,
                token,
                headers,
                limit.min(MAX_BODY),
            )
            .await
    }
}

use reqwest::{Response, StatusCode, header};
use tokio_util::sync::CancellationToken;

use crate::{
    ApiCredential, Result, SecureClientConfig, SecureClientError,
    catalog::{error_for_status, relay_endpoint},
    http::{bounded_body, pinned_client},
    relay::dto::{RelayChatRequest, RelayCompletion},
};

pub(crate) struct RelayClient<'a> {
    config: &'a SecureClientConfig,
    credential: &'a ApiCredential,
}

impl<'a> RelayClient<'a> {
    pub(crate) const fn new(config: &'a SecureClientConfig, credential: &'a ApiCredential) -> Self {
        Self { config, credential }
    }

    pub(crate) async fn complete(
        &self,
        request: &RelayChatRequest,
        cancellation: &CancellationToken,
    ) -> Result<RelayCompletion> {
        let response = self.send(request, cancellation).await?;
        let body = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            body = bounded_body(response, self.config.limits.relay_response_bytes) => body?,
        };
        serde_json::from_slice(&body).map_err(|_| {
            SecureClientError::new(
                axiom_inference::ProviderFailureKind::InvalidResponse,
                "relay completion schema is invalid",
            )
        })
    }

    pub(crate) async fn stream(
        &self,
        request: &RelayChatRequest,
        cancellation: &CancellationToken,
    ) -> Result<Response> {
        self.send(request, cancellation).await
    }

    async fn send(
        &self,
        request: &RelayChatRequest,
        cancellation: &CancellationToken,
    ) -> Result<Response> {
        if cancellation.is_cancelled() {
            return Err(SecureClientError::cancelled());
        }
        let endpoint = relay_endpoint(
            &self.config.relay_base_url,
            "/api/v1/relay/chat/completions",
        )?;
        let client = pinned_client(
            &endpoint,
            &self.config.endpoint_policy,
            self.config.request_timeout,
            false,
        )
        .await?;
        let body = request.serialize_bounded(self.config.limits.serialized_relay_request_bytes)?;
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            response = client
                .post(endpoint)
                .header(header::ACCEPT, if request.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                })
                .header(header::CONTENT_TYPE, "application/json")
                .bearer_auth(self.credential.expose())
                .body(body)
                .send() => response.map_err(|_| SecureClientError::new(
                    axiom_inference::ProviderFailureKind::Transient,
                    "relay request failed",
                ))?,
        };
        if response.status() != StatusCode::OK {
            // Consume only a bounded amount so the connection can be reclaimed,
            // but never reflect an upstream body into a local error.
            let status = response.status();
            let _ = bounded_body(response, self.config.limits.relay_error_bytes).await;
            return Err(error_for_status(status));
        }
        Ok(response)
    }
}

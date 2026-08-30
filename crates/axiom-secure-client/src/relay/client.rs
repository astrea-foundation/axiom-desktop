use reqwest::{Response, StatusCode, header};
use serde::{Deserialize, de::DeserializeOwned};
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

    pub(crate) async fn provider_e2ee_key<T: DeserializeOwned>(
        &self,
        model_id: &str,
        observed_key_fingerprint: Option<&str>,
        observed_keyset_digest: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<T> {
        let mut query = vec![("model_id", model_id)];
        if let Some(fingerprint) = observed_key_fingerprint {
            query.push(("observed_key_fingerprint", fingerprint));
        }
        if let Some(digest) = observed_keyset_digest {
            query.push(("observed_keyset_digest", digest));
        }
        self.get_json(
            "/api/v1/provider/e2ee-key",
            &query,
            self.config.limits.attestation_report_bytes,
            cancellation,
        )
        .await
    }

    pub(crate) async fn provider_attestation_report<T: DeserializeOwned>(
        &self,
        model_id: &str,
        nonce: &str,
        cancellation: &CancellationToken,
    ) -> Result<T> {
        self.get_json(
            "/api/v1/provider/attestation/report",
            &[("model_id", model_id), ("nonce", nonce)],
            self.config.limits.attestation_report_bytes,
            cancellation,
        )
        .await
    }

    pub(crate) async fn signed_trust_policy(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>> {
        self.get_body(
            "/api/v1/relay/trust-policy",
            &[],
            self.config.limits.trust_policy_bytes,
            cancellation,
        )
        .await
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        body_limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<T> {
        let body = self.get_body(path, query, body_limit, cancellation).await?;
        serde_json::from_slice(&body).map_err(|_| {
            SecureClientError::new(
                axiom_inference::ProviderFailureKind::InvalidResponse,
                "provider evidence schema is invalid",
            )
        })
    }

    async fn get_body(
        &self,
        path: &str,
        query: &[(&str, &str)],
        body_limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>> {
        if cancellation.is_cancelled() {
            return Err(SecureClientError::cancelled());
        }
        let mut endpoint = relay_endpoint(&self.config.relay_base_url, path)?;
        endpoint
            .query_pairs_mut()
            .extend_pairs(query.iter().copied());
        let client = pinned_client(
            &endpoint,
            &self.config.endpoint_policy,
            self.config.attestation_timeout,
            false,
        )
        .await?;
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            response = client
                .get(endpoint)
                .header(header::ACCEPT, "application/json")
                .bearer_auth(self.credential.expose())
                .send() => response.map_err(|_| SecureClientError::new(
                    axiom_inference::ProviderFailureKind::AttestationUnavailable,
                    "provider evidence request failed",
                ))?,
        };
        if response.status() != StatusCode::OK {
            return Err(self.error_for_response(response).await);
        }
        bounded_body(response, body_limit).await
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
            return Err(self.error_for_response(response).await);
        }
        Ok(response)
    }

    async fn error_for_response(&self, response: Response) -> SecureClientError {
        let status = response.status();
        let body = bounded_body(response, self.config.limits.relay_error_bytes)
            .await
            .unwrap_or_default();
        error_for_status_body(status, &body)
    }
}

fn error_for_status_body(status: StatusCode, body: &[u8]) -> SecureClientError {
    let public = serde_json::from_slice::<PublicErrorEnvelope>(body).ok();
    let code = public
        .as_ref()
        .and_then(|envelope| envelope.detail.as_ref())
        .and_then(|detail| detail.code.as_deref());
    match code {
        Some("ATTESTATION_KEY_CHANGED") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::AttestationRejected,
            crate::SecureErrorCode::AttestationKeyChanged,
            "provider attestation key changed before inference",
            true,
        ),
        Some("ATTESTATION_EXPIRED") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::AttestationUnavailable,
            crate::SecureErrorCode::AttestationExpired,
            "provider attestation lease expired",
            true,
        ),
        Some("ATTESTATION_UNAVAILABLE" | "PROVIDER_ATTESTATION_UNAVAILABLE") => {
            SecureClientError::with_code(
                axiom_inference::ProviderFailureKind::AttestationUnavailable,
                crate::SecureErrorCode::AttestationUnavailable,
                "provider attestation is temporarily unavailable",
                true,
            )
        }
        Some("WORKLOAD_NOT_APPROVED") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::AttestationRejected,
            crate::SecureErrorCode::WorkloadNotApproved,
            "provider workload is not approved",
            false,
        ),
        Some("PROTOCOL_UNSUPPORTED") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::CapabilityMismatch,
            crate::SecureErrorCode::ProtocolUnsupported,
            "provider protocol is not supported",
            false,
        ),
        Some("SESSION_NOT_ACCEPTED") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::SessionEstablishment,
            crate::SecureErrorCode::SessionNotAccepted,
            "provider worker session was not accepted",
            true,
        ),
        Some("RECEIPT_UNAVAILABLE") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::InvalidResponse,
            crate::SecureErrorCode::ReceiptUnavailable,
            "provider response receipt is unavailable",
            true,
        ),
        _ => error_for_status(status),
    }
}

#[derive(Debug, Deserialize)]
struct PublicErrorEnvelope {
    #[serde(default)]
    detail: Option<PublicErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct PublicErrorDetail {
    #[serde(default)]
    code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_rotation_error_is_retryable_without_reflecting_remote_text() {
        let body = br#"{
            "detail": {
                "code": "ATTESTATION_KEY_CHANGED",
                "message": "secret upstream diagnostic",
                "message_sent": false,
                "action": "reattest_and_reencrypt"
            }
        }"#;
        let error = error_for_status_body(StatusCode::CONFLICT, body);
        assert_eq!(error.code(), crate::SecureErrorCode::AttestationKeyChanged);
        assert!(error.retryable());
        assert!(error.requires_reattest_and_reencrypt());
        assert!(!error.safe_detail().contains("secret"));
    }
}

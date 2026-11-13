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

    pub(crate) async fn accounting(
        &self,
        request_ids: &[String],
        cancellation: &CancellationToken,
    ) -> Result<Vec<axiom_inference::RequestUsage>> {
        #[derive(Deserialize)]
        struct Accounting {
            requests: Vec<axiom_inference::RequestUsage>,
        }
        if request_ids.is_empty() {
            return Ok(Vec::new());
        }

        if request_ids.len() > 100
            || request_ids.iter().any(|id| {
                id.len() != 32
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err(SecureClientError::configuration(
                "invalid accounting lookup IDs",
            ));
        }
        let endpoint = relay_endpoint(&self.config.relay_base_url, "/api/v1/usage/requests")?;
        let client = pinned_client(
            &endpoint,
            &self.config.endpoint_policy,
            std::time::Duration::from_secs(10),
            false,
        )
        .await?;
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            response = client.post(endpoint).bearer_auth(self.credential.expose()).json(&serde_json::json!({"request_ids":request_ids})).send() =>
                response.map_err(|_| SecureClientError::new(axiom_inference::ProviderFailureKind::Transient, "Accounting recovery is unavailable."))?,
        };
        if response.status() != StatusCode::OK {
            return Err(self.error_for_response(response).await);
        }

        let bytes = bounded_body(response, 256 * 1024).await?;
        let data: Accounting = serde_json::from_slice(&bytes)
            .map_err(|_| SecureClientError::catalog("invalid accounting response"))?;
        let mut seen = std::collections::HashSet::new();
        if data.requests.len() > request_ids.len()
            || data.requests.iter().any(|record| {
                !request_ids.contains(&record.request_id)
                    || !record.validate_counters()
                    || record.response_verified
                    || !seen.insert(&record.request_id)
            })
        {
            return Err(SecureClientError::catalog(
                "invalid accounting response identity",
            ));
        }
        Ok(data.requests)
    }

    pub(crate) async fn complete(
        &self,
        request: &RelayChatRequest,
        cancellation: &CancellationToken,
    ) -> Result<RelayCompletion> {
        let response = self.send(request, cancellation).await?;
        let body = tokio::select! {
            () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
            body = bounded_body(response, self.config.limits.relay_response_bytes) => body.map_err(|error| {
                if error.safe_detail() == "remote response exceeds the configured limit" {
                    SecureClientError::with_code(axiom_inference::ProviderFailureKind::InvalidResponse,
                        crate::SecureErrorCode::ResponseTooLarge, "Response exceeded the allowed size.", false)
                } else {
                    SecureClientError::with_code(axiom_inference::ProviderFailureKind::Transient,
                        crate::SecureErrorCode::ProviderDisconnected, "The provider connection was interrupted.", true)
                }
            })?,
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

    pub(crate) async fn provider_attestation_report<T: DeserializeOwned>(
        &self,
        model_id: &str,
        nonce: &str,
        e2ee_protocol: &str,
        e2ee_encryption_version: u16,
        attestation_protocol: &str,
        cancellation: &CancellationToken,
    ) -> Result<T> {
        let encryption_version = e2ee_encryption_version.to_string();
        self.get_json(
            "/api/v1/provider/attestation/report",
            &[
                ("model_id", model_id),
                ("nonce", nonce),
                ("e2ee_protocol", e2ee_protocol),
                ("encryption_version", encryption_version.as_str()),
                ("attestation_protocol", attestation_protocol),
                (
                    "allow_outdated_tcb",
                    if e2ee_protocol == "near-v3" && self.config.accepts_outdated_tcb("near") {
                        "true"
                    } else {
                        "false"
                    },
                ),
            ],
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
            // Static route and status only: never log the credential, query,
            // evidence body, or any request/response ciphertext.
            tracing::warn!(
                path,
                status = response.status().as_u16(),
                "Axiom evidence endpoint rejected request"
            );
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
            if request.stream {
                None
            } else {
                Some(std::time::Duration::from_secs(360))
            },
            false,
        )
        .await?;
        let body = request.serialize_bounded(self.config.limits.serialized_relay_request_bytes)?;
        let response = tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(300)) => return Err(SecureClientError::with_code(axiom_inference::ProviderFailureKind::Transient, crate::SecureErrorCode::StreamTimeout, "Timed out starting the encrypted request.", true)),
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
            tracing::warn!(
                path = "/api/v1/relay/chat/completions",
                status = response.status().as_u16(),
                "Axiom encrypted relay rejected request"
            );
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

pub(crate) fn stream_failure(code: Option<&str>) -> SecureClientError {
    use crate::SecureErrorCode;
    let (code, detail) = match code {
        Some("ATTESTATION_KEY_CHANGED") => (
            SecureErrorCode::AttestationKeyChanged,
            "The worker changed before inference. Verify a new session.",
        ),
        Some("response_too_large") => (
            SecureErrorCode::ResponseTooLarge,
            "The model response exceeded the response size budget.",
        ),
        Some("provider_timeout") => (
            SecureErrorCode::StreamTimeout,
            "The provider stopped responding. Please retry.",
        ),
        Some("provider_disconnected") => (
            SecureErrorCode::ProviderDisconnected,
            "The provider connection was interrupted.",
        ),
        _ => (
            SecureErrorCode::Transient,
            "The provider could not finish this response. Please retry.",
        ),
    };
    SecureClientError::with_code(
        axiom_inference::ProviderFailureKind::Transient,
        code,
        detail,
        true,
    )
}

pub(crate) fn error_for_status_body(status: StatusCode, body: &[u8]) -> SecureClientError {
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
        Some("PROVIDER_TDX_OUT_OF_DATE") => SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::AttestationRejected,
            crate::SecureErrorCode::OutdatedTee,
            "The provider's Intel TDX environment is OutOfDate and requires security updates.",
            false,
        ),
        Some(
            "PROVIDER_TDX_REJECTED"
            | "PROVIDER_GPU_ATTESTATION_REJECTED"
            | "PROVIDER_TLS_BINDING_REJECTED"
            | "ATTESTATION_REJECTED"
            | "PROVIDER_ATTESTATION_REJECTED",
        ) => SecureClientError::attestation(
            "The provider's security evidence failed verification. See verification diagnostics.",
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

    #[test]
    fn stream_rotation_is_reestablished_but_ambiguous_inference_is_not_replayed() {
        assert!(stream_failure(Some("ATTESTATION_KEY_CHANGED")).requires_reattest_and_reencrypt());
        for code in [
            "provider_timeout",
            "provider_disconnected",
            "RECEIPT_UNAVAILABLE",
        ] {
            assert!(!stream_failure(Some(code)).requires_reattest_and_reencrypt());
        }
        let receipt = error_for_status_body(
            StatusCode::BAD_GATEWAY,
            br#"{"detail":{"code":"RECEIPT_UNAVAILABLE"}}"#,
        );
        assert!(!receipt.requires_reattest_and_reencrypt());
    }
}

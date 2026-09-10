use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

use futures_util::StreamExt;
use reqwest::{Client, Response, redirect::Policy};
use url::Url;

use crate::{EndpointPolicy, Result, SecureClientError, config::is_public_address};

pub(crate) async fn pinned_client(
    url: &Url,
    endpoint_policy: &EndpointPolicy,
    timeout: impl Into<Option<Duration>>,
    expose_tls_info: bool,
) -> Result<Client> {
    let host = url
        .host_str()
        .ok_or_else(|| SecureClientError::catalog("endpoint URL has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| SecureClientError::catalog("endpoint URL has no usable port"))?;

    let mut addresses = BTreeSet::new();
    if let Ok(ip) = host.parse() {
        addresses.insert(SocketAddr::new(ip, port));
    } else {
        let resolved = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| SecureClientError::session("endpoint DNS resolution timed out"))?
        .map_err(|_| SecureClientError::session("endpoint DNS resolution failed"))?;
        addresses.extend(resolved);
    }
    if addresses.is_empty()
        || (endpoint_policy.reject_local_addresses
            && addresses
                .iter()
                .any(|address| !is_public_address(address.ip())))
    {
        return Err(SecureClientError::session(
            "endpoint resolved to a disallowed address",
        ));
    }

    let addresses: Vec<_> = addresses.into_iter().collect();
    let redirect_policy = if endpoint_policy.allow_redirects {
        Policy::limited(3)
    } else {
        Policy::none()
    };
    let mut builder = Client::builder()
        .redirect(redirect_policy)
        .connect_timeout(Duration::from_secs(15));
    if let Some(timeout) = timeout.into() {
        builder = builder.timeout(timeout);
    }
    builder
        .tls_info(expose_tls_info)
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| SecureClientError::session("HTTP client construction failed"))
}

pub(crate) async fn bounded_body(response: Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > limit as u64)
    {
        return Err(SecureClientError::session(
            "remote response exceeds the configured limit",
        ));
    }

    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| SecureClientError::session("remote response body failed"))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(SecureClientError::session(
                "remote response exceeds the configured limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::bounded_body;
    use axiom_inference::ProviderFailureKind;

    #[tokio::test]
    async fn bounded_body_rejects_actual_oversize_before_parsing() {
        let response = reqwest::Response::from(
            http::Response::builder()
                .header("content-length", "100")
                .body("tiny")
                .unwrap(),
        );
        let error = bounded_body(response, 3).await.unwrap_err();
        assert_eq!(error.kind(), ProviderFailureKind::SessionEstablishment);
    }
}

use std::{collections::BTreeSet, io::Read as _, net::SocketAddr, time::Duration};

use futures_util::StreamExt;
use reqwest::{Client, Response, redirect::Policy};
use url::Url;

use crate::{
    EndpointPolicy, Result, SecureClientConfig, SecureClientError, config::is_public_address,
};

fn loopback_https(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        && url.username().is_empty()
        && url.password().is_none()
}

/// Explicit per-process trust for a local HTTPS API. Never changes provider
/// attestation roots or the OS trust store. Remote origins cannot use it.
pub fn local_relay_ca_pem(origin: &Url) -> Result<Option<Vec<u8>>> {
    let path = std::env::var_os("AXIOM_LOCAL_RELAY_CA_FILE");
    read_local_ca(origin, path.as_deref().map(std::path::Path::new))
}

fn read_local_ca(origin: &Url, path: Option<&std::path::Path>) -> Result<Option<Vec<u8>>> {
    let Some(path) = path else { return Ok(None) };
    if !loopback_https(origin) {
        return Err(SecureClientError::configuration(
            "local relay CA requires a loopback HTTPS API",
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|_| SecureClientError::configuration("local relay CA is unavailable"))?;
    let mut pem = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut pem)
        .map_err(|_| SecureClientError::configuration("local relay CA is unreadable"))?;
    let valid = x509_parser::pem::parse_x509_pem(&pem).is_ok_and(|(remaining, block)| {
        block.label == "CERTIFICATE"
            && remaining.iter().all(u8::is_ascii_whitespace)
            && block.parse_x509().is_ok()
    });
    if pem.len() > 64 * 1024 || !valid {
        return Err(SecureClientError::configuration("invalid local relay CA"));
    }
    Ok(Some(pem))
}

pub(crate) async fn relay_client(
    config: &SecureClientConfig,
    url: &Url,
    timeout: impl Into<Option<Duration>>,
) -> Result<Client> {
    if url.origin() != config.relay_base_url.origin() {
        return Err(SecureClientError::configuration("relay origin changed"));
    }
    let mut policy = config.endpoint_policy.clone();
    if loopback_https(&config.relay_base_url) {
        policy.reject_local_addresses = false;
    }
    let ca = local_relay_ca_pem(&config.relay_base_url)?;
    pinned_client_inner(url, &policy, timeout, false, ca.as_deref()).await
}

pub(crate) async fn pinned_client(
    url: &Url,
    endpoint_policy: &EndpointPolicy,
    timeout: impl Into<Option<Duration>>,
    expose_tls_info: bool,
) -> Result<Client> {
    pinned_client_inner(url, endpoint_policy, timeout, expose_tls_info, None).await
}

async fn pinned_client_inner(
    url: &Url,
    endpoint_policy: &EndpointPolicy,
    timeout: impl Into<Option<Duration>>,
    expose_tls_info: bool,
    ca_pem: Option<&[u8]>,
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
        .no_proxy()
        .redirect(redirect_policy)
        .connect_timeout(Duration::from_secs(15));
    if let Some(timeout) = timeout.into() {
        builder = builder.timeout(timeout);
    }
    if let Some(pem) = ca_pem {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(pem)
                .map_err(|_| SecureClientError::configuration("invalid local relay CA"))?,
        );
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
    use super::{bounded_body, read_local_ca, relay_client};
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

    #[test]
    fn local_root_is_bounded_and_never_trusts_a_remote_or_http_origin() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("local-ca.pem");
        let key = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        std::fs::write(&path, key.cert.pem()).unwrap();
        for origin in [
            "https://localhost:9443",
            "https://127.0.0.1:9443",
            "https://[::1]:9443",
        ] {
            assert!(
                read_local_ca(&url::Url::parse(origin).unwrap(), Some(&path))
                    .unwrap()
                    .is_some()
            );
        }
        for origin in [
            "http://127.0.0.1:9443",
            "https://api.example",
            "https://localhost.attacker.example",
            "https://user@localhost:9443",
        ] {
            assert!(read_local_ca(&url::Url::parse(origin).unwrap(), Some(&path)).is_err());
        }
        let origin = url::Url::parse("https://localhost:9443").unwrap();
        std::fs::write(&path, vec![b'x'; 64 * 1024 + 1]).unwrap();
        assert!(read_local_ca(&origin, Some(&path)).is_err());
        std::fs::write(&path, "invalid PEM").unwrap();
        assert!(read_local_ca(&origin, Some(&path)).is_err());
    }

    #[tokio::test]
    async fn relay_root_and_loopback_exception_cannot_follow_an_origin_change() {
        let config = crate::SecureClientConfig::new("https://localhost:9443").unwrap();
        assert!(
            relay_client(
                &config,
                &url::Url::parse("https://localhost:9444/anything").unwrap(),
                None
            )
            .await
            .is_err()
        );
        assert!(
            relay_client(
                &config,
                &url::Url::parse("https://api.example/anything").unwrap(),
                None
            )
            .await
            .is_err()
        );
        assert!(
            config
                .endpoint_policy
                .validate_provider_url("https://localhost:9443")
                .is_err()
        );
    }
}

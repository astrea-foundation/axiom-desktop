use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt as _;
use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result, auth::AuthManager};

const MAX_SEARCH_QUERY_BYTES: usize = 2048;
const MAX_SEARCH_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: Url,
    pub snippet: String,
    pub source: String,
    pub score: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchReport {
    pub results: Vec<SearchResult>,
    pub warnings: Vec<String>,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn provenance(&self) -> &str;
    fn description(&self) -> &'static str {
        "Search the web and return source URLs."
    }
    async fn search(&self, query: &str, cancellation: CancellationToken) -> Result<SearchReport>;
}

/// Non-inference account service: only the explicit search query leaves the
/// native tool. Provider credentials, history, and model-message construction
/// never enter this path. No unauthenticated fallback on failure.
#[derive(Clone, Debug)]
pub struct HostedSearchClient {
    auth: AuthManager,
    endpoint: Url,
    client: reqwest::Client,
}

impl HostedSearchClient {
    pub fn new(auth: AuthManager) -> Result<Self> {
        // Derive the destination from the credential audience, not an
        // independently configurable URL that could receive another token.
        let endpoint = auth
            .account_api_origin()
            .join("/api/v1/search")
            .map_err(|_| AxiomError::Config("invalid account search origin".into()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| AxiomError::Config("could not initialize hosted search".into()))?;
        Ok(Self {
            auth,
            endpoint,
            client,
        })
    }
}

#[async_trait]
impl SearchProvider for HostedSearchClient {
    fn provenance(&self) -> &str {
        self.endpoint.as_str()
    }

    fn description(&self) -> &'static str {
        "Search the web using Axiom's hosted Decodo search. The search query is shared with Axiom and Decodo, not E2EE. Send only a concise search query, never conversation history. Free, limited to 60 searches per minute per account; do not immediately retry rate-limit errors."
    }

    async fn search(&self, query: &str, cancellation: CancellationToken) -> Result<SearchReport> {
        let query = query.trim();
        if query.is_empty()
            || query.len() > MAX_SEARCH_QUERY_BYTES
            || query.chars().any(char::is_control)
        {
            return Err(AxiomError::Tool(
                "search query must contain 1 to 2048 UTF-8 bytes without control characters".into(),
            ));
        }
        let lease = self
            .auth
            .access_token_for_account_operation(&cancellation)
            .await?;
        if !self.auth.account_access_is_current(&lease) {
            return Err(AxiomError::Cancelled);
        }
        let response = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            response = self.client.post(self.endpoint.clone())
                .bearer_auth(lease.expose_for_authorization())
                .json(&serde_json::json!({ "query": query })).send() => {
                    response.map_err(|_| AxiomError::Provider("Axiom web search is unavailable".into()))?
                },
        };
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(60)
                .clamp(1, 60);
            return Err(AxiomError::Tool(format!(
                "Search limit reached (60 per minute per account). Retry after {retry_after} seconds."
            )));
        }
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(AxiomError::Tool(
                "Sign in to Axiom again to use web search.".into(),
            ));
        }
        if response.status() != StatusCode::OK {
            return Err(AxiomError::Provider(
                "Axiom web search is temporarily unavailable".into(),
            ));
        }
        let bytes = read_bounded_response(
            response,
            MAX_SEARCH_RESPONSE_BYTES,
            &cancellation,
            "search response",
        )
        .await?;
        let mut report: SearchReport = serde_json::from_slice(&bytes)
            .map_err(|_| AxiomError::Protocol("invalid hosted search response".into()))?;
        if !self.auth.account_access_is_current(&lease) || cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        report
            .results
            .retain(|result| is_safe_result_url(&result.url));
        report.results.truncate(20);
        for result in &mut report.results {
            result.title = bounded_external_text(&result.title, 512);
            result.snippet = bounded_external_text(&result.snippet, 4096);
            result.source = bounded_external_text(&result.source, 128);
        }
        report.warnings.truncate(20);
        for warning in &mut report.warnings {
            *warning = bounded_external_text(warning, 512);
        }
        Ok(report)
    }
}

async fn read_bounded_response(
    response: reqwest::Response,
    maximum: usize,
    cancellation: &CancellationToken,
    label: &str,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(AxiomError::Provider(format!(
            "{label} exceeds {maximum} bytes"
        )));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::select! {
        () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
        chunk = stream.next() => chunk,
    } {
        let chunk =
            chunk.map_err(|error| AxiomError::Provider(format!("{label} failed: {error}")))?;
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(AxiomError::Provider(format!(
                "{label} exceeds {maximum} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn is_safe_result_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => is_public_ip(IpAddr::V4(address)),
        Some(url::Host::Ipv6(address)) => is_public_ip(IpAddr::V6(address)),
        Some(url::Host::Domain(domain)) => {
            let domain = domain.to_ascii_lowercase();
            domain != "localhost"
                && !domain.ends_with(".localhost")
                && domain
                    .rsplit_once('.')
                    .is_none_or(|(_, suffix)| suffix != "local")
                && !domain.ends_with(".internal")
        }
        None => false,
    }
}

fn bounded_external_text(input: &str, maximum: usize) -> String {
    let sanitized = input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect::<String>();
    let sanitized = sanitized.trim();
    if sanitized.len() <= maximum {
        return sanitized.to_owned();
    }
    let mut end = maximum;
    while !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    sanitized[..end].to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchResult {
    pub final_url: Url,
    pub content_type: Option<String>,
    pub content: String,
    pub truncated: bool,
    pub provenance: String,
}

#[derive(Clone, Debug)]
pub struct SafeFetcher {
    timeout: Duration,
    max_bytes: usize,
    max_redirects: usize,
}

impl SafeFetcher {
    #[must_use]
    pub fn new(timeout: Duration, max_bytes: usize) -> Self {
        Self {
            timeout,
            max_bytes: max_bytes.clamp(1024, 4 * 1024 * 1024),
            max_redirects: 5,
        }
    }

    pub async fn fetch(&self, input: &str, cancellation: CancellationToken) -> Result<FetchResult> {
        let mut url = Url::parse(input)
            .map_err(|error| AxiomError::Tool(format!("invalid fetch URL: {error}")))?;
        for redirect in 0..=self.max_redirects {
            let resolved = validate_public_destination(&url).await?;
            let host = url
                .host_str()
                .ok_or_else(|| AxiomError::Tool("fetch URL has no host".into()))?;
            let client = reqwest::Client::builder()
                .timeout(self.timeout)
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .user_agent(concat!("AxiomCLI/", env!("CARGO_PKG_VERSION")))
                .resolve_to_addrs(host, &resolved)
                .build()
                .map_err(|error| AxiomError::Tool(error.to_string()))?;
            let response = tokio::select! {
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                response = client.get(url.clone()).send() => response.map_err(|error| AxiomError::Tool(format!("fetch failed: {error}")))?,
            };
            if response.status().is_redirection() {
                if redirect == self.max_redirects {
                    return Err(AxiomError::Tool("too many redirects".into()));
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| AxiomError::Tool("redirect has no valid Location".into()))?;
                url = url
                    .join(location)
                    .map_err(|error| AxiomError::Tool(format!("invalid redirect: {error}")))?;
                continue;
            }
            if response.status() != StatusCode::OK {
                return Err(AxiomError::Tool(format!(
                    "fetch returned HTTP {}",
                    response.status()
                )));
            }
            if response
                .content_length()
                .is_some_and(|length| length > self.max_bytes as u64)
            {
                return Err(AxiomError::Tool("fetch body exceeds size limit".into()));
            }
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if content_type.as_deref().is_some_and(|kind| {
                !kind.starts_with("text/")
                    && !kind.contains("json")
                    && !kind.contains("xml")
                    && !kind.contains("javascript")
            }) {
                return Err(AxiomError::Tool(format!(
                    "fetch content type is not text-readable: {}",
                    content_type.as_deref().unwrap_or_default()
                )));
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            let mut truncated = false;
            while let Some(chunk) = tokio::select! {
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                chunk = stream.next() => chunk,
            } {
                let chunk = chunk.map_err(|error| AxiomError::Tool(error.to_string()))?;
                let remaining = self.max_bytes.saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if chunk.len() > remaining {
                    truncated = true;
                    break;
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            let content = if content_type
                .as_deref()
                .is_some_and(|kind| kind.contains("html"))
            {
                strip_html(&text)
            } else {
                text.into_owned()
            };
            return Ok(FetchResult {
                final_url: url,
                content_type,
                content,
                truncated,
                provenance: "direct local fetch (untrusted content)".into(),
            });
        }
        Err(AxiomError::Tool("redirect limit exhausted".into()))
    }
}

async fn validate_public_destination(url: &Url) -> Result<Vec<SocketAddr>> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AxiomError::PermissionDenied(
            "fetch permits only http and https".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AxiomError::PermissionDenied(
            "credential-bearing URLs are not allowed".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AxiomError::Tool("fetch URL has no host".into()))?;
    let lowercase = host.to_ascii_lowercase();
    if lowercase == "localhost"
        || lowercase.ends_with(".localhost")
        || lowercase
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix == "local")
        || lowercase.ends_with(".internal")
    {
        return Err(AxiomError::PermissionDenied(
            "local network destinations are blocked".into(),
        ));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| AxiomError::Tool("URL has no usable port".into()))?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| AxiomError::Tool(format!("DNS lookup failed: {error}")))?
        .collect();
    validate_resolved_addresses(&addresses)?;
    Ok(addresses)
}

fn validate_resolved_addresses(addresses: &[SocketAddr]) -> Result<()> {
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(AxiomError::PermissionDenied(
            "destination resolves to a non-public address".into(),
        ));
    }
    Ok(())
}

#[must_use]
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let value = u32::from(ip);
    let in_prefix = |network: Ipv4Addr, bits: u32| {
        let mask = if bits == 0 {
            0
        } else {
            u32::MAX << (32 - bits)
        };
        value & mask == u32::from(network) & mask
    };
    !(in_prefix(Ipv4Addr::UNSPECIFIED, 8)
        || in_prefix(Ipv4Addr::new(10, 0, 0, 0), 8)
        || in_prefix(Ipv4Addr::new(100, 64, 0, 0), 10)
        || in_prefix(Ipv4Addr::new(127, 0, 0, 0), 8)
        || in_prefix(Ipv4Addr::new(169, 254, 0, 0), 16)
        || ip == Ipv4Addr::new(168, 63, 129, 16)
        || in_prefix(Ipv4Addr::new(172, 16, 0, 0), 12)
        || in_prefix(Ipv4Addr::new(192, 0, 0, 0), 24)
        || in_prefix(Ipv4Addr::new(192, 0, 2, 0), 24)
        || in_prefix(Ipv4Addr::new(192, 168, 0, 0), 16)
        || in_prefix(Ipv4Addr::new(192, 88, 99, 0), 24)
        || in_prefix(Ipv4Addr::new(198, 18, 0, 0), 15)
        || in_prefix(Ipv4Addr::new(198, 51, 100, 0), 24)
        || in_prefix(Ipv4Addr::new(203, 0, 113, 0), 24)
        || in_prefix(Ipv4Addr::new(224, 0, 0, 0), 4)
        || in_prefix(Ipv4Addr::new(240, 0, 0, 0), 4))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let first = ip.segments()[0];
    first & 0xe000 == 0x2000
        && !(ip.segments()[0] == 0x2001 && matches!(ip.segments()[1], 0x0000 | 0x0db8))
        && ip.segments()[0] != 0x2002
}

fn strip_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len().min(64 * 1024));
    let mut in_tag = false;
    let mut last_was_space = false;
    for character in input.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_was_space {
                    output.push(' ');
                    last_was_space = true;
                }
            }
            _ if in_tag => {}
            _ if character.is_whitespace() => {
                if !last_was_space {
                    output.push(' ');
                    last_was_space = true;
                }
            }
            _ => {
                output.push(character);
                last_was_space = false;
            }
        }
    }
    output.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};

    use super::*;

    #[test]
    fn ssrf_address_classifier_blocks_non_public_ranges() {
        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(blocked.parse().expect("IP")), "{blocked}");
        }
        assert!(is_public_ip("1.1.1.1".parse().expect("IP")));
        assert!(is_public_ip("2606:4700:4700::1111".parse().expect("IP")));
    }

    #[tokio::test]
    async fn alternate_loopback_url_encodings_are_blocked() {
        for input in [
            "http://127.1/",
            "http://0x7f000001/",
            "http://2130706433/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            let url = Url::parse(input).expect("URL parser accepts encoding");
            assert!(validate_public_destination(&url).await.is_err(), "{input}");
        }
    }

    #[test]
    fn dns_rebinding_mixed_answers_and_metadata_ranges_are_blocked() {
        let mixed = [
            SocketAddr::new("1.1.1.1".parse().expect("public"), 443),
            SocketAddr::new("127.0.0.1".parse().expect("loopback"), 443),
        ];
        assert!(validate_resolved_addresses(&mixed).is_err());
        for address in [
            "169.254.169.254",
            "168.63.129.16",
            "100.100.100.200",
            "192.0.0.192",
            "2002:7f00:1::",
        ] {
            assert!(!is_public_ip(address.parse().expect("metadata IP")));
        }
    }

    #[tokio::test]
    async fn redirect_pivots_and_local_services_fail_before_a_request() {
        for target in [
            "http://localhost/admin",
            "http://127.0.0.1:8888/search",
            "http://169.254.169.254/latest/meta-data/",
            "http://metadata.google.internal/computeMetadata/v1/",
        ] {
            let url = Url::parse(target).expect("URL");
            assert!(validate_public_destination(&url).await.is_err(), "{target}");
        }
    }

    #[test]
    fn search_result_urls_and_external_text_are_normalized_safely() {
        assert!(!is_safe_result_url(
            &Url::parse("http://user:pass@example.com/").expect("URL")
        ));
        assert!(!is_safe_result_url(
            &Url::parse("http://127.0.0.1/").expect("URL")
        ));
        assert_eq!(bounded_external_text("\u{1b}[31mcherry", 64), "[31mcherry");
        assert_eq!(bounded_external_text("ééé", 3), "é");
    }

    #[test]
    fn html_is_reduced_to_bounded_plain_text() {
        assert_eq!(
            strip_html("<h1>Hello</h1><p>cherry tree</p>"),
            "Hello cherry tree"
        );
    }

    #[tokio::test]
    async fn raw_search_response_is_bounded_before_json_parsing() {
        let app = Router::new().route(
            "/search",
            get(|| async { vec![b'x'; MAX_SEARCH_RESPONSE_BYTES + 1] }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("test service");
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/search"))
            .send()
            .await
            .expect("response");
        let error = read_bounded_response(
            response,
            MAX_SEARCH_RESPONSE_BYTES,
            &CancellationToken::new(),
            "search response",
        )
        .await
        .expect_err("oversized raw response must fail");
        assert!(error.to_string().contains("response exceeds"));
        server.abort();
    }
}

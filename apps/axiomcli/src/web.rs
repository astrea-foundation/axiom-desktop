use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt as _;
use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result};

const MAX_SEARCH_QUERY_BYTES: usize = 8 * 1024;
const MAX_SEARCH_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

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
    async fn search(&self, query: &str, cancellation: CancellationToken) -> Result<SearchReport>;
}

#[derive(Clone, Debug)]
pub struct SearxngClient {
    base_url: Url,
    client: reqwest::Client,
    max_results: usize,
    concurrency: Arc<Semaphore>,
}

impl SearxngClient {
    pub fn new(base_url: &str, timeout: Duration, max_results: usize) -> Result<Self> {
        let mut base_url = Url::parse(base_url)
            .map_err(|error| AxiomError::Config(format!("invalid SearXNG URL: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(AxiomError::Config(
                "SearXNG URL must use http or https".into(),
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("AxiomCLI/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| AxiomError::Provider(error.to_string()))?;
        Ok(Self {
            base_url,
            client,
            max_results: max_results.clamp(1, 100),
            concurrency: Arc::new(Semaphore::new(4)),
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &Url {
        &self.base_url
    }
}

#[async_trait]
impl SearchProvider for SearxngClient {
    fn provenance(&self) -> &str {
        self.base_url.as_str()
    }

    async fn search(&self, query: &str, cancellation: CancellationToken) -> Result<SearchReport> {
        if query.trim().is_empty() {
            return Err(AxiomError::Tool("search query cannot be empty".into()));
        }
        if query.len() > MAX_SEARCH_QUERY_BYTES {
            return Err(AxiomError::Tool(format!(
                "search query exceeds {MAX_SEARCH_QUERY_BYTES} bytes"
            )));
        }
        let endpoint = self
            .base_url
            .join("search")
            .map_err(|error| AxiomError::Provider(error.to_string()))?;
        let permit = tokio::select! {
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            permit = self.concurrency.acquire() => permit.map_err(|_| AxiomError::Provider("SearXNG request queue closed".into()))?,
        };
        let request = self.client.get(endpoint).query(&[
            ("q", query),
            ("format", "json"),
            ("safesearch", "1"),
        ]);
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            response = request.send() => response.map_err(|error| AxiomError::Provider(format!("SearXNG request failed: {error}")))?,
        };
        if !response.status().is_success() {
            return Err(AxiomError::Provider(format!(
                "SearXNG returned HTTP {}",
                response.status()
            )));
        }
        let payload = read_bounded_response(
            response,
            MAX_SEARCH_RESPONSE_BYTES,
            &cancellation,
            "SearXNG response",
        )
        .await?;
        let payload: SearxResponse = serde_json::from_slice(&payload)
            .map_err(|error| AxiomError::Provider(format!("invalid SearXNG JSON: {error}")))?;
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for raw in payload.results {
            if results.len() >= self.max_results {
                break;
            }
            let Ok(mut url) = Url::parse(&raw.url) else {
                continue;
            };
            if !is_safe_result_url(&url) {
                continue;
            }
            url.set_fragment(None);
            if url.port() == url.port_or_known_default() {
                let _ = url.set_port(None);
            }
            if !seen.insert(url.to_string()) {
                continue;
            }
            results.push(SearchResult {
                title: bounded_external_text(&raw.title, 512),
                url,
                snippet: bounded_external_text(raw.content.as_deref().unwrap_or_default(), 4096),
                source: bounded_external_text(raw.engine.as_deref().unwrap_or("searxng"), 128),
                score: raw.score,
            });
        }
        drop(permit);
        let mut warnings = payload
            .unresponsive_engines
            .into_iter()
            .take(20)
            .map(|failure| {
                bounded_external_text(
                    &format!(
                        "SearXNG engine unavailable: {}",
                        format_engine_failure(&failure)
                    ),
                    512,
                )
            })
            .collect::<Vec<_>>();
        if results.is_empty() && warnings.is_empty() {
            warnings.push("configured SearXNG service returned no results".into());
        }
        Ok(SearchReport { results, warnings })
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

#[derive(Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
    #[serde(default)]
    unresponsive_engines: Vec<serde_json::Value>,
}

fn format_engine_failure(value: &serde_json::Value) -> String {
    value
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(" — ")
        })
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "unknown failure".into())
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

#[derive(Deserialize)]
struct SearxResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    score: Option<f64>,
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
    async fn search_query_and_raw_response_are_bounded_before_json_parsing() {
        let unavailable =
            SearxngClient::new("http://127.0.0.1:9", Duration::from_secs(1), 10).expect("client");
        let oversized_query = "q".repeat(MAX_SEARCH_QUERY_BYTES + 1);
        let error = unavailable
            .search(&oversized_query, CancellationToken::new())
            .await
            .expect_err("oversized query must fail before the request");
        assert!(error.to_string().contains("query exceeds"));

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
        let client = SearxngClient::new(&format!("http://{address}"), Duration::from_secs(5), 10)
            .expect("client");
        let error = client
            .search("bounded response", CancellationToken::new())
            .await
            .expect_err("oversized raw response must fail");
        assert!(error.to_string().contains("response exceeds"));
        server.abort();
    }
}

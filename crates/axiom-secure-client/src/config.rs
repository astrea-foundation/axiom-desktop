use std::{
    net::IpAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use url::Url;

use crate::{Result, SecureClientError};

#[derive(Clone, Debug)]
pub struct SecureClientConfig {
    // Shared by this runtime's client clones, never serialized or written to disk.
    accept_outdated_near_tcb: Arc<AtomicBool>,
    pub relay_base_url: Url,
    pub request_timeout: Duration,
    pub attestation_timeout: Duration,
    /// Last-known-good model metadata is for discovery only. Establishment
    /// still performs fresh endpoint validation and attestation.
    pub catalog_max_stale: Duration,
    pub verified_session_ttl: Duration,
    /// Optional shared rollback journal for signed trust-policy updates.
    pub trust_policy_cache_path: Option<PathBuf>,
    pub trust_policy_refresh_interval: Duration,
    pub limits: SecureClientLimits,
    pub endpoint_policy: EndpointPolicy,
    #[cfg(feature = "test-fixture")]
    test_fixture: bool,
}

impl SecureClientConfig {
    pub fn new(relay_base_url: &str) -> Result<Self> {
        let relay_base_url = Url::parse(relay_base_url)
            .map_err(|_| SecureClientError::configuration("invalid relay URL"))?;
        if relay_base_url.scheme() != "https" {
            return Err(SecureClientError::configuration(
                "production relay URL must use HTTPS",
            ));
        }
        Ok(Self {
            accept_outdated_near_tcb: Arc::new(AtomicBool::new(false)),
            relay_base_url,
            request_timeout: Duration::from_secs(120),
            attestation_timeout: Duration::from_secs(60),
            catalog_max_stale: Duration::from_secs(30 * 60),
            verified_session_ttl: Duration::from_secs(240),
            trust_policy_cache_path: None,
            trust_policy_refresh_interval: Duration::from_secs(300),
            limits: SecureClientLimits::default(),
            endpoint_policy: EndpointPolicy::default(),
            #[cfg(feature = "test-fixture")]
            test_fixture: false,
        })
    }

    /// Construct configuration for the deterministic local encrypted fixture.
    ///
    /// This constructor is not compiled into normal builds. The caller must
    /// still perform a separate runtime opt-in so a test binary cannot be
    /// selected accidentally.
    #[cfg(feature = "test-fixture")]
    pub fn new_test_fixture(relay_base_url: &str) -> Result<Self> {
        let relay_base_url = Url::parse(relay_base_url)
            .map_err(|_| SecureClientError::configuration("invalid fixture relay URL"))?;
        if !matches!(relay_base_url.scheme(), "http" | "https") {
            return Err(SecureClientError::configuration(
                "fixture relay URL must use HTTP or HTTPS",
            ));
        }
        Ok(Self {
            accept_outdated_near_tcb: Arc::new(AtomicBool::new(false)),
            relay_base_url,
            request_timeout: Duration::from_secs(15),
            attestation_timeout: Duration::from_secs(10),
            catalog_max_stale: Duration::from_secs(60),
            verified_session_ttl: Duration::from_secs(30),
            trust_policy_cache_path: None,
            trust_policy_refresh_interval: Duration::from_secs(30),
            limits: SecureClientLimits::default(),
            endpoint_policy: EndpointPolicy {
                require_https: false,
                reject_local_addresses: false,
                allow_redirects: false,
            },
            test_fixture: true,
        })
    }

    #[cfg(feature = "test-fixture")]
    pub(crate) const fn uses_test_fixture(&self) -> bool {
        self.test_fixture
    }

    /// Explicit local-user consent for missing Intel updates, for this runtime only.
    /// This cannot relax any cryptographic, identity, freshness or GPU check.
    pub fn accept_outdated_tee_for_provider(&self, provider: &str) -> Result<()> {
        if provider != "near" {
            return Err(SecureClientError::configuration(
                "this provider has no supported outdated-TEE exception",
            ));
        }
        self.accept_outdated_near_tcb.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn accepts_outdated_tcb(&self, provider: &str) -> bool {
        provider == "near" && self.accept_outdated_near_tcb.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug)]
pub struct SecureClientLimits {
    pub model_catalog_bytes: usize,
    pub trust_policy_bytes: usize,
    pub attestation_report_bytes: usize,
    pub attestation_quote_bytes: usize,
    pub nvidia_evidence_bytes: usize,
    pub nvidia_jwt_bytes: usize,
    pub nvidia_jwks_bytes: usize,
    pub intel_collateral_bytes: usize,
    pub relay_error_bytes: usize,
    pub relay_response_bytes: usize,
    pub relay_sse_event_bytes: usize,
    pub relay_stream_bytes: usize,
    pub serialized_relay_request_bytes: usize,
}

impl Default for SecureClientLimits {
    fn default() -> Self {
        Self {
            model_catalog_bytes: 4 * 1024 * 1024,
            trust_policy_bytes: 128 * 1024,
            // Bounded provider evidence, independently verified on this device.
            attestation_report_bytes: 16 * 1024 * 1024,
            attestation_quote_bytes: 256 * 1024,
            nvidia_evidence_bytes: 1024 * 1024,
            nvidia_jwt_bytes: 256 * 1024,
            nvidia_jwks_bytes: 256 * 1024,
            intel_collateral_bytes: 2 * 1024 * 1024,
            relay_error_bytes: 4 * 1024,
            // A completion receipt contains the bounded 32 MiB provider
            // encrypted response again as base64. Leave room for that proof and its
            // surrounding JSON without relaxing the total stream bound.
            // Non-stream responses carry both the encrypted provider JSON fields
            // and a base64 copy of those same signed bytes.
            relay_response_bytes: 96 * 1024 * 1024,
            relay_sse_event_bytes: 48 * 1024 * 1024,
            relay_stream_bytes: 128 * 1024 * 1024,
            serialized_relay_request_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EndpointPolicy {
    pub require_https: bool,
    pub reject_local_addresses: bool,
    pub allow_redirects: bool,
}

impl Default for EndpointPolicy {
    fn default() -> Self {
        Self {
            require_https: true,
            reject_local_addresses: true,
            allow_redirects: false,
        }
    }
}

impl EndpointPolicy {
    pub fn validate_provider_url(&self, value: &str) -> Result<Url> {
        let url =
            Url::parse(value).map_err(|_| SecureClientError::catalog("provider URL is invalid"))?;
        if self.require_https && url.scheme() != "https" {
            return Err(SecureClientError::catalog("provider URL must use HTTPS"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(SecureClientError::catalog(
                "provider URL must not include credentials",
            ));
        }
        if self.reject_local_addresses {
            if matches!(url.host_str(), Some("localhost")) {
                return Err(SecureClientError::catalog(
                    "provider URL must not target localhost",
                ));
            }
            if let Ok(address) = url.host_str().unwrap_or_default().parse::<IpAddr>()
                && !is_public_address(address)
            {
                return Err(SecureClientError::catalog(
                    "provider URL must use a public address",
                ));
            }
        }
        Ok(url)
    }
}

pub(crate) fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            !(address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_documentation()
                || address.is_unspecified()
                || address.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || octets[0] >= 240)
        }
        IpAddr::V6(address) => {
            !(address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_multicast()
                || matches!(address.segments(), [0x2001, 0x0db8, ..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EndpointPolicy, SecureClientConfig};
    use crate::{config::SecureClientLimits, http::bounded_body};
    use axiom_inference::ProviderFailureKind;

    #[test]
    fn production_config_and_provider_urls_require_https() {
        assert!(SecureClientConfig::new("http://api.axiom.stream").is_err());
        assert!(SecureClientConfig::new("https://api.axiom.stream").is_ok());

        let policy = EndpointPolicy::default();
        assert!(
            policy
                .validate_provider_url("https://model.completions.near.ai/v1")
                .is_ok()
        );
        for rejected in [
            "http://model.example/v1",
            "https://localhost/v1",
            "https://127.0.0.1/v1",
            "https://169.254.169.254/latest",
            "https://100.64.0.1/v1",
            "https://198.18.0.1/v1",
            "https://224.0.0.1/v1",
            "https://user:password@example.com/v1",
        ] {
            assert!(
                policy.validate_provider_url(rejected).is_err(),
                "{rejected}"
            );
        }
    }

    #[tokio::test]
    async fn attestation_report_limit_accepts_the_boundary_and_rejects_the_next_byte() {
        let limit = SecureClientLimits::default().attestation_report_bytes;
        assert_eq!(limit, 16 * 1024 * 1024);

        let exact = reqwest::Response::from(
            http::Response::builder()
                .body(vec![0_u8; limit])
                .expect("exact-limit response"),
        );
        assert_eq!(
            bounded_body(exact, limit)
                .await
                .expect("exact-limit body")
                .len(),
            limit
        );

        let oversized = reqwest::Response::from(
            http::Response::builder()
                .body(vec![0_u8; limit + 1])
                .expect("oversized response"),
        );
        let error = bounded_body(oversized, limit)
            .await
            .expect_err("limit plus one must fail closed");
        assert_eq!(error.kind(), ProviderFailureKind::SessionEstablishment);
    }
}

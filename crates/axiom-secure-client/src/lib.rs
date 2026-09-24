//! Fail-closed, provider-modular secure inference for Axiom applications.
//!
//! Applications receive only fully established [`VerifiedSession`] values.
//! Provider-specific attestation and E2EE remain behind [`SecureProvider`].

mod catalog;
mod client;
mod config;
mod error;
mod http;
mod lease;
mod provider;
mod providers;
mod relay;
mod security;

pub use client::SecureClient;
pub use config::{EndpointPolicy, SecureClientConfig, SecureClientLimits};
pub use error::{Result, SecureClientError, SecureErrorCode};
pub use provider::{ProviderRegistry, SecureProvider, VerifiedSession};
pub use security::{EvidenceCheck, EvidenceClaim, SecurityEvidence, SecurityState, TrustPolicy};

/// Cryptographic helpers for the deterministic process-level fixture.
///
/// This module is absent from production builds. It exists so integration
/// tests can act as an independent encrypted provider without duplicating the
/// protocol implementation in the proxy crate.
#[cfg(feature = "test-fixture")]
pub mod test_support;

use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};

/// An upstream credential whose debug representation is always redacted.
#[derive(Clone)]
pub struct ApiCredential(Arc<SecretString>);

impl ApiCredential {
    #[must_use]
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(Arc::new(SecretString::new(value.into())))
    }

    #[must_use]
    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.expose().is_empty()
    }
}

impl std::fmt::Debug for ApiCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ApiCredential([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::ApiCredential;

    #[test]
    fn credential_debug_is_redacted() {
        let credential = ApiCredential::new("axm_private_value");
        assert!(!credential.is_empty());
        let debug = format!("{credential:?}");
        assert!(!debug.contains("axm_private_value"));
        assert!(debug.contains("REDACTED"));
    }
}

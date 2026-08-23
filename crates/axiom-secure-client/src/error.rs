use axiom_inference::ProviderFailureKind;

pub type Result<T> = std::result::Result<T, SecureClientError>;

#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {safe_detail}")]
pub struct SecureClientError {
    kind: ProviderFailureKind,
    safe_detail: &'static str,
}

impl SecureClientError {
    #[must_use]
    pub const fn new(kind: ProviderFailureKind, safe_detail: &'static str) -> Self {
        Self { kind, safe_detail }
    }

    #[must_use]
    pub const fn kind(&self) -> ProviderFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn safe_detail(&self) -> &'static str {
        self.safe_detail
    }

    pub(crate) const fn configuration(detail: &'static str) -> Self {
        Self::new(ProviderFailureKind::Configuration, detail)
    }

    pub(crate) const fn catalog(detail: &'static str) -> Self {
        Self::new(ProviderFailureKind::InvalidResponse, detail)
    }

    pub(crate) const fn capability(detail: &'static str) -> Self {
        Self::new(ProviderFailureKind::CapabilityMismatch, detail)
    }

    pub(crate) const fn cancelled() -> Self {
        Self::new(ProviderFailureKind::Cancelled, "operation cancelled")
    }

    pub(crate) const fn attestation(detail: &'static str) -> Self {
        Self::new(ProviderFailureKind::AttestationRejected, detail)
    }

    pub(crate) const fn session(detail: &'static str) -> Self {
        Self::new(ProviderFailureKind::SessionEstablishment, detail)
    }
}

use axiom_inference::ProviderFailureKind;

pub type Result<T> = std::result::Result<T, SecureClientError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SecureErrorCode {
    Configuration,
    Authentication,
    InsufficientCredit,
    RateLimited,
    ModelUnavailable,
    CapabilityMismatch,
    AttestationUnavailable,
    AttestationRejected,
    OutdatedTee,
    AttestationExpired,
    AttestationKeyChanged,
    ProtocolUnsupported,
    SessionNotAccepted,
    ReceiptUnavailable,
    InvalidRequest,
    InvalidResponse,
    Cancelled,
    Transient,
    StreamTimeout,
    ResponseTooLarge,
    ProviderDisconnected,
}

impl SecureErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "CONFIGURATION_ERROR",
            Self::Authentication => "AUTHENTICATION_FAILED",
            Self::InsufficientCredit => "INSUFFICIENT_CREDIT",
            Self::RateLimited => "RATE_LIMITED",
            Self::ModelUnavailable => "MODEL_UNAVAILABLE",
            Self::CapabilityMismatch => "CAPABILITY_MISMATCH",
            Self::AttestationUnavailable => "ATTESTATION_UNAVAILABLE",
            Self::AttestationRejected => "ATTESTATION_REJECTED",
            Self::OutdatedTee => "PROVIDER_TDX_OUT_OF_DATE",
            Self::AttestationExpired => "ATTESTATION_EXPIRED",
            Self::AttestationKeyChanged => "ATTESTATION_KEY_CHANGED",
            Self::ProtocolUnsupported => "PROTOCOL_UNSUPPORTED",
            Self::SessionNotAccepted => "SESSION_NOT_ACCEPTED",
            Self::ReceiptUnavailable => "RECEIPT_UNAVAILABLE",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::InvalidResponse => "INVALID_RESPONSE",
            Self::Cancelled => "CANCELLED",
            Self::Transient => "TRANSIENT_FAILURE",
            Self::StreamTimeout => "STREAM_TIMEOUT",
            Self::ResponseTooLarge => "RESPONSE_TOO_LARGE",
            Self::ProviderDisconnected => "PROVIDER_DISCONNECTED",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {safe_detail}")]
pub struct SecureClientError {
    kind: ProviderFailureKind,
    code: SecureErrorCode,
    safe_detail: &'static str,
    retryable: bool,
}

impl SecureClientError {
    #[must_use]
    pub const fn new(kind: ProviderFailureKind, safe_detail: &'static str) -> Self {
        Self {
            kind,
            code: default_code(kind),
            safe_detail,
            retryable: default_retryable(kind),
        }
    }

    #[must_use]
    pub const fn with_code(
        kind: ProviderFailureKind,
        code: SecureErrorCode,
        safe_detail: &'static str,
        retryable: bool,
    ) -> Self {
        Self {
            kind,
            code,
            safe_detail,
            retryable,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> ProviderFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn safe_detail(&self) -> &'static str {
        self.safe_detail
    }

    #[must_use]
    pub const fn code(&self) -> SecureErrorCode {
        self.code
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    #[must_use]
    pub const fn requires_reattest_and_reencrypt(&self) -> bool {
        matches!(self.code, SecureErrorCode::AttestationKeyChanged)
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

const fn default_code(kind: ProviderFailureKind) -> SecureErrorCode {
    match kind {
        ProviderFailureKind::Configuration => SecureErrorCode::Configuration,
        ProviderFailureKind::LocalAuthentication | ProviderFailureKind::Authentication => {
            SecureErrorCode::Authentication
        }
        ProviderFailureKind::InsufficientCredit => SecureErrorCode::InsufficientCredit,
        ProviderFailureKind::RateLimited => SecureErrorCode::RateLimited,
        ProviderFailureKind::ModelUnavailable => SecureErrorCode::ModelUnavailable,
        ProviderFailureKind::CapabilityMismatch => SecureErrorCode::CapabilityMismatch,
        ProviderFailureKind::AttestationUnavailable | ProviderFailureKind::SessionEstablishment => {
            SecureErrorCode::AttestationUnavailable
        }
        ProviderFailureKind::AttestationRejected => SecureErrorCode::AttestationRejected,
        ProviderFailureKind::Encryption | ProviderFailureKind::InvalidRequest => {
            SecureErrorCode::InvalidRequest
        }
        ProviderFailureKind::Decryption | ProviderFailureKind::InvalidResponse => {
            SecureErrorCode::InvalidResponse
        }
        ProviderFailureKind::Transient => SecureErrorCode::Transient,
        ProviderFailureKind::Cancelled => SecureErrorCode::Cancelled,
    }
}

const fn default_retryable(kind: ProviderFailureKind) -> bool {
    matches!(
        kind,
        ProviderFailureKind::RateLimited
            | ProviderFailureKind::AttestationUnavailable
            | ProviderFailureKind::SessionEstablishment
            | ProviderFailureKind::Transient
    )
}

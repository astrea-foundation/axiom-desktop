//! Public attestation reports. Verification remains in secure provider adapters.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityState {
    Unverified,
    Verifying,
    Verified,
    Degraded,
    Rejected,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceCheck {
    pub id: String,
    pub label: String,
    pub status: String,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceClaim {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEvidence {
    pub state: SecurityState,
    pub provider_id: String,
    pub model_id: String,
    pub attestation_protocol: String,
    pub e2ee_protocol: String,
    pub e2ee_encryption_version: u16,
    pub trust_policy_version: String,
    pub verified_at_unix_seconds: u64,
    #[serde(default)]
    pub attestation_generation: Option<u64>,
    #[serde(default)]
    pub hard_expires_at_unix_seconds: Option<u64>,
    pub model_key_fingerprint: String,
    pub tls_spki_fingerprint: Option<String>,
    pub checks: Vec<EvidenceCheck>,
    pub provider_claims: Vec<EvidenceClaim>,
    /// Optional retained workload/provenance document. Its format and verified
    /// bindings are provider-specific: for example, a quote-bound manifest or
    /// a verified router report. Absence does not mean verification failed;
    /// checks and provider claims may contain all retained evidence. This is
    /// public data, but callers must sanitize it before terminal display.
    #[serde(default)]
    pub workload_manifest: Option<String>,
}

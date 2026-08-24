use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustPolicy {
    pub version: String,
    pub accepted_intel_statuses: Vec<String>,
    pub require_nvidia_verified: bool,
    pub require_model_binding: bool,
    pub require_live_tls_binding: bool,
    pub approved_workload_measurements: Vec<String>,
}

impl Default for TrustPolicy {
    fn default() -> Self {
        Self {
            version: "near-v2-trust-v1".into(),
            accepted_intel_statuses: vec!["UpToDate".into(), "SWHardeningNeeded".into()],
            require_nvidia_verified: true,
            require_model_binding: true,
            require_live_tls_binding: true,
            approved_workload_measurements: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityState {
    Unverified,
    Verifying,
    Verified,
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
    pub model_key_fingerprint: String,
    pub tls_spki_fingerprint: Option<String>,
    pub checks: Vec<EvidenceCheck>,
    pub provider_claims: Vec<EvidenceClaim>,
    /// Exact provider-supplied workload manifest whose digest was verified
    /// against the hardware quote. This is public attestation evidence, not a
    /// secret, but callers should still sanitize it before terminal display.
    #[serde(default)]
    pub workload_manifest: Option<String>,
    /// Whether this verification policy had an explicit workload allowlist.
    /// This distinguishes "not evaluated" from "evaluated and rejected" in
    /// user interfaces.
    #[serde(default)]
    pub workload_identity_allowlist_configured: bool,
    pub approved_workload_identity: bool,
}

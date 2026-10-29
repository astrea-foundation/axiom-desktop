use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{Result, SecureClientError};

const POLICY_SIGNING_KEY_HEX: &str =
    "ba3609d6b3164683e1fa9ff70471b893c165cd6f59a4b6c19e5e6b4b89ccf766";
const BUNDLED_POLICY: &str = include_str!("../policy/axiom-production-v2.json");

#[cfg(test)]
pub(crate) const fn bundled_signed_policy() -> &'static str {
    BUNDLED_POLICY
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustPolicy {
    pub sequence: u64,
    pub version: String,
    pub expires_at_unix_seconds: u64,
    pub accepted_intel_statuses: Vec<String>,
    pub require_nvidia_verified: bool,
    pub require_model_binding: bool,
    pub require_live_tls_binding: bool,
    pub nvidia_anchor_sha256: Vec<String>,
}

impl Default for TrustPolicy {
    fn default() -> Self {
        Self {
            sequence: 0,
            version: "development".into(),
            expires_at_unix_seconds: u64::MAX,
            // Keep the browser, backend, CLI, and desktop on one TCB policy.
            // A runtime user exception for OutOfDate is separate from signed policy.
            accepted_intel_statuses: vec!["UpToDate".into()],
            require_nvidia_verified: true,
            require_model_binding: true,
            require_live_tls_binding: true,
            nvidia_anchor_sha256: vec![
                "2df8907cf4d6c277b855d407ec178a649930a8e73f7294e8fce66e6ee27c5175".into(),
            ],
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedPolicyDocument {
    schema_version: u16,
    sequence: u64,
    version: String,
    issued_at_unix_seconds: u64,
    not_before_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    accepted_intel_statuses: Vec<String>,
    require_nvidia_verified: bool,
    require_model_binding: bool,
    require_live_tls_binding: bool,
    nvidia_anchor_sha256: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedPolicyEnvelope {
    policy: SignedPolicyDocument,
    signature: String,
}

impl TrustPolicy {
    pub fn production() -> Result<Self> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        Self::from_signed_json(BUNDLED_POLICY, 0, now)
    }

    pub fn from_signed_json(input: &str, minimum_sequence: u64, now: u64) -> Result<Self> {
        Self::from_signed_json_inner(input, minimum_sequence, now, false)
    }

    /// Verify an expired cached policy only to recover its monotonic sequence.
    /// Callers must still refuse to use the returned rules for attestation.
    pub(crate) fn from_signed_json_for_rollback(
        input: &str,
        minimum_sequence: u64,
        now: u64,
    ) -> Result<Self> {
        Self::from_signed_json_inner(input, minimum_sequence, now, true)
    }

    fn from_signed_json_inner(
        input: &str,
        minimum_sequence: u64,
        now: u64,
        permit_expired_for_rollback: bool,
    ) -> Result<Self> {
        let envelope: SignedPolicyEnvelope = serde_json::from_str(input)
            .map_err(|_| SecureClientError::configuration("trust policy schema is invalid"))?;
        let public_key = hex::decode(POLICY_SIGNING_KEY_HEX)
            .map_err(|_| SecureClientError::configuration("trust policy key is invalid"))?;
        let public_key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| SecureClientError::configuration("trust policy key is invalid"))?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| SecureClientError::configuration("trust policy key is invalid"))?;
        let signature = hex::decode(&envelope.signature)
            .ok()
            .and_then(|value| Signature::from_slice(&value).ok())
            .ok_or_else(|| SecureClientError::configuration("trust policy signature is invalid"))?;
        let canonical = serde_json::to_vec(&envelope.policy)
            .map_err(|_| SecureClientError::configuration("trust policy is invalid"))?;
        verifying_key
            .verify_strict(&canonical, &signature)
            .map_err(|_| SecureClientError::configuration("trust policy signature is invalid"))?;
        let document = envelope.policy;
        if document.schema_version != 2
            || document.sequence < minimum_sequence
            || document.issued_at_unix_seconds > now
            || document.not_before_unix_seconds > now
            || (!permit_expired_for_rollback && document.expires_at_unix_seconds <= now)
            || document.not_before_unix_seconds > document.expires_at_unix_seconds
            || document.accepted_intel_statuses != ["UpToDate"]
            || !document.require_nvidia_verified
            || !document.require_model_binding
            || !document.require_live_tls_binding
            || document.nvidia_anchor_sha256.is_empty()
            || document.nvidia_anchor_sha256.iter().any(|fingerprint| {
                fingerprint.len() != 64
                    || !fingerprint
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            || document.version.trim().is_empty()
        {
            return Err(SecureClientError::configuration(
                "trust policy is expired, rolled back, or weakens mandatory checks",
            ));
        }
        Ok(Self {
            sequence: document.sequence,
            version: document.version,
            expires_at_unix_seconds: document.expires_at_unix_seconds,
            accepted_intel_statuses: document.accepted_intel_statuses,
            require_nvidia_verified: document.require_nvidia_verified,
            require_model_binding: document.require_model_binding,
            require_live_tls_binding: document.require_live_tls_binding,
            nvidia_anchor_sha256: document.nvidia_anchor_sha256,
        })
    }

    #[must_use]
    pub fn is_valid_at(&self, now: u64) -> bool {
        self.expires_at_unix_seconds > now
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_policy_is_signed_time_bounded_and_rollback_protected() {
        let policy = TrustPolicy::from_signed_json(BUNDLED_POLICY, 3, 1_800_000_000).unwrap();
        assert_eq!(policy.sequence, 3);
        assert_eq!(policy.accepted_intel_statuses, ["UpToDate"]);
        assert!(TrustPolicy::from_signed_json(BUNDLED_POLICY, 4, 1_800_000_000).is_err());
        assert!(TrustPolicy::from_signed_json(BUNDLED_POLICY, 0, u64::MAX).is_err());
        let expired =
            TrustPolicy::from_signed_json_for_rollback(BUNDLED_POLICY, 3, u64::MAX).unwrap();
        assert!(!expired.is_valid_at(u64::MAX));

        let tampered = BUNDLED_POLICY.replace("\"sequence\": 3", "\"sequence\": 9");
        assert!(TrustPolicy::from_signed_json(&tampered, 0, 1_800_000_000).is_err());
    }
}

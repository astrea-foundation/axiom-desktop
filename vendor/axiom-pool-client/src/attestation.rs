use anyhow::{Context, Result, ensure};
use axiom_pool_protocol::{
    PROTOCOL, strict,
    types::{WorkerEvidence, WorkerIdentity},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierConfig {
    pub binary: PathBuf,
    pub binary_sha256: String,
    pub publisher_key: String,
    pub minimum_policy_sequence: u64,
    #[cfg(feature = "test-fixture")]
    #[serde(skip)]
    fixture_key: Option<ed25519_dalek::VerifyingKey>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chutes_metadata_cannot_be_promoted_to_a_verified_pool_worker() {
        let worker = WorkerIdentity {
            worker_id: "11".repeat(16),
            boot_id: "22".repeat(32),
            encryption_key: "33".repeat(32),
            identity_key: "44".repeat(32),
            model_manifest: "55".repeat(32),
            logical_origin: "https://api.chutes.ai".into(),
            evidence_profile: "chutes-gpu-v1".into(),
            deployment_digest: "66".repeat(32),
        };
        worker.validate(false).unwrap();
        let config: VerifierConfig = serde_json::from_value(json!({
            "binary": "/does-not-exist",
            "binary_sha256": "77".repeat(32),
            "publisher_key": "88".repeat(32),
            "minimum_policy_sequence": 1
        }))
        .unwrap();
        let empty = axiom_pool_protocol::SignedDocument {
            payload: String::new(),
            signature: String::new(),
        };
        let evidence = WorkerEvidence {
            protocol: PROTOCOL.into(),
            challenge: "99".repeat(32),
            identity: empty.clone(),
            document: json!({"verified": true, "workload_binding_verified": true}),
            manifest: empty.clone(),
            policy: empty,
        };
        let result = config
            .verify(
                &reqwest::Client::new(),
                evidence,
                &"99".repeat(32),
                &worker,
                1,
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("not qualified"));
    }
}
/// No public constructor or Deserialize: keys enter through the verifier only.
pub struct VerifiedWorker {
    worker: WorkerIdentity,
    expires_at: u64,
    policy_sequence: u64,
    generation: u64,
    capabilities: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Appraisal {
    worker: WorkerIdentity,
    expires_at: u64,
    policy_sequence: u64,
    generation: u64,
    #[serde(default)]
    capabilities: Vec<String>,
}
impl VerifiedWorker {
    pub fn identity(&self) -> &WorkerIdentity {
        &self.worker
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
    pub fn policy_sequence(&self) -> u64 {
        self.policy_sequence
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|value| value == capability)
    }
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }
}
pub async fn bounded_json(mut response: reqwest::Response, limit: usize) -> Result<Value> {
    let mut raw = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            raw.len() + chunk.len() <= limit,
            "verification/metadata response limit"
        );
        raw.extend_from_slice(&chunk);
    }
    strict::decode(&raw)
}
impl VerifierConfig {
    #[cfg(feature = "test-fixture")]
    pub fn fixture(identity_key: &str) -> Result<Self> {
        ensure!(
            std::env::var("AXIOM_POOL_TEST_ONLY").as_deref() == Ok("1"),
            "explicit test process required"
        );
        let key = ed25519_dalek::VerifyingKey::from_bytes(
            &axiom_pool_protocol::hex_bytes(identity_key, 32)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("key length"))?,
        )?;
        Ok(Self {
            binary: PathBuf::new(),
            binary_sha256: String::new(),
            publisher_key: String::new(),
            minimum_policy_sequence: 0,
            fixture_key: Some(key),
        })
    }
    pub async fn verify(
        &self,
        http: &reqwest::Client,
        evidence: WorkerEvidence,
        challenge: &str,
        expected: &WorkerIdentity,
        minimum_sequence: u64,
        cancel: &CancellationToken,
    ) -> Result<VerifiedWorker> {
        #[cfg(feature = "test-fixture")]
        if let Some(key) = &self.fixture_key {
            ensure!(
                std::env::var("AXIOM_POOL_TEST_ONLY").as_deref() == Ok("1"),
                "explicit test process required"
            );
            expected.validate(true)?;
            let statement: axiom_pool_protocol::types::IdentityStatement =
                axiom_pool_protocol::verify(
                    &evidence.identity,
                    key,
                    axiom_pool_protocol::IDENTITY_DOMAIN,
                )?;
            ensure!(
                expected.evidence_profile == "test-fixture-only"
                    && expected.identity_key == hex::encode(key.as_bytes())
                    && statement.worker == *expected
                    && statement.challenge == challenge
                    && evidence.challenge == challenge
                    && evidence.document == json!({"test_fixture_only":true})
                    && statement.expires_at > super::now()
                    && statement.expires_at <= super::now() + 240,
                "invalid test fixture proof"
            );
            return Ok(VerifiedWorker {
                worker: expected.clone(),
                expires_at: statement.expires_at,
                policy_sequence: 0,
                generation: 0,
                capabilities: Vec::new(),
            });
        }
        expected.validate(false)?;
        // Structural recognition is not a trust decision. Chutes quotes bind
        // its ML-KEM key, not this worker's HPKE/identity keys or signed model
        // release. Never feed that evidence to the Tinfoil verifier or treat
        // a provider's `verified` flag as an Axiom workload appraisal.
        ensure!(
            matches!(
                expected.evidence_profile.as_str(),
                "tinfoil-gpu-v3" | "axiom-snp-nvidia-v1"
            ),
            "Chutes workload/key attestation is not qualified for pool admission"
        );
        ensure!(
            evidence.protocol == PROTOCOL && evidence.challenge == challenge,
            "attestation nonce/protocol mismatch"
        );
        axiom_pool_protocol::hex_bytes(challenge, 32)?;
        axiom_pool_protocol::hex_bytes(&self.publisher_key, 32)?;
        ensure!(
            self.binary.is_absolute()
                && axiom_pool_protocol::sha256(tokio::fs::read(&self.binary).await?)
                    == self.binary_sha256,
            "installed verifier digest mismatch"
        );
        let nvidia = if expected.evidence_profile == "axiom-snp-nvidia-v1" {
            // The pinned direct verifier validates AMD collateral and performs
            // strict NVIDIA v4/claims-v3 appraisal itself. Never route SNP
            // evidence through the Tinfoil/TDX parser or learn a trust root.
            Vec::<Value>::new()
        } else {
            let encoded = evidence.document["device_evidence"]
                .as_str()
                .context("missing GPU evidence")?;
            ensure!(encoded.len() <= 4 * 1024 * 1024, "GPU evidence limit");
            let devices: Value = strict::decode(&STANDARD.decode(encoded)?)?;
            let devices = devices["items"]
                .as_array()
                .context("GPU evidence items missing")?;
            ensure!(devices.len() == 1, "one confidential GPU required");
            let response = http
                .get("https://nras.attestation.nvidia.com/.well-known/jwks.json")
                .timeout(Duration::from_secs(20))
                .send()
                .await?
                .error_for_status()?;
            let jwks = bounded_json(response, 2 * 1024 * 1024).await?;
            let mut nvidia = Vec::new();
            for device in devices {
                ensure!(
                    device["kind"] == "gpu"
                        && device["vendor"] == "nvidia"
                        && device["format"] == "https://tinfoil.sh/format/nvidia-gpu-evidence/v1",
                    "unsupported GPU evidence"
                );
                let payload = match &device["evidence"] {
                    Value::String(s) => strict::decode(s.as_bytes())?,
                    other => other.clone(),
                };
                ensure!(payload["nonce"] == challenge, "GPU nonce mismatch");
                let response = http
                    .post("https://nras.attestation.nvidia.com/v3/attest/gpu")
                    .timeout(Duration::from_secs(20))
                    .json(&payload)
                    .send()
                    .await?
                    .error_for_status()?;
                let value = bounded_json(response, 2 * 1024 * 1024).await?;
                ensure!(
                    value.as_array().is_some_and(|v| v.len() == 2)
                        && value[0][0] == "JWT"
                        && value[1].as_object().is_some_and(|v| v.len() == 1),
                    "complete NVIDIA detached EAT bundle required"
                );
                let jwt = value[0][1].as_str().context("NVIDIA overall JWT missing")?;
                nvidia.push(
                    json!({"device_id":device["id"],"jwt":jwt,"jwks":jwks,"detached":value[1]}),
                );
            }
            nvidia
        };
        let current = super::now();
        let input = json!({"evidence":evidence,"context":{"challenge":challenge,"origin":expected.logical_origin,"publisher_key":self.publisher_key,"model_manifest":expected.model_manifest,"now":current,"minimum_policy_sequence":minimum_sequence.max(self.minimum_policy_sequence)},"nvidia":nvidia});
        let raw = serde_json::to_vec(&input)?;
        ensure!(raw.len() <= 8 * 1024 * 1024, "verifier input limit");
        let mut child = tokio::process::Command::new(&self.binary)
            .env_clear()
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let mut stdin = child.stdin.take().context("verifier input")?;
        stdin.write_all(&raw).await?;
        drop(stdin);
        let output = tokio::select! {_=cancel.cancelled()=>anyhow::bail!("verification cancelled"),output=tokio::time::timeout(Duration::from_secs(60),child.wait_with_output())=>output??};
        ensure!(
            output.status.success() && output.stdout.len() <= 64 * 1024,
            "worker attestation rejected"
        );
        let appraisal: Appraisal = strict::decode(&output.stdout)?;
        ensure!(
            &appraisal.worker == expected
                && appraisal.expires_at > super::now()
                && appraisal.expires_at <= current + 240
                && appraisal.policy_sequence >= minimum_sequence
                && appraisal.generation > 0,
            "worker appraisal mismatch"
        );
        Ok(VerifiedWorker {
            worker: appraisal.worker,
            expires_at: appraisal.expires_at,
            policy_sequence: appraisal.policy_sequence,
            generation: appraisal.generation,
            capabilities: appraisal.capabilities,
        })
    }
}

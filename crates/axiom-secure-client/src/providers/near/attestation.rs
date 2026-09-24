use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, bail};
use base64::{Engine, engine::general_purpose};
use dcap_qvl::{
    collateral::CollateralClient,
    http::{HttpClient, HttpResponse},
};
use p384::{
    ecdsa::{Signature, VerifyingKey, signature::Verifier},
    pkcs8::DecodePublicKey,
};
use rand_core::{OsRng, RngCore};
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, de::Error as _};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;
use x509_parser::prelude::*;

use super::crypto::Ed25519PublicKey;
use crate::{
    EndpointPolicy, EvidenceCheck, EvidenceClaim, Result, SecureClientConfig, SecureClientError,
    SecurityEvidence, SecurityState, TrustPolicy,
    http::{bounded_body, pinned_client},
};

const INTEL_PCCS: &str = "https://api.trustedservices.intel.com/";
const NVIDIA_NRAS: &str = "https://nras.attestation.nvidia.com/v3/attest/gpu";
const NVIDIA_JWKS: &str = "https://nras.attestation.nvidia.com/.well-known/jwks.json";

#[derive(Clone, Debug, Deserialize)]
struct AttestationReport {
    #[serde(default)]
    signing_algo: Option<String>,
    #[serde(default)]
    signing_public_key: Option<String>,
    #[serde(default)]
    signing_address: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    request_nonce: Option<String>,
    #[serde(default)]
    tls_fingerprint: Option<String>,
    #[serde(default)]
    tls_cert_fingerprint: Option<String>,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    intel_quote: Option<String>,
    #[serde(default, deserialize_with = "deserialize_json_object")]
    nvidia_payload: Option<Value>,
    #[serde(default)]
    app_compose: Option<String>,
    #[serde(default)]
    info: Option<Value>,
    #[serde(default)]
    tcb_info: Option<Value>,
}

impl AttestationReport {
    fn echoed_nonce(&self) -> Option<&str> {
        self.request_nonce.as_deref().or(self.nonce.as_deref())
    }

    fn tls_fingerprint(&self) -> Option<&str> {
        self.tls_cert_fingerprint
            .as_deref()
            .or(self.tls_fingerprint.as_deref())
    }

    fn model_key(&self) -> Result<Ed25519PublicKey> {
        let primary = self
            .signing_public_key
            .as_deref()
            .or(self.signing_address.as_deref())
            .ok_or_else(|| SecureClientError::attestation("attestation omitted the model key"))?;
        let key = Ed25519PublicKey::from_hex(primary)
            .map_err(|_| SecureClientError::attestation("attestation model key is invalid"))?;
        if let (Some(public), Some(address)) = (&self.signing_public_key, &self.signing_address) {
            let address_key = Ed25519PublicKey::from_hex(address).map_err(|_| {
                SecureClientError::attestation("attestation signing address is invalid")
            })?;
            if key != address_key
                || key
                    != Ed25519PublicKey::from_hex(public).map_err(|_| {
                        SecureClientError::attestation("attestation model key is invalid")
                    })?
            {
                return Err(SecureClientError::attestation(
                    "attestation model key aliases disagree",
                ));
            }
        }
        Ok(key)
    }

    fn workload_manifest(&self) -> Result<String> {
        if let Some(value) = self
            .app_compose
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return Ok(value.to_owned());
        }
        let info = self.info.as_ref().and_then(parse_json_object);
        let nested = info
            .as_ref()
            .and_then(|object| object.get("tcb_info"))
            .and_then(parse_json_object)
            .or_else(|| self.tcb_info.as_ref().and_then(parse_json_object));
        nested
            .as_ref()
            .and_then(|object| object.get("app_compose"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                SecureClientError::attestation("attestation omitted the workload manifest")
            })
    }
}

#[derive(Clone, Debug)]
struct IntelEvidence {
    status: String,
    advisory_ids: Vec<String>,
    mr_td: String,
    compose_hash: String,
}

#[derive(Clone, Debug)]
pub(crate) struct VerifiedNearAttestation {
    pub(crate) model_key: Ed25519PublicKey,
    pub(crate) response_signing_address: String,
    pub(crate) evidence: SecurityEvidence,
}

#[derive(Deserialize)]
struct WorkerSessionEnvelope {
    provider: String,
    model_id: String,
    attestation_protocol: String,
    evidence: Value,
}

fn accept_intel_status(
    status: &str,
    policy: &TrustPolicy,
    config: &SecureClientConfig,
) -> Result<bool> {
    if status == "UpToDate"
        && policy
            .accepted_intel_statuses
            .iter()
            .any(|accepted| accepted == status)
    {
        return Ok(false);
    }
    if status == "OutOfDate" {
        if config.accepts_outdated_tcb(super::PROVIDER_ID) {
            return Ok(true);
        }
        return Err(SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::AttestationRejected,
            crate::SecureErrorCode::OutdatedTee,
            "The provider's Intel TDX environment is OutOfDate and requires security updates.",
            false,
        ));
    }
    Err(SecureClientError::attestation(match status {
        "OutOfDateConfigurationNeeded" => "Intel TDX status is OutOfDateConfigurationNeeded",
        "ConfigurationNeeded" => "Intel TDX status is ConfigurationNeeded",
        "SWHardeningNeeded" => "Intel TDX status is SWHardeningNeeded",
        "ConfigurationAndSWHardeningNeeded" => {
            "Intel TDX status is ConfigurationAndSWHardeningNeeded"
        }
        "Revoked" => "Intel TDX status is Revoked",
        _ => "Intel TDX status is rejected by policy; see verification diagnostics",
    }))
}

pub(crate) async fn verify_model(
    config: &SecureClientConfig,
    credential: &crate::ApiCredential,
    model: &axiom_inference::ModelInfo,
    policy: &TrustPolicy,
    cancellation: &CancellationToken,
) -> Result<(
    VerifiedNearAttestation,
    crate::relay::dto::ProviderKeyLease,
    String,
)> {
    if cancellation.is_cancelled() {
        return Err(SecureClientError::cancelled());
    }
    config
        .endpoint_policy
        .validate_provider_url(&model.provider_base_url)?;
    let mut nonce = [0_u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let wire: WorkerSessionEnvelope = crate::relay::client::RelayClient::new(config, credential)
        .provider_attestation_report(
            &model.id,
            &hex::encode(nonce),
            super::E2EE_PROTOCOL,
            super::ENCRYPTION_VERSION,
            super::ATTESTATION_PROTOCOL,
            cancellation,
        )
        .await?;
    if wire.provider != super::PROVIDER_ID
        || wire.model_id != model.id
        || wire.attestation_protocol != super::ATTESTATION_PROTOCOL
    {
        return Err(SecureClientError::attestation(
            "NEAR worker session contract is invalid",
        ));
    }
    let worker_session_id = wire
        .evidence
        .get("worker_session_id")
        .and_then(Value::as_str)
        .filter(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or_else(|| SecureClientError::attestation("NEAR worker session ID is invalid"))?
        .to_owned();
    let lease = serde_json::from_value(wire.evidence.get("lease").cloned().unwrap_or(Value::Null))
        .map_err(|_| SecureClientError::attestation("NEAR worker session lease is invalid"))?;
    let report: AttestationReport = serde_json::from_value(normalize_report(wire.evidence)?)
        .map_err(|_| SecureClientError::attestation("NEAR worker evidence schema is invalid"))?;
    validate_report_identity(&report, &nonce, &model.upstream_model, "ed25519")?;
    let model_key = report.model_key()?;
    let tls_fingerprint = decode_hex_32(
        report
            .tls_fingerprint()
            .ok_or_else(|| SecureClientError::attestation("attestation omitted TLS binding"))?,
        "attestation TLS fingerprint is invalid",
    )?;
    let manifest = report.workload_manifest()?;
    let quote = decode_quote(
        report
            .intel_quote
            .as_deref()
            .ok_or_else(|| SecureClientError::attestation("attestation omitted Intel evidence"))?,
        config.limits.attestation_quote_bytes,
    )?;
    let nvidia_payload = report
        .nvidia_payload
        .as_ref()
        .ok_or_else(|| SecureClientError::attestation("attestation omitted NVIDIA evidence"))?;
    // The relay transports untrusted evidence. Local vendor signature, challenge,
    // workload and key checks authenticate it; a server 'verified' flag does not.
    let (intel, ()) = tokio::select! {
        () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
        result = async {
            tokio::try_join!(
                verify_intel(&quote, model_key.as_bytes(), &tls_fingerprint, &nonce, &manifest, config),
                verify_nvidia(nvidia_payload, &nonce, config, &policy.nvidia_anchor_sha256),
            )
        } => result?,
    };
    let outdated_accepted = accept_intel_status(&intel.status, policy, config).inspect_err(|_| {
        tracing::warn!(status = %intel.status, advisory_ids = ?intel.advisory_ids, "NEAR Intel TDX rejected by policy");
    })?;
    if !policy.require_nvidia_verified {
        return Err(SecureClientError::configuration(
            "near-v3 requires NVIDIA verification",
        ));
    }
    let mut evidence = build_evidence(
        &model.id,
        policy,
        model_key,
        &tls_fingerprint,
        &intel,
        manifest,
    );
    if outdated_accepted {
        evidence.state = SecurityState::Degraded;
        if let Some(check) = evidence
            .checks
            .iter_mut()
            .find(|check| check.id == "intel_tdx")
        {
            check.passed = false;
        }
        evidence.provider_claims.push(EvidenceClaim {
            name: "outdated_tcb_accepted".into(),
            value: "Accepted by user for this provider until restart".into(),
        });
    }
    evidence.checks.push(check(
        "response_signature",
        "Provider response signature",
        "ed25519-attested",
    ));
    evidence.provider_claims.push(EvidenceClaim {
        name: "response_signing_address".into(),
        value: model_key.to_hex(),
    });
    evidence.provider_claims.push(EvidenceClaim {
        name: "response_signing_key_fingerprint".into(),
        value: hex::encode(Sha256::digest(model_key.as_bytes())),
    });
    Ok((
        VerifiedNearAttestation {
            model_key,
            response_signing_address: model_key.to_hex(),
            evidence,
        },
        lease,
        worker_session_id,
    ))
}

fn normalize_report(mut value: Value) -> Result<Value> {
    let Some(top) = value.as_object_mut() else {
        return Err(SecureClientError::attestation(
            "attestation report must be an object",
        ));
    };
    let Some(attestations) = top.remove("model_attestations") else {
        return Ok(value);
    };
    let mut reports = attestations
        .as_array()
        .cloned()
        .ok_or_else(|| SecureClientError::attestation("model attestations must be an array"))?;
    if reports.len() != 1 {
        return Err(SecureClientError::attestation(
            "attestation response must select exactly one model",
        ));
    }
    let mut selected = reports.remove(0);
    let selected_object = selected.as_object_mut().ok_or_else(|| {
        SecureClientError::attestation("selected model attestation must be an object")
    })?;
    for (name, field) in top.iter() {
        selected_object
            .entry(name.clone())
            .or_insert_with(|| field.clone());
    }
    Ok(selected)
}

fn validate_report_identity(
    report: &AttestationReport,
    nonce: &[u8; 32],
    upstream_model: &str,
    expected_signing_algo: &str,
) -> Result<()> {
    if report.signing_algo.as_deref() != Some(expected_signing_algo) {
        return Err(SecureClientError::attestation(
            "attestation signing algorithm does not match the request",
        ));
    }
    let echoed_nonce = report.echoed_nonce().ok_or_else(|| {
        SecureClientError::attestation("attestation did not echo the request nonce")
    })?;
    if decode_hex_32(echoed_nonce, "attestation nonce is invalid")? != *nonce {
        return Err(SecureClientError::attestation(
            "attestation nonce does not match the challenge",
        ));
    }
    if report.model_name.as_deref() != Some(upstream_model) {
        return Err(SecureClientError::attestation(
            "attested model does not match the selected model",
        ));
    }
    decode_hex_32(
        report
            .tls_fingerprint()
            .ok_or_else(|| SecureClientError::attestation("attestation omitted TLS binding"))?,
        "attestation TLS fingerprint is invalid",
    )?;
    Ok(())
}

fn decode_quote(value: &str, limit: usize) -> Result<Vec<u8>> {
    let normalized = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    let bytes = hex::decode(normalized)
        .or_else(|_| general_purpose::STANDARD.decode(normalized))
        .map_err(|_| SecureClientError::attestation("Intel quote encoding is invalid"))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(SecureClientError::attestation(
            "Intel quote exceeds its configured bounds",
        ));
    }
    Ok(bytes)
}

async fn verify_intel(
    quote: &[u8],
    signing_material: &[u8],
    tls_fingerprint: &[u8; 32],
    nonce: &[u8; 32],
    workload_manifest: &str,
    config: &SecureClientConfig,
) -> Result<IntelEvidence> {
    let http = BoundedCollateralHttp::new(
        config.attestation_timeout,
        config.limits.intel_collateral_bytes,
    );
    let client = CollateralClient::<dcap_qvl::configs::DefaultConfig, _>::new(http, INTEL_PCCS);
    let verified = client.fetch_and_verify(quote).await.map_err(|_| {
        SecureClientError::attestation("Intel TDX quote or collateral verification failed")
    })?;
    let td = verified.report.as_td10().ok_or_else(|| {
        SecureClientError::attestation("Intel quote does not contain a supported TDX report")
    })?;
    verify_report_data(&td.report_data, signing_material, tls_fingerprint, nonce)?;

    let compose_hash = Sha256::digest(workload_manifest.as_bytes());
    let mut expected_config_id = [0_u8; 48];
    expected_config_id[0] = 1;
    expected_config_id[1..33].copy_from_slice(&compose_hash);
    if td.mr_config_id != expected_config_id {
        return Err(SecureClientError::attestation(
            "Intel quote does not bind the workload manifest",
        ));
    }

    Ok(IntelEvidence {
        status: verified.status,
        advisory_ids: verified.advisory_ids,
        mr_td: hex::encode(td.mr_td),
        compose_hash: hex::encode(compose_hash),
    })
}

fn verify_report_data(
    report_data: &[u8],
    signing_material: &[u8],
    tls_fingerprint: &[u8; 32],
    nonce: &[u8; 32],
) -> Result<()> {
    if report_data.len() != 64 {
        return Err(SecureClientError::attestation(
            "Intel report data must be exactly 64 bytes",
        ));
    }
    if signing_material.is_empty() {
        return Err(SecureClientError::attestation(
            "attested signing material is invalid",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(signing_material);
    hasher.update(tls_fingerprint);
    let expected_binding = hasher.finalize();
    if report_data[..32] != expected_binding[..] || report_data[32..] != nonce[..] {
        return Err(SecureClientError::attestation(
            "Intel report data does not bind the signing key, TLS key, and nonce",
        ));
    }
    Ok(())
}

async fn verify_nvidia(
    payload: &Value,
    expected_nonce: &[u8; 32],
    config: &SecureClientConfig,
    approved_anchor_sha256: &[String],
) -> Result<()> {
    let payload_bytes = serde_json::to_vec(payload)
        .map_err(|_| SecureClientError::attestation("NVIDIA evidence is invalid"))?;
    if payload_bytes.len() > config.limits.nvidia_evidence_bytes {
        return Err(SecureClientError::attestation(
            "NVIDIA evidence exceeds its configured bound",
        ));
    }
    let expected_nonce_hex = hex::encode(expected_nonce);
    if payload
        .get("nonce")
        .and_then(Value::as_str)
        .is_none_or(|nonce| !nonce.eq_ignore_ascii_case(&expected_nonce_hex))
    {
        return Err(SecureClientError::attestation(
            "NVIDIA evidence nonce does not match the challenge",
        ));
    }

    let nras_url = Url::parse(NVIDIA_NRAS).expect("static NVIDIA NRAS URL is valid");
    let http = pinned_client(
        &nras_url,
        &EndpointPolicy::default(),
        config.attestation_timeout,
        false,
    )
    .await?;
    let response = http
        .post(nras_url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .body(payload_bytes)
        .send()
        .await
        .map_err(|_| SecureClientError::attestation("NVIDIA NRAS request failed"))?;
    if response.status() != StatusCode::OK {
        return Err(SecureClientError::attestation(
            "NVIDIA NRAS rejected the evidence",
        ));
    }
    let response = bounded_body(response, config.limits.nvidia_jwt_bytes).await?;
    let nras: Value = serde_json::from_slice(&response)
        .map_err(|_| SecureClientError::attestation("NVIDIA NRAS response is invalid"))?;
    let jwt = nras
        .get(0)
        .and_then(|entry| entry.get(1))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            SecureClientError::attestation("NVIDIA NRAS response omitted its verdict")
        })?;
    if jwt.len() > config.limits.nvidia_jwt_bytes {
        return Err(SecureClientError::attestation(
            "NVIDIA verdict exceeds its configured bound",
        ));
    }

    let jwks_url = Url::parse(NVIDIA_JWKS).expect("static NVIDIA JWKS URL is valid");
    let jwks_response = http
        .get(jwks_url)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|_| SecureClientError::attestation("NVIDIA JWKS request failed"))?;
    if jwks_response.status() != StatusCode::OK {
        return Err(SecureClientError::attestation(
            "NVIDIA JWKS request was rejected",
        ));
    }
    let jwks = bounded_body(jwks_response, config.limits.nvidia_jwks_bytes).await?;
    verify_nvidia_jwt(jwt, &jwks, &expected_nonce_hex, approved_anchor_sha256)
}

#[derive(Deserialize)]
struct JwsHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    x5c: Option<Vec<String>>,
}

fn verify_nvidia_jwt(
    jwt: &str,
    jwks_json: &[u8],
    expected_nonce: &str,
    approved_anchor_sha256: &[String],
) -> Result<()> {
    let parts: Vec<_> = jwt.split('.').collect();
    let [encoded_header, encoded_payload, encoded_signature] = parts.as_slice() else {
        return Err(SecureClientError::attestation(
            "NVIDIA verdict JWT is malformed",
        ));
    };
    let header: JwsHeader = decode_jws_json(encoded_header, "NVIDIA JWT header is invalid")?;
    if header.alg != "ES384" || header.kid.is_empty() {
        return Err(SecureClientError::attestation(
            "NVIDIA verdict uses an unsupported signing algorithm",
        ));
    }
    let jwks: Jwks = serde_json::from_slice(jwks_json)
        .map_err(|_| SecureClientError::attestation("NVIDIA JWKS is invalid"))?;
    let chain = jwks
        .keys
        .iter()
        .find(|key| key.kid.as_deref() == Some(header.kid.as_str()))
        .and_then(|key| key.x5c.as_ref())
        .filter(|chain| chain.len() >= 2)
        .ok_or_else(|| SecureClientError::attestation("NVIDIA signing key is absent from JWKS"))?;
    let leaf_der = decode_certificate(&chain[0])?;
    let presented_intermediate = decode_certificate(&chain[1])?;
    let presented_fingerprint = hex::encode(Sha256::digest(&presented_intermediate));
    if !approved_anchor_sha256
        .iter()
        .any(|approved| approved == &presented_fingerprint)
    {
        return Err(SecureClientError::attestation(
            "NVIDIA certificate chain does not match the signed trust policy",
        ));
    }
    let (_, anchor) = X509Certificate::from_der(&presented_intermediate)
        .map_err(|_| SecureClientError::attestation("NVIDIA anchor is invalid"))?;
    let (_, leaf) = X509Certificate::from_der(&leaf_der)
        .map_err(|_| SecureClientError::attestation("NVIDIA leaf certificate is invalid"))?;
    if !leaf.validity().is_valid() || !anchor.validity().is_valid() {
        return Err(SecureClientError::attestation(
            "NVIDIA signing certificate is outside its validity period",
        ));
    }
    leaf.verify_signature(Some(anchor.public_key()))
        .map_err(|_| {
            SecureClientError::attestation("NVIDIA leaf is not signed by the pinned anchor")
        })?;

    let verifying_key = VerifyingKey::from_public_key_der(leaf.public_key().raw)
        .map_err(|_| SecureClientError::attestation("NVIDIA leaf does not contain a P-384 key"))?;
    let signature_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| SecureClientError::attestation("NVIDIA JWT signature is malformed"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| SecureClientError::attestation("NVIDIA JWT signature is malformed"))?;
    verifying_key
        .verify(
            format!("{encoded_header}.{encoded_payload}").as_bytes(),
            &signature,
        )
        .map_err(|_| SecureClientError::attestation("NVIDIA JWT signature is invalid"))?;

    let payload: Value = decode_jws_json(encoded_payload, "NVIDIA JWT payload is invalid")?;
    let verdict = payload.get("x-nvidia-overall-att-result");
    let passed = verdict == Some(&Value::Bool(true))
        || verdict.and_then(Value::as_str).is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "true" | "success" | "pass"
            )
        });
    if !passed {
        return Err(SecureClientError::attestation(
            "NVIDIA attestation verdict is not a pass",
        ));
    }
    if payload
        .get("eat_nonce")
        .and_then(Value::as_str)
        .is_none_or(|nonce| !nonce.eq_ignore_ascii_case(expected_nonce))
    {
        return Err(SecureClientError::attestation(
            "NVIDIA verdict nonce does not match the challenge",
        ));
    }
    Ok(())
}

fn decode_jws_json<T: for<'de> Deserialize<'de>>(encoded: &str, detail: &'static str) -> Result<T> {
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| SecureClientError::attestation(detail))?;
    serde_json::from_slice(&bytes).map_err(|_| SecureClientError::attestation(detail))
}

fn decode_certificate(encoded: &str) -> Result<Vec<u8>> {
    if encoded.len() > 64 * 1024 {
        return Err(SecureClientError::attestation(
            "NVIDIA certificate exceeds its configured bound",
        ));
    }
    general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| SecureClientError::attestation("NVIDIA certificate encoding is invalid"))
}

fn build_evidence(
    model_id: &str,
    policy: &TrustPolicy,
    model_key: Ed25519PublicKey,
    tls_fingerprint: &[u8; 32],
    intel: &IntelEvidence,
    workload_manifest: String,
) -> SecurityEvidence {
    SecurityEvidence {
        state: SecurityState::Verified,
        provider_id: super::PROVIDER_ID.into(),
        model_id: model_id.into(),
        attestation_protocol: super::ATTESTATION_PROTOCOL.into(),
        e2ee_protocol: super::E2EE_PROTOCOL.into(),
        e2ee_encryption_version: super::ENCRYPTION_VERSION,
        trust_policy_version: policy.version.clone(),
        verified_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        attestation_generation: None,
        hard_expires_at_unix_seconds: None,
        model_key_fingerprint: hex::encode(Sha256::digest(model_key.as_bytes())),
        tls_spki_fingerprint: Some(hex::encode(tls_fingerprint)),
        checks: vec![
            check("nonce_freshness", "Fresh challenge", "matched"),
            check("model_binding", "Model identity", "matched"),
            check("model_key_binding", "Model encryption key", "bound"),
            check("tls_binding", "Attested worker TLS key", "quote-bound"),
            check("intel_tdx", "Intel TDX", &intel.status),
            check("nvidia_gpu", "NVIDIA GPU", "verified"),
            check("workload_manifest", "Workload manifest", "quote-bound"),
        ],
        provider_claims: vec![
            EvidenceClaim {
                name: "intel_mr_td".into(),
                value: intel.mr_td.clone(),
            },
            EvidenceClaim {
                name: "workload_manifest_sha256".into(),
                value: intel.compose_hash.clone(),
            },
            EvidenceClaim {
                name: "intel_advisory_ids".into(),
                value: intel.advisory_ids.join(","),
            },
        ],
        workload_manifest: Some(workload_manifest),
    }
}

fn check(id: &str, label: &str, status: &str) -> EvidenceCheck {
    EvidenceCheck {
        id: id.into(),
        label: label.into(),
        status: status.into(),
        passed: true,
    }
}

fn decode_hex_32(value: &str, detail: &'static str) -> Result<[u8; 32]> {
    let normalized = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    if normalized.len() != 64 {
        return Err(SecureClientError::attestation(detail));
    }
    hex::decode(normalized)
        .map_err(|_| SecureClientError::attestation(detail))?
        .try_into()
        .map_err(|_| SecureClientError::attestation(detail))
}

fn deserialize_json_object<'de, D>(deserializer: D) -> std::result::Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(object)) => Ok(Some(Value::Object(object))),
        Some(Value::String(encoded)) if encoded.trim().is_empty() => Ok(None),
        Some(Value::String(encoded)) => {
            let value: Value = serde_json::from_str(&encoded).map_err(D::Error::custom)?;
            if value.is_object() {
                Ok(Some(value))
            } else {
                Err(D::Error::custom("expected a JSON object"))
            }
        }
        Some(_) => Err(D::Error::custom("expected a JSON object")),
    }
}

fn parse_json_object(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(object) => Some(object.clone()),
        Value::String(encoded) => serde_json::from_str::<Value>(encoded)
            .ok()
            .and_then(|value| value.as_object().cloned()),
        _ => None,
    }
}

#[derive(Clone)]
struct BoundedCollateralHttp {
    timeout: std::time::Duration,
    remaining: Arc<AtomicUsize>,
}

impl BoundedCollateralHttp {
    fn new(timeout: std::time::Duration, total_limit: usize) -> Self {
        Self {
            timeout,
            remaining: Arc::new(AtomicUsize::new(total_limit)),
        }
    }

    fn consume(&self, amount: usize) -> anyhow::Result<()> {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(amount)
            })
            .map(|_| ())
            .map_err(|_| anyhow!("Intel collateral exceeded its configured bound"))
    }
}

impl HttpClient for BoundedCollateralHttp {
    async fn get(&self, value: &str) -> anyhow::Result<HttpResponse> {
        let url = Url::parse(value).context("Intel collateral URL is invalid")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("Intel collateral URL is disallowed");
        }
        let host = url.host_str().context("Intel collateral URL has no host")?;
        if host != "api.trustedservices.intel.com" && !host.ends_with(".trustedservices.intel.com")
        {
            bail!("Intel collateral host is outside the trusted service scope");
        }
        let endpoint_policy = EndpointPolicy {
            require_https: false,
            reject_local_addresses: true,
            allow_redirects: false,
        };
        let client = pinned_client(&url, &endpoint_policy, self.timeout, false)
            .await
            .map_err(|_| anyhow!("Intel collateral connection was rejected"))?;
        let response = client
            .get(url)
            .send()
            .await
            .context("Intel collateral request failed")?;
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        let mut header_bytes = 0_usize;
        for (name, value) in response.headers() {
            let value = value
                .to_str()
                .context("Intel collateral header is not text")?;
            header_bytes = header_bytes
                .checked_add(name.as_str().len() + value.len())
                .context("Intel collateral headers overflowed")?;
            headers.insert(name.as_str().to_owned(), value.to_owned());
        }
        self.consume(header_bytes)?;
        let available = self.remaining.load(Ordering::Acquire);
        let body = bounded_body(response, available)
            .await
            .map_err(|_| anyhow!("Intel collateral response was rejected"))?;
        self.consume(body.len())?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn outdated_consent_is_runtime_scoped_and_cannot_accept_other_tcb_failures() {
        let config = crate::SecureClientConfig::new("https://relay.example").unwrap();
        let policy = crate::TrustPolicy::default();
        assert!(!super::accept_intel_status("UpToDate", &policy, &config).unwrap());
        assert_eq!(
            super::accept_intel_status("OutOfDate", &policy, &config)
                .unwrap_err()
                .code(),
            crate::SecureErrorCode::OutdatedTee
        );
        assert!(config.accept_outdated_tee_for_provider("tinfoil").is_err());
        assert!(config.accept_outdated_tee_for_provider("*").is_err());
        let sibling = config.clone();
        config.accept_outdated_tee_for_provider("near").unwrap();
        assert!(super::accept_intel_status("OutOfDate", &policy, &sibling).unwrap());
        for status in [
            "Revoked",
            "OutOfDateConfigurationNeeded",
            "ConfigurationNeeded",
            "SWHardeningNeeded",
            "ConfigurationAndSWHardeningNeeded",
            "unknown",
            "",
        ] {
            assert!(
                super::accept_intel_status(status, &policy, &config).is_err(),
                "{status}"
            );
        }
        assert_eq!(
            policy.accepted_intel_statuses,
            ["UpToDate"],
            "consent never rewrites signed policy"
        );
        let restarted = crate::SecureClientConfig::new("https://relay.example").unwrap();
        assert!(super::accept_intel_status("OutOfDate", &policy, &restarted).is_err());
    }

    use base64::Engine;
    use p384::{
        ecdsa::{Signature, SigningKey, signature::Signer},
        pkcs8::DecodePrivateKey,
    };
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose,
        PKCS_ECDSA_P384_SHA384, date_time_ymd,
    };

    use super::*;

    fn report() -> AttestationReport {
        AttestationReport {
            signing_algo: Some("ed25519".into()),
            signing_public_key: Some(
                "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737".into(),
            ),
            signing_address: Some(
                "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737".into(),
            ),
            nonce: Some("11".repeat(32)),
            request_nonce: None,
            tls_fingerprint: Some("22".repeat(32)),
            tls_cert_fingerprint: None,
            model_name: Some("model/a".into()),
            intel_quote: Some("00".into()),
            nvidia_payload: Some(serde_json::json!({"nonce": "11".repeat(32)})),
            app_compose: Some("services: {}".into()),
            info: None,
            tcb_info: None,
        }
    }

    #[test]
    fn report_identity_rejects_mutated_nonce_model_and_algorithm() {
        let mut candidate = report();
        assert!(validate_report_identity(&candidate, &[0x11; 32], "model/a", "ed25519",).is_ok());

        candidate.nonce = Some("12".repeat(32));
        assert!(validate_report_identity(&candidate, &[0x11; 32], "model/a", "ed25519",).is_err());
        candidate = report();
        candidate.model_name = Some("model/b".into());
        assert!(validate_report_identity(&candidate, &[0x11; 32], "model/a", "ed25519",).is_err());
        candidate = report();
        candidate.model_name = None;
        assert!(validate_report_identity(&candidate, &[0x11; 32], "model/a", "ed25519",).is_err());
        candidate = report();
        candidate.signing_algo = Some("ecdsa".into());
        assert!(validate_report_identity(&candidate, &[0x11; 32], "model/a", "ed25519",).is_err());
        candidate = report();
        assert!(validate_report_identity(&candidate, &[0x12; 32], "model/a", "ed25519",).is_err());
    }

    #[test]
    fn report_data_binds_raw_key_tls_and_nonce() {
        let key = report().model_key().unwrap();
        let mut report_data = [0_u8; 64];
        let key_bytes = hex::decode(key.to_hex()).unwrap();
        report_data[..32].copy_from_slice(&Sha256::digest(
            [key_bytes.as_slice(), &[0x22; 32]].concat(),
        ));
        report_data[32..].copy_from_slice(&[0x11; 32]);
        assert!(
            verify_report_data(&report_data, key.as_bytes(), &[0x22; 32], &[0x11; 32],).is_ok()
        );

        report_data[0] ^= 1;
        assert!(
            verify_report_data(&report_data, key.as_bytes(), &[0x22; 32], &[0x11; 32],).is_err()
        );

        let response_address = [0x33_u8; 20];
        let mut response_report_data = [0_u8; 64];
        response_report_data[..32].copy_from_slice(&Sha256::digest(
            [response_address.as_slice(), &[0x22; 32]].concat(),
        ));
        response_report_data[32..].copy_from_slice(&[0x11; 32]);
        assert!(
            verify_report_data(
                &response_report_data,
                &response_address,
                &[0x22; 32],
                &[0x11; 32],
            )
            .is_ok()
        );
        assert!(
            verify_report_data(&response_report_data, &[0x34; 20], &[0x22; 32], &[0x11; 32],)
                .is_err()
        );
    }

    #[test]
    fn authoritative_report_vectors_use_the_production_verifier() {
        #[derive(serde::Deserialize)]
        struct Vectors {
            report_data_vectors: Vec<ReportVector>,
        }
        #[derive(serde::Deserialize)]
        struct ReportVector {
            signing_key_hex: String,
            tls_fingerprint_hex: String,
            nonce_hex: String,
            report_data_hex: String,
            valid: bool,
        }
        let vectors: Vectors =
            serde_json::from_str(include_str!("../../../../../fixtures/e2ee/near-v2.json"))
                .unwrap();
        for vector in vectors.report_data_vectors {
            let report = hex::decode(vector.report_data_hex).unwrap();
            let key = hex::decode(vector.signing_key_hex).unwrap();
            let tls =
                <[u8; 32]>::try_from(hex::decode(vector.tls_fingerprint_hex).unwrap()).unwrap();
            let nonce = <[u8; 32]>::try_from(hex::decode(vector.nonce_hex).unwrap()).unwrap();
            assert_eq!(
                verify_report_data(&report, &key, &tls, &nonce).is_ok(),
                vector.valid
            );
            assert!(verify_report_data(&report[..63], &key, &tls, &nonce).is_err());
            assert!(
                verify_report_data(&[report.clone(), vec![0]].concat(), &key, &tls, &nonce)
                    .is_err()
            );
            assert!(verify_report_data(&report, &[], &tls, &nonce).is_err());
            let mut wrong_nonce = nonce;
            wrong_nonce[0] ^= 1;
            assert!(verify_report_data(&report, &key, &tls, &wrong_nonce).is_err());
            let mut wrong_tls = tls;
            wrong_tls[0] ^= 1;
            assert!(verify_report_data(&report, &key, &wrong_tls, &nonce).is_err());
        }
    }

    struct NvidiaFixture {
        jwt: String,
        jwks: Vec<u8>,
        anchor_hash: String,
        nonce: String,
    }

    fn nvidia_fixture(result: &Value, verdict_nonce: Option<String>) -> NvidiaFixture {
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Test NVIDIA Intermediate");
        ca_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
        ];
        ca_params.not_before = date_time_ymd(2025, 1, 1);
        ca_params.not_after = date_time_ymd(2035, 1, 1);
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let mut leaf_params = CertificateParams::default();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "Test NVIDIA Leaf");
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.not_before = date_time_ymd(2025, 1, 1);
        leaf_params.not_after = date_time_ymd(2035, 1, 1);
        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
        let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).unwrap();

        let nonce = "ab".repeat(32);
        let header = general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&serde_json::json!({"alg":"ES384","kid":"test"})).unwrap());
        let payload = general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "x-nvidia-overall-att-result": result,
                "eat_nonce": verdict_nonce.unwrap_or_else(|| nonce.clone()),
            }))
            .unwrap(),
        );
        let signing_key = SigningKey::from_pkcs8_der(&leaf_key.serialize_der()).unwrap();
        let signature: Signature = signing_key.sign(format!("{header}.{payload}").as_bytes());
        let signature = general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
        let jwt = format!("{header}.{payload}.{signature}");
        let jwks = serde_json::to_vec(&serde_json::json!({
            "keys": [{
                "kid": "test",
                "x5c": [
                    general_purpose::STANDARD.encode(leaf_cert.der()),
                    general_purpose::STANDARD.encode(ca_cert.der()),
                ]
            }]
        }))
        .unwrap();
        NvidiaFixture {
            jwt,
            jwks,
            anchor_hash: hex::encode(Sha256::digest(ca_cert.der())),
            nonce,
        }
    }

    #[test]
    fn nvidia_verdict_requires_pinned_chain_signature_pass_and_fresh_nonce() {
        let fixture = nvidia_fixture(&Value::Bool(true), None);
        assert!(
            verify_nvidia_jwt(
                &fixture.jwt,
                &fixture.jwks,
                &fixture.nonce,
                std::slice::from_ref(&fixture.anchor_hash),
            )
            .is_ok()
        );

        let mut tampered = fixture.jwt.clone().into_bytes();
        let last = tampered.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        assert!(
            verify_nvidia_jwt(
                std::str::from_utf8(&tampered).unwrap(),
                &fixture.jwks,
                &fixture.nonce,
                std::slice::from_ref(&fixture.anchor_hash),
            )
            .is_err()
        );
        assert!(
            verify_nvidia_jwt(
                &fixture.jwt,
                &fixture.jwks,
                &"cd".repeat(32),
                std::slice::from_ref(&fixture.anchor_hash),
            )
            .is_err()
        );
        let rejected = nvidia_fixture(&Value::Bool(false), None);
        assert!(
            verify_nvidia_jwt(
                &rejected.jwt,
                &rejected.jwks,
                &rejected.nonce,
                std::slice::from_ref(&rejected.anchor_hash),
            )
            .is_err()
        );
        assert!(
            verify_nvidia_jwt(
                &fixture.jwt,
                &fixture.jwks,
                &fixture.nonce,
                &["00".repeat(32)],
            )
            .is_err()
        );
    }

    #[test]
    fn normalization_rejects_ambiguous_or_missing_model_selection() {
        assert!(normalize_report(serde_json::json!({"model_attestations": []})).is_err());
        assert!(
            normalize_report(serde_json::json!({
                "model_attestations": [{}, {}]
            }))
            .is_err()
        );
        let normalized = normalize_report(serde_json::json!({
            "nonce": "top",
            "model_attestations": [{"nonce": "selected"}]
        }))
        .unwrap();
        assert_eq!(normalized["nonce"], "selected");
    }

    #[tokio::test]
    #[ignore = "requires a live NEAR provider in AXIOM_NEAR_MODEL_BASE_URL and AXIOM_NEAR_UPSTREAM_MODEL"]
    async fn live_provider_attestation_verifies_locally() {
        let base_url = std::env::var("AXIOM_NEAR_MODEL_BASE_URL")
            .expect("AXIOM_NEAR_MODEL_BASE_URL is required for the ignored live test");
        let upstream_model = std::env::var("AXIOM_NEAR_UPSTREAM_MODEL")
            .expect("AXIOM_NEAR_UPSTREAM_MODEL is required for the ignored live test");
        let config = SecureClientConfig::new(
            &std::env::var("AXIOM_RELAY_URL").unwrap_or_else(|_| "https://api.axiom.stream".into()),
        )
        .unwrap();
        let credential = crate::ApiCredential::new(
            std::env::var("AXIOM_API_KEY").expect("AXIOM_API_KEY is required"),
        );
        let model = axiom_inference::ModelInfo {
            id: std::env::var("AXIOM_NEAR_MODEL_ID").expect("AXIOM_NEAR_MODEL_ID is required"),
            upstream_model,
            provider_base_url: base_url,
            ..Default::default()
        };
        let (verified, _, _) = Box::pin(verify_model(
            &config,
            &credential,
            &model,
            &TrustPolicy::default(),
            &CancellationToken::new(),
        ))
        .await
        .unwrap();

        assert_eq!(verified.evidence.state, SecurityState::Verified);
        assert!(verified.evidence.checks.iter().all(|check| check.passed));
        assert_eq!(verified.model_key.to_hex().len(), 64);
        assert_eq!(
            verified.response_signing_address,
            verified.model_key.to_hex()
        );
    }
}

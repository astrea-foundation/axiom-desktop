//! Phala ACI v2 provider engine. The local client verifies the nonce-bound gateway TDX quote,
//! measured gateway source, X25519/Ed25519 keyset, and content-addressed worker sessions before it
//! encrypts. It decrypts a completion only after the gateway's signed receipt binds the exact
//! restored plaintext request body, exact encrypted response bytes, requested model, and cited
//! worker; the relay separately binds the encrypted request wire hash.

use std::{
    collections::BTreeMap,
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use async_trait::async_trait;
use base64::Engine;
use dcap_qvl::{verify::rustcrypto, QuoteCollateralV3};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hkdf::Hkdf;
use k256::ecdsa::{
    RecoveryId as K256RecoveryId, Signature as K256Signature, VerifyingKey as K256VerifyingKey,
};
use p384::{
    ecdsa::{Signature as P384Signature, VerifyingKey as P384VerifyingKey},
    pkcs8::DecodePublicKey,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::ser::Formatter;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256, Sha384};
use x25519_dalek::{PublicKey, StaticSecret};
use x509_parser::prelude::*;

use crate::error::{CoreError, Result};
use crate::provider::{
    AttestationRequest, Attestor, CipherSession, DecryptedProviderCompletion, ProviderCipher,
    ProviderWireRequest, VerificationCheck, VerifiedModel,
};
use crate::relay::RelayCompletion;

pub const PHALA_PROVIDER_ID: &str = "phala";
pub const PHALA_ATTESTATION_PROTOCOL: &str = "phala-aci-tdx-v1";
pub const PHALA_ACI_V2_PROTOCOL: &str = "phala-aci-v2";
pub const PHALA_ACI_V2_ENCRYPTION_VERSION: u8 = 2;

const E2EE_ALGORITHM: &str = "x25519-aes-256-gcm-hkdf-sha256";
const E2EE_KEY_ID: &str = "dstack-kms-e2ee-x25519-v1";
const HKDF_INFO: &[u8] = b"aci.e2ee.v2.x25519";
const GATEWAY_REPOSITORY_ENV: &str = "PRIVATE_AI_GATEWAY_REPO_URL";
const GATEWAY_COMMIT_ENV: &str = "PRIVATE_AI_GATEWAY_REPO_COMMIT";
const NVIDIA_JWKS_URL: &str = "https://nras.attestation.nvidia.com/.well-known/jwks.json";
const NVIDIA_NRAS_INTERMEDIATE_SHA256: &str =
    "2df8907cf4d6c277b855d407ec178a649930a8e73f7294e8fce66e6ee27c5175";
const MAX_PROOF_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

// Historical suffixes identify wire formats, not approved gateway source revisions.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
enum PhalaAciProfile {
    #[serde(rename = "aci-v1-08d550")]
    AciV1_08d550,
    #[serde(rename = "dstack-aci-draft-d567f6")]
    DstackAciDraftD567f6,
}

impl PhalaAciProfile {
    const fn artifact_profile(self) -> &'static str {
        match self {
            Self::AciV1_08d550 => "aci-v1-08d550",
            Self::DstackAciDraftD567f6 => "dstack-aci-draft-d567f6",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ReceiptPublicKey {
    algo: String,
    public_key_hex: String,
}

#[derive(Debug, Deserialize)]
struct CollateralWire {
    pck_crl_issuer_chain: String,
    root_ca_crl: String,
    pck_crl: String,
    tcb_info_issuer_chain: String,
    tcb_info: String,
    tcb_info_signature: String,
    qe_identity_issuer_chain: String,
    qe_identity: String,
    qe_identity_signature: String,
    pck_certificate_chain: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NvidiaJwsHeader {
    alg: String,
    kid: String,
}

#[derive(Debug, Deserialize)]
struct NvidiaJwks {
    keys: Vec<NvidiaJwk>,
}

#[derive(Debug, Deserialize)]
struct NvidiaJwk {
    kid: Option<String>,
    x5c: Option<Vec<String>>,
}

impl CollateralWire {
    fn into_qvl(self) -> Result<QuoteCollateralV3> {
        Ok(QuoteCollateralV3 {
            pck_crl_issuer_chain: self.pck_crl_issuer_chain,
            root_ca_crl: decode_hex("root_ca_crl", &self.root_ca_crl)?,
            pck_crl: decode_hex("pck_crl", &self.pck_crl)?,
            tcb_info_issuer_chain: self.tcb_info_issuer_chain,
            tcb_info: self.tcb_info,
            tcb_info_signature: decode_hex("tcb_info_signature", &self.tcb_info_signature)?,
            qe_identity_issuer_chain: self.qe_identity_issuer_chain,
            qe_identity: self.qe_identity,
            qe_identity_signature: decode_hex(
                "qe_identity_signature",
                &self.qe_identity_signature,
            )?,
            pck_certificate_chain: self.pck_certificate_chain,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PhalaWorkerSession {
    session_id: String,
    established_at: u64,
    expires_at: u64,
    record: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PhalaAttestationState {
    profile: PhalaAciProfile,
    model: String,
    workload_id: Option<String>,
    keyset_digest: String,
    source_repository: String,
    source_commit: String,
    gateway_public_key_hex: String,
    receipt_public_keys: BTreeMap<String, ReceiptPublicKey>,
    worker_sessions: BTreeMap<String, PhalaWorkerSession>,
}

fn attestation(message: impl Into<String>) -> CoreError {
    CoreError::Attestation(message.into())
}

fn provider(message: impl Into<String>) -> CoreError {
    CoreError::Provider(message.into())
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| attestation(format!("Phala {label} is not an object")))
}

fn required_str<'a>(object: &'a Map<String, Value>, key: &str, label: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| attestation(format!("Phala {label} is missing")))
}

fn pinned_worker_report_bindings(report: &Map<String, Value>) -> Result<(&str, &str)> {
    Ok((
        required_str(report, "request_nonce", "worker attestation nonce")?,
        required_str(
            report,
            "tls_cert_fingerprint",
            "worker attested TLS fingerprint",
        )?,
    ))
}

fn decode_hex(label: &str, value: &str) -> Result<Vec<u8>> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| attestation(format!("Phala {label} must be hex: {error}")))
}

fn decode_32(label: &str, value: &str) -> Result<[u8; 32]> {
    decode_hex(label, value)?
        .try_into()
        .map_err(|_| attestation(format!("Phala {label} must be exactly 32 bytes")))
}

fn k256_verifying_key(label: &str, value: &str) -> Result<K256VerifyingKey> {
    let mut bytes = decode_hex(label, value)?;
    // ACI requires verifiers to accept the EVM/dstack 64-byte uncompressed form without the
    // SEC1 0x04 prefix as well as ordinary compressed/uncompressed SEC1 encodings.
    if bytes.len() == 64 {
        bytes.insert(0, 0x04);
    }
    K256VerifyingKey::from_sec1_bytes(&bytes)
        .map_err(|_| attestation(format!("Phala {label} is invalid")))
}

fn sorted_json(value: &Value) -> Result<Value> {
    match value {
        Value::Array(values) => values
            .iter()
            .map(sorted_json)
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut output = Map::new();
            for key in keys {
                output.insert(key.clone(), sorted_json(&values[key])?);
            }
            Ok(Value::Object(output))
        }
        Value::Number(number) => {
            if number.as_i64().is_none() && number.as_u64().is_none() {
                return Err(attestation(
                    "Phala canonical JSON accepts only integer numbers",
                ));
            }
            Ok(value.clone())
        }
        _ => Ok(value.clone()),
    }
}

fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(&sorted_json(value)?)
        .map_err(|error| attestation(format!("Phala canonical JSON failed: {error}")))
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 71
        && value
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.chars().all(|character| character.is_ascii_hexdigit()))
        && value == value.to_ascii_lowercase()
}

fn random_nonce_hex() -> String {
    let mut nonce = [0_u8; 32];
    OsRng.fill_bytes(&mut nonce);
    hex::encode(nonce)
}

fn asserted(record: &Map<String, Value>, claim: &str) -> bool {
    record
        .get("claims")
        .and_then(Value::as_object)
        .and_then(|claims| claims.get(claim))
        .and_then(Value::as_object)
        .and_then(|claim| claim.get("status"))
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("asserted"))
}

fn replay_rtmr3(event_log: &str) -> Result<([u8; 48], String)> {
    let events: Value = serde_json::from_str(event_log)
        .map_err(|error| attestation(format!("Phala gateway event log is invalid: {error}")))?;
    let events = events
        .as_array()
        .ok_or_else(|| attestation("Phala gateway event log is not an array"))?;
    let mut current = [0_u8; 48];
    let mut compose_hashes = Vec::new();
    let mut system_ready = false;
    for (index, event) in events.iter().enumerate() {
        let event = object(event, &format!("gateway event {index}"))?;
        if event.get("imr").and_then(Value::as_u64) != Some(3) {
            continue;
        }
        let digest = decode_hex(
            &format!("gateway event {index} digest"),
            required_str(event, "digest", "gateway event digest")?,
        )?;
        if digest.len() > 48 {
            return Err(attestation("Phala gateway event digest is too long"));
        }
        let mut padded = [0_u8; 48];
        padded[..digest.len()].copy_from_slice(&digest);
        current = Sha384::digest([current.as_slice(), padded.as_slice()].concat()).into();
        match event.get("event").and_then(Value::as_str) {
            Some("system-ready") => system_ready = true,
            Some("compose-hash") if !system_ready => compose_hashes.push(
                event
                    .get("event_payload")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase(),
            ),
            _ => {}
        }
    }
    if compose_hashes.len() != 1 {
        return Err(attestation(
            "Phala gateway event log has no unique compose measurement",
        ));
    }
    Ok((current, compose_hashes.remove(0)))
}

fn measured_gateway_source(app_compose: &str) -> Result<(String, String)> {
    let manifest: Value = serde_json::from_str(app_compose).map_err(|error| {
        attestation(format!(
            "Phala gateway compose manifest is invalid: {error}"
        ))
    })?;
    let compose_file = manifest
        .as_object()
        .and_then(|value| value.get("docker_compose_file"))
        .and_then(Value::as_str)
        .ok_or_else(|| attestation("Phala gateway manifest has no Docker Compose workflow"))?;
    let environment_value = |name: &str| -> Result<String> {
        let prefix = format!("- {name}=");
        let values = compose_file
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix(&prefix))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if values.len() != 1 {
            return Err(attestation(format!(
                "Phala measured workflow has no unique {name}"
            )));
        }
        Ok(values[0].to_string())
    };
    let repository = environment_value(GATEWAY_REPOSITORY_ENV)?;
    if !repository.starts_with("https://") || repository.chars().any(char::is_whitespace) {
        return Err(attestation("Phala measured source repository is invalid"));
    }
    let commit = environment_value(GATEWAY_COMMIT_ENV)?.to_lowercase();
    if commit.len() != 40 || !commit.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(attestation("Phala measured source commit is invalid"));
    }
    Ok((repository, commit))
}

fn canonical_repository_url(value: &str) -> &str {
    let value = value.strip_suffix('/').unwrap_or(value);
    value.strip_suffix(".git").unwrap_or(value)
}

fn verify_gateway_source(
    gateway_attestation: &Map<String, Value>,
    source_repository: &str,
    source_commit: &str,
) -> Result<()> {
    let provenance = gateway_attestation
        .get("source_provenance")
        .and_then(Value::as_object)
        .ok_or_else(|| attestation("Phala gateway source provenance is missing"))?;
    if canonical_repository_url(required_str(
        provenance,
        "repo_url",
        "gateway source repository",
    )?) != canonical_repository_url(source_repository)
    {
        return Err(attestation(
            "Phala gateway source repository does not match the measured workflow",
        ));
    }
    if required_str(provenance, "repo_commit", "gateway source commit")?.to_lowercase()
        != source_commit
    {
        return Err(attestation(
            "Phala gateway source commit does not match the measured workflow",
        ));
    }
    Ok(())
}

fn parse_json_object(value: &Value, label: &str) -> Result<Map<String, Value>> {
    match value {
        Value::Object(object) => Ok(object.clone()),
        Value::String(encoded) => serde_json::from_str::<Value>(encoded)
            .map_err(|error| attestation(format!("Phala {label} is invalid JSON: {error}")))
            .and_then(|decoded| object(&decoded, label).cloned()),
        _ => Err(attestation(format!("Phala {label} is not an object"))),
    }
}

fn normalized_worker_report(value: Value) -> Result<Value> {
    let top = value
        .as_object()
        .ok_or_else(|| attestation("Phala worker attestation report is not an object"))?;
    if top.contains_key("model_attestations") {
        return Err(attestation(
            "Phala worker attestation used an unsupported multi-model wrapper",
        ));
    }
    Ok(value)
}

fn worker_workload_manifest(report: &Map<String, Value>) -> Result<String> {
    let info = report
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| attestation("Phala worker attestation info is not an object"))?;
    let tcb_info = info
        .get("tcb_info")
        .ok_or_else(|| attestation("Phala worker attestation has no TCB info"))?;
    let tcb_info = parse_json_object(tcb_info, "worker TCB info")?;
    Ok(required_str(&tcb_info, "app_compose", "worker workload manifest")?.to_string())
}

fn worker_endpoint_origin(value: &str) -> Result<String> {
    let url =
        reqwest::Url::parse(value).map_err(|_| attestation("Phala worker endpoint is invalid"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(attestation("Phala worker endpoint is not HTTPS"));
    }
    Ok(url.origin().ascii_serialization())
}

#[derive(Clone, Copy)]
struct ExpectedWorkerIdentity {
    upstream_name: &'static str,
    provider_type: &'static str,
    canonical_model_id: &'static str,
}

fn expected_worker_identity(
    profile: PhalaAciProfile,
    model: &str,
) -> Result<ExpectedWorkerIdentity> {
    match (profile, model) {
        (PhalaAciProfile::AciV1_08d550, "deepseek/deepseek-v4-flash") => {
            Ok(ExpectedWorkerIdentity {
                upstream_name: "phala-deepseek-v4-flash",
                provider_type: "phala-direct",
                canonical_model_id: "deepseek/deepseek-v4-flash",
            })
        }
        (PhalaAciProfile::DstackAciDraftD567f6, "deepseek/deepseek-v4-flash") => {
            Ok(ExpectedWorkerIdentity {
                upstream_name: "phala-188",
                provider_type: "phala-direct",
                canonical_model_id: "deepseek/deepseek-v4-flash",
            })
        }
        (PhalaAciProfile::AciV1_08d550, "z-ai/glm-5.2") => Ok(ExpectedWorkerIdentity {
            upstream_name: "phala-glm-5-2",
            provider_type: "phala-direct",
            canonical_model_id: "glm-5.2",
        }),
        (PhalaAciProfile::DstackAciDraftD567f6, "z-ai/glm-5.2") => Ok(ExpectedWorkerIdentity {
            upstream_name: "phala-177",
            provider_type: "phala-direct",
            canonical_model_id: "glm-5.2",
        }),
        (PhalaAciProfile::AciV1_08d550, "openai/gpt-oss-20b") => Ok(ExpectedWorkerIdentity {
            upstream_name: "phala-gpt-oss-20b",
            provider_type: "phala-direct",
            canonical_model_id: "openai/gpt-oss-20b",
        }),
        (PhalaAciProfile::DstackAciDraftD567f6, "openai/gpt-oss-20b") => {
            Ok(ExpectedWorkerIdentity {
                upstream_name: "phala-23",
                provider_type: "phala-direct",
                canonical_model_id: "openai/gpt-oss-20b",
            })
        }
        _ => Err(attestation(format!(
            "Phala model {model} has no pinned upstream identity mapping for {}",
            profile.artifact_profile()
        ))),
    }
}

fn canonical_worker_model(record: &Map<String, Value>) -> Option<&str> {
    record
        .get("claims")
        .and_then(Value::as_object)
        .and_then(|claims| claims.get("extra"))
        .and_then(Value::as_object)
        .and_then(|extra| extra.get("canonical_model_id"))
        .and_then(Value::as_str)
}

fn select_gateway_profile(
    report: &Map<String, Value>,
    gateway_attestation: &Map<String, Value>,
    keyset: &Map<String, Value>,
) -> Result<PhalaAciProfile> {
    let capabilities = report
        .get("service_capabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| attestation("Phala gateway capabilities are missing"))?;
    let standard_shape = report.contains_key("workload_id")
        && keyset.contains_key("workload_identity")
        && keyset.contains_key("keyset_epoch")
        && gateway_attestation.contains_key("keyset_endorsement")
        && !capabilities.contains_key("serving")
        && !keyset.contains_key("not_after")
        && !keyset.contains_key("subject");
    let draft_shape = !report.contains_key("workload_id")
        && !keyset.contains_key("workload_identity")
        && !keyset.contains_key("keyset_epoch")
        && !gateway_attestation.contains_key("keyset_endorsement")
        && capabilities.get("serving").and_then(Value::as_str) == Some("aggregator")
        && keyset.contains_key("not_after")
        && keyset.contains_key("subject");
    match (standard_shape, draft_shape) {
        (true, false) => Ok(PhalaAciProfile::AciV1_08d550),
        (false, true) => Ok(PhalaAciProfile::DstackAciDraftD567f6),
        _ => Err(attestation(
            "Phala gateway report has an unsupported or mixed ACI artifact shape",
        )),
    }
}

fn verify_keyset_endorsement(
    algo: &str,
    public_key_hex: &str,
    signature_hex: &str,
    payload: &[u8],
) -> Result<()> {
    let public_key = decode_hex("workload identity key", public_key_hex)?;
    let signature = decode_hex("keyset endorsement", signature_hex)?;
    match algo {
        "ed25519" => {
            let verifier = VerifyingKey::from_bytes(
                &public_key
                    .try_into()
                    .map_err(|_| attestation("Phala workload identity key is invalid"))?,
            )
            .map_err(|_| attestation("Phala workload identity key is invalid"))?;
            let signature = Signature::from_slice(&signature)
                .map_err(|_| attestation("Phala keyset endorsement is invalid"))?;
            verifier
                .verify(payload, &signature)
                .map_err(|_| attestation("Phala keyset endorsement is invalid"))
        }
        "ecdsa-secp256k1" => {
            let verifier = k256_verifying_key("workload identity key", public_key_hex)?;
            let signature = K256Signature::from_slice(&signature)
                .map_err(|_| attestation("Phala keyset endorsement is invalid"))?;
            verifier
                .verify(payload, &signature)
                .map_err(|_| attestation("Phala keyset endorsement is invalid"))
        }
        _ => Err(attestation(
            "Phala workload identity uses an unsupported signing algorithm",
        )),
    }
}

fn parse_receipt_public_keys(
    profile: PhalaAciProfile,
    entries: &[Value],
) -> Result<BTreeMap<String, ReceiptPublicKey>> {
    let mut keys = BTreeMap::new();
    for entry in entries {
        let entry = object(entry, "receipt key")?;
        let algo = required_str(entry, "algo", "receipt key algorithm")?;
        let key_id = required_str(entry, "key_id", "receipt key id")?;
        let public_key = required_str(entry, "public_key", "receipt public key")?;
        match (profile, algo) {
            (_, "ed25519") => {
                decode_32("receipt public key", public_key)?;
            }
            (PhalaAciProfile::AciV1_08d550, "ecdsa-secp256k1") => {
                k256_verifying_key("receipt public key", public_key)?;
            }
            (PhalaAciProfile::DstackAciDraftD567f6, _) => {
                return Err(attestation(
                    "Phala draft receipt key uses an unsupported algorithm",
                ));
            }
            _ => {
                return Err(attestation(
                    "Phala standard receipt key uses an unsupported algorithm",
                ));
            }
        }
        if keys
            .insert(
                key_id.to_string(),
                ReceiptPublicKey {
                    algo: algo.into(),
                    public_key_hex: public_key.to_lowercase(),
                },
            )
            .is_some()
        {
            return Err(attestation("Phala receipt key ids are not unique"));
        }
    }
    if keys.is_empty() {
        return Err(attestation("Phala has no valid receipt key"));
    }
    Ok(keys)
}

fn select_gateway_e2ee_key(profile: PhalaAciProfile, entries: &[Value]) -> Result<String> {
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = object(entry, "E2EE key")?;
        if entry.get("algo").and_then(Value::as_str) != Some(E2EE_ALGORITHM) {
            continue;
        }
        let key_id = required_str(entry, "key_id", "E2EE key id")?;
        if profile == PhalaAciProfile::DstackAciDraftD567f6 && key_id != E2EE_KEY_ID {
            continue;
        }
        let public_key = required_str(entry, "public_key", "gateway X25519 E2EE key")?;
        decode_32("gateway X25519 key", public_key)?;
        candidates.push(public_key.to_lowercase());
    }
    if candidates.len() != 1 {
        return Err(attestation(
            "Phala gateway has no unique approved X25519 E2EE key",
        ));
    }
    Ok(candidates.remove(0))
}

fn verify_receipt_signature(
    profile: PhalaAciProfile,
    key: &ReceiptPublicKey,
    signature_hex: &str,
    canonical_bytes: &[u8],
) -> Result<()> {
    let signature_bytes = decode_hex("receipt signature", signature_hex)
        .map_err(|_| provider("Phala receipt signature is invalid"))?;
    match key.algo.as_str() {
        "ed25519" => {
            let signature = Signature::from_slice(&signature_bytes)
                .map_err(|_| provider("Phala receipt signature is invalid"))?;
            let verifying_key = VerifyingKey::from_bytes(
                &decode_32("receipt public key", &key.public_key_hex)
                    .map_err(|_| provider("Phala receipt public key is invalid"))?,
            )
            .map_err(|_| provider("Phala receipt public key is invalid"))?;
            verifying_key
                .verify(canonical_bytes, &signature)
                .map_err(|_| provider("Phala receipt signature is invalid"))
        }
        "ecdsa-secp256k1" if profile == PhalaAciProfile::AciV1_08d550 => {
            // ACI §8.5 deliberately uses the recoverable EVM shape here. Endorsements are
            // 64-byte r||s, but receipt signatures are exactly r||s||v and 64-byte receipt
            // signatures must fail closed.
            if signature_bytes.len() != 65 {
                return Err(provider(
                    "Phala secp256k1 receipt signature is not 65-byte recoverable form",
                ));
            }
            let recovery_byte = match signature_bytes[64] {
                value @ 0..=3 => value,
                value @ 27..=30 => value - 27,
                _ => return Err(provider("Phala receipt recovery id is invalid")),
            };
            let recovery_id = K256RecoveryId::try_from(recovery_byte)
                .map_err(|_| provider("Phala receipt recovery id is invalid"))?;
            let signature = K256Signature::from_slice(&signature_bytes[..64])
                .map_err(|_| provider("Phala receipt signature is invalid"))?;
            let verifying_key = k256_verifying_key("receipt public key", &key.public_key_hex)
                .map_err(|_| provider("Phala receipt public key is invalid"))?;
            verifying_key
                .verify(canonical_bytes, &signature)
                .map_err(|_| provider("Phala receipt signature is invalid"))?;
            let recovered = K256VerifyingKey::recover_from_digest(
                Sha256::new_with_prefix(canonical_bytes),
                &signature,
                recovery_id,
            )
            .map_err(|_| provider("Phala receipt signature recovery failed"))?;
            if recovered != verifying_key {
                return Err(provider(
                    "Phala receipt recovery id does not identify the attested key",
                ));
            }
            Ok(())
        }
        _ => Err(provider(
            "Phala receipt uses an unsupported signing algorithm",
        )),
    }
}

fn decode_quote(value: &str) -> Result<Vec<u8>> {
    hex::decode(value.trim())
        .map_err(|error| attestation(format!("Phala worker Intel quote is invalid: {error}")))
}

fn minimum_worker_ttl(policy: &Map<String, Value>) -> Result<u64> {
    let value = policy
        .get("minimum_session_ttl_seconds")
        .and_then(Value::as_u64)
        .ok_or_else(|| attestation("Phala minimum worker session TTL is missing"))?;
    if value < 30 {
        return Err(attestation("Phala minimum worker session TTL is too short"));
    }
    Ok(value)
}

fn decode_jws_json<T: for<'de> Deserialize<'de>>(encoded: &str, label: &str) -> Result<T> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| attestation(format!("Phala {label} is invalid")))?;
    serde_json::from_slice(&bytes).map_err(|_| attestation(format!("Phala {label} is invalid")))
}

fn verify_nvidia_jwt(jwt: &str, jwks_json: &[u8], expected_nonce: &str) -> Result<()> {
    let parts = jwt.split('.').collect::<Vec<_>>();
    let [encoded_header, encoded_payload, encoded_signature] = parts.as_slice() else {
        return Err(attestation("Phala worker NVIDIA verdict is malformed"));
    };
    let header: NvidiaJwsHeader = decode_jws_json(encoded_header, "NVIDIA JWT header")?;
    if header.alg != "ES384" || header.kid.is_empty() {
        return Err(attestation(
            "Phala worker NVIDIA verdict uses an unsupported signing algorithm",
        ));
    }
    let jwks: NvidiaJwks = serde_json::from_slice(jwks_json)
        .map_err(|_| attestation("Phala NVIDIA JWKS is invalid"))?;
    let chain = jwks
        .keys
        .iter()
        .find(|key| key.kid.as_deref() == Some(header.kid.as_str()))
        .and_then(|key| key.x5c.as_ref())
        .filter(|chain| chain.len() >= 2)
        .ok_or_else(|| attestation("Phala NVIDIA signing key is absent from JWKS"))?;
    let leaf_der = base64::engine::general_purpose::STANDARD
        .decode(&chain[0])
        .map_err(|_| attestation("Phala NVIDIA leaf certificate is invalid"))?;
    let intermediate_der = base64::engine::general_purpose::STANDARD
        .decode(&chain[1])
        .map_err(|_| attestation("Phala NVIDIA intermediate certificate is invalid"))?;
    if hex::encode(Sha256::digest(&intermediate_der)) != NVIDIA_NRAS_INTERMEDIATE_SHA256 {
        return Err(attestation(
            "Phala NVIDIA certificate chain does not match the pinned trust anchor",
        ));
    }
    let (_, intermediate) = X509Certificate::from_der(&intermediate_der)
        .map_err(|_| attestation("Phala NVIDIA intermediate certificate is invalid"))?;
    let (_, leaf) = X509Certificate::from_der(&leaf_der)
        .map_err(|_| attestation("Phala NVIDIA leaf certificate is invalid"))?;
    if !leaf.validity().is_valid() || !intermediate.validity().is_valid() {
        return Err(attestation(
            "Phala NVIDIA signing certificate is outside its validity period",
        ));
    }
    leaf.verify_signature(Some(intermediate.public_key()))
        .map_err(|_| attestation("Phala NVIDIA leaf is not signed by the pinned anchor"))?;
    let key = P384VerifyingKey::from_public_key_der(leaf.public_key().raw)
        .map_err(|_| attestation("Phala NVIDIA leaf does not contain a P-384 key"))?;
    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| attestation("Phala NVIDIA verdict signature is invalid"))?;
    let signature = P384Signature::from_slice(&signature_bytes)
        .map_err(|_| attestation("Phala NVIDIA verdict signature is invalid"))?;
    key.verify(
        format!("{encoded_header}.{encoded_payload}").as_bytes(),
        &signature,
    )
    .map_err(|_| attestation("Phala NVIDIA verdict signature is invalid"))?;
    let payload: Value = decode_jws_json(encoded_payload, "NVIDIA JWT payload")?;
    let verdict = payload.get("x-nvidia-overall-att-result");
    let passed = verdict == Some(&Value::Bool(true))
        || verdict.and_then(Value::as_str).is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "true" | "success" | "pass"
            )
        });
    if !passed {
        return Err(attestation("Phala worker NVIDIA verdict is not a pass"));
    }
    if payload
        .get("eat_nonce")
        .and_then(Value::as_str)
        .is_none_or(|nonce| !nonce.eq_ignore_ascii_case(expected_nonce))
    {
        return Err(attestation(
            "Phala worker NVIDIA verdict nonce does not match its evidence",
        ));
    }
    Ok(())
}

async fn fetch_nvidia_jwks() -> Result<Vec<u8>> {
    let response = reqwest::Client::new()
        .get(NVIDIA_JWKS_URL)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|_| attestation("Phala NVIDIA JWKS request failed"))?;
    if !response.status().is_success() {
        return Err(attestation("Phala NVIDIA JWKS request was rejected"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| attestation("Phala NVIDIA JWKS response is invalid"))?;
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err(attestation("Phala NVIDIA JWKS exceeds its size bound"));
    }
    Ok(bytes.to_vec())
}

fn standard_session_id(record: &Map<String, Value>) -> Result<String> {
    let evidence_digest = record
        .get("evidence")
        .and_then(Value::as_object)
        .and_then(|evidence| evidence.get("digest"))
        .cloned()
        .unwrap_or(Value::Null);
    let material = serde_json::json!({
        "upstream_name": record.get("upstream_name").cloned().unwrap_or(Value::Null),
        "endpoint": record.get("endpoint").cloned().unwrap_or(Value::Null),
        "verifier_id": record.get("verifier_id").cloned().unwrap_or(Value::Null),
        "identity": record.get("identity").cloned().unwrap_or(Value::Null),
        "channel_binding": record.get("channel_binding").cloned().unwrap_or(Value::Null),
        "claims": record.get("claims").cloned().unwrap_or(Value::Null),
        "evidence_digest": evidence_digest,
    });
    Ok(format!(
        "as_{}",
        hex::encode(Sha256::digest(canonical_json(&material)?))
    ))
}

async fn verify_worker_session(
    value: &Value,
    verification_value: &Value,
    minimum_ttl: u64,
    nvidia_jwks: &[u8],
    profile: PhalaAciProfile,
    expected_model: &str,
) -> Result<PhalaWorkerSession> {
    let mut record = object(value, "worker session")?.clone();
    let session_id = required_str(&record, "session_id", "worker session id")?.to_lowercase();
    record.remove("session_id");
    if record.get("api_version").and_then(Value::as_str) != Some("aci/1") {
        return Err(attestation("Phala worker session is not aci/1"));
    }
    let record_value = Value::Object(record.clone());
    let expected_session_id = match profile {
        PhalaAciProfile::AciV1_08d550 => standard_session_id(&record)?,
        PhalaAciProfile::DstackAciDraftD567f6 => {
            hex::encode(Sha256::digest(canonical_json(&record_value)?))
        }
    };
    if session_id != expected_session_id {
        return Err(attestation("Phala worker session is not content addressed"));
    }
    let established_at = record
        .get("established_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| attestation("Phala worker session has no establishment time"))?;
    let expires_at = record
        .get("expires_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| attestation("Phala worker session has no expiry"))?;
    if expires_at <= now_unix().saturating_add(minimum_ttl) {
        return Err(attestation(
            "Phala worker session is expired or too close to expiry",
        ));
    }
    for claim in ["tee_attested", "tcb_up_to_date", "gpu_attested"] {
        if !asserted(&record, claim) {
            return Err(attestation(format!("Phala worker does not assert {claim}")));
        }
    }
    let bindings = record
        .get("channel_binding")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 1)
        .ok_or_else(|| attestation("Phala worker must have exactly one TLS channel binding"))?;
    let binding = object(&bindings[0], "worker channel binding")?;
    if binding.get("type").and_then(Value::as_str) != Some("tls_spki_sha256") {
        return Err(attestation(
            "Phala worker session has an unsupported channel binding",
        ));
    }
    let endpoint = required_str(&record, "endpoint", "worker endpoint")?;
    if required_str(binding, "origin", "worker channel origin")?
        != worker_endpoint_origin(endpoint)?
    {
        return Err(attestation(
            "Phala worker TLS binding origin does not match its endpoint",
        ));
    }
    let binding_spki = required_str(binding, "spki_sha256", "worker TLS SPKI")?.to_lowercase();
    let binding_spki_bytes = decode_32("worker TLS SPKI", &binding_spki)?;

    let evidence = record
        .get("evidence")
        .and_then(Value::as_object)
        .filter(|evidence| !evidence.is_empty())
        .ok_or_else(|| attestation("Phala worker session has no evidence"))?;
    let data_uri = required_str(evidence, "data", "worker evidence data")?;
    let encoded = data_uri
        .strip_prefix("data:application/json;base64,")
        .ok_or_else(|| attestation("Phala worker evidence is not JSON/base64"))?;
    let evidence_bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| attestation(format!("Phala worker evidence is invalid: {error}")))?;
    if evidence_bytes.is_empty() || evidence_bytes.len() > 2 * 1024 * 1024 {
        return Err(attestation("Phala worker evidence exceeds its size bound"));
    }
    let evidence_digest = sha256_digest(&evidence_bytes);
    if evidence.get("digest").and_then(Value::as_str) != Some(evidence_digest.as_str()) {
        return Err(attestation("Phala worker evidence digest mismatch"));
    }
    let decoded: Value = serde_json::from_slice(&evidence_bytes)
        .map_err(|_| attestation("Phala worker evidence JSON is invalid"))?;
    let report_value = normalized_worker_report(decoded)?;
    let report = object(&report_value, "worker attestation report")?;
    if !required_str(report, "signing_algo", "worker signing algorithm")?
        .eq_ignore_ascii_case("ecdsa")
    {
        return Err(attestation("Phala worker attestation is not ECDSA"));
    }
    let (nonce_value, tls_fingerprint) = pinned_worker_report_bindings(report)?;
    let nonce = decode_32("worker attestation nonce", nonce_value)?;
    let signing_address =
        required_str(report, "signing_address", "worker signing address")?.to_lowercase();
    let signing_material = decode_hex("worker signing address", &signing_address)?;
    if !signing_address.starts_with("0x") || signing_material.len() != 20 {
        return Err(attestation("Phala worker signing address is invalid"));
    }
    let upstream_name = required_str(&record, "upstream_name", "worker name")?;
    let expected_worker = expected_worker_identity(profile, expected_model)?;
    if upstream_name != expected_worker.upstream_name
        || canonical_worker_model(&record) != Some(expected_worker.canonical_model_id)
    {
        return Err(attestation(
            "Phala worker does not match the catalog's pinned upstream identity",
        ));
    }
    if report
        .get("model_name")
        .and_then(Value::as_str)
        .is_some_and(|model| model != upstream_name)
    {
        return Err(attestation(
            "Phala worker attestation model does not match its session",
        ));
    }
    let identity = record
        .get("identity")
        .and_then(Value::as_object)
        .ok_or_else(|| attestation("Phala worker session has no attested identity"))?;
    if required_str(
        identity,
        "signing_address",
        "worker identity signing address",
    )?
    .to_lowercase()
        != signing_address
    {
        return Err(attestation(
            "Phala worker identity does not match its attested signing address",
        ));
    }
    let tls_fingerprint = tls_fingerprint.to_lowercase();
    if tls_fingerprint != binding_spki {
        return Err(attestation(
            "Phala worker TLS binding does not match its attested fingerprint",
        ));
    }
    let nvidia_payload = report
        .get("nvidia_payload")
        .ok_or_else(|| attestation("Phala worker NVIDIA evidence is missing"))?;
    let nvidia_payload = parse_json_object(nvidia_payload, "worker NVIDIA evidence")?;
    if nvidia_payload
        .get("nonce")
        .and_then(Value::as_str)
        .is_none_or(|value| !value.eq_ignore_ascii_case(nonce_value))
    {
        return Err(attestation("Phala worker NVIDIA evidence nonce mismatch"));
    }
    let app_compose = worker_workload_manifest(report)?;
    let verification = object(verification_value, "worker verification supplement")?;
    let collateral: CollateralWire = serde_json::from_value(
        verification
            .get("intel_collateral")
            .cloned()
            .ok_or_else(|| attestation("Phala worker Intel collateral is missing"))?,
    )
    .map_err(|error| attestation(format!("Phala worker Intel collateral is invalid: {error}")))?;
    let quote = decode_quote(required_str(report, "intel_quote", "worker Intel quote")?)?;
    if quote
        .get(168)
        .copied()
        .is_none_or(|attributes| attributes != 0)
    {
        return Err(attestation(
            "Phala worker Intel quote is invalid or debug-enabled",
        ));
    }
    let verified =
        rustcrypto::verify(&quote, &collateral.into_qvl()?, now_unix()).map_err(|error| {
            attestation(format!(
                "Phala worker Intel DCAP verification failed: {error:#}"
            ))
        })?;
    if verified.status != "UpToDate" {
        return Err(attestation(format!(
            "Phala worker TDX status is {}; UpToDate is required",
            verified.status
        )));
    }
    let td = verified
        .report
        .as_td10()
        .ok_or_else(|| attestation("Phala worker quote is not a supported TDX report"))?;
    let expected_binding =
        Sha256::digest([signing_material.as_slice(), binding_spki_bytes.as_slice()].concat());
    if td.report_data[..32] != expected_binding[..] || td.report_data[32..] != nonce[..] {
        return Err(attestation(
            "Phala worker quote does not bind its identity, TLS key, and nonce",
        ));
    }
    let compose_hash: [u8; 32] = Sha256::digest(app_compose.as_bytes()).into();
    let mut expected_config_id = [0_u8; 48];
    expected_config_id[0] = 1;
    expected_config_id[1..33].copy_from_slice(&compose_hash);
    if td.mr_config_id != expected_config_id {
        return Err(attestation(
            "Phala worker workload is not bound to its Intel quote",
        ));
    }
    verify_nvidia_jwt(
        required_str(verification, "nvidia_jwt", "worker NVIDIA verdict")?,
        nvidia_jwks,
        nonce_value,
    )?;
    Ok(PhalaWorkerSession {
        session_id,
        established_at,
        expires_at,
        record: record_value,
    })
}

pub struct PhalaAttestor;

#[async_trait]
impl Attestor for PhalaAttestor {
    async fn verify_model(&self, request: AttestationRequest<'_>) -> Result<VerifiedModel> {
        let nonce = random_nonce_hex();
        let evidence = request
            .relay
            .provider_attestation_report(request.model_id, &nonce)
            .await?;
        if evidence.provider != PHALA_PROVIDER_ID
            || evidence.attestation_protocol != PHALA_ATTESTATION_PROTOCOL
        {
            return Err(attestation("Phala attestation identity mismatch"));
        }
        let bundle = object(&evidence.evidence, "evidence bundle")?;
        let report = object(
            bundle
                .get("gateway_report")
                .ok_or_else(|| attestation("Phala gateway report is missing"))?,
            "gateway report",
        )?;
        if report.get("api_version").and_then(Value::as_str) != Some("aci/1") {
            return Err(attestation("Phala gateway is not aci/1"));
        }
        let capabilities = report
            .get("service_capabilities")
            .and_then(Value::as_object)
            .ok_or_else(|| attestation("Phala gateway capabilities are missing"))?;
        if !capabilities
            .get("supported_e2ee_versions")
            .and_then(Value::as_array)
            .is_some_and(|versions| versions.iter().any(|value| value.as_str() == Some("2")))
        {
            return Err(attestation(
                "Phala gateway lacks the required ACI E2EE capability",
            ));
        }
        let gateway_attestation = report
            .get("attestation")
            .and_then(Value::as_object)
            .ok_or_else(|| attestation("Phala gateway attestation is missing"))?;
        if gateway_attestation.get("tee_type").and_then(Value::as_str) != Some("tdx") {
            return Err(attestation("Phala gateway is not Intel TDX"));
        }
        let keyset = gateway_attestation
            .get("workload_keyset")
            .ok_or_else(|| attestation("Phala gateway keyset is missing"))?;
        let keyset_object = object(keyset, "gateway keyset")?;
        let keyset_digest = sha256_digest(&canonical_json(keyset)?);
        if report.get("workload_keyset_digest").and_then(Value::as_str) != Some(&keyset_digest) {
            return Err(attestation("Phala gateway keyset digest mismatch"));
        }
        let gateway_evidence = gateway_attestation
            .get("evidence")
            .and_then(Value::as_object)
            .ok_or_else(|| attestation("Phala gateway TDX evidence is missing"))?;
        let app_compose = required_str(gateway_evidence, "app_compose", "gateway compose file")?;
        let (source_repository, source_commit) = measured_gateway_source(app_compose)?;
        let profile = select_gateway_profile(report, gateway_attestation, keyset_object)?;

        let (workload_id, keyset_expires, report_statement) = match profile {
            PhalaAciProfile::AciV1_08d550 => {
                let identity_key = keyset_object
                    .get("workload_identity")
                    .and_then(Value::as_object)
                    .and_then(|identity| identity.get("public_key"))
                    .ok_or_else(|| attestation("Phala workload identity key is missing"))?;
                let identity_key_object = object(identity_key, "workload identity key")?;
                let identity_algo =
                    required_str(identity_key_object, "algo", "workload identity algorithm")?;
                let identity_public_key =
                    required_str(identity_key_object, "public_key", "workload identity key")?;
                let workload_id = sha256_digest(&canonical_json(identity_key)?);
                if report.get("workload_id").and_then(Value::as_str) != Some(&workload_id) {
                    return Err(attestation("Phala workload identity digest mismatch"));
                }
                let endorsement = gateway_attestation
                    .get("keyset_endorsement")
                    .and_then(Value::as_object)
                    .ok_or_else(|| attestation("Phala keyset endorsement is missing"))?;
                if required_str(endorsement, "algo", "keyset endorsement algorithm")?
                    != identity_algo
                {
                    return Err(attestation("Phala keyset endorsement algorithm mismatch"));
                }
                let endorsement_payload = serde_json::json!({
                    "purpose": "aci.keyset.endorsement.v1",
                    "workload_keyset_digest": keyset_digest,
                });
                verify_keyset_endorsement(
                    identity_algo,
                    identity_public_key,
                    required_str(endorsement, "value", "keyset endorsement")?,
                    &canonical_json(&endorsement_payload)?,
                )?;
                let expires = keyset_object
                    .get("keyset_epoch")
                    .and_then(Value::as_object)
                    .and_then(|epoch| epoch.get("not_after"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| attestation("Phala gateway keyset epoch has no expiry"))?;
                let statement = serde_json::json!({
                    "purpose": "aci.report_data.v1",
                    "workload_id": workload_id,
                    "workload_keyset_digest": keyset_digest,
                    "nonce": nonce,
                });
                (Some(workload_id), expires, statement)
            }
            PhalaAciProfile::DstackAciDraftD567f6 => {
                let expires = keyset_object
                    .get("not_after")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| attestation("Phala gateway keyset has no expiry"))?;
                let statement = serde_json::json!({
                    "keyset_digest": keyset_digest,
                    "nonce": nonce,
                    "purpose": "aci.report_data.v1",
                });
                (None, expires, statement)
            }
        };
        if keyset_expires <= now_unix() {
            return Err(attestation("Phala gateway keyset has expired"));
        }
        let expected_report_data: [u8; 32] =
            Sha256::digest(canonical_json(&report_statement)?).into();
        if decode_32(
            "gateway report_data",
            required_str(gateway_attestation, "report_data", "gateway report data")?,
        )? != expected_report_data
        {
            return Err(attestation(
                "Phala gateway report data is not nonce/keyset bound",
            ));
        }
        let quote = decode_hex(
            "gateway quote",
            required_str(gateway_evidence, "quote", "gateway quote")?,
        )?;
        let collateral: CollateralWire = serde_json::from_value(
            bundle
                .get("intel_collateral")
                .cloned()
                .ok_or_else(|| attestation("Phala Intel collateral is missing"))?,
        )
        .map_err(|error| attestation(format!("Phala Intel collateral is invalid: {error}")))?;
        let verified =
            rustcrypto::verify(&quote, &collateral.into_qvl()?, now_unix()).map_err(|error| {
                attestation(format!("Phala Intel DCAP verification failed: {error:#}"))
            })?;
        if verified.status != "UpToDate" {
            return Err(attestation(format!(
                "Phala gateway TDX status is {}; UpToDate is required",
                verified.status
            )));
        }
        let td = verified
            .report
            .as_td10()
            .ok_or_else(|| attestation("Phala quote is not a supported TDX report"))?;
        let mut expected_quoted_data = [0_u8; 64];
        expected_quoted_data[..32].copy_from_slice(&expected_report_data);
        if td.report_data != expected_quoted_data {
            return Err(attestation(
                "Phala TDX quote does not bind the challenged keyset",
            ));
        }
        let event_log = required_str(gateway_evidence, "event_log", "gateway event log")?;
        let (replayed_rtmr3, measured_compose_hash) = replay_rtmr3(event_log)?;
        if td.rt_mr3 != replayed_rtmr3 {
            return Err(attestation(
                "Phala gateway event log does not replay to quoted RTMR3",
            ));
        }
        if hex::encode(Sha256::digest(app_compose.as_bytes())) != measured_compose_hash {
            return Err(attestation(
                "Phala gateway compose file is not measured by the quote",
            ));
        }
        verify_gateway_source(gateway_attestation, &source_repository, &source_commit)?;

        let e2ee_entries = keyset_object
            .get("e2ee_public_keys")
            .and_then(Value::as_array)
            .ok_or_else(|| attestation("Phala gateway has no E2EE keys"))?;
        let gateway_public_key_hex = select_gateway_e2ee_key(profile, e2ee_entries)?;

        let receipt_entries = keyset_object
            .get("receipt_signing_keys")
            .and_then(Value::as_array)
            .ok_or_else(|| attestation("Phala gateway has no receipt keys"))?;
        let receipt_public_keys = parse_receipt_public_keys(profile, receipt_entries)?;

        let policy = bundle
            .get("policy")
            .and_then(Value::as_object)
            .ok_or_else(|| attestation("Phala verification policy is missing"))?;
        if policy
            .get("required_worker_channel_binding")
            .and_then(Value::as_str)
            != Some("tls_spki_sha256")
            || policy.get("worker_evidence_policy").and_then(Value::as_str)
                != Some("near-direct-intel-tdx-nvidia-v1")
        {
            return Err(attestation(
                "Phala worker verification policy is unsupported",
            ));
        }
        let minimum_ttl = minimum_worker_ttl(policy)?;
        let session_values = bundle
            .get("worker_sessions")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .ok_or_else(|| attestation("Phala supplied no worker sessions"))?;
        let worker_verification = bundle
            .get("worker_verification_evidence")
            .and_then(Value::as_object)
            .ok_or_else(|| attestation("Phala worker verification evidence is missing"))?;
        let nvidia_jwks = fetch_nvidia_jwks().await?;
        let mut worker_sessions = BTreeMap::new();
        let mut expires_at = keyset_expires;
        for value in session_values {
            let session_id = value
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| attestation("Phala worker session id is missing"))?
                .to_lowercase();
            let supplement = worker_verification
                .get(&session_id)
                .ok_or_else(|| attestation("Phala worker verification supplement is missing"))?;
            let session = verify_worker_session(
                value,
                supplement,
                minimum_ttl,
                &nvidia_jwks,
                profile,
                request.expected_model,
            )
            .await?;
            expires_at = expires_at.min(session.expires_at);
            worker_sessions.insert(session.session_id.clone(), session);
        }
        let source_repository = canonical_repository_url(&source_repository).to_string();
        let state = PhalaAttestationState {
            profile,
            model: request.expected_model.to_string(),
            workload_id,
            keyset_digest,
            source_repository,
            source_commit: source_commit.clone(),
            gateway_public_key_hex: gateway_public_key_hex.clone(),
            receipt_public_keys,
            worker_sessions,
        };
        let profile_name = match profile {
            PhalaAciProfile::AciV1_08d550 => "aci-v1-08d550",
            PhalaAciProfile::DstackAciDraftD567f6 => "dstack-aci-draft-d567f6",
        };
        Ok(VerifiedModel {
            model_public_key_hex: gateway_public_key_hex,
            tls_fingerprint: None,
            checks: vec![
                VerificationCheck::new("gateway_tdx", "Gateway TDX", "UpToDate", true),
                VerificationCheck::new(
                    "gateway_source",
                    "Gateway workload",
                    &source_commit[..12],
                    true,
                ),
                VerificationCheck::new("gateway_profile", "ACI profile", profile_name, true),
                VerificationCheck::new("gateway_keyset", "Encrypted channel", "nonce-bound", true),
                VerificationCheck::new("worker_tee", "Worker TEE", "verified locally", true),
                VerificationCheck::new("worker_gpu", "Worker GPU", "verified locally", true),
                VerificationCheck::new("receipt", "Signed receipt", "required", true),
            ],
            provider_state: Some(
                serde_json::to_value(state)
                    .map_err(|error| attestation(format!("Phala state failed: {error}")))?,
            ),
            expires_at_unix: Some(expires_at.saturating_sub(15)),
        })
    }
}

pub struct PhalaCipher;

impl ProviderCipher for PhalaCipher {
    fn new_session(&self, verified: &VerifiedModel) -> Result<Box<dyn CipherSession>> {
        let state: PhalaAttestationState = serde_json::from_value(
            verified
                .provider_state
                .clone()
                .ok_or_else(|| provider("missing fresh locally verified Phala state"))?,
        )
        .map_err(|error| provider(format!("invalid verified Phala state: {error}")))?;
        if state.gateway_public_key_hex != verified.model_public_key_hex {
            return Err(provider("verified Phala gateway key mismatch"));
        }
        if verified
            .expires_at_unix
            .is_some_and(|expires| expires <= now_unix())
        {
            return Err(provider("verified Phala evidence has expired"));
        }
        let client_secret = StaticSecret::random_from_rng(OsRng);
        let client_public = PublicKey::from(&client_secret);
        let mut eligible_session_ids = state
            .worker_sessions
            .iter()
            .map(|(session_id, worker)| (session_id.clone(), worker.expires_at))
            .collect::<Vec<_>>();
        eligible_session_ids.sort_by(|(left_id, left_expiry), (right_id, right_expiry)| {
            right_expiry
                .cmp(left_expiry)
                .then_with(|| left_id.cmp(right_id))
        });
        eligible_session_ids.truncate(128);
        let eligible_session_ids = eligible_session_ids
            .into_iter()
            .map(|(session_id, _)| session_id)
            .collect::<Vec<_>>();
        // Keep freshness as the selection criterion, but canonicalize the selected set before it
        // becomes part of the exact request body and provider context. The backend rejects
        // duplicate or non-canonical session-id lists before relaying ciphertext.
        let mut eligible_session_ids = eligible_session_ids;
        eligible_session_ids.sort();
        if eligible_session_ids.is_empty() {
            return Err(provider("Phala has no preverified worker sessions"));
        }
        Ok(Box::new(PhalaSession {
            state,
            client_secret,
            client_public_key_hex: hex::encode(client_public.as_bytes()),
            eligible_session_ids,
            nonce: random_nonce_hex(),
            timestamp: now_unix(),
            request_body_hash: None,
            request_plaintext_body_hash: None,
            request_plaintext_fields: BTreeMap::new(),
            streamed_content: String::new(),
            streamed_reasoning: String::new(),
        }))
    }
}

struct PhalaSession {
    state: PhalaAttestationState,
    client_secret: StaticSecret,
    client_public_key_hex: String,
    eligible_session_ids: Vec<String>,
    nonce: String,
    timestamp: u64,
    request_body_hash: Option<String>,
    request_plaintext_body_hash: Option<String>,
    request_plaintext_fields: BTreeMap<String, String>,
    streamed_content: String,
    streamed_reasoning: String,
}

fn request_aad(model: &str, nonce: &str, timestamp: u64, field: &str) -> Result<Vec<u8>> {
    canonical_json(&serde_json::json!({
        "purpose": "aci.e2ee.request.v2",
        "algo": E2EE_ALGORITHM,
        "model": model,
        "field": field,
        "nonce": nonce,
        "ts": timestamp,
    }))
}

fn response_aad(
    model: &str,
    nonce: &str,
    timestamp: u64,
    response_id: &str,
    field: &str,
) -> Result<Vec<u8>> {
    canonical_json(&serde_json::json!({
        "purpose": "aci.e2ee.response.v2",
        "algo": E2EE_ALGORITHM,
        "model": model,
        "id": response_id,
        "field": field,
        "nonce": nonce,
        "ts": timestamp,
    }))
}

fn derive_key(shared: &[u8; 32]) -> Result<[u8; 32]> {
    let mut key = [0_u8; 32];
    Hkdf::<Sha256>::new(None, shared)
        .expand(HKDF_INFO, &mut key)
        .map_err(|_| provider("Phala HKDF expansion failed"))?;
    Ok(key)
}

fn encrypt_field(plaintext: &[u8], recipient_hex: &str, aad: &[u8]) -> Result<String> {
    let recipient = PublicKey::from(decode_32("gateway X25519 key", recipient_hex)?);
    let ephemeral_secret = StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let key = derive_key(ephemeral_secret.diffie_hellman(&recipient).as_bytes())?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| provider("Phala AES key setup failed"))?;
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| provider("Phala request encryption failed"))?;
    Ok(hex::encode(
        [ephemeral_public.as_bytes().as_slice(), &nonce, &ciphertext].concat(),
    ))
}

fn decrypt_field(wire_hex: &str, secret: &StaticSecret, aad: &[u8]) -> Result<String> {
    let wire = hex::decode(wire_hex).map_err(|_| provider("Phala response envelope is not hex"))?;
    if wire.len() < 60 {
        return Err(provider("Phala response envelope is too short"));
    }
    let ephemeral = PublicKey::from(
        <[u8; 32]>::try_from(&wire[..32])
            .map_err(|_| provider("Phala response ephemeral key is invalid"))?,
    );
    let key = derive_key(secret.diffie_hellman(&ephemeral).as_bytes())?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| provider("Phala AES key setup failed"))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&wire[32..44]),
            Payload {
                msg: &wire[44..],
                aad,
            },
        )
        .map_err(|_| provider("Phala response decryption failed"))?;
    String::from_utf8(plaintext).map_err(|_| provider("Phala response is not UTF-8"))
}

struct PythonCompactUtf8Formatter;

impl Formatter for PythonCompactUtf8Formatter {
    fn write_f32<W>(&mut self, writer: &mut W, value: f32) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        writer.write_all(python_float_repr(f64::from(value)).as_bytes())
    }

    fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        writer.write_all(python_float_repr(value).as_bytes())
    }

    fn write_number_str<W>(&mut self, writer: &mut W, value: &str) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        if value.contains('.') || value.contains('e') || value.contains('E') {
            if let Ok(number) = value.parse::<f64>() {
                if number.is_finite() {
                    return writer.write_all(python_float_repr(number).as_bytes());
                }
            }
        }
        writer.write_all(value.as_bytes())
    }
}

fn python_float_repr(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    let mut buffer = ryu::Buffer::new();
    let rendered = buffer.format_finite(value);
    let (negative, unsigned) = rendered
        .strip_prefix('-')
        .map_or((false, rendered), |value| (true, value));
    let (mantissa, explicit_exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, None), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().ok())
        });
    let decimal_index = mantissa.find('.').unwrap_or(mantissa.len());
    let mut digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let first_nonzero = digits.find(|character| character != '0').unwrap_or(0);
    let decimal_index = i32::try_from(decimal_index).expect("a float spelling is bounded");
    let first_nonzero_i32 = i32::try_from(first_nonzero).expect("a float spelling is bounded");
    let exponent = explicit_exponent.unwrap_or(decimal_index - first_nonzero_i32 - 1);
    digits.drain(..first_nonzero);
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }

    let mut output = String::new();
    if negative {
        output.push('-');
    }
    if !(-4..16).contains(&exponent) {
        output.push(digits.as_bytes()[0] as char);
        if digits.len() > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        output.push('e');
        output.push(if exponent < 0 { '-' } else { '+' });
        let absolute = exponent.unsigned_abs();
        if absolute < 10 {
            output.push('0');
        }
        output.push_str(&absolute.to_string());
    } else if exponent < 0 {
        output.push_str("0.");
        let zero_count = usize::try_from(-exponent - 1).expect("negative exponent was checked");
        output.extend(std::iter::repeat_n('0', zero_count));
        output.push_str(&digits);
    } else {
        let integer_digits = usize::try_from(exponent).expect("positive exponent was checked") + 1;
        if digits.len() <= integer_digits {
            output.push_str(&digits);
            output.extend(std::iter::repeat_n('0', integer_digits - digits.len()));
            output.push_str(".0");
        } else {
            output.push_str(&digits[..integer_digits]);
            output.push('.');
            output.push_str(&digits[integer_digits..]);
        }
    }
    output
}

fn python_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, PythonCompactUtf8Formatter);
    value
        .serialize(&mut serializer)
        .map_err(|error| provider(format!("JSON failed: {error}")))?;
    Ok(bytes)
}

fn append_json_member<T>(output: &mut Vec<u8>, first: &mut bool, key: &str, value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    if !*first {
        output.push(b',');
    }
    *first = false;
    output.extend_from_slice(&python_json_bytes(key)?);
    output.push(b':');
    output.extend_from_slice(&python_json_bytes(value)?);
    Ok(())
}

#[derive(Serialize)]
struct ExactWireMessage<'a> {
    role: &'a str,
    // The upstream distinguishes an omitted field, an explicit assistant null, and ciphertext.
    #[allow(clippy::option_option)]
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Option<&'a str>>,
}

#[derive(Serialize)]
struct ExactProviderOptions<'a> {
    aci_verified: bool,
    aci_session_ids: &'a [String],
    zdr: bool,
}

#[derive(Serialize)]
struct ExactStreamOptions {
    include_usage: bool,
}

fn exact_provider_body(request: &ProviderWireRequest, session_ids: &[String]) -> Result<Vec<u8>> {
    if request.has_tools
        || request
            .messages
            .iter()
            .any(|message| message.has_extended_fields)
    {
        return Err(provider(
            "Phala ACI tool and extended-message fields are unavailable until their v2 AAD contract is implemented",
        ));
    }
    let mut messages = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        let content = if let Some(content) = message.content.as_deref() {
            Some(Some(content))
        } else if message.assistant_null_content {
            Some(None)
        } else {
            None
        };
        messages.push(ExactWireMessage {
            role: &message.role,
            content,
        });
    }

    let mut output = vec![b'{'];
    let mut first = true;
    append_json_member(&mut output, &mut first, "model", &request.model)?;
    append_json_member(&mut output, &mut first, "messages", &messages)?;
    append_json_member(&mut output, &mut first, "max_tokens", &request.max_tokens)?;
    append_json_member(&mut output, &mut first, "stream", &request.stream)?;
    append_json_member(
        &mut output,
        &mut first,
        "provider",
        &ExactProviderOptions {
            aci_verified: true,
            aci_session_ids: session_ids,
            zdr: true,
        },
    )?;
    if request.stream {
        append_json_member(
            &mut output,
            &mut first,
            "stream_options",
            &ExactStreamOptions {
                include_usage: true,
            },
        )?;
    }
    if let Some(sampling) = &request.sampling {
        let values = sampling
            .as_object()
            .ok_or_else(|| provider("Phala sampling options are invalid"))?;
        if values.keys().any(|key| {
            !matches!(
                key.as_str(),
                "temperature"
                    | "top_p"
                    | "top_k"
                    | "min_p"
                    | "frequency_penalty"
                    | "presence_penalty"
                    | "seed"
            )
        }) {
            return Err(provider("Phala sampling options are invalid"));
        }
        for key in [
            "temperature",
            "top_p",
            "top_k",
            "min_p",
            "frequency_penalty",
            "presence_penalty",
            "seed",
        ] {
            if let Some(value) = values.get(key) {
                if !valid_sampling_number(key, value) {
                    return Err(provider("Phala sampling options are invalid"));
                }
                append_json_member(&mut output, &mut first, key, value)?;
            }
        }
    }
    if let Some(response_format) = &request.response_format {
        if !fixed_public_response_format(response_format) {
            return Err(provider(
                "Phala schema-bearing response formats require provider-E2EE metadata support",
            ));
        }
        append_json_member(&mut output, &mut first, "response_format", response_format)?;
    }
    if let Some(reasoning_effort) = &request.reasoning_effort {
        let reasoning_effort = if reasoning_effort == "xhigh" {
            "max"
        } else {
            reasoning_effort
        };
        append_json_member(
            &mut output,
            &mut first,
            "reasoning_effort",
            reasoning_effort,
        )?;
    }
    output.push(b'}');
    Ok(output)
}

fn valid_sampling_number(name: &str, value: &Value) -> bool {
    match name {
        "temperature" => value
            .as_f64()
            .is_some_and(|number| number.is_finite() && (0.0..=2.0).contains(&number)),
        "top_p" | "min_p" => value
            .as_f64()
            .is_some_and(|number| number.is_finite() && (0.0..=1.0).contains(&number)),
        "top_k" => value.as_u64().is_some_and(|number| number <= 1_000_000),
        "frequency_penalty" | "presence_penalty" => value
            .as_f64()
            .is_some_and(|number| number.is_finite() && (-2.0..=2.0).contains(&number)),
        "seed" => value
            .as_i64()
            .is_some_and(|number| (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&number)),
        _ => false,
    }
}

fn fixed_public_response_format(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 1
            && matches!(
                object.get("type").and_then(Value::as_str),
                Some("text" | "json_object")
            )
    })
}

fn exact_plaintext_provider_body(
    request: &ProviderWireRequest,
    session_ids: &[String],
    plaintext_fields: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut body = exact_provider_body(request, session_ids)?;
    for (index, message) in request.messages.iter().enumerate() {
        if let Some(wire) = message.content.as_deref() {
            let field = format!("messages.{index}.content");
            let plaintext = plaintext_fields
                .get(&field)
                .ok_or_else(|| provider(format!("Phala omitted plaintext for {field}")))?;
            // Restore each JSON string in place instead of parsing and reserializing the body.
            // That preserves the exact compact serialization (including Python-compatible float
            // spellings) that the gateway hashes after field decryption.
            replace_json_string_once(&mut body, wire, plaintext)?;
        }
    }
    Ok(body)
}

fn receipt_event<'a>(
    receipt: &'a Map<String, Value>,
    event_type: &str,
) -> Result<&'a Map<String, Value>> {
    receipt
        .get("event_log")
        .and_then(Value::as_array)
        .and_then(|events| {
            events
                .iter()
                .filter_map(Value::as_object)
                .find(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        })
        .ok_or_else(|| provider(format!("Phala receipt omitted {event_type}")))
}

fn validate_receipt_document(profile: PhalaAciProfile, receipt: &Map<String, Value>) -> Result<()> {
    let allowed = |field: &str| {
        matches!(
            field,
            "api_version"
                | "receipt_id"
                | "chat_id"
                | "model"
                | "workload_keyset_digest"
                | "endpoint"
                | "method"
                | "served_at"
                | "event_log"
                | "signature"
        ) || match profile {
            PhalaAciProfile::AciV1_08d550 => field == "workload_id",
            PhalaAciProfile::DstackAciDraftD567f6 => field == "key_id",
        }
    };
    if receipt.keys().any(|field| !allowed(field))
        || receipt
            .get("receipt_id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || receipt.get("endpoint").and_then(Value::as_str) != Some("/v1/chat/completions")
        || receipt.get("method").and_then(Value::as_str) != Some("POST")
        || !receipt
            .get("chat_id")
            .is_some_and(|value| value.is_null() || value.is_string())
        || !receipt
            .get("event_log")
            .and_then(Value::as_array)
            .is_some_and(|events| !events.is_empty() && events.iter().all(Value::is_object))
    {
        return Err(provider("Phala receipt document shape is invalid"));
    }
    Ok(())
}

fn replace_json_string_once(body: &mut Vec<u8>, wire: &str, plaintext: &str) -> Result<()> {
    let needle = python_json_bytes(wire)?;
    let replacement = python_json_bytes(plaintext)?;
    let positions = body
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, value)| (value == needle).then_some(index))
        .collect::<Vec<_>>();
    if positions.len() != 1 {
        return Err(provider(
            "Phala could not reconstruct the exact cleartext response bytes",
        ));
    }
    body.splice(positions[0]..positions[0] + needle.len(), replacement);
    Ok(())
}

impl PhalaSession {
    fn verify_cleartext_hash(&self, proof: &Value, cleartext: &[u8]) -> Result<()> {
        if self.state.profile == PhalaAciProfile::DstackAciDraftD567f6 {
            return Ok(());
        }
        let receipt = proof
            .get("receipt")
            .and_then(Value::as_object)
            .ok_or_else(|| provider("Phala proof omitted its receipt"))?;
        if receipt_event(receipt, "response.returned")?
            .get("cleartext_hash")
            .and_then(Value::as_str)
            != Some(&sha256_digest(cleartext))
        {
            return Err(provider(
                "Phala receipt does not bind the exact cleartext response bytes",
            ));
        }
        Ok(())
    }

    fn verify_proof_bytes(&self, proof: &Value) -> Result<Vec<u8>> {
        let expected_request_hash = self
            .request_plaintext_body_hash
            .as_deref()
            .ok_or_else(|| provider("Phala request context was not prepared"))?;
        let proof = proof
            .as_object()
            .ok_or_else(|| provider("Phala stream omitted its proof"))?;
        let response_base64 = proof
            .get("response_body_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| provider("Phala proof omitted encrypted response bytes"))?;
        if response_base64.len() > (MAX_PROOF_RESPONSE_BYTES / 3 + 1) * 4 {
            return Err(provider("Phala response proof body is too large"));
        }
        let response_bytes = base64::engine::general_purpose::STANDARD
            .decode(response_base64)
            .map_err(|_| provider("Phala encrypted response base64 is invalid"))?;
        if response_bytes.is_empty() || response_bytes.len() > MAX_PROOF_RESPONSE_BYTES {
            return Err(provider("Phala response proof body is invalid"));
        }
        let receipt = proof
            .get("receipt")
            .and_then(Value::as_object)
            .ok_or_else(|| provider("Phala proof omitted its receipt"))?;
        if receipt.get("api_version").and_then(Value::as_str) != Some("aci/1") {
            return Err(provider("Phala receipt is not aci/1"));
        }
        match self.state.profile {
            PhalaAciProfile::AciV1_08d550 if receipt.contains_key("key_id") => {
                return Err(provider(
                    "Phala standard receipt used the retired draft shape",
                ));
            }
            PhalaAciProfile::DstackAciDraftD567f6 if receipt.contains_key("workload_id") => {
                return Err(provider("Phala draft receipt used a hybrid shape"));
            }
            _ => {}
        }
        validate_receipt_document(self.state.profile, receipt)?;
        if receipt
            .get("workload_keyset_digest")
            .and_then(Value::as_str)
            != Some(&self.state.keyset_digest)
            || receipt.get("model").and_then(Value::as_str) != Some(&self.state.model)
        {
            return Err(provider("Phala receipt identity mismatch"));
        }
        if self.state.profile == PhalaAciProfile::AciV1_08d550
            && receipt.get("workload_id").and_then(Value::as_str)
                != self.state.workload_id.as_deref()
        {
            return Err(provider("Phala receipt workload identity mismatch"));
        }
        let (key_id, signature_algo, signature_value, unsigned) = match self.state.profile {
            PhalaAciProfile::AciV1_08d550 => {
                if receipt.contains_key("key_id") {
                    return Err(provider(
                        "Phala standard receipt used the retired draft shape",
                    ));
                }
                let signature = receipt
                    .get("signature")
                    .and_then(Value::as_object)
                    .ok_or_else(|| provider("Phala receipt signature block is missing"))?;
                let key_id = required_str(signature, "key_id", "receipt signature key id")
                    .map_err(|_| provider("Phala receipt key id is missing"))?;
                let algo = required_str(signature, "algo", "receipt signature algorithm")
                    .map_err(|_| provider("Phala receipt signature algorithm is missing"))?;
                let value = required_str(signature, "value", "receipt signature value")
                    .map_err(|_| provider("Phala receipt signature is missing"))?;
                let mut unsigned = receipt.clone();
                unsigned
                    .get_mut("signature")
                    .and_then(Value::as_object_mut)
                    .expect("signature object was checked")
                    .remove("value");
                (key_id, algo, value, unsigned)
            }
            PhalaAciProfile::DstackAciDraftD567f6 => {
                if receipt.contains_key("workload_id")
                    || receipt.get("signature").is_some_and(Value::is_object)
                {
                    return Err(provider("Phala draft receipt used a hybrid shape"));
                }
                let key_id = receipt
                    .get("key_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| provider("Phala receipt key id is missing"))?;
                let value = receipt
                    .get("signature")
                    .and_then(Value::as_str)
                    .ok_or_else(|| provider("Phala receipt signature is missing"))?;
                let mut unsigned = receipt.clone();
                unsigned.remove("signature");
                (key_id, "ed25519", value, unsigned)
            }
        };
        let receipt_key = self
            .state
            .receipt_public_keys
            .get(key_id)
            .ok_or_else(|| provider("Phala receipt key was not attested"))?;
        if signature_algo != receipt_key.algo {
            return Err(provider("Phala receipt signature algorithm mismatch"));
        }
        let canonical_receipt = canonical_json(&Value::Object(unsigned))
            .map_err(|_| provider("Phala receipt canonicalization failed"))?;
        verify_receipt_signature(
            self.state.profile,
            receipt_key,
            signature_value,
            &canonical_receipt,
        )?;
        if receipt_event(receipt, "request.received")?
            .get("body_hash")
            .and_then(Value::as_str)
            != Some(expected_request_hash)
        {
            return Err(provider(
                "Phala receipt does not bind the exact local request",
            ));
        }
        let events = receipt
            .get("event_log")
            .and_then(Value::as_array)
            .ok_or_else(|| provider("Phala receipt has no event log"))?;
        if events
            .first()
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str)
            != Some("request.received")
        {
            return Err(provider(
                "Phala receipt does not begin with request.received",
            ));
        }
        let returned = receipt_event(receipt, "response.returned")?;
        let wire_hash_field = match self.state.profile {
            PhalaAciProfile::AciV1_08d550 => "wire_hash",
            PhalaAciProfile::DstackAciDraftD567f6 => "body_hash",
        };
        if returned.get(wire_hash_field).and_then(Value::as_str)
            != Some(&sha256_digest(&response_bytes))
        {
            return Err(provider(
                "Phala receipt does not bind the encrypted response bytes",
            ));
        }
        if self.state.profile == PhalaAciProfile::AciV1_08d550 {
            if events.iter().enumerate().any(|(index, event)| {
                event.get("seq").and_then(Value::as_u64) != Some(index as u64)
            }) {
                return Err(provider("Phala receipt event sequence is invalid"));
            }
            for required_type in ["request.received", "request.forwarded", "response.returned"] {
                if events
                    .iter()
                    .filter(|event| {
                        event.get("type").and_then(Value::as_str) == Some(required_type)
                    })
                    .count()
                    != 1
                {
                    return Err(provider(format!(
                        "Phala receipt has duplicate or missing {required_type} events"
                    )));
                }
            }
            let forwarded = receipt_event(receipt, "request.forwarded")?;
            let forwarded_hash = forwarded
                .get("body_hash")
                .and_then(Value::as_str)
                .filter(|value| is_sha256_digest(value))
                .ok_or_else(|| provider("Phala forwarded request hash is invalid"))?;
            let request_modified_count = events
                .iter()
                .filter(|event| {
                    event.get("type").and_then(Value::as_str)
                        == Some("transparency.request_modified")
                })
                .count();
            if (forwarded_hash != expected_request_hash) != (request_modified_count == 1)
                || request_modified_count > 1
            {
                return Err(provider(
                    "Phala receipt request transparency events are inconsistent",
                ));
            }
            let cleartext_hash = returned
                .get("cleartext_hash")
                .and_then(Value::as_str)
                .filter(|value| is_sha256_digest(value))
                .ok_or_else(|| provider("Phala response cleartext hash is missing"))?;
            let response_modified_count = events
                .iter()
                .filter(|event| {
                    event.get("type").and_then(Value::as_str)
                        == Some("transparency.response_modified")
                })
                .count();
            if (Some(cleartext_hash) != returned.get("wire_hash").and_then(Value::as_str))
                != (response_modified_count == 1)
                || response_modified_count > 1
            {
                return Err(provider(
                    "Phala receipt response transparency events are inconsistent",
                ));
            }
        } else {
            if events.iter().any(|event| {
                event
                    .as_object()
                    .is_some_and(|event| event.contains_key("seq"))
            }) {
                return Err(provider("Phala draft receipt used a hybrid event shape"));
            }
            for required_type in ["request.received", "response.returned"] {
                if events
                    .iter()
                    .filter(|event| {
                        event.get("type").and_then(Value::as_str) == Some(required_type)
                    })
                    .count()
                    != 1
                {
                    return Err(provider(format!(
                        "Phala draft receipt has duplicate or missing {required_type} events"
                    )));
                }
            }
        }
        let matching_upstreams = events
            .iter()
            .filter_map(Value::as_object)
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some("upstream.verified")
                    && event.get("result").and_then(Value::as_str) == Some("verified")
                    && event
                        .get("session_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| self.eligible_session_ids.iter().any(|value| value == id))
            })
            .collect::<Vec<_>>();
        if matching_upstreams.len() != 1 {
            return Err(provider(
                "Phala receipt has no unique verified eligible upstream",
            ));
        }
        let upstream = matching_upstreams[0];
        if upstream.get("result").and_then(Value::as_str) != Some("verified")
            || upstream.get("required").and_then(Value::as_bool) != Some(true)
            || upstream.get("model_id").and_then(Value::as_str)
                != Some(
                    expected_worker_identity(self.state.profile, &self.state.model)
                        .map_err(|_| provider("Phala model has no pinned worker mapping"))?
                        .canonical_model_id,
                )
        {
            return Err(provider("Phala did not require a verified model worker"));
        }
        let session_id = upstream
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        if self.state.profile == PhalaAciProfile::AciV1_08d550
            && (session_id.len() != 67 || !session_id.starts_with("as_"))
        {
            return Err(provider("Phala standard receipt has an invalid session id"));
        }
        if self.state.profile == PhalaAciProfile::DstackAciDraftD567f6
            && (session_id.len() != 64 || session_id.starts_with("as_"))
        {
            return Err(provider("Phala draft receipt has an invalid session id"));
        }
        if !self.eligible_session_ids.contains(&session_id) {
            return Err(provider(
                "Phala receipt cites a worker outside the preverified eligible set",
            ));
        }
        if proof.get("worker_session_id").and_then(Value::as_str) != Some(session_id.as_str()) {
            return Err(provider(
                "Phala response proof worker identity does not match its signed receipt",
            ));
        }
        let worker = self
            .state
            .worker_sessions
            .get(&session_id)
            .ok_or_else(|| provider("Phala receipt cites a worker not preverified locally"))?;
        let served_at = receipt
            .get("served_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| provider("Phala receipt served_at is invalid"))?;
        if !(worker.established_at <= served_at && served_at <= worker.expires_at) {
            return Err(provider(
                "Phala receipt falls outside the worker session lifetime",
            ));
        }
        if self.state.profile == PhalaAciProfile::AciV1_08d550 {
            let worker_record = worker
                .record
                .as_object()
                .ok_or_else(|| provider("Phala verified worker record is invalid"))?;
            let expected_worker =
                expected_worker_identity(self.state.profile, &self.state.model)
                    .map_err(|_| provider("Phala model has no pinned worker mapping"))?;
            let expected_origin = worker_endpoint_origin(
                required_str(worker_record, "endpoint", "worker endpoint")
                    .map_err(|_| provider("Phala worker endpoint is missing"))?,
            )
            .map_err(|_| provider("Phala worker endpoint is invalid"))?;
            let provider_claims = upstream
                .get("provider_claims")
                .and_then(Value::as_object)
                .ok_or_else(|| provider("Phala receipt omitted verified provider facts"))?;
            let claims_extra = worker_record
                .get("claims")
                .and_then(Value::as_object)
                .and_then(|claims| claims.get("extra"))
                .and_then(Value::as_object)
                .ok_or_else(|| provider("Phala worker omitted verified provider facts"))?;
            let fields_match = upstream.get("upstream_name").and_then(Value::as_str)
                == Some(expected_worker.upstream_name)
                && upstream.get("provider_type").and_then(Value::as_str)
                    == Some(expected_worker.provider_type)
                && upstream.get("model_id").and_then(Value::as_str)
                    == Some(expected_worker.canonical_model_id)
                && upstream.get("url_origin").and_then(Value::as_str)
                    == Some(expected_origin.as_str())
                && upstream.get("verifier_id") == worker_record.get("verifier_id")
                && upstream.get("channel_bindings") == worker_record.get("channel_binding")
                && upstream.get("claims") == worker_record.get("claims")
                && upstream.get("reason").is_some_and(Value::is_null)
                && provider_claims == claims_extra;
            if !fields_match {
                return Err(provider(
                    "Phala receipt upstream facts do not match the locally verified session",
                ));
            }
        }
        if let Some(headers) = proof.get("response_headers").and_then(Value::as_object) {
            let header = |name: &str| {
                headers.iter().find_map(|(key, value)| {
                    key.eq_ignore_ascii_case(name)
                        .then(|| value.as_str())
                        .flatten()
                })
            };
            if self.state.profile == PhalaAciProfile::AciV1_08d550
                && (header("x-aci-version") != Some("aci/1")
                    || header("x-aci-identity") != self.state.workload_id.as_deref()
                    || header("x-aci-keyset-digest") != Some(self.state.keyset_digest.as_str())
                    || header("x-e2ee-applied") != Some("true")
                    || header("x-e2ee-version") != Some("2")
                    || header("x-e2ee-algo") != Some(E2EE_ALGORITHM))
            {
                return Err(provider(
                    "Phala response headers do not match attested state",
                ));
            }
        }
        Ok(response_bytes)
    }

    fn decrypt_verified_stream(
        &self,
        response_bytes: &[u8],
    ) -> Result<(String, String, Vec<u8>, String)> {
        let raw = std::str::from_utf8(response_bytes)
            .map_err(|_| provider("Phala stream is not UTF-8 SSE"))?;
        let normalized = raw.replace("\r\n", "\n");
        let mut response_id = String::new();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut replacements = Vec::new();
        let mut done = false;
        for frame in normalized.split("\n\n") {
            let data_lines = frame
                .lines()
                .filter_map(|line| {
                    if line == "data" {
                        Some("")
                    } else {
                        line.strip_prefix("data:")
                            .map(|value| value.strip_prefix(' ').unwrap_or(value))
                    }
                })
                .collect::<Vec<_>>();
            if data_lines.is_empty() {
                continue;
            }
            let data = data_lines.join("\n");
            if data == "[DONE]" {
                if done {
                    return Err(provider("Phala stream has duplicate completion markers"));
                }
                done = true;
                continue;
            }
            if done {
                return Err(provider("Phala stream has data after completion"));
            }
            let event: Value = serde_json::from_str(&data)
                .map_err(|_| provider("Phala stream contains invalid JSON"))?;
            let event = event
                .as_object()
                .ok_or_else(|| provider("Phala stream event is not an object"))?;
            if let Some(event_id) = event.get("id").and_then(Value::as_str) {
                if !event_id.is_empty() {
                    if !response_id.is_empty() && response_id != event_id {
                        return Err(provider("Phala stream changed response id"));
                    }
                    response_id = event_id.to_string();
                }
            }
            let Some(choices) = event.get("choices") else {
                continue;
            };
            let choices = choices
                .as_array()
                .ok_or_else(|| provider("Phala stream choices are invalid"))?;
            for (position, choice) in choices.iter().enumerate() {
                let choice = choice
                    .as_object()
                    .ok_or_else(|| provider("Phala stream choice is invalid"))?;
                let choice_index = choice
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(position as u64);
                if choice_index != 0 {
                    return Err(provider("Phala stream returned an unexpected choice"));
                }
                let Some(delta) = choice.get("delta") else {
                    continue;
                };
                let delta = delta
                    .as_object()
                    .ok_or_else(|| provider("Phala stream delta is invalid"))?;
                for member in ["content", "reasoning_content", "reasoning"] {
                    let Some(wire) = delta.get(member).and_then(Value::as_str) else {
                        continue;
                    };
                    if wire.is_empty() {
                        continue;
                    }
                    if response_id.is_empty() {
                        return Err(provider("Phala encrypted delta omitted response id"));
                    }
                    let plaintext = decrypt_field(
                        wire,
                        &self.client_secret,
                        &response_aad(
                            &self.state.model,
                            &self.nonce,
                            self.timestamp,
                            &response_id,
                            &format!("choices.{choice_index}.delta.{member}"),
                        )?,
                    )?;
                    replacements.push((wire.to_string(), plaintext.clone()));
                    if member == "content" {
                        content.push_str(&plaintext);
                    } else {
                        reasoning.push_str(&plaintext);
                    }
                }
            }
        }
        if !done {
            return Err(provider("Phala stream ended without its completion marker"));
        }
        if response_id.is_empty() {
            return Err(provider("Phala stream omitted its response id"));
        }
        let mut cleartext = response_bytes.to_vec();
        for (wire, plaintext) in replacements {
            replace_json_string_once(&mut cleartext, &wire, &plaintext)?;
        }
        Ok((content, reasoning, cleartext, response_id))
    }
}

impl CipherSession for PhalaSession {
    fn client_public_key_hex(&self) -> &str {
        &self.client_public_key_hex
    }

    fn is_ready(&self) -> bool {
        !self.state.worker_sessions.is_empty()
            && !self.state.receipt_public_keys.is_empty()
            && self.timestamp.saturating_add(300) >= now_unix()
    }

    fn decrypt_stream_field(
        &mut self,
        wire: &str,
        response_id: Option<&str>,
        field: Option<&str>,
    ) -> Result<Vec<u8>> {
        let response_id = response_id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| provider("Phala stream delta omitted response id"))?;
        let field = field
            .filter(|value| {
                matches!(
                    *value,
                    "choices.0.delta.content"
                        | "choices.0.delta.reasoning"
                        | "choices.0.delta.reasoning_content"
                )
            })
            .ok_or_else(|| provider("Phala stream delta has an unexpected response field"))?;
        let plaintext = decrypt_field(
            wire,
            &self.client_secret,
            &response_aad(
                &self.state.model,
                &self.nonce,
                self.timestamp,
                response_id,
                field,
            )?,
        )?;
        if field == "choices.0.delta.content" {
            self.streamed_content.push_str(&plaintext);
        } else {
            self.streamed_reasoning.push_str(&plaintext);
        }
        Ok(plaintext.into_bytes())
    }

    fn encrypt_field(&mut self, plaintext: &[u8], field: &str) -> Result<String> {
        let plaintext_utf8 = std::str::from_utf8(plaintext)
            .map_err(|_| provider("Phala encrypted fields must be UTF-8"))?;
        if self
            .request_plaintext_fields
            .insert(field.to_string(), plaintext_utf8.to_string())
            .is_some()
        {
            return Err(provider("Phala request field was encrypted more than once"));
        }
        encrypt_field(
            plaintext,
            &self.state.gateway_public_key_hex,
            &request_aad(&self.state.model, &self.nonce, self.timestamp, field)?,
        )
    }

    fn prepare_request_context(&mut self, request: &ProviderWireRequest) -> Result<Value> {
        if request.model != self.state.model {
            return Err(provider("Phala provider request model mismatch"));
        }
        let session_ids = self.eligible_session_ids.clone();
        let request_hash = sha256_digest(&exact_provider_body(request, &session_ids)?);
        let plaintext_request_hash = sha256_digest(&exact_plaintext_provider_body(
            request,
            &session_ids,
            &self.request_plaintext_fields,
        )?);
        self.request_body_hash = Some(request_hash.clone());
        self.request_plaintext_body_hash = Some(plaintext_request_hash.clone());
        Ok(serde_json::json!({
            "artifact_profile": self.state.profile.artifact_profile(),
            "nonce": self.nonce,
            "timestamp": self.timestamp,
            "session_ids": session_ids,
            "keyset_digest": self.state.keyset_digest,
            "request_body_hash": request_hash,
            "request_plaintext_body_hash": plaintext_request_hash,
        }))
    }

    fn decrypt_verified_completion(
        &mut self,
        completion: &RelayCompletion,
    ) -> Result<DecryptedProviderCompletion> {
        let proof_value = &completion.proof;
        let response_bytes = self.verify_proof_bytes(proof_value)?;

        let response: Value = serde_json::from_slice(&response_bytes)
            .map_err(|error| provider(format!("Phala response JSON is invalid: {error}")))?;
        let response = response
            .as_object()
            .ok_or_else(|| provider("Phala response is not an object"))?;
        let response_id = required_str(response, "id", "response id")
            .map_err(|_| provider("Phala response id is missing"))?;
        if proof_value
            .get("receipt")
            .and_then(|value| value.get("chat_id"))
            .and_then(Value::as_str)
            != Some(response_id)
        {
            return Err(provider("Phala receipt chat identity mismatch"));
        }
        let choice = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(Value::as_object)
            .ok_or_else(|| provider("Phala response has no choice"))?;
        let choice_index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
        let message = choice
            .get("message")
            .and_then(Value::as_object)
            .ok_or_else(|| provider("Phala response has no message"))?;
        let decrypt_member = |member: &str| -> Result<Option<String>> {
            let Some(wire) = message.get(member).and_then(Value::as_str) else {
                return Ok(None);
            };
            if wire.is_empty() {
                return Ok(None);
            }
            Ok(Some(decrypt_field(
                wire,
                &self.client_secret,
                &response_aad(
                    &self.state.model,
                    &self.nonce,
                    self.timestamp,
                    response_id,
                    &format!("choices.{choice_index}.message.{member}"),
                )?,
            )?))
        };
        let content = decrypt_member("content")?;
        let reasoning_content = match decrypt_member("reasoning_content")? {
            Some(reasoning) => Some(reasoning),
            None => decrypt_member("reasoning")?,
        };
        if content.is_none() && reasoning_content.is_none() {
            return Err(provider("Phala completion contained no encrypted output"));
        }
        let mut cleartext_bytes = response_bytes.clone();
        if let (Some(wire), Some(plaintext)) = (
            message.get("content").and_then(Value::as_str),
            content.as_deref(),
        ) {
            replace_json_string_once(&mut cleartext_bytes, wire, plaintext)?;
        }
        let reasoning_member = if message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            "reasoning_content"
        } else {
            "reasoning"
        };
        if let (Some(wire), Some(plaintext)) = (
            message.get(reasoning_member).and_then(Value::as_str),
            reasoning_content.as_deref(),
        ) {
            replace_json_string_once(&mut cleartext_bytes, wire, plaintext)?;
        }
        self.verify_cleartext_hash(proof_value, &cleartext_bytes)?;
        Ok(DecryptedProviderCompletion {
            content,
            reasoning_content,
            refusal: None,
        })
    }

    fn verify_stream_completion(&mut self, proof: &Value) -> Result<()> {
        let response_bytes = self.verify_proof_bytes(proof)?;
        let (verified_content, verified_reasoning, cleartext_bytes, response_id) =
            self.decrypt_verified_stream(&response_bytes)?;
        if proof
            .get("receipt")
            .and_then(|value| value.get("chat_id"))
            .and_then(Value::as_str)
            != Some(response_id.as_str())
        {
            return Err(provider("Phala receipt chat identity mismatch"));
        }
        self.verify_cleartext_hash(proof, &cleartext_bytes)?;
        if verified_content != self.streamed_content
            || verified_reasoning != self.streamed_reasoning
        {
            return Err(provider(
                "Phala provisional deltas do not match the receipt-verified stream",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn worker_report_rejects_unpinned_nonce_and_tls_aliases() {
        let pinned = serde_json::json!({
            "request_nonce": "11".repeat(32),
            "tls_cert_fingerprint": "22".repeat(32),
        });
        assert!(pinned_worker_report_bindings(pinned.as_object().unwrap()).is_ok());

        let aliases = serde_json::json!({
            "nonce": "11".repeat(32),
            "tls_fingerprint": "22".repeat(32),
        });
        assert!(pinned_worker_report_bindings(aliases.as_object().unwrap()).is_err());
    }

    #[test]
    fn unpinned_worker_schema_fallbacks_are_rejected() {
        assert!(normalized_worker_report(serde_json::json!({
            "model_attestations": [{}],
        }))
        .is_err());
        assert!(worker_workload_manifest(
            serde_json::json!({"app_compose": "services: {}"})
                .as_object()
                .unwrap(),
        )
        .is_err());
        assert!(decode_quote("0x0102").is_err());
        assert!(decode_quote("AQI=").is_err());
        assert!(minimum_worker_ttl(serde_json::json!({}).as_object().unwrap()).is_err());
        assert!(minimum_worker_ttl(
            serde_json::json!({"minimum_session_ttl_seconds": 29})
                .as_object()
                .unwrap(),
        )
        .is_err());
    }

    #[test]
    fn exact_encrypted_body_matches_aci_order_and_hash() {
        let plaintext = "plaintext sentinel";
        let ciphertext = "ab".repeat(60);
        let request = ProviderWireRequest {
            model: "org/model".into(),
            messages: vec![crate::provider::ProviderWireMessage {
                role: "user".into(),
                content: Some(ciphertext.clone()),
                assistant_null_content: false,
                has_extended_fields: false,
            }],
            max_tokens: 8,
            stream: false,
            sampling: None,
            response_format: None,
            reasoning_effort: None,
            has_tools: false,
        };
        let bytes = exact_provider_body(&request, &["aa".repeat(32)]).unwrap();
        let expected = format!(
            "{{\"model\":\"org/model\",\"messages\":[{{\"role\":\"user\",\"content\":\"{ciphertext}\"}}],\"max_tokens\":8,\"stream\":false,\"provider\":{{\"aci_verified\":true,\"aci_session_ids\":[\"{}\"],\"zdr\":true}}}}",
            "aa".repeat(32)
        );
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected);
        assert_eq!(
            sha256_digest(&bytes),
            "sha256:19c29b365f60dedc3fe40b98084b1e2473e34e6e2298ae5dc723cfaa3738dccb"
        );
        assert!(!expected.contains(plaintext));
    }

    #[test]
    fn exact_encrypted_stream_body_matches_numeric_metadata_contract() {
        let request = ProviderWireRequest {
            model: "org/model".into(),
            messages: vec![
                crate::provider::ProviderWireMessage {
                    role: "user".into(),
                    content: Some("cd".repeat(60)),
                    assistant_null_content: false,
                    has_extended_fields: false,
                },
                crate::provider::ProviderWireMessage {
                    role: "assistant".into(),
                    content: None,
                    assistant_null_content: true,
                    has_extended_fields: false,
                },
            ],
            max_tokens: 8_192,
            stream: true,
            sampling: Some(serde_json::json!({
                "temperature": 1e-5,
                "top_p": 0.9,
                "top_k": 40,
                "min_p": 0.05,
                "frequency_penalty": -0.0,
                "presence_penalty": 2.0,
                "seed": 7,
            })),
            response_format: Some(serde_json::json!({"type": "json_object"})),
            reasoning_effort: Some("xhigh".into()),
            has_tools: false,
        };
        let session_id = "11".repeat(32);
        let body = String::from_utf8(
            exact_provider_body(&request, std::slice::from_ref(&session_id)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            body,
            format!(
                "{{\"model\":\"org/model\",\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}},{{\"role\":\"assistant\",\"content\":null}}],\"max_tokens\":8192,\"stream\":true,\"provider\":{{\"aci_verified\":true,\"aci_session_ids\":[\"{session_id}\"],\"zdr\":true}},\"stream_options\":{{\"include_usage\":true}},\"temperature\":1e-05,\"top_p\":0.9,\"top_k\":40,\"min_p\":0.05,\"frequency_penalty\":-0.0,\"presence_penalty\":2.0,\"seed\":7,\"response_format\":{{\"type\":\"json_object\"}},\"reasoning_effort\":\"max\"}}",
                "cd".repeat(60),
            )
        );
        assert_eq!(
            sha256_digest(body.as_bytes()),
            "sha256:4e43b0d0637584af4b8c0b0a409b80ba61d55efd497039c726ef4a3d0b2b7848"
        );

        let plaintext = "service-observed café 😀";
        let cleartext_body = exact_plaintext_provider_body(
            &request,
            std::slice::from_ref(&session_id),
            &BTreeMap::from([("messages.0.content".into(), plaintext.into())]),
        )
        .unwrap();
        let cleartext_value: Value = serde_json::from_slice(&cleartext_body).unwrap();
        assert_eq!(cleartext_value["messages"][0]["content"], plaintext);
        let mut expected_cleartext = body.into_bytes();
        replace_json_string_once(&mut expected_cleartext, &"cd".repeat(60), plaintext).unwrap();
        assert_eq!(cleartext_body, expected_cleartext);
        let cleartext_body = String::from_utf8(cleartext_body).unwrap();
        assert!(cleartext_body.contains("\"temperature\":1e-05"));
        assert!(!cleartext_body.contains(&"cd".repeat(60)));
    }

    #[test]
    fn schema_and_non_numeric_sampling_metadata_fail_before_phala_wire_serialization() {
        let base = ProviderWireRequest {
            model: "org/model".into(),
            messages: vec![crate::provider::ProviderWireMessage {
                role: "user".into(),
                content: Some("ab".repeat(60)),
                assistant_null_content: false,
                has_extended_fields: false,
            }],
            max_tokens: 128,
            stream: true,
            sampling: Some(serde_json::json!({"temperature": 1.0, "top_p": 0.9})),
            response_format: Some(serde_json::json!({"type": "json_object"})),
            reasoning_effort: Some("medium".into()),
            has_tools: false,
        };
        assert!(exact_provider_body(&base, &["11".repeat(32)]).is_ok());

        for response_format in [
            serde_json::json!({
                "type": "json_schema",
                "json_schema": {"description": "plaintext instruction"},
            }),
            serde_json::json!({"type": "json_object", "description": "plaintext instruction"}),
        ] {
            let mut request = base.clone();
            request.response_format = Some(response_format);
            assert!(exact_provider_body(&request, &["11".repeat(32)]).is_err());
        }

        for sampling in [
            serde_json::json!({"stop": ["plaintext instruction"]}),
            serde_json::json!({"temperature": "1"}),
            serde_json::json!({"temperature": 2.01}),
            serde_json::json!({"top_p": -0.01}),
            serde_json::json!({"presence_penalty": 2.01}),
            serde_json::json!({"top_k": 1_000_001}),
            serde_json::json!({"seed": 2_147_483_648_i64}),
        ] {
            let mut request = base.clone();
            request.sampling = Some(sampling);
            assert!(exact_provider_body(&request, &["11".repeat(32)]).is_err());
        }
    }

    #[test]
    fn encrypted_request_matches_shared_backend_hash_vector() {
        #[derive(Deserialize)]
        struct Fixture {
            vectors: Vec<Vector>,
        }
        #[derive(Deserialize)]
        struct Vector {
            provider: String,
            protocol: String,
            body_utf8: String,
            sha256: String,
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../../fixtures/request_hash_vectors/v1.json"
        ))
        .unwrap();
        let vector = fixture
            .vectors
            .iter()
            .find(|vector| vector.provider == "phala" && vector.protocol == PHALA_ACI_V2_PROTOCOL)
            .unwrap();
        let request = ProviderWireRequest {
            model: "deepseek-ai/DeepSeek-V4-Flash".into(),
            messages: vec![
                crate::provider::ProviderWireMessage {
                    role: "system".into(),
                    content: Some("ab".repeat(80)),
                    assistant_null_content: false,
                    has_extended_fields: false,
                },
                crate::provider::ProviderWireMessage {
                    role: "user".into(),
                    content: Some("cd".repeat(80)),
                    assistant_null_content: false,
                    has_extended_fields: false,
                },
            ],
            max_tokens: 8_192,
            stream: true,
            sampling: Some(serde_json::json!({"temperature": 0.25, "top_p": 0.9})),
            response_format: Some(serde_json::json!({"type": "json_object"})),
            reasoning_effort: Some("medium".into()),
            has_tools: false,
        };
        let body = exact_provider_body(&request, &["11".repeat(32), "22".repeat(32)]).unwrap();
        assert_eq!(body, vector.body_utf8.as_bytes());
        assert_eq!(sha256_digest(&body), vector.sha256);
    }

    #[test]
    fn standard_aci_vectors_match_pinned_spec() {
        let identity_key = serde_json::json!({
            "algo": "ed25519",
            "public_key": "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
        });
        let workload_id = sha256_digest(&canonical_json(&identity_key).unwrap());
        assert_eq!(
            workload_id,
            "sha256:57c2c8fa98bcf11441f1eff9ef087db67a5560a026082e96903e15365677b8c0"
        );
        let keyset = serde_json::json!({
            "workload_identity": {"public_key": identity_key, "subject": null},
            "keyset_epoch": {"version": 1, "not_after": 1800000000_u64},
            "receipt_signing_keys": [{
                "key_id": "receipt-1",
                "algo": "ed25519",
                "public_key": "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
            }],
            "e2ee_public_keys": [{
                "key_id": "e2ee-1",
                "algo": E2EE_ALGORITHM,
                "public_key": "ab".repeat(32),
            }],
            "tls_public_keys": [{
                "spki_sha256": "c0".repeat(32),
                "domain": "api.example.com",
            }],
        });
        let keyset_digest = sha256_digest(&canonical_json(&keyset).unwrap());
        assert_eq!(
            keyset_digest,
            "sha256:f2fba7e1b1451e0c0231df624f293407692ef939d3e0e55bca723131bea3f1ff"
        );
        let statement = serde_json::json!({
            "purpose": "aci.report_data.v1",
            "workload_id": workload_id,
            "workload_keyset_digest": keyset_digest,
            "nonce": "test-nonce",
        });
        assert_eq!(
            hex::encode(Sha256::digest(canonical_json(&statement).unwrap())),
            "0b8cc28d7e989a88b1e969af20aa2b224afdc2c99f24c97c31a4af330c964ecf"
        );
        let endorsement_payload = serde_json::json!({
            "purpose": "aci.keyset.endorsement.v1",
            "workload_keyset_digest": keyset_digest,
        });
        let identity_verifier = VerifyingKey::from_bytes(
            &decode_32(
                "identity key",
                "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
            )
            .unwrap(),
        )
        .unwrap();
        let signature = Signature::from_slice(
            &hex::decode("64e0a4f5d7af28dfdacc102d14c13470b4ddbd90708e190fc0e787f07b36f20eda0ef1f42ea96b8a7f290eb64a918574dc914ce06b6ea023d2153275f06fd201").unwrap(),
        )
        .unwrap();
        identity_verifier
            .verify(&canonical_json(&endorsement_payload).unwrap(), &signature)
            .unwrap();

        let secp_signing_key = k256::ecdsa::SigningKey::from_bytes((&[7_u8; 32]).into()).unwrap();
        let payload = canonical_json(&endorsement_payload).unwrap();
        let secp_signature: K256Signature = secp_signing_key.sign(&payload);
        verify_keyset_endorsement(
            "ecdsa-secp256k1",
            &hex::encode(
                secp_signing_key
                    .verifying_key()
                    .to_encoded_point(true)
                    .as_bytes(),
            ),
            &hex::encode(secp_signature.to_bytes()),
            &payload,
        )
        .unwrap();
    }

    #[test]
    fn standard_session_id_matches_pinned_spec_vector() {
        let record = serde_json::json!({
            "api_version": "aci/1",
            "session_id": "ignored",
            "upstream_name": "demo-upstream",
            "endpoint": "https://upstream.example.com",
            "verifier_id": "example/1",
            "established_at": 1750000000_u64,
            "expires_at": 1750003600_u64,
            "channel_binding": [{
                "type": "tls_spki_sha256",
                "origin": "https://upstream.example.com",
                "spki_sha256": "d1".repeat(32),
            }],
            "claims": {
                "gpu_attested": {"status": "unknown"},
                "model_weights_provenance": {"status": "unknown"},
                "os_known_good": {"status": "unknown"},
                "serving_software_known_good": {"status": "unknown"},
                "tcb_up_to_date": {"status": "unknown"},
                "tee_attested": {
                    "reason": "example quote verified",
                    "source": "hardware_proven",
                    "status": "asserted",
                },
            },
            "evidence": {
                "digest": "sha256:80d70e44d0ae1e829fd5f37c3ee4a60dfbea8d3aa18407ea3f34cf7ec91da34d",
                "data": "data:text/plain;base64,ZXhhbXBsZS1ldmlkZW5jZQ==",
            },
        });
        assert_eq!(
            standard_session_id(record.as_object().unwrap()).unwrap(),
            "as_2e9011abafe00fc2902aaa5dedf8373f14e2c4f1a456b854ddb475413547188e"
        );
    }

    #[test]
    fn receipt_key_algorithms_are_profile_exact() {
        let secp = k256::ecdsa::SigningKey::from_bytes((&[9_u8; 32]).into()).unwrap();
        let uncompressed = secp.verifying_key().to_encoded_point(false);
        let without_sec1_prefix = &uncompressed.as_bytes()[1..];
        let entries = vec![serde_json::json!({
            "key_id": "receipt-secp",
            "algo": "ecdsa-secp256k1",
            "public_key": hex::encode(without_sec1_prefix),
        })];

        let standard = parse_receipt_public_keys(PhalaAciProfile::AciV1_08d550, &entries).unwrap();
        assert_eq!(standard["receipt-secp"].algo, "ecdsa-secp256k1");
        assert!(
            parse_receipt_public_keys(PhalaAciProfile::DstackAciDraftD567f6, &entries,)
                .unwrap_err()
                .to_string()
                .contains("draft receipt key")
        );
    }

    #[test]
    fn e2ee_key_selection_uses_each_profiles_key_contract() {
        let standard_entries = vec![serde_json::json!({
            "key_id": "rotated-standard-key",
            "algo": E2EE_ALGORITHM,
            "public_key": "ab".repeat(32),
        })];
        assert_eq!(
            select_gateway_e2ee_key(PhalaAciProfile::AciV1_08d550, &standard_entries).unwrap(),
            "ab".repeat(32)
        );
        assert!(
            select_gateway_e2ee_key(PhalaAciProfile::DstackAciDraftD567f6, &standard_entries,)
                .is_err()
        );

        let draft_entries = vec![serde_json::json!({
            "key_id": E2EE_KEY_ID,
            "algo": E2EE_ALGORITHM,
            "public_key": "cd".repeat(32),
        })];
        assert_eq!(
            select_gateway_e2ee_key(PhalaAciProfile::DstackAciDraftD567f6, &draft_entries,)
                .unwrap(),
            "cd".repeat(32)
        );
    }

    #[test]
    fn gateway_profiles_require_disjoint_shape_without_source_pins() {
        let standard_report = serde_json::json!({
            "workload_id": "sha256:test",
            "service_capabilities": {"supported_e2ee_versions": ["2"]},
        });
        let standard_attestation = serde_json::json!({"keyset_endorsement": {}});
        let standard_keyset = serde_json::json!({"workload_identity": {}, "keyset_epoch": {}});
        assert_eq!(
            select_gateway_profile(
                standard_report.as_object().unwrap(),
                standard_attestation.as_object().unwrap(),
                standard_keyset.as_object().unwrap(),
            )
            .unwrap(),
            PhalaAciProfile::AciV1_08d550
        );

        let draft_report = serde_json::json!({
            "service_capabilities": {"serving": "aggregator", "supported_e2ee_versions": ["2"]},
        });
        let draft_attestation = serde_json::json!({});
        let draft_keyset = serde_json::json!({"not_after": 1, "subject": "gateway"});
        assert_eq!(
            select_gateway_profile(
                draft_report.as_object().unwrap(),
                draft_attestation.as_object().unwrap(),
                draft_keyset.as_object().unwrap(),
            )
            .unwrap(),
            PhalaAciProfile::DstackAciDraftD567f6
        );
        assert_eq!(
            canonical_repository_url("https://github.com/Dstack-TEE/private-ai-gateway.git/"),
            "https://github.com/Dstack-TEE/private-ai-gateway"
        );
        assert!(select_gateway_profile(
            standard_report.as_object().unwrap(),
            standard_attestation.as_object().unwrap(),
            &serde_json::json!({
                "workload_identity": {},
                "keyset_epoch": {},
                "not_after": 1,
                "subject": "hybrid",
            })
            .as_object()
            .unwrap()
            .clone(),
        )
        .is_err());
        assert!(select_gateway_profile(
            draft_report.as_object().unwrap(),
            draft_attestation.as_object().unwrap(),
            &Map::new(),
        )
        .is_err());
    }

    #[test]
    fn source_updates_are_transparent_but_must_match_the_measured_workflow() {
        let repository = "https://github.com/provider/updated-gateway";
        let commit = "66".repeat(20);
        let compose = serde_json::json!({
            "docker_compose_file": format!(
                "services:\n  gateway:\n    environment:\n      - {GATEWAY_REPOSITORY_ENV}={repository}\n      - {GATEWAY_COMMIT_ENV}={commit}"
            ),
        }).to_string();
        let (measured_repository, measured_commit) = measured_gateway_source(&compose).unwrap();
        let mut attestation = serde_json::json!({"source_provenance": {
            "repo_url": format!("{repository}.git/"),
            "repo_commit": commit,
        }});
        verify_gateway_source(
            attestation.as_object().unwrap(),
            &measured_repository,
            &measured_commit,
        )
        .unwrap();
        attestation["source_provenance"]["repo_commit"] = serde_json::json!("77".repeat(20));
        assert!(verify_gateway_source(
            attestation.as_object().unwrap(),
            &measured_repository,
            &measured_commit
        )
        .unwrap_err()
        .to_string()
        .contains("source commit does not match"));
        attestation["source_provenance"]["repo_commit"] = serde_json::json!(measured_commit);
        attestation["source_provenance"]["repo_url"] =
            serde_json::json!("https://github.com/provider/other");
        assert!(verify_gateway_source(
            attestation.as_object().unwrap(),
            &measured_repository,
            &measured_commit
        )
        .unwrap_err()
        .to_string()
        .contains("source repository does not match"));
    }

    #[test]
    fn worker_names_are_pinned_per_artifact_profile() {
        let cases = [
            (
                "deepseek/deepseek-v4-flash",
                "phala-deepseek-v4-flash",
                "phala-188",
            ),
            ("z-ai/glm-5.2", "phala-glm-5-2", "phala-177"),
            ("openai/gpt-oss-20b", "phala-gpt-oss-20b", "phala-23"),
        ];
        for (model, standard_name, draft_name) in cases {
            assert_eq!(
                expected_worker_identity(PhalaAciProfile::AciV1_08d550, model)
                    .unwrap()
                    .upstream_name,
                standard_name
            );
            assert_eq!(
                expected_worker_identity(PhalaAciProfile::DstackAciDraftD567f6, model)
                    .unwrap()
                    .upstream_name,
                draft_name
            );
        }
    }

    #[test]
    fn standard_receipt_requires_nested_signature_and_exact_worker_facts() {
        let signing_key = SigningKey::from_bytes(&[2_u8; 32]);
        let response_bytes = br#"{"choices":[],"id":"chatcmpl-123"}"#.to_vec();
        let response_hash = sha256_digest(&response_bytes);
        let request_hash = sha256_digest(br#"{"model":"z-ai/glm-5.2"}"#);
        let claims = serde_json::json!({
            "tee_attested": {"status": "asserted"},
            "tcb_up_to_date": {"status": "asserted"},
            "gpu_attested": {"status": "asserted"},
            "extra": {"canonical_model_id": "glm-5.2"},
        });
        let binding = serde_json::json!([{
            "type": "tls_spki_sha256",
            "origin": "https://worker.invalid",
            "spki_sha256": "44".repeat(32),
        }]);
        let worker_record = serde_json::json!({
            "api_version": "aci/1",
            "upstream_name": "phala-glm-5-2",
            "endpoint": "https://worker.invalid/v1",
            "verifier_id": "private-ai-verifier/phala-direct/v1",
            "established_at": 1_u64,
            "expires_at": u64::MAX,
            "identity": {"signing_address": format!("0x{}", "66".repeat(20))},
            "channel_binding": binding,
            "claims": claims,
            "evidence": {"digest": format!("sha256:{}", "77".repeat(32))},
        });
        let session_id = standard_session_id(worker_record.as_object().unwrap()).unwrap();
        let workload_id = format!("sha256:{}", "11".repeat(32));
        let keyset_digest = format!("sha256:{}", "22".repeat(32));
        let mut receipt = serde_json::json!({
            "api_version": "aci/1",
            "receipt_id": "rcpt-test",
            "chat_id": "chatcmpl-123",
            "model": "z-ai/glm-5.2",
            "workload_id": workload_id,
            "workload_keyset_digest": keyset_digest,
            "endpoint": "/v1/chat/completions",
            "method": "POST",
            "served_at": 2_u64,
            "event_log": [
                {"seq": 0, "type": "request.received", "body_hash": request_hash},
                {"seq": 1, "type": "request.forwarded", "body_hash": request_hash},
                {
                    "seq": 2,
                    "type": "upstream.verified",
                    "upstream_name": "phala-glm-5-2",
                    "provider_type": "phala-direct",
                    "model_id": "glm-5.2",
                    "url_origin": "https://worker.invalid",
                    "verifier_id": "private-ai-verifier/phala-direct/v1",
                    "result": "verified",
                    "required": true,
                    "reason": null,
                    "channel_bindings": binding,
                    "provider_claims": {"canonical_model_id": "glm-5.2"},
                    "session_id": session_id,
                    "claims": claims,
                },
                {
                    "seq": 3,
                    "type": "response.returned",
                    "cleartext_hash": response_hash,
                    "wire_hash": response_hash,
                },
            ],
            "signature": {"algo": "ed25519", "key_id": "receipt-1", "value": ""},
        });
        let sign = |receipt: &mut Value| {
            let mut unsigned = receipt.as_object().unwrap().clone();
            unsigned
                .get_mut("signature")
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove("value");
            let signature = signing_key.sign(&canonical_json(&Value::Object(unsigned)).unwrap());
            receipt["signature"]["value"] = Value::String(hex::encode(signature.to_bytes()));
        };
        sign(&mut receipt);
        let state = PhalaAttestationState {
            profile: PhalaAciProfile::AciV1_08d550,
            model: "z-ai/glm-5.2".into(),
            workload_id: Some(workload_id),
            keyset_digest,
            source_repository: "https://github.com/provider/updated-gateway".into(),
            source_commit: "66".repeat(20),
            gateway_public_key_hex: "33".repeat(32),
            receipt_public_keys: BTreeMap::from([(
                "receipt-1".into(),
                ReceiptPublicKey {
                    algo: "ed25519".into(),
                    public_key_hex: hex::encode(signing_key.verifying_key().as_bytes()),
                },
            )]),
            worker_sessions: BTreeMap::from([(
                session_id.clone(),
                PhalaWorkerSession {
                    session_id: session_id.clone(),
                    established_at: 1,
                    expires_at: u64::MAX,
                    record: worker_record,
                },
            )]),
        };
        let mut session = PhalaSession {
            state,
            client_secret: StaticSecret::from([3_u8; 32]),
            client_public_key_hex: "44".repeat(32),
            eligible_session_ids: vec![session_id.clone()],
            nonce: "55".repeat(32),
            timestamp: 2,
            request_body_hash: Some("unused-draft-hash".into()),
            request_plaintext_body_hash: Some(request_hash),
            request_plaintext_fields: BTreeMap::new(),
            streamed_content: String::new(),
            streamed_reasoning: String::new(),
        };
        let proof = serde_json::json!({
            "receipt": receipt,
            "response_body_base64": base64::engine::general_purpose::STANDARD.encode(&response_bytes),
            "worker_session_id": session_id,
        });
        assert_eq!(session.verify_proof_bytes(&proof).unwrap(), response_bytes);

        let mut retired = proof.clone();
        let signature_value = retired["receipt"]["signature"]["value"]
            .as_str()
            .unwrap()
            .to_string();
        retired["receipt"]
            .as_object_mut()
            .unwrap()
            .remove("signature");
        retired["receipt"]["key_id"] = Value::String("receipt-1".into());
        retired["receipt"]["signature"] = Value::String(signature_value);
        assert!(session
            .verify_proof_bytes(&retired)
            .unwrap_err()
            .to_string()
            .contains("retired draft shape"));

        let mut narrowed_claims = proof.clone();
        narrowed_claims["receipt"]["event_log"][2]["provider_claims"] = serde_json::json!({});
        sign(&mut narrowed_claims["receipt"]);
        assert!(session
            .verify_proof_bytes(&narrowed_claims)
            .unwrap_err()
            .to_string()
            .contains("locally verified session"));

        let mut invalid_forwarded_hash = proof.clone();
        invalid_forwarded_hash["receipt"]["event_log"][1]["body_hash"] =
            Value::String("not-a-digest".into());
        invalid_forwarded_hash["receipt"]["event_log"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "seq": 4,
                "type": "transparency.request_modified",
            }));
        sign(&mut invalid_forwarded_hash["receipt"]);
        assert!(session
            .verify_proof_bytes(&invalid_forwarded_hash)
            .unwrap_err()
            .to_string()
            .contains("forwarded request hash is invalid"));

        let mut extraneous_transparency = proof.clone();
        extraneous_transparency["receipt"]["event_log"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "seq": 4,
                "type": "transparency.request_modified",
            }));
        sign(&mut extraneous_transparency["receipt"]);
        assert!(session
            .verify_proof_bytes(&extraneous_transparency)
            .unwrap_err()
            .to_string()
            .contains("request transparency events are inconsistent"));

        let mut wrong_worker = proof.clone();
        wrong_worker["receipt"]["event_log"][2]["model_id"] = Value::String("other".into());
        sign(&mut wrong_worker["receipt"]);
        assert!(session
            .verify_proof_bytes(&wrong_worker)
            .unwrap_err()
            .to_string()
            .contains("verified model worker"));

        let mut mismatched_proof_worker = proof.clone();
        mismatched_proof_worker["worker_session_id"] = Value::String("as_invalid".into());
        assert!(session
            .verify_proof_bytes(&mismatched_proof_worker)
            .unwrap_err()
            .to_string()
            .contains("proof worker identity"));

        let secp = k256::ecdsa::SigningKey::from_bytes((&[7_u8; 32]).into()).unwrap();
        session.state.receipt_public_keys.insert(
            "receipt-secp".into(),
            ReceiptPublicKey {
                algo: "ecdsa-secp256k1".into(),
                public_key_hex: hex::encode(secp.verifying_key().to_encoded_point(true).as_bytes()),
            },
        );
        let mut secp_proof = proof;
        secp_proof["receipt"]["signature"] = serde_json::json!({
            "algo": "ecdsa-secp256k1",
            "key_id": "receipt-secp",
            "value": "",
        });
        let mut unsigned = secp_proof["receipt"].as_object().unwrap().clone();
        unsigned
            .get_mut("signature")
            .and_then(Value::as_object_mut)
            .unwrap()
            .remove("value");
        let canonical = canonical_json(&Value::Object(unsigned)).unwrap();
        let (signature, recovery_id) = secp
            .sign_digest_recoverable(Sha256::new_with_prefix(&canonical))
            .unwrap();
        let mut recoverable = signature.to_bytes().to_vec();
        recoverable.push(recovery_id.to_byte());
        secp_proof["receipt"]["signature"]["value"] = Value::String(hex::encode(&recoverable));
        assert_eq!(
            session.verify_proof_bytes(&secp_proof).unwrap(),
            response_bytes
        );

        secp_proof["receipt"]["signature"]["value"] =
            Value::String(hex::encode(&recoverable[..64]));
        assert!(session
            .verify_proof_bytes(&secp_proof)
            .unwrap_err()
            .to_string()
            .contains("65-byte recoverable"));
    }

    #[test]
    fn draft_receipt_remains_an_exact_protocol_profile() {
        let signing_key = SigningKey::from_bytes(&[4_u8; 32]);
        let session_id = "66".repeat(32);
        let request_hash = format!("sha256:{}", "11".repeat(32));
        let response_bytes = br#"{"id":"draft-response"}"#.to_vec();
        let mut receipt = serde_json::json!({
            "api_version": "aci/1",
            "receipt_id": "rcpt-draft",
            "chat_id": "draft-response",
            "model": "z-ai/glm-5.2",
            "workload_keyset_digest": format!("sha256:{}", "22".repeat(32)),
            "endpoint": "/v1/chat/completions",
            "method": "POST",
            "key_id": "receipt-v1",
            "served_at": 2_u64,
            "event_log": [
                {"type": "request.received", "body_hash": request_hash},
                {
                    "type": "upstream.verified",
                    "result": "verified",
                    "required": true,
                    "model_id": "glm-5.2",
                    "session_id": session_id,
                },
                {"type": "response.returned", "body_hash": sha256_digest(&response_bytes)},
            ],
        });
        let signature = signing_key
            .sign(&canonical_json(&receipt).expect("draft receipt canonicalization must succeed"));
        receipt["signature"] = Value::String(hex::encode(signature.to_bytes()));
        let state = PhalaAttestationState {
            profile: PhalaAciProfile::DstackAciDraftD567f6,
            model: "z-ai/glm-5.2".into(),
            workload_id: None,
            keyset_digest: format!("sha256:{}", "22".repeat(32)),
            source_repository: "https://github.com/provider/updated-gateway".into(),
            source_commit: "66".repeat(20),
            gateway_public_key_hex: "33".repeat(32),
            receipt_public_keys: BTreeMap::from([(
                "receipt-v1".into(),
                ReceiptPublicKey {
                    algo: "ed25519".into(),
                    public_key_hex: hex::encode(signing_key.verifying_key().as_bytes()),
                },
            )]),
            worker_sessions: BTreeMap::from([(
                session_id.clone(),
                PhalaWorkerSession {
                    session_id: session_id.clone(),
                    established_at: 1,
                    expires_at: u64::MAX,
                    record: Value::Null,
                },
            )]),
        };
        let session = PhalaSession {
            state,
            client_secret: StaticSecret::from([5_u8; 32]),
            client_public_key_hex: "44".repeat(32),
            eligible_session_ids: vec![session_id.clone()],
            nonce: "55".repeat(32),
            timestamp: 2,
            request_body_hash: Some("sha256:encrypted-wire-is-not-the-receipt-hash".into()),
            request_plaintext_body_hash: Some(request_hash),
            request_plaintext_fields: BTreeMap::new(),
            streamed_content: String::new(),
            streamed_reasoning: String::new(),
        };
        let proof = serde_json::json!({
            "receipt": receipt,
            "response_body_base64": base64::engine::general_purpose::STANDARD.encode(&response_bytes),
            "worker_session_id": session_id,
        });
        assert_eq!(session.verify_proof_bytes(&proof).unwrap(), response_bytes);

        let resign = |receipt: &mut Value| {
            let mut unsigned = receipt.as_object().unwrap().clone();
            unsigned.remove("signature");
            let signature = signing_key.sign(&canonical_json(&Value::Object(unsigned)).unwrap());
            receipt["signature"] = Value::String(hex::encode(signature.to_bytes()));
        };
        let mut wrong_first = proof.clone();
        wrong_first["receipt"]["event_log"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        resign(&mut wrong_first["receipt"]);
        assert!(session
            .verify_proof_bytes(&wrong_first)
            .unwrap_err()
            .to_string()
            .contains("begin with request.received"));

        let mut wrong_model = proof.clone();
        wrong_model["receipt"]["event_log"][1]["model_id"] = Value::String("other".into());
        resign(&mut wrong_model["receipt"]);
        assert!(session
            .verify_proof_bytes(&wrong_model)
            .unwrap_err()
            .to_string()
            .contains("verified model worker"));

        let mut hybrid_events = proof.clone();
        hybrid_events["receipt"]["event_log"][0]["seq"] = serde_json::json!(0);
        resign(&mut hybrid_events["receipt"]);
        assert!(session
            .verify_proof_bytes(&hybrid_events)
            .unwrap_err()
            .to_string()
            .contains("hybrid event shape"));

        let mut hybrid = proof;
        hybrid["receipt"]["workload_id"] = Value::String(format!("sha256:{}", "77".repeat(32)));
        assert!(session
            .verify_proof_bytes(&hybrid)
            .unwrap_err()
            .to_string()
            .contains("hybrid shape"));
    }

    #[tokio::test]
    async fn worker_session_rejects_wrong_content_address() {
        let value = serde_json::json!({
            "session_id": "00".repeat(32),
            "api_version": "aci/1",
            "established_at": now_unix(),
            "expires_at": now_unix() + 600,
            "claims": {
                "tee_attested": {"status": "asserted"},
                "tcb_up_to_date": {"status": "asserted"},
                "gpu_attested": {"status": "asserted"}
            },
            "channel_binding": [{"type": "e2ee_public_key_sha256"}],
            "evidence": {}
        });
        assert!(verify_worker_session(
            &value,
            &Value::Null,
            30,
            &[],
            PhalaAciProfile::DstackAciDraftD567f6,
            "z-ai/glm-5.2",
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("content addressed"));
    }

    #[tokio::test]
    async fn worker_session_rejects_content_addressed_redacted_evidence() {
        let mut record = serde_json::json!({
            "api_version": "aci/1",
            "upstream_name": "worker",
            "endpoint": "https://worker.invalid/v1",
            "verifier_id": "private-ai-verifier/phala-direct/v1",
            "established_at": now_unix(),
            "expires_at": now_unix() + 600,
            "identity": {"signing_address": format!("0x{}", "66".repeat(20))},
            "claims": {
                "tee_attested": {"status": "asserted"},
                "tcb_up_to_date": {"status": "asserted"},
                "gpu_attested": {"status": "asserted"}
            },
            "channel_binding": [{
                "type": "tls_spki_sha256",
                "origin": "https://worker.invalid",
                "spki_sha256": "44".repeat(32)
            }],
            "evidence": {}
        });
        let session_id = hex::encode(Sha256::digest(canonical_json(&record).unwrap()));
        record
            .as_object_mut()
            .unwrap()
            .insert("session_id".into(), Value::String(session_id));

        assert!(verify_worker_session(
            &record,
            &Value::Null,
            30,
            &[],
            PhalaAciProfile::DstackAciDraftD567f6,
            "z-ai/glm-5.2",
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("has no evidence"));
    }

    #[test]
    fn gateway_source_is_read_from_the_measured_workflow_without_an_allowlist() {
        let repository = "https://github.com/example/private-gateway.git";
        let commit = "66".repeat(20);
        let compose = serde_json::json!({
            "manifest_version": 2,
            "docker_compose_file": format!(
                "services:\n  launcher:\n    environment:\n      - {GATEWAY_REPOSITORY_ENV}={repository}\n      - {GATEWAY_COMMIT_ENV}={commit}"
            ),
        })
        .to_string();
        assert_eq!(
            measured_gateway_source(&compose).unwrap(),
            (repository.to_string(), commit.clone())
        );

        let duplicate = serde_json::json!({
            "docker_compose_file": format!(
                "- {GATEWAY_REPOSITORY_ENV}={repository}\n- {GATEWAY_COMMIT_ENV}={commit}\n- {GATEWAY_COMMIT_ENV}={} ",
                "77".repeat(20)
            ),
        })
        .to_string();
        assert!(measured_gateway_source(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("no unique"));
    }

    #[test]
    fn selected_worker_sessions_are_canonical_after_freshness_selection() {
        let now = now_unix();
        let older_id = "aa".repeat(32);
        let fresher_id = "ff".repeat(32);
        let worker_sessions = BTreeMap::from([
            (
                older_id.clone(),
                PhalaWorkerSession {
                    session_id: older_id.clone(),
                    established_at: now.saturating_sub(20),
                    expires_at: now + 300,
                    record: Value::Null,
                },
            ),
            (
                fresher_id.clone(),
                PhalaWorkerSession {
                    session_id: fresher_id.clone(),
                    established_at: now.saturating_sub(10),
                    expires_at: now + 600,
                    record: Value::Null,
                },
            ),
        ]);
        let state = PhalaAttestationState {
            profile: PhalaAciProfile::DstackAciDraftD567f6,
            model: "org/model".into(),
            workload_id: None,
            keyset_digest: format!("sha256:{}", "12".repeat(32)),
            source_repository: "https://github.com/Dstack-TEE/private-ai-gateway".into(),
            source_commit: "ab".repeat(20),
            gateway_public_key_hex: "34".repeat(32),
            receipt_public_keys: BTreeMap::new(),
            worker_sessions,
        };
        let verified = VerifiedModel {
            model_public_key_hex: state.gateway_public_key_hex.clone(),
            tls_fingerprint: None,
            checks: Vec::new(),
            provider_state: Some(serde_json::to_value(state).unwrap()),
            expires_at_unix: Some(now + 300),
        };
        let mut session = PhalaCipher.new_session(&verified).unwrap();
        let encrypted_content = session
            .encrypt_field(b"draft plaintext request", "messages.0.content")
            .unwrap();
        let request = ProviderWireRequest {
            model: "org/model".into(),
            messages: vec![crate::provider::ProviderWireMessage {
                role: "user".into(),
                content: Some(encrypted_content),
                assistant_null_content: false,
                has_extended_fields: false,
            }],
            max_tokens: 8,
            stream: true,
            sampling: None,
            response_format: None,
            reasoning_effort: None,
            has_tools: false,
        };
        let context = session.prepare_request_context(&request).unwrap();
        assert_eq!(
            context["session_ids"],
            serde_json::json!([older_id, fresher_id])
        );
        assert_eq!(
            context["artifact_profile"],
            PhalaAciProfile::DstackAciDraftD567f6.artifact_profile()
        );
        assert!(context["request_body_hash"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:")));
        assert!(context["request_plaintext_body_hash"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:")));
        assert_ne!(
            context["request_body_hash"],
            context["request_plaintext_body_hash"]
        );
        assert_eq!(context.as_object().unwrap().len(), 7);
    }
}

//! Independent native appraisal of the installed AstreaAI SNP/NVIDIA profile.
//! Caller-installed publisher/AMD/NVIDIA keys are the trust roots. Attester URLs,
//! booleans and supplied collateral never select trust or bypass verification.
#![forbid(unsafe_code)]
mod cert;
mod json;
mod nvidia;
mod release;
mod snp;
mod strict;

use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strict::*;

pub const PROFILE: &str = "axiom-snp-nvidia-v1";
pub const MAX_INPUT: usize = 8 * 1024 * 1024;
const VENDOR_LIMIT: usize = 2 * 1024 * 1024;

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn fresh_at_return(result: Result<Value>) -> Result<Value> {
    let accepted = result?;
    ensure!(
        number(&accepted["expires_at"])? > now()?,
        "evidence expired during appraisal"
    );
    Ok(accepted)
}

struct Prepared {
    value: Value,
    manifest: Value,
    policy: Value,
    accepted: Vec<String>,
    expires: u64,
    now: u64,
}
impl Prepared {
    fn new(raw: &[u8], local_now: u64) -> Result<Self> {
        ensure!(raw.len() <= MAX_INPUT, "verifier input bound");
        let v: Value = json::decode(raw)?;
        exact(&v, &["evidence", "context", "nvidia"])?;
        ensure!(
            v["nvidia"] == json!([]),
            "direct verifier performs vendor appraisal"
        );
        let c = &v["context"];
        let e = &v["evidence"];
        exact(
            c,
            &[
                "challenge",
                "origin",
                "publisher_key",
                "model_manifest",
                "now",
                "minimum_policy_sequence",
            ],
        )?;
        let now = number(&c["now"])?;
        ensure!(now.abs_diff(local_now) <= 30, "verification clock mismatch");
        hex_bytes(&c["challenge"], 32)?;
        hex_bytes(&c["publisher_key"], 32)?;
        exact(
            e,
            &[
                "protocol",
                "challenge",
                "identity",
                "document",
                "manifest",
                "policy",
            ],
        )?;
        ensure!(
            e["protocol"] == "axiom-pool-e2ee-v1" && e["challenge"] == c["challenge"],
            "evidence challenge/protocol mismatch"
        );
        let d = &e["document"];
        exact(
            d,
            &[
                "profile",
                "challenge",
                "worker",
                "snp_report",
                "gpu_evidence",
                "tls_certificate",
                "amd_collateral",
            ],
        )?;
        ensure!(
            d["profile"] == PROFILE && d["challenge"] == c["challenge"],
            "document mismatch"
        );
        let w = &d["worker"];
        worker(w)?;
        ensure!(
            w["logical_origin"] == c["origin"] && w["model_manifest"] == c["model_manifest"],
            "unexpected worker model/origin"
        );
        let (identity, _) = signed(&e["identity"], &w["identity_key"], "axiom-pool-identity-v1")?;
        exact(
            &identity,
            &["worker", "challenge", "issued_at", "expires_at"],
        )?;
        ensure!(
            &identity["worker"] == w && identity["challenge"] == c["challenge"],
            "identity statement mismatch"
        );
        let issued = number(&identity["issued_at"])?;
        let expires = number(&identity["expires_at"])?;
        ensure!(
            issued <= now + 30 && now < expires && expires > issued && expires - issued <= 240,
            "expired worker identity"
        );
        let (manifest, policy, accepted) = release::verify(
            &e["manifest"],
            &e["policy"],
            &c["publisher_key"],
            w,
            now,
            integer(&c["minimum_policy_sequence"], 1, (1 << 53) - 1)?,
        )?;
        let gpu = d["gpu_evidence"]
            .as_array()
            .context("GPU evidence required")?;
        ensure!(gpu.len() == 1, "one GPU required");
        exact(&gpu[0], &["arch", "evidence", "certificate"])?;
        ensure!(gpu[0]["arch"] == "BLACKWELL", "Blackwell required");
        b64(&gpu[0]["evidence"], 8192)?;
        b64(&gpu[0]["certificate"], 32 * 1024)?;
        snp::parse(&b64(&d["snp_report"], 1184)?)?;
        b64(&d["tls_certificate"], 16 * 1024)?;
        Ok(Self {
            value: v,
            manifest,
            policy,
            accepted,
            expires,
            now,
        })
    }
    fn finish(self, jwks: &Value, bundle: &Value, final_now: u64) -> Result<Value> {
        let c = &self.value["context"];
        let d = &self.value["evidence"]["document"];
        let w = &d["worker"];
        let mut binding = b"axiom-direct-snp-key-device-binding-v1\0".to_vec();
        binding.extend_from_slice(&canonical(&json!({"challenge":c["challenge"],"worker":w,
            "gpu_evidence_sha256":sha256(canonical(&d["gpu_evidence"])?),
            "tls_certificate_sha256":sha256(b64(&d["tls_certificate"],16*1024)?)}))?);
        let expected: [u8; 64] = Sha512::digest(binding).into();
        let (measurement, cpu_expiry) = snp::verify(
            &b64(&d["snp_report"], 1184)?,
            &expected,
            &self.policy["amd"],
            &d["amd_collateral"],
            self.now,
        )?;
        ensure!(
            self.accepted.contains(&measurement),
            "unapproved guest measurement"
        );
        let gpu_expiry = nvidia::verify(
            bundle,
            jwks,
            &self.policy["nvidia_anchors"],
            &c["challenge"],
            final_now,
        )?;
        let mut expires = self
            .expires
            .min(gpu_expiry)
            .min(cpu_expiry)
            .min(self.now + 240);
        if !self.policy["expires_at"].is_null() {
            expires = expires.min(number(&self.policy["expires_at"])?);
        }
        ensure!(expires > final_now, "evidence expired during appraisal");
        Ok(
            json!({"worker":w,"expires_at":expires,"policy_sequence":self.policy["sequence"],
            "generation":self.manifest["generation"],"capabilities":release::capabilities(&self.manifest)?}),
        )
    }
}
async fn bounded(response: reqwest::Response) -> Result<Value> {
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "vendor appraisal unavailable"
    );
    if let Some(size) = response.content_length() {
        ensure!(size <= VENDOR_LIMIT as u64, "vendor response bound");
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(part) = stream.next().await {
        let part = part?;
        ensure!(
            bytes.len().saturating_add(part.len()) <= VENDOR_LIMIT,
            "vendor response bound"
        );
        bytes.extend_from_slice(&part);
    }
    json::decode(&bytes)
}
/// Verify fresh evidence using only the fixed NVIDIA endpoints and supplied,
/// independently authenticated AMD collateral. No environment proxies/redirects.
pub async fn verify(raw: &[u8]) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(55),async {
        let prepared=Prepared::new(raw,now()?)?;
        let d=&prepared.value["evidence"]["document"];let gpu=&d["gpu_evidence"][0];
        let http=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).https_only(true).timeout(Duration::from_secs(20)).build()?;
        let jwks=bounded(http.get(format!("{}/.well-known/jwks.json",nvidia::NRAS)).send().await?).await?;
        let bundle=bounded(http.post(format!("{}/v4/attest/gpu",nvidia::NRAS)).json(&json!({
            "nonce":prepared.value["context"]["challenge"],"arch":gpu["arch"],"claims_version":"3.0",
            "evidence_list":[{"evidence":gpu["evidence"],"certificate":gpu["certificate"]}]
        })).send().await?).await?;
        fresh_at_return(prepared.finish(&jwks, &bundle, now()?))
    }).await?
}
/// An offline transport adapter can supply vendor responses, but cannot bypass
/// their chain/signatures/nonce/freshness or the caller-installed trust policy.
pub fn verify_with_vendor(raw: &[u8], jwks: &[u8], bundle: &[u8]) -> Result<Value> {
    ensure!(
        jwks.len() <= VENDOR_LIMIT && bundle.len() <= VENDOR_LIMIT,
        "vendor response bound"
    );
    let prepared = Prepared::new(raw, now()?)?;
    fresh_at_return(prepared.finish(&json::decode(jwks)?, &json::decode(bundle)?, now()?))
}
/// Deterministic cryptographic check for packaged native executables.
pub fn self_test() -> Result<()> {
    let key = hex::decode("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(
        &key.try_into().map_err(|_| anyhow::anyhow!("key length"))?,
    )?;
    let sig = ed25519_dalek::Signature::from_slice(&hex::decode(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    )?)?;
    key.verify_strict(b"", &sig)?;
    ensure!(
        key.verify_strict(b"tampered", &sig).is_err(),
        "negative crypto vector failed"
    );
    ensure!(
        json::decode::<Value>(br#"{"duplicate":0,"duplicate":1}"#).is_err(),
        "strict parser failed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_python_equivalence_corpus() {
        let fixture: Value =
            json::decode(include_bytes!("../tests/fixtures/equivalence.json")).unwrap();
        let instant = fixture["now"].as_u64().unwrap();
        let mut count = 0;
        let mut positive = std::collections::BTreeSet::new();
        for case in fixture["cases"].as_array().unwrap() {
            let raw = serde_json::to_vec(&case["input"]).unwrap();
            let result = Prepared::new(&raw, instant)
                .and_then(|p| p.finish(&fixture["jwks"], &case["bundle"], instant));
            if case["expected"].is_null() {
                assert!(result.is_err(), "accepted negative {}", case["name"]);
            } else {
                positive.insert(case["name"].as_str().unwrap());
                assert_eq!(
                    result.unwrap_or_else(|e| panic!("{}: {e:#}", case["name"])),
                    case["expected"],
                    "{}",
                    case["name"]
                );
            }
            count += 1;
        }
        assert!(count >= 40);
        assert_eq!(
            positive,
            std::collections::BTreeSet::from([
                "valid",
                "vision-tools",
                "permanent-policy",
                "crl-short-expiry",
                "valid-rsa-pss-amd-chain"
            ])
        );
    }
    #[test]
    fn nvidia_uses_post_io_clock_and_caps_every_authenticated_signer_chain() {
        let fixture: Value = json::decode(include_bytes!("../tests/fixtures/timing.json")).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 7);
        for case in cases {
            let raw = serde_json::to_vec(&case["input"]).unwrap();
            let result = Prepared::new(&raw, case["start_now"].as_u64().unwrap()).and_then(|p| {
                p.finish(
                    &case["jwks"],
                    &case["bundle"],
                    case["final_now"].as_u64().unwrap(),
                )
            });
            if case["expected"].is_null() {
                assert!(result.is_err(), "accepted {}", case["name"]);
            } else {
                assert_eq!(
                    result.unwrap_or_else(|e| panic!("{}: {e:#}", case["name"])),
                    case["expected"],
                    "{}",
                    case["name"]
                );
            }
            if case["intentional_difference"].is_null() {
                assert_eq!(
                    case["expected"], case["python_expected"],
                    "Python post-I/O equivalence {}",
                    case["name"]
                );
            } else {
                assert!(
                    case["expected"]["expires_at"].as_u64().unwrap()
                        < case["python_expected"]["expires_at"].as_u64().unwrap()
                );
            }
        }
    }

    #[test]
    fn hostile_inputs_and_clock_skew_fail_before_vendor_io() {
        for raw in [
            b"{}".to_vec(),
            br#"{"evidence":0,"evidence":1}"#.to_vec(),
            vec![b'['; 1024],
            vec![b' '; MAX_INPUT + 1],
        ] {
            assert!(Prepared::new(&raw, 1000).is_err());
        }
        let fixture: Value =
            json::decode(include_bytes!("../tests/fixtures/equivalence.json")).unwrap();
        let raw = serde_json::to_vec(&fixture["cases"][0]["input"]).unwrap();
        assert!(Prepared::new(&raw, fixture["now"].as_u64().unwrap() + 31).is_err());
        assert!(strict::b64(&json!("YR=="), 4).is_err());
        assert!(strict::url64("YR", 4).is_err());
        self_test().unwrap();
    }
    #[test]
    fn canonical_binding_matches_python_ascii_escaping() {
        assert_eq!(
            strict::canonical(&json!({"z":"café 😀\u{7f}","a":1})).unwrap(),
            br#"{"a":1,"z":"caf\u00e9 \ud83d\ude00\u007f"}"#
        );
    }
}

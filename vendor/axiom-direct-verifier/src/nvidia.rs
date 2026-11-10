use crate::{cert, strict::*};
use anyhow::{Context, Result, ensure};
use p384::ecdsa::{Signature, signature::Verifier};
use serde_json::Value;
pub const NRAS: &str = "https://nras.attestation.nvidia.com";

fn jwt(
    token: &Value,
    jwks: &Value,
    anchors: &Value,
    nonce: &Value,
    now: u64,
) -> Result<(Value, u64)> {
    let token = text(token)?;
    ensure!(token.len() <= 2 * 1024 * 1024, "NVIDIA JWT limit");
    let parts = token.split('.').collect::<Vec<_>>();
    ensure!(parts.len() == 3, "invalid NVIDIA JWT");
    let header: Value = crate::json::decode(&url64(parts[0], 2 * 1024 * 1024)?)?;
    let h = header.as_object().context("JWT header object required")?;
    ensure!(
        h.keys()
            .all(|k| ["alg", "kid", "typ"].contains(&k.as_str()))
            && header["alg"] == "ES384"
            && !text(&header["kid"])?.is_empty(),
        "unsupported NVIDIA JWT header"
    );
    let keys = jwks["keys"].as_array().context("JWKS required")?;
    ensure!(keys.len() <= 4096, "JWKS key bound");
    let matching = keys
        .iter()
        .filter(|k| k["kid"] == header["kid"])
        .collect::<Vec<_>>();
    ensure!(matching.len() == 1, "ambiguous NVIDIA signer");
    let chain = matching[0]["x5c"]
        .as_array()
        .context("signing chain required")?;
    ensure!((2..=4).contains(&chain.len()), "NVIDIA chain length");
    let ders = chain
        .iter()
        .map(|v| b64(v, 64 * 1024))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        anchors
            .as_array()
            .context("anchors required")?
            .contains(&Value::String(sha256(
                ders.last().context("root required")?
            ))),
        "unapproved NVIDIA anchor"
    );
    let certs = ders
        .iter()
        .map(|d| cert::parse(d))
        .collect::<Result<Vec<_>>>()?;
    let mut certificate_expiry = u64::MAX;
    for c in &certs {
        certificate_expiry = certificate_expiry.min(cert::validity(c, now)?);
    }
    for (i, pair) in certs.windows(2).enumerate() {
        cert::ca(&pair[1], Some(i))?;
        cert::issued(&pair[0], &pair[1])?;
    }
    let key = cert::p384_key(&certs[0])?;
    let sig = Signature::from_slice(&url64(parts[2], 96)?)?;
    key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)?;
    let claims: Value = crate::json::decode(&url64(parts[1], 2 * 1024 * 1024)?)?;
    ensure!(
        claims.is_object() && claims["iss"] == NRAS && &claims["eat_nonce"] == nonce,
        "NVIDIA issuer/nonce mismatch"
    );
    if let Some(v) = claims.get("exp") {
        ensure!(number(v)? > now, "expired NVIDIA JWT");
    }
    for name in ["iat", "nbf"] {
        if let Some(v) = claims.get(name) {
            ensure!(number(v)? <= now + 30, "future NVIDIA JWT");
        }
    }
    Ok((claims, certificate_expiry))
}
pub fn verify(
    bundle: &Value,
    jwks: &Value,
    anchors: &Value,
    nonce: &Value,
    now: u64,
) -> Result<u64> {
    ensure!(
        bundle.as_array().is_some_and(|a| a.len() == 2)
            && bundle[0].as_array().is_some_and(|a| a.len() == 2)
            && bundle[0][0] == "JWT"
            && bundle[1].as_object().is_some_and(|o| o.len() == 1),
        "one complete NVIDIA detached EAT required"
    );
    let (overall, certificate_expiry) = jwt(&bundle[0][1], jwks, anchors, nonce, now)?;
    let issued = number(&overall["iat"])?;
    let mut expires = number(&overall["exp"])?.min(certificate_expiry);
    ensure!(
        overall["x-nvidia-overall-att-result"] == true
            && overall["x-nvidia-ver"] == "3.0"
            && issued >= now.saturating_sub(120)
            && number(&overall["exp"])? > issued,
        "NVIDIA overall appraisal failed"
    );
    let submods = overall["submods"]
        .as_object()
        .context("NVIDIA submods required")?;
    let devices = bundle[1].as_object().context("detached devices required")?;
    ensure!(
        submods.len() == devices.len() && devices.keys().all(|k| submods.contains_key(k)),
        "NVIDIA detached set mismatch"
    );
    for (label, token) in devices {
        ensure!(
            submods[label]
                == serde_json::json!(["DIGEST", ["SHA-256", sha256(text(token)?.as_bytes())]]),
            "detached token digest mismatch"
        );
        let (c, certificate_expiry) = jwt(token, jwks, anchors, nonce, now)?;
        expires = expires.min(certificate_expiry);
        if let Some(exp) = c.get("exp") {
            expires = expires.min(number(exp)?);
        }
        ensure!(
            c["secboot"] == true && c["dbgstat"] == "disabled" && c["measres"] == "success",
            "GPU secure boot/debug/measurement policy failed"
        );
        for field in [
            "x-nvidia-gpu-attestation-report-parsed",
            "x-nvidia-gpu-attestation-report-signature-verified",
            "x-nvidia-gpu-attestation-report-nonce-match",
            "x-nvidia-gpu-attestation-report-cert-chain-fwid-match",
            "x-nvidia-gpu-driver-rim-signature-verified",
            "x-nvidia-gpu-vbios-rim-signature-verified",
        ] {
            ensure!(c[field] == true, "GPU report/RIM verification failed");
        }
        for field in ["x-nvidia-error-details", "x-nvidia-attestation-warning"] {
            let v = &c[field];
            ensure!(
                v.is_null()
                    || v == &Value::Bool(false)
                    || v == ""
                    || v == &serde_json::json!([])
                    || v == &serde_json::json!({}),
                "GPU warning/error forbidden"
            );
        }
        for field in [
            "x-nvidia-gpu-attestation-report-cert-chain",
            "x-nvidia-gpu-driver-rim-cert-chain",
            "x-nvidia-gpu-vbios-rim-cert-chain",
        ] {
            let c = &c[field];
            ensure!(
                c["x-nvidia-cert-status"] == "valid"
                    && c["x-nvidia-cert-ocsp-status"] == "good"
                    && c["x-nvidia-cert-ocsp-nonce-matches"] == true
                    && c["x-nvidia-cert-ocsp-response-valid"] == true,
                "GPU revocation freshness failed"
            );
        }
    }
    Ok(expires)
}

use crate::strict::*;
use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub fn capabilities(m: &Value) -> Result<Vec<String>> {
    let fallback = serde_json::json!(["chat", "image"]);
    let a = m
        .get("capabilities")
        .unwrap_or(&fallback)
        .as_array()
        .context("capability list required")?;
    let caps = a
        .iter()
        .map(|v| Ok(text(v)?.to_owned()))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        caps.len() <= 4
            && caps.iter().any(|s| s == "chat")
            && caps
                .iter()
                .all(|s| ["chat", "image", "tools", "vision"].contains(&s.as_str()))
            && caps.windows(2).all(|w| w[0] < w[1]),
        "invalid capabilities"
    );
    Ok(caps)
}
pub fn verify(
    m: &Value,
    p: &Value,
    key: &Value,
    w: &Value,
    now: u64,
    minimum: u64,
) -> Result<(Value, Value, Vec<String>)> {
    let (manifest, raw) = signed(m, key, "axiom-direct-release-v1")?;
    let (policy, _) = signed(p, key, "axiom-direct-policy-v1")?;
    let mut fields = vec![
        "schema",
        "generation",
        "worker_id",
        "origin",
        "model_manifest",
        "rootfs",
        "source_sha256",
        "boot",
        "engine_config_sha256",
        "build",
    ];
    if manifest.get("capabilities").is_some() {
        fields.push("capabilities");
    }
    exact(&manifest, &fields)?;
    capabilities(&manifest)?;
    ensure!(
        manifest["schema"] == "axiom-direct-release-v1"
            && sha256(raw) == text(&w["deployment_digest"])?,
        "release digest mismatch"
    );
    integer(&manifest["generation"], 1, (1 << 53) - 1)?;
    let b = &manifest["build"];
    exact(
        b,
        &[
            "kind",
            "base_image",
            "builder_image_sha256",
            "recipe_sha256",
        ],
    )?;
    ensure!(b["kind"] == "local-oci", "trusted local build required");
    let base = text(&b["base_image"])?;
    let digest = base
        .strip_prefix("docker.io/library/ubuntu@sha256:")
        .context("pinned OS base required")?;
    hex_bytes(&Value::String(digest.into()), 32)?;
    for k in ["builder_image_sha256", "recipe_sha256"] {
        hex_bytes(&b[k], 32)?;
    }
    hex_bytes(&manifest["worker_id"], 16)?;
    origin(&manifest["origin"])?;
    for k in ["model_manifest", "source_sha256", "engine_config_sha256"] {
        hex_bytes(&manifest[k], 32)?;
    }
    ensure!(
        w["model_manifest"] == manifest["model_manifest"]
            && w["worker_id"] == manifest["worker_id"]
            && w["logical_origin"] == manifest["origin"],
        "worker/release mismatch"
    );
    let root = &manifest["rootfs"];
    exact(root, &["root_hash", "salt", "data_bytes"])?;
    hex_bytes(&root["root_hash"], 32)?;
    hex_bytes(&root["salt"], 32)?;
    ensure!(
        integer(&root["data_bytes"], 4096, 200 * 1024u64.pow(3))? % 4096 == 0,
        "verity alignment"
    );
    let boot = &manifest["boot"];
    exact(
        boot,
        &[
            "firmware_sha256",
            "kernel_sha256",
            "vcpus",
            "vcpu_sig",
            "guest_features",
        ],
    )?;
    for k in ["firmware_sha256", "kernel_sha256"] {
        hex_bytes(&boot[k], 32)?;
    }
    integer(&boot["vcpus"], 1, 360)?;
    integer(&boot["vcpu_sig"], 1, u32::MAX.into())?;
    integer(&boot["guest_features"], 0, u64::MAX)?;
    exact(
        &policy,
        &[
            "schema",
            "sequence",
            "issued_at",
            "expires_at",
            "minimum_generation",
            "releases",
            "revoked_deployments",
            "amd",
            "nvidia_anchors",
        ],
    )?;
    ensure!(
        policy["schema"] == "axiom-direct-policy-v1",
        "wrong policy schema"
    );
    integer(&policy["sequence"], minimum.max(1), (1 << 53) - 1)?;
    let issued = number(&policy["issued_at"])?;
    ensure!(issued <= now + 30, "future policy");
    if !policy["expires_at"].is_null() {
        let expires = number(&policy["expires_at"])?;
        ensure!(
            now < expires && expires > issued && expires - issued <= 7 * 86400,
            "expired policy"
        );
    }
    ensure!(
        number(&manifest["generation"])?
            >= integer(&policy["minimum_generation"], 1, (1 << 53) - 1)?,
        "revoked generation"
    );
    let revoked = policy["revoked_deployments"]
        .as_array()
        .context("revocations required")?;
    ensure!(revoked.len() <= 1024, "revocation bound");
    for d in revoked {
        hex_bytes(d, 32)?;
        ensure!(d != &w["deployment_digest"], "revoked deployment");
    }
    let entries = policy["releases"].as_array().context("releases required")?;
    ensure!((1..=64).contains(&entries.len()), "release bound");
    let mut accepted = Vec::new();
    for e in entries {
        exact(e, &["deployment_digest", "measurement"])?;
        hex_bytes(&e["deployment_digest"], 32)?;
        hex_bytes(&e["measurement"], 48)?;
        if e["deployment_digest"] == w["deployment_digest"] {
            let measurement = text(&e["measurement"])?.to_owned();
            ensure!(!accepted.contains(&measurement), "duplicate measurement");
            accepted.push(measurement);
        }
    }
    ensure!(!accepted.is_empty(), "no approved launch measurement");
    let anchors = policy["nvidia_anchors"]
        .as_array()
        .context("NVIDIA roots required")?;
    ensure!((1..=8).contains(&anchors.len()), "NVIDIA root bound");
    for a in anchors {
        hex_bytes(a, 32)?;
    }
    Ok((manifest, policy, accepted))
}

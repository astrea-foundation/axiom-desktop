"""First-party signed release and rollback-protected boot approval policy."""

from . import PROFILE
from .strict import canonical, hex_bytes, integer, origin, require, sha256, signed_payload

MANIFEST_DOMAIN = "axiom-direct-release-v1"
POLICY_DOMAIN = "axiom-direct-policy-v1"


def capabilities(manifest):
    value = manifest.get("capabilities", ["chat", "image"])
    require(isinstance(value, list) and len(value) <= 4 and all(isinstance(item, str) for item in value)
            and value == sorted(set(value))
            and "chat" in value and set(value) <= {"chat", "image", "tools", "vision"},
            "invalid signed workload capabilities")
    return value


def policy_expiration(issued_at, lifetime):
    """A zero lifetime explicitly signs permanent release approval."""
    integer(issued_at)
    integer(lifetime, 0, 7*86400)
    require(lifetime == 0 or lifetime >= 300, "policy lifetime must be zero or 300–604800 seconds")
    return issued_at + lifetime if lifetime else None


def verify_release(manifest_document, policy_document, publisher_key, worker, now, minimum_sequence):
    manifest, raw = signed_payload(manifest_document, publisher_key, MANIFEST_DOMAIN)
    policy, _ = signed_payload(policy_document, publisher_key, POLICY_DOMAIN)
    require(isinstance(manifest, dict) and set(manifest) - {"capabilities"} == {"schema", "generation", "worker_id",
            "origin", "model_manifest", "rootfs", "source_sha256", "boot", "engine_config_sha256", "build"},
            "invalid direct release manifest")
    capabilities(manifest)
    require(manifest["schema"] == MANIFEST_DOMAIN and sha256(raw) == worker["deployment_digest"],
            "direct release digest mismatch")
    integer(manifest["generation"], 1)
    build = manifest["build"]
    require(isinstance(build, dict) and set(build) == {"kind", "base_image", "builder_image_sha256", "recipe_sha256"}
            and build["kind"] == "local-oci", "tenant-controlled trusted build provenance required")
    import re
    require(isinstance(build["base_image"], str) and re.fullmatch(
        r"docker.io/library/ubuntu@sha256:[0-9a-f]{64}", build["base_image"]), "pinned official OS base required")
    hex_bytes(build["builder_image_sha256"], 32)
    hex_bytes(build["recipe_sha256"], 32)
    hex_bytes(manifest["worker_id"], 16)
    origin(manifest["origin"])
    for key in ("model_manifest", "source_sha256", "engine_config_sha256"):
        hex_bytes(manifest[key], 32)
    require(worker["model_manifest"] == manifest["model_manifest"] and worker["worker_id"] == manifest["worker_id"]
            and worker["logical_origin"] == manifest["origin"], "worker/release mismatch")
    rootfs = manifest["rootfs"]
    require(isinstance(rootfs, dict) and set(rootfs) == {"root_hash", "salt", "data_bytes"},
            "verity-protected guest root required")
    hex_bytes(rootfs["root_hash"], 32)
    hex_bytes(rootfs["salt"], 32)
    integer(rootfs["data_bytes"], 4096, 200 * 1024**3)
    require(rootfs["data_bytes"] % 4096 == 0, "verity block alignment")
    boot = manifest["boot"]
    require(isinstance(boot, dict) and set(boot) == {"firmware_sha256", "kernel_sha256", "vcpus", "vcpu_sig", "guest_features"},
            "complete measured boot description required")
    hex_bytes(boot["firmware_sha256"], 32)
    hex_bytes(boot["kernel_sha256"], 32)
    integer(boot["vcpus"], 1, 360)
    integer(boot["vcpu_sig"], 1, 2**32-1)
    integer(boot["guest_features"], 0, 2**64-1)
    require(isinstance(policy, dict) and set(policy) == {"schema", "sequence", "issued_at", "expires_at",
            "minimum_generation", "releases", "revoked_deployments", "amd", "nvidia_anchors"},
            "invalid direct trust policy")
    require(policy["schema"] == POLICY_DOMAIN, "wrong policy schema")
    integer(policy["sequence"], max(1, minimum_sequence))
    issued = integer(policy["issued_at"])
    require(issued <= now + 30, "direct policy issued in the future")
    # Null is explicit signed approval without a calendar deadline. Existing
    # timed policies keep their original expiry checks; omission is invalid.
    if policy["expires_at"] is not None:
        expires = integer(policy["expires_at"])
        require(now < expires and 0 < expires-issued <= 7*86400, "expired/invalid direct policy")
    require(manifest["generation"] >= integer(policy["minimum_generation"], 1), "revoked release generation")
    revoked = policy["revoked_deployments"]
    require(isinstance(revoked, list) and len(revoked) <= 1024, "invalid release revocations")
    for item in revoked:
        hex_bytes(item, 32)
    require(worker["deployment_digest"] not in revoked, "revoked deployment")
    releases = policy["releases"]
    require(isinstance(releases, list) and 1 <= len(releases) <= 64, "approved measured releases required")
    accepted = []
    for entry in releases:
        require(isinstance(entry, dict) and set(entry) == {"deployment_digest", "measurement"}, "invalid boot policy entry")
        hex_bytes(entry["deployment_digest"], 32)
        hex_bytes(entry["measurement"], 48)
        if entry["deployment_digest"] == worker["deployment_digest"]:
            accepted.append(entry["measurement"])
    require(accepted and len(set(accepted)) == len(accepted), "deployment has no approved launch measurement")
    anchors = policy["nvidia_anchors"]
    require(isinstance(anchors, list) and 1 <= len(anchors) <= 8, "installed NVIDIA roots required")
    for anchor in anchors:
        hex_bytes(anchor, 32)
    return manifest, policy, accepted

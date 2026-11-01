"""Stable publisher authority for automatic signed release updates.

This is release authorization, not a claim that a hardware quote measures every
published artifact. A pinned publisher may approve new releases and hardware
baselines without changing the installed client. GitHub/Sigstore remains a
separate supported authority in release.py.
"""
from dataclasses import dataclass
import re
import time
from pathlib import Path
from urllib.parse import urlsplit

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

from .release import MAX_DOCUMENT, RollbackState, digest, integer, read_file, require, sha256
from .wire import canonical, decode_json
from .runtime_policy import validate_runtime

AUTHORITY_SCHEMA = "axiom-inference-publisher-v1"
METADATA_SCHEMA = "axiom-inference-publisher-metadata-v1"
MANIFEST_DOMAIN = b"axiom-inference-publisher-release-v1\0"
POLICY_DOMAIN = b"axiom-inference-publisher-policy-v1\0"


@dataclass(frozen=True)
class PublisherAuthority:
    repository: str
    public_key: str
    channel: str
    minimum_policy_sequence: int
    maximum_policy_lifetime_seconds: int

    @classmethod
    def load(cls, path):
        value = decode_json(read_file(path), MAX_DOCUMENT)
        require(isinstance(value, dict) and set(value) == {"schema", "repository", "public_key",
            "channel", "minimum_policy_sequence", "maximum_policy_lifetime_seconds"}
            and value["schema"] == AUTHORITY_SCHEMA, "invalid installed publisher authority")
        require(isinstance(value["repository"], str) and re.fullmatch(
            r"[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9_.-]{1,100}", value["repository"]), "repository required")
        digest(value["public_key"])
        require(value["channel"] in ("staging", "production"), "explicit release channel required")
        integer(value["minimum_policy_sequence"])
        integer(value["maximum_policy_lifetime_seconds"], 300, 7 * 86400)
        return cls(**{k: v for k, v in value.items() if k != "schema"})

    @property
    def identity(self):
        return sha256(canonical({"repository": self.repository, "public_key": self.public_key,
                                 "channel": self.channel, "schema": AUTHORITY_SCHEMA}))


def sign_document(private_key, payload, domain):
    return {"payload": payload, "signature": private_key.sign(domain + canonical(payload)).hex()}


def verify_document(authority, document, domain):
    require(isinstance(document, dict) and set(document) == {"payload", "signature"}, "signed document required")
    signature = document["signature"]
    require(isinstance(signature, str) and re.fullmatch(r"[0-9a-f]{128}", signature), "Ed25519 signature required")
    payload = document["payload"]
    require(isinstance(payload, dict), "signed object required")
    raw = canonical(payload)
    require(len(raw) <= MAX_DOCUMENT, "signed document limit")
    try:
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(authority.public_key)).verify(
            bytes.fromhex(signature), domain + raw)
    except Exception as error:
        raise ValueError("publisher signature verification failed") from error
    require(payload.get("repository") == authority.repository and payload.get("channel") == authority.channel,
            "publisher repository or channel differs")
    return payload, raw


def hardware_policy(value):
    require(isinstance(value, dict) and set(value) == {"measurements", "nvidia_anchors"}, "hardware policy required")
    rows = value["measurements"]
    require(isinstance(rows, list) and 1 <= len(rows) <= 32, "bounded hardware measurements required")
    for row in rows:
        require(isinstance(row, dict) and set(row) == {"mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3"},
                "complete TDX measurement set required")
        require(all(isinstance(v, str) and re.fullmatch(r"[0-9a-f]{96}", v) for v in row.values()),
                "TDX measurements must be SHA-384 values")
    anchors = value["nvidia_anchors"]
    require(isinstance(anchors, list) and 1 <= len(anchors) <= 16 and len(set(anchors)) == len(anchors),
            "bounded NVIDIA trust anchors required")
    for anchor in anchors:
        digest(anchor)
    return value


@dataclass(frozen=True)
class PublishedRelease:
    repository: str
    release_id: str
    manifest_sha256: str
    policy_sequence: int
    expires_at: int
    authority_fingerprint: str
    channel: str
    hardware: dict
    chute_id: str
    artifacts: tuple
    runtime: dict | None = None

    def public_report(self):
        return {k: getattr(self, k) for k in ("repository", "release_id", "manifest_sha256", "policy_sequence",
            "expires_at", "authority_fingerprint", "channel")} | {"method": "ed25519-publisher"}


def verify_metadata(authority, metadata_raw, state, *, now=None):
    now = int(time.time()) if now is None else integer(now)
    value = decode_json(metadata_raw, 3 * MAX_DOCUMENT)
    require(isinstance(value, dict) and set(value) == {"schema", "manifest", "policy"}
            and value["schema"] == METADATA_SCHEMA, "invalid publisher metadata")
    policy, raw = verify_document(authority, value["policy"], POLICY_DOMAIN)
    require(set(policy) == {"schema", "repository", "channel", "sequence", "issued_at", "expires_at",
        "accepted_releases", "revoked_releases"} and policy["schema"] == "axiom-inference-publisher-policy-v1",
        "invalid publisher policy")
    integer(policy["sequence"], authority.minimum_policy_sequence)
    integer(policy["issued_at"])
    integer(policy["expires_at"])
    require(policy["issued_at"] <= now + 60 < policy["expires_at"]
            and 0 < policy["expires_at"] - policy["issued_at"] <= authority.maximum_policy_lifetime_seconds,
            "publisher policy expired or outside freshness bounds")
    for name in ("accepted_releases", "revoked_releases"):
        rows = policy[name]
        require(isinstance(rows, list) and len(rows) <= 256
                and all(isinstance(row, str) for row in rows) and len(set(rows)) == len(rows), "invalid release set")
        for row in rows:
            digest(row)
    # Record authenticated revocations before considering the selected release.
    state.observe(authority, policy, raw, now)
    manifest, manifest_raw = verify_document(authority, value["manifest"], MANIFEST_DOMAIN)
    manifest_hash = sha256(manifest_raw)
    require(manifest_hash in policy["accepted_releases"] and manifest_hash not in policy["revoked_releases"],
            "release is unapproved or revoked")
    require(set(manifest) - {"runtime"} == {"schema", "repository", "channel", "release_id", "chute_id", "hardware", "artifacts"}
            and manifest["schema"] == "axiom-inference-publisher-release-v1", "invalid release manifest")
    require(isinstance(manifest["release_id"], str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", manifest["release_id"]),
            "release identity required")
    require(isinstance(manifest["chute_id"], str) and re.fullmatch(
        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", manifest["chute_id"]), "Chute identity required")
    hardware = hardware_policy(manifest["hardware"])
    rows = manifest["artifacts"]
    require(isinstance(rows, list) and 1 <= len(rows) <= 64, "bounded release artifact inventory required")
    names = set()
    for row in rows:
        require(isinstance(row, dict) and set(row) == {"role", "name", "sha256"}, "invalid release artifact")
        require(row["role"] in {"chute_source", "model_manifest", "launch_spec", "source_archive"}
                and isinstance(row["name"], str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", row["name"])
                and row["name"] not in names, "invalid or duplicate artifact")
        names.add(row["name"])
        digest(row["sha256"])
    return PublishedRelease(authority.repository, manifest["release_id"], manifest_hash, policy["sequence"],
        policy["expires_at"], sha256(bytes.fromhex(authority.public_key)), authority.channel,
        hardware, manifest["chute_id"], tuple(rows),
        validate_runtime(manifest["runtime"]) if "runtime" in manifest else None)


def fetch_metadata(url):
    require(isinstance(url, str) and len(url) <= 2048, "fixed metadata URL required")
    parsed = urlsplit(url)
    require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password
            and not parsed.query and not parsed.fragment, "fixed HTTPS metadata URL required")
    import httpx
    with httpx.Client(trust_env=False, follow_redirects=False, timeout=httpx.Timeout(20, connect=5)) as http:
        with http.stream("GET", url, headers={"Accept": "application/json"}) as response:
            require(response.status_code == 200, "publisher metadata unavailable")
            raw = bytearray()
            for chunk in response.iter_bytes():
                raw.extend(chunk)
                require(len(raw) <= 3 * MAX_DOCUMENT, "publisher metadata size limit")
    return bytes(raw)


def verify_files(config, authority):
    if "directory" in config:
        directory = Path(config["directory"])
        require(directory.is_absolute(), "absolute release directory required")
        raw = read_file(directory / "metadata.json", 3 * MAX_DOCUMENT)
    else:
        raw = fetch_metadata(config["metadata_url"])
    return verify_metadata(authority, raw, RollbackState(config["state"]))

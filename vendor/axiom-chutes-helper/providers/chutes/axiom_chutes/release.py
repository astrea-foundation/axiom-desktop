"""Repository-authorized releases, separate from proof of runtime execution.

GitHub's Sigstore verifier authenticates the bytes and certificate identity.
This module then applies Axiom's release, freshness, revocation and rollback
policy. It cannot turn a Chutes hardware quote into a workload endorsement.
"""
from dataclasses import dataclass
from datetime import datetime
import hashlib
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile
import time

from .wire import canonical, decode_json
from .private_state import atomic_state, check_private, state_lock

MAX_DOCUMENT = 1024 * 1024
MAX_BUNDLE = 4 * MAX_DOCUMENT
PREDICATE = "https://slsa.dev/provenance/v1"
ISSUER = "https://token.actions.githubusercontent.com"
ROLES = {"container_image", "chute_source", "model_manifest", "launch_spec", "source_archive"}


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), "SHA-256 digest required")
    return value


def integer(value, minimum=1, maximum=2**53 - 1):
    require(type(value) is int and minimum <= value <= maximum, "bounded integer required")
    return value


def read_file(path, limit=MAX_DOCUMENT):
    path = Path(path)
    require(not path.is_symlink() and path.is_file(), "regular release file required")
    with path.open("rb") as stream:
        raw = stream.read(limit + 1)
    require(len(raw) <= limit, "release file size limit")
    return raw


@dataclass(frozen=True)
class Authority:
    repository: str
    repository_id: str
    owner_id: str
    workflow: str
    source_ref: str
    verifier: Path
    verifier_sha256: str
    trusted_root: Path
    trusted_root_sha256: str
    minimum_policy_sequence: int
    maximum_policy_lifetime_seconds: int

    @classmethod
    def load(cls, path):
        value = decode_json(read_file(path), MAX_DOCUMENT)
        require(isinstance(value, dict) and set(value) == {
            "schema_version", "repository", "repository_id", "owner_id", "workflow", "source_ref",
            "verifier", "verifier_sha256", "trusted_root", "trusted_root_sha256",
            "minimum_policy_sequence", "maximum_policy_lifetime_seconds"}, "invalid release authority")
        require(type(value["schema_version"]) is int and value.pop("schema_version") == 1, "unsupported release authority")
        require(isinstance(value["repository"], str) and re.fullmatch(
            r"[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9_.-]{1,100}", value["repository"]), "exact GitHub repository required")
        for name in ("repository_id", "owner_id"):
            require(isinstance(value[name], str) and re.fullmatch(r"[1-9][0-9]{0,19}", value[name]), "immutable GitHub IDs required")
        require(isinstance(value["workflow"], str) and re.fullmatch(
            r"\.github/workflows/[a-zA-Z0-9_-]+\.ya?ml", value["workflow"]), "exact workflow path required")
        require(isinstance(value["source_ref"], str) and re.fullmatch(
            r"refs/heads/[A-Za-z0-9][A-Za-z0-9_/-]{0,120}", value["source_ref"]), "exact protected branch ref required")
        for name in ("verifier", "trusted_root"):
            value[name] = Path(value[name])
            require(value[name].is_absolute(), "absolute verifier and trusted-root paths required")
            digest(value[name + "_sha256"])
        integer(value["minimum_policy_sequence"])
        integer(value["maximum_policy_lifetime_seconds"], 300, 7 * 86400)
        return cls(**value)

    @property
    def identity(self):
        # Binary/root maintenance does not reset the policy high-water mark.
        return sha256(canonical({key: getattr(self, key) for key in (
            "repository", "repository_id", "owner_id", "workflow", "source_ref")}))

    @property
    def certificate_identity(self):
        return f"https://github.com/{self.repository}/{self.workflow}@{self.source_ref}"


@dataclass(frozen=True)
class Provenance:
    commit: str
    signed_at: int


class GitHubVerifier:
    """Pinned, offline Sigstore verification; no API token is passed to gh."""
    def __init__(self, authority):
        self.authority = authority

    def verify(self, artifact, bundle, expected_commit):
        authority = self.authority
        require(isinstance(artifact, bytes) and 0 < len(artifact) <= MAX_DOCUMENT, "bounded release document required")
        require(isinstance(bundle, bytes) and 0 < len(bundle) <= MAX_BUNDLE, "bounded Sigstore bundle required")
        require(isinstance(expected_commit, str) and re.fullmatch(r"[0-9a-f]{40}", expected_commit), "exact source commit required")
        binary = authority.verifier.resolve(strict=True)
        require(binary.is_file() and sha256(read_file(binary, 128 * MAX_DOCUMENT)) == authority.verifier_sha256,
                "installed provenance verifier digest mismatch")
        roots = read_file(authority.trusted_root, MAX_BUNDLE)
        require(sha256(roots) == authority.trusted_root_sha256, "installed Sigstore trust roots differ")
        with tempfile.TemporaryDirectory(prefix="axiom-release-") as directory:
            root = Path(directory)
            for name, data in (("artifact.json", artifact), ("bundle.jsonl", bundle), ("trusted-root.jsonl", roots)):
                (root / name).write_bytes(data)
            argv = [str(binary), "attestation", "verify", str(root / "artifact.json"),
                    "--hostname", "github.com", "--repo", authority.repository,
                    "--bundle", str(root / "bundle.jsonl"),
                    "--custom-trusted-root", str(root / "trusted-root.jsonl"),
                    "--cert-identity", authority.certificate_identity,
                    "--cert-oidc-issuer", ISSUER, "--source-ref", authority.source_ref,
                    "--source-digest", expected_commit, "--deny-self-hosted-runners",
                    "--predicate-type", PREDICATE, "--format", "json"]
            try:
                result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, timeout=45, check=False,
                    env={"PATH": "/usr/bin:/bin", "GH_CONFIG_DIR": directory,
                         "GH_PROMPT_DISABLED": "1", "GH_NO_UPDATE_NOTIFIER": "1"})
            except (OSError, subprocess.TimeoutExpired) as error:
                raise ValueError("provenance verifier unavailable") from error
        require(result.returncode == 0, "GitHub/Sigstore provenance verification failed")
        verified = decode_json(result.stdout, 12 * MAX_DOCUMENT)
        require(isinstance(verified, list) and 1 <= len(verified) <= 16, "invalid provenance verifier result")
        for item in verified:
            verification = item.get("verificationResult", {})
            certificate = verification.get("signature", {}).get("certificate", {})
            # Read identity from authenticated certificate extensions, never
            # user-controlled statement.predicate or convenience API metadata.
            if (certificate.get("sourceRepositoryIdentifier") != authority.repository_id
                    or certificate.get("sourceRepositoryOwnerIdentifier") != authority.owner_id
                    or certificate.get("sourceRepositoryDigest") != expected_commit
                    or certificate.get("sourceRepositoryRef") != authority.source_ref
                    or certificate.get("sourceRepositoryURI") != f"https://github.com/{authority.repository}"
                    or certificate.get("subjectAlternativeName") != authority.certificate_identity
                    or certificate.get("issuer") != ISSUER
                    or certificate.get("runnerEnvironment") != "github-hosted"):
                continue
            statement = verification.get("statement", {})
            subjects = statement.get("subject", [])
            if (statement.get("predicateType") != PREDICATE or not isinstance(subjects, list)
                    or not any(subject.get("digest", {}).get("sha256") == sha256(artifact)
                               for subject in subjects if isinstance(subject, dict))):
                continue
            timestamps = verification.get("verifiedTimestamps", [])
            times = []
            for timestamp in timestamps if isinstance(timestamps, list) else []:
                if (not isinstance(timestamp, dict) or timestamp.get("type") not in ("TimestampAuthority", "TransparencyLog")
                        or not isinstance(timestamp.get("timestamp"), str)):
                    continue
                parsed = datetime.fromisoformat(timestamp["timestamp"].replace("Z", "+00:00"))
                if parsed.tzinfo is not None:
                    times.append(int(parsed.timestamp()))
            require(times, "witnessed signing timestamp required")
            return Provenance(expected_commit, max(times))
        raise ValueError("provenance certificate repository, owner, commit or subject differs")


def parse_manifest(raw, authority):
    value = decode_json(raw, MAX_DOCUMENT)
    require(isinstance(value, dict) and set(value) == {
        "schema", "repository", "commit", "source_ref", "artifacts"}, "invalid release manifest")
    require(value["schema"] == "axiom-inference-release-v1" and value["repository"] == authority.repository
            and value["source_ref"] == authority.source_ref, "release authority differs")
    require(isinstance(value["commit"], str) and re.fullmatch(r"[0-9a-f]{40}", value["commit"]), "exact source commit required")
    rows = value["artifacts"]
    require(isinstance(rows, list) and len(rows) == len(ROLES), "complete application inventory required")
    roles = set()
    for row in rows:
        require(isinstance(row, dict) and set(row) == {"role", "name", "sha256"}, "invalid release artifact")
        require(row["role"] in ROLES and row["role"] not in roles, "duplicate or unknown release artifact")
        roles.add(row["role"])
        digest(row["sha256"])
        if row["role"] == "container_image":
            require(isinstance(row["name"], str) and re.fullmatch(
                r"[a-z0-9][a-z0-9._/-]+@sha256:" + row["sha256"], row["name"]), "immutable OCI image digest required")
        else:
            require(isinstance(row["name"], str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", row["name"]),
                    "flat release artifact name required")
    require(roles == ROLES, "complete release inventory required")
    return value


def parse_policy(raw, authority, now):
    value = decode_json(raw, MAX_DOCUMENT)
    require(isinstance(value, dict) and set(value) == {
        "schema", "repository", "commit", "sequence", "issued_at", "expires_at",
        "accepted_releases", "revoked_releases"}, "invalid release policy")
    require(value["schema"] == "axiom-inference-release-policy-v1"
            and value["repository"] == authority.repository, "release policy authority differs")
    require(isinstance(value["commit"], str) and re.fullmatch(r"[0-9a-f]{40}", value["commit"]), "exact policy source commit required")
    integer(value["sequence"], authority.minimum_policy_sequence)
    integer(value["issued_at"])
    integer(value["expires_at"])
    require(value["issued_at"] <= now + 60 < value["expires_at"]
            and 0 < value["expires_at"] - value["issued_at"] <= authority.maximum_policy_lifetime_seconds,
            "release policy expired or outside freshness bounds")
    for name in ("accepted_releases", "revoked_releases"):
        rows = value[name]
        require(isinstance(rows, list) and len(rows) <= 256 and all(isinstance(entry, str) for entry in rows), "invalid release set")
        require(len(set(rows)) == len(rows), "duplicate release policy entries")
        for entry in rows:
            digest(entry)
    require(value["accepted_releases"], "at least one authorized release required")
    return value


class RollbackState:
    """Persistent high-water mark, including revocations and clock rollback."""
    def __init__(self, path):
        self.path = Path(path)

    def observe(self, authority, policy, raw, now):
        parent = self.path.parent
        lock_path = parent / (self.path.name + ".lock")
        with state_lock(lock_path):
            previous = None
            if self.path.exists() or self.path.is_symlink():
                check_private(self.path)
                previous = decode_json(read_file(self.path, 8192), 8192)
                require(isinstance(previous, dict) and set(previous) == {
                    "authority", "sequence", "policy_sha256", "observed_at"}, "invalid release rollback state")
                require(previous["authority"] == authority.identity, "release-state authority changed")
                integer(previous["sequence"])
                integer(previous["observed_at"])
                digest(previous["policy_sha256"])
                require(policy["sequence"] >= previous["sequence"], "release policy rollback rejected")
                require(now + 300 >= previous["observed_at"], "system clock rollback rejected")
                if policy["sequence"] == previous["sequence"]:
                    require(sha256(raw) == previous["policy_sha256"], "release policy sequence equivocation rejected")
            state = {"authority": authority.identity, "sequence": policy["sequence"],
                     "policy_sha256": sha256(raw), "observed_at": max(now, previous["observed_at"] if previous else now)}
            atomic_state(self.path, canonical(state))


@dataclass(frozen=True)
class AuthorizedRelease:
    repository: str
    commit: str
    manifest_sha256: str
    policy_sequence: int
    expires_at: int
    artifacts: tuple

    def public_report(self):
        return {"repository": self.repository, "commit": self.commit, "manifest_sha256": self.manifest_sha256,
                "policy_sequence": self.policy_sequence, "expires_at": self.expires_at}


def verify_release(authority, manifest_raw, manifest_bundle, policy_raw, policy_bundle, state, *, now=None, verifier=None):
    now = int(time.time()) if now is None else integer(now)
    verifier = GitHubVerifier(authority) if verifier is None else verifier
    policy = parse_policy(policy_raw, authority, now)
    provenance = verifier.verify(policy_raw, policy_bundle, policy["commit"])
    require(abs(policy["issued_at"] - provenance.signed_at) <= 600 and provenance.signed_at <= now + 60,
            "policy freshness differs from witnessed signing time")
    # Observe an authentic new revocation policy even if this particular
    # worker's release has been revoked or its manifest is invalid.
    state.observe(authority, policy, policy_raw, now)
    release_hash = sha256(manifest_raw)
    require(release_hash in policy["accepted_releases"] and release_hash not in policy["revoked_releases"],
            "release is unapproved or revoked")
    manifest = parse_manifest(manifest_raw, authority)
    signed = verifier.verify(manifest_raw, manifest_bundle, manifest["commit"])
    require(signed.signed_at <= now + 60, "release signing timestamp is in the future")
    return AuthorizedRelease(authority.repository, manifest["commit"], release_hash,
                             policy["sequence"], policy["expires_at"], tuple(manifest["artifacts"]))


def check_artifacts(release, directory):
    """Compare deployment files with authenticated release bytes. No execution claim."""
    directory = Path(directory).resolve(strict=True)
    for artifact in release.artifacts:
        if artifact["role"] == "container_image":
            continue  # Must be compared with a provider-authenticated runtime endorsement.
        path = directory / artifact["name"]
        require(path.parent == directory and PurePosixPath(artifact["name"]).name == artifact["name"], "artifact path escaped release")
        require(sha256(read_file(path, 32 * MAX_DOCUMENT)) == artifact["sha256"], "deployment artifact differs from authorized release")


def chutes_qualification():
    # No provider/app supplied JSON field, code self-hash, GitHub signature or
    # nonce embedding may satisfy the missing hardware-to-application chain.
    return {"profile": "chutes-hardware-development-v1", "production_admission": False,
            "workload_binding_verified": False,
            "missing_authenticated_bindings": ["container_image_digest", "chute_source_digest",
                "launch_spec_digest", "model_manifest_digest", "application_keys_to_workload"],
            "reason": "The current public Chutes quote and proxy signature authenticate hardware and the SDK recipient key, not an Axiom release inventory."}

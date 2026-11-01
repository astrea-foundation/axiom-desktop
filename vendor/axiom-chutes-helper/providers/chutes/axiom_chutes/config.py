"""Public deployment configuration; never contains provider/account credentials."""

import os
from pathlib import Path
import re
from urllib.parse import urlsplit

from .wire import decode_json, unbase64


def load_deployment(path=None):
    if path is None and os.environ.get("AXIOM_CHUTES_DEPLOYMENT_B64"):
        raw = unbase64(os.environ["AXIOM_CHUTES_DEPLOYMENT_B64"], 64 * 1024)
    elif path is None and os.environ.get("AXIOM_CHUTES_DEPLOYMENT_JSON"):
        raw = os.environ["AXIOM_CHUTES_DEPLOYMENT_JSON"].encode()
    else:
        path = path or os.environ.get("AXIOM_CHUTES_DEPLOYMENT_FILE")
        if not path:
            raise ValueError("AXIOM_CHUTES_DEPLOYMENT_FILE is required")
        raw = Path(path).read_bytes()
    value = decode_json(raw, 64 * 1024)
    required = {"schema_version", "username", "name", "tag", "base_worker_image", "worker"}
    if not isinstance(value, dict) or set(value) != required or value["schema_version"] != 1:
        raise ValueError("invalid Chutes deployment document")
    for field in ("username", "name", "tag"):
        if not isinstance(value[field], str) or not re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,62}", value[field]):
            raise ValueError("invalid deployment identifier")
    if not isinstance(value["base_worker_image"], str) or not re.fullmatch(
            r"[a-z0-9][a-z0-9._/-]+@sha256:[0-9a-f]{64}", value["base_worker_image"]):
        raise ValueError("a published digest-pinned worker image is required")
    worker = value["worker"]
    if (not isinstance(worker, dict) or worker.get("provider") != "chutes"
            or worker.get("fixture", False) or worker.get("listen") != "127.0.0.1:8005"
            or worker.get("encryption_key_file") or worker.get("identity_key_file")):
        raise ValueError("Chutes requires loopback-only ingress and in-process keys")
    for field in ("public_origin", "controller_origin", "issuer"):
        uri = urlsplit(worker[field])
        if (uri.scheme != "https" or not uri.hostname or uri.username or uri.password
                or uri.path or uri.query or uri.fragment or worker[field] != "https://" + uri.netloc):
            raise ValueError("fixed HTTPS origins required")
    for field, length in (("worker_id", 32), ("issuer_key", 64), ("deployment_digest", 64)):
        if not re.fullmatch(r"[0-9a-f]{" + str(length) + r"}", worker.get(field, "")):
            raise ValueError("replace every deployment placeholder before building")
    verifier = worker.get("verifier", {})
    for field in ("binary_sha256", "publisher_key"):
        if not re.fullmatch(r"[0-9a-f]{64}", verifier.get(field, "")):
            raise ValueError("installed verifier and publisher pins are required")
    if verifier.get("binary") != "/usr/local/bin/axiom-pool-verify" or verifier.get("minimum_policy_sequence", 0) < 1:
        raise ValueError("a pinned installed verifier is required")
    return value

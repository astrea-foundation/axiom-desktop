"""Bounded wire and signing primitives shared by the direct tools."""

import base64
import hashlib
import re
from urllib.parse import urlsplit

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

from .wire import canonical, decode_json, unbase64


def require(condition, message):
    if not condition:
        raise ValueError(message)


def hex_bytes(value, size):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{%d}" % (size * 2), value),
            "invalid hexadecimal value")
    return bytes.fromhex(value)


def integer(value, minimum=0, maximum=2**53-1):
    require(type(value) is int and minimum <= value <= maximum, "invalid integer")
    return value


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def origin(value):
    require(isinstance(value, str) and len(value) <= 2048, "HTTPS origin required")
    p = urlsplit(value)
    require(p.scheme == "https" and p.hostname and not p.username and not p.password
            and not p.path and not p.query and not p.fragment and value == "https://" + p.netloc,
            "HTTPS origin required")
    return value


def signed_payload(document, public_key, domain):
    require(isinstance(document, dict) and set(document) == {"payload", "signature"}, "signed document required")
    p = document["payload"]
    require(isinstance(p, str) and len(p) <= 180 * 1024 and re.fullmatch(r"[A-Za-z0-9_-]+", p),
            "invalid signed payload")
    raw = base64.urlsafe_b64decode(p + "=" * (-len(p) % 4))
    require(base64.urlsafe_b64encode(raw).decode().rstrip("=") == p and len(raw) <= 128 * 1024,
            "noncanonical signed payload")
    Ed25519PublicKey.from_public_bytes(hex_bytes(public_key, 32)).verify(
        hex_bytes(document["signature"], 64), domain.encode() + b"\0" + raw)
    return decode_json(raw), raw


def sign_document(value, private_key, domain):
    raw = canonical(value)
    require(len(raw) <= 128 * 1024, "signed payload limit")
    return {"payload": base64.urlsafe_b64encode(raw).decode().rstrip("="),
            "signature": private_key.sign(domain.encode() + b"\0" + raw).hex()}


def worker_identity(worker):
    require(isinstance(worker, dict) and set(worker) == {"worker_id", "boot_id", "encryption_key",
            "identity_key", "model_manifest", "logical_origin", "evidence_profile", "deployment_digest"},
            "invalid worker identity")
    for key in ("boot_id", "encryption_key", "identity_key", "model_manifest", "deployment_digest"):
        hex_bytes(worker[key], 32)
    hex_bytes(worker["worker_id"], 16)
    origin(worker["logical_origin"])
    require(worker["evidence_profile"] == "axiom-snp-nvidia-v1", "wrong direct evidence profile")
    return worker

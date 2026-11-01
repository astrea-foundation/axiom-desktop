"""The published Chutes E2EE wire format, with bounded parsing.

These helpers do not authenticate a recipient. Callers must verify the TDX
quote, measurements, GPU evidence, and recipient binding before sealing.
Chutes' outer SSE completion marker does not authenticate stream completion;
the opaque Axiom exchange retains its own ordered frames and terminal.
"""

import base64
import gzip
import hashlib
import hmac
import io
import json
import os

from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives import hashes

MAX_METADATA = 8 * 1024 * 1024
PUBLIC_KEY_BYTES = 1184
CIPHERTEXT_BYTES = 1088


def unique(items):
    out = {}
    for key, value in items:
        if key in out:
            raise ValueError("duplicate JSON field")
        out[key] = value
    return out


def decode_json(raw, limit=MAX_METADATA):
    if not isinstance(raw, bytes) or len(raw) > limit:
        raise ValueError("metadata size limit")
    def no_constant(_):
        raise ValueError("invalid JSON number")
    return json.loads(raw, object_pairs_hook=unique, parse_constant=no_constant)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def unbase64(value, limit=MAX_METADATA):
    if not isinstance(value, str) or len(value) > (limit + 2) // 3 * 4:
        raise ValueError("base64 size limit")
    raw = base64.b64decode(value, validate=True)
    if len(raw) > limit or base64.b64encode(raw).decode() != value:
        raise ValueError("noncanonical base64")
    return raw


def key(shared, ciphertext, info):
    return HKDF(algorithm=hashes.SHA256(), length=32,
                salt=ciphertext[:16], info=info).derive(shared)


def seal(public_key, payload, *, limit=MAX_METADATA):
    from pqcrypto.kem import ml_kem_768
    if len(public_key) != PUBLIC_KEY_BYTES or "e2e_response_pk" in payload:
        raise ValueError("invalid recipient/payload")
    response_pk, response_sk = ml_kem_768.keygen()
    ciphertext, shared = ml_kem_768.encaps(public_key)
    raw = canonical({**payload, "e2e_response_pk": base64.b64encode(response_pk).decode()})
    if len(raw) > limit:
        raise ValueError("request size limit")
    nonce = os.urandom(12)
    encrypted = ChaCha20Poly1305(key(shared, ciphertext, b"e2e-req-v1")).encrypt(
        nonce, gzip.compress(raw, mtime=0), None)
    return ciphertext + nonce + encrypted, response_sk


def open_response(blob, response_sk, *, limit=MAX_METADATA):
    from pqcrypto.kem import ml_kem_768
    if not CIPHERTEXT_BYTES + 28 <= len(blob) <= limit:
        raise ValueError("encrypted response size limit")
    ciphertext = blob[:CIPHERTEXT_BYTES]
    shared = ml_kem_768.decaps(response_sk, ciphertext)
    raw = ChaCha20Poly1305(key(shared, ciphertext, b"e2e-resp-v1")).decrypt(
        blob[CIPHERTEXT_BYTES:CIPHERTEXT_BYTES + 12], blob[CIPHERTEXT_BYTES + 12:], None)
    with gzip.GzipFile(fileobj=io.BytesIO(raw)) as stream:
        decoded = stream.read(limit + 1)
    return decode_json(decoded, limit)


class StreamCipher:
    """Chutes outer AEAD only; inner request-bound frames authenticate sender."""
    def __init__(self, response_sk):
        self.response_sk = response_sk
        self.stream_key = None
        self.nonces = set()

    def open(self, event):
        from pqcrypto.kem import ml_kem_768
        if not isinstance(event, dict):
            raise ValueError("invalid encrypted stream event")
        if set(event) == {"e2e_init"}:
            ciphertext = unbase64(event["e2e_init"], CIPHERTEXT_BYTES)
            if self.stream_key is not None or len(ciphertext) != CIPHERTEXT_BYTES:
                raise ValueError("invalid or repeated stream key exchange")
            shared = ml_kem_768.decaps(self.response_sk, ciphertext)
            self.stream_key = key(shared, ciphertext, b"e2e-stream-v1")
            self.response_sk = None
            return b""
        if set(event) != {"e2e"} or self.stream_key is None:
            raise ValueError("encrypted stream key missing or unexpected event")
        blob = unbase64(event["e2e"], 256 * 1024)
        if len(blob) < 28 or blob[:12] in self.nonces or len(self.nonces) >= 32768:
            raise ValueError("invalid or repeated stream ciphertext")
        plaintext = ChaCha20Poly1305(self.stream_key).decrypt(blob[:12], blob[12:], None)
        self.nonces.add(blob[:12])
        return plaintext


def confirmation(secret, evidence):
    if len(secret) != 32:
        raise ValueError("confirmation key length")
    return hmac.new(secret, b"axiom-chutes-bootstrap-v1\0" + canonical(evidence), hashlib.sha256).hexdigest()


def verify_confirmation(secret, evidence, value):
    if not isinstance(value, str) or not hmac.compare_digest(confirmation(secret, evidence), value):
        raise ValueError("bootstrap response belongs to a different exchange")

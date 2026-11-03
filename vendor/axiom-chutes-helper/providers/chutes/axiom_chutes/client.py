"""Instance-pinned Chutes transport and hardware qualification.

This intentionally cannot construct the Rust client's VerifiedWorker. A
Chutes hardware quote does not by itself endorse an Axiom image, model bundle,
or our separate HPKE/Ed25519 keys.
"""

import asyncio
import hashlib
import os
import re
import time
import uuid
from datetime import datetime, timezone
from dataclasses import dataclass, field

import httpx
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa

from .wire import (MAX_METADATA, PUBLIC_KEY_BYTES, decode_json,
                   open_response, seal, unbase64, verify_confirmation)

API = "https://api.chutes.ai"


def identifier(value):
    if not isinstance(value, str) or str(uuid.UUID(value)) != value:
        raise ValueError("canonical Chutes UUID required")
    return value


async def read_limited(response, limit=MAX_METADATA):
    chunks, size = [], 0
    async for chunk in response.aiter_bytes():
        size += len(chunk)
        if size > limit:
            raise ValueError("Chutes response size limit")
        chunks.append(chunk)
    return b"".join(chunks)


@dataclass(frozen=True)
class Instance:
    chute_id: str
    instance_id: str
    public_key: str
    invocation_nonce: str
    nonce_expires_at: int


@dataclass(frozen=True)
class HardwareReport:
    instance: Instance
    expires_at: int
    quote_sha256: str
    # No implicit conversion of hardware proof into workload/key admission.
    workload_binding_verified: bool = field(default=False, init=False)
    worker_key_custody_verified: bool = field(default=False, init=False)


def check_quote_fields(quote, certificate, challenge, public_key, measurements):
    """Inspect fields only AFTER the DCAP library authenticates the quote."""
    if (len(quote) < 636 or int.from_bytes(quote[:2], "little") != 4
            or int.from_bytes(quote[4:8], "little") != 0x81):
        raise ValueError("only Intel TDX quote v4 / TD 1.0 is supported")
    body = quote[48:632]
    if int.from_bytes(body[120:128], "little") & 1:
        raise ValueError("debug TDX is forbidden")
    expected = hashlib.sha256((challenge + public_key).encode()).digest()
    cert = x509.load_der_x509_certificate(certificate)
    if not cert.not_valid_before_utc <= datetime.now(timezone.utc) < cert.not_valid_after_utc:
        raise ValueError("Chutes attestation certificate is expired")
    spki = cert.public_key().public_bytes(serialization.Encoding.DER,
                                         serialization.PublicFormat.SubjectPublicKeyInfo)
    if body[520:584] != expected + hashlib.sha256(spki).digest():
        raise ValueError("nonce, recipient key, or attestation certificate mismatch")
    actual = {"mrtd": body[136:184].hex(),
              **{f"rtmr{i}": body[328 + i * 48:376 + i * 48].hex() for i in range(4)}}
    if (not isinstance(measurements, list) or not 1 <= len(measurements) <= 32
            or not any(actual == candidate for candidate in measurements)):
        raise ValueError("unapproved Chutes runtime measurements")
    return expected.hex(), cert


def check_proxy_signature(evidence, cert, quote, challenge):
    body = unbase64(evidence["attested_body"])
    signature = unbase64(evidence["signature"], 8192)
    public = cert.public_key()
    if not isinstance(public, rsa.RSAPublicKey) or public.key_size < 2048:
        raise ValueError("unsupported attestation proxy signing key")
    public.verify(signature, body, padding.PKCS1v15(), hashes.SHA256())
    parsed = decode_json(body)
    # The proxy signs the unmodified SDK evidence response. Compare its actual
    # contents, rather than checking an unrelated signature and trusting the
    # API's separately reserialized evidence.
    if not isinstance(parsed, dict) or parsed.get("nonce") != challenge:
        raise ValueError("signed proxy body differs from returned evidence")
    inner = parsed.get("evidence")
    if not isinstance(inner, dict):
        raise ValueError("signed proxy evidence is missing")
    # Current servers sign `quote`; older servers used `tdx_quote`. Require
    # exactly one spelling and authenticate its exact bytes. Never select one
    # of two ambiguous values or trust an unsigned reserialization.
    fields = set(inner) & {"quote", "tdx_quote"}
    if (len(fields) != 1 or unbase64(inner[next(iter(fields))]) != quote
            or decode_json(inner["nvtrust_evidence"].encode()) != evidence["gpu_evidence"]):
        raise ValueError("signed proxy body differs from returned evidence")


class ChutesClient:
    invocation_url = API + "/e2e/invoke"
    def __init__(self, api_key, *, http=None):
        if not isinstance(api_key, str) or not api_key.strip() or "\n" in api_key or "\r" in api_key:
            raise ValueError("Chutes API key required")
        self._http = http or httpx.AsyncClient(
            trust_env=False, follow_redirects=False,
            timeout=httpx.Timeout(30, connect=10),
            limits=httpx.Limits(max_connections=8, max_keepalive_connections=4))
        self._headers = {"Authorization": f"Bearer {api_key}"}
        self._used = {}

    def _take_nonce(self, instance):
        current = time.monotonic()
        self._used = {key: deadline for key, deadline in self._used.items() if deadline > current}
        identity = (instance.chute_id, instance.instance_id, instance.invocation_nonce)
        remaining = instance.nonce_expires_at - time.time()
        if remaining <= 0 or identity in self._used or len(self._used) >= 4096:
            raise ValueError("invocation nonce expired, replayed or active budget exhausted")
        self._used[identity] = current + remaining

    async def close(self):
        await self._http.aclose()

    async def _json(self, method, path, **kwargs):
        async with self._http.stream(method, API + path, headers=self._headers, **kwargs) as response:
            if response.status_code != 200:
                raise RuntimeError(f"Chutes metadata rejected ({response.status_code})")
            return decode_json(await read_limited(response))

    async def discover(self, chute_id):
        identifier(chute_id)
        value = await self._json("GET", f"/e2e/instances/{chute_id}")
        expiry = value["nonce_expires_at"]
        if type(expiry) is not int or not time.time() < expiry <= time.time() + 75:
            raise ValueError("invalid discovery nonce expiry")
        rows = value["instances"]
        if not isinstance(rows, list) or not 1 <= len(rows) <= 5:
            raise ValueError("invalid instance inventory")
        instances, seen = [], set()
        for row in rows:
            instance_id = identifier(row["instance_id"])
            if instance_id in seen or len(unbase64(row["e2e_pubkey"])) != PUBLIC_KEY_BYTES:
                raise ValueError("ambiguous instance or invalid ML-KEM key")
            seen.add(instance_id)
            nonces = row["nonces"]
            if (not isinstance(nonces, list) or not 1 <= len(nonces) <= 10
                    or len(set(nonces)) != len(nonces)
                    or any(not isinstance(n, str) or not re.fullmatch(r"[A-Za-z0-9_-]{32}", n) for n in nonces)):
                raise ValueError("invalid invocation nonces")
            instances.extend(Instance(chute_id, instance_id, row["e2e_pubkey"], n, expiry) for n in nonces)
        return instances

    async def attest_hardware(self, instance, measurements, nvidia_anchors):
        """Verify vendor hardware. Does not authorize an inference request."""
        from dcap_qvl import get_collateral_and_verify
        started = int(time.time())
        challenge = os.urandom(32).hex()
        evidence = await self._json("GET", f"/instances/{identifier(instance.instance_id)}/evidence",
                                    params={"nonce": challenge})
        # Some live responses leave this unsigned convenience field null.
        # Recipient authentication comes from the quote's nonce/ML-KEM/SPKI
        # bindings and signed proxy body below, never from this API string.
        if evidence.get("instance_id") not in (None, instance.instance_id):
            raise ValueError("attestation instance mismatch")
        quote = unbase64(evidence["quote"], 128 * 1024)
        result = await asyncio.wait_for(get_collateral_and_verify(quote), timeout=30)
        report = decode_json(result.to_json().encode())
        if report.get("status") != "UpToDate":
            raise ValueError("strict Intel TCB status required")
        nonce, cert = check_quote_fields(quote, unbase64(evidence["certificate"], 64 * 1024),
                                         challenge, instance.public_key, measurements)
        check_proxy_signature(evidence, cert, quote, challenge)
        from .gpu import appraise
        gpu_expiry = await appraise(evidence["gpu_evidence"], nonce, nvidia_anchors)
        expiry = min(started + 120, gpu_expiry)
        if expiry <= time.time():
            raise ValueError("hardware qualification expired")
        return HardwareReport(instance, expiry, hashlib.sha256(quote).hexdigest())

    async def refresh_invocation_nonce(self, hardware):
        if hardware.expires_at <= time.time():
            raise ValueError("hardware proof expired")
        rows = await self.discover(hardware.instance.chute_id)
        matches = [row for row in rows if row.instance_id == hardware.instance.instance_id]
        if not matches or any(row.public_key != hardware.instance.public_key for row in matches):
            raise ValueError("instance/key changed; fresh hardware verification required")
        return HardwareReport(matches[0], hardware.expires_at, hardware.quote_sha256)

    async def metering_session(self, hardware):
        """Endorse a public usage key through the freshly attested SDK channel."""
        from .metering import verify_session
        instance = hardware.instance
        if hardware.expires_at <= time.time() or instance.nonce_expires_at <= time.time():
            raise ValueError("fresh metering hardware and nonce required")
        self._take_nonce(instance)
        secret, challenge = os.urandom(32), os.urandom(32).hex()
        blob, response_key = seal(unbase64(instance.public_key), {
            "challenge": challenge, "confirmation_secret": secret.hex()})
        headers = {**self._headers, "X-Chute-Id": instance.chute_id,
            "X-Instance-Id": instance.instance_id, "X-E2E-Nonce": instance.invocation_nonce,
            "X-E2E-Path": "/metering-session", "X-E2E-Stream": "false",
            "Content-Type": "application/octet-stream"}
        async with self._http.stream("POST", self.invocation_url, headers=headers, content=blob) as response:
            if response.status_code != 200:
                raise RuntimeError("metering channel failed; no replay")
            result = open_response(await read_limited(response), response_key)
        if hardware.expires_at <= time.time():
            raise ValueError("metering evidence expired during confirmation")
        return verify_session(result, secret, challenge)

    async def confirm_bootstrap(self, hardware, challenge):
        """Confirm a response came from the possessor of the quoted public key.

        This proves channel possession, not private-key custody or execution
        of our exact Axiom release. An arbitrary quote-data oracle is not a
        key endorsement. Returned documents remain untrusted pool evidence.
        """
        if not re.fullmatch(r"[0-9a-f]{64}", challenge):
            raise ValueError("invalid bootstrap challenge")
        instance = hardware.instance
        if hardware.expires_at <= time.time() or instance.nonce_expires_at <= time.time():
            raise ValueError("attestation/discovery expired; rediscover and verify")
        self._take_nonce(instance)  # Before dispatch: a timeout must not replay it.
        secret = os.urandom(32)
        payload = {"challenge": challenge, "confirmation_secret": secret.hex()}
        blob, response_sk = seal(unbase64(instance.public_key), payload)
        headers = {**self._headers, "X-Chute-Id": instance.chute_id,
                   "X-Instance-Id": instance.instance_id, "X-E2E-Nonce": instance.invocation_nonce,
                   "X-E2E-Path": "/bootstrap", "X-E2E-Stream": "false",
                   "Content-Type": "application/octet-stream"}
        async with self._http.stream("POST", self.invocation_url, headers=headers, content=blob) as response:
            if response.status_code != 200:
                raise RuntimeError(f"Chutes bootstrap rejected ({response.status_code}); no automatic replay")
            value = open_response(await read_limited(response), response_sk)
        if not isinstance(value, dict) or set(value) != {"evidence", "confirmation"}:
            raise ValueError("invalid bootstrap response")
        verify_confirmation(secret, value["evidence"], value["confirmation"])
        if hardware.expires_at <= time.time():
            raise ValueError("hardware proof expired during bootstrap")
        if value["evidence"].get("challenge") != challenge:
            raise ValueError("bootstrap challenge mismatch")
        return value["evidence"]

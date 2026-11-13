"""Strict NVIDIA NRAS v4 / claims-v3 appraisal at fixed vendor endpoints.

The SDK's default overall boolean is insufficient for Axiom's strict debug,
secure-boot, measurements, revocation and per-device signature requirements.
This applies the same claims policy as the existing Axiom pool verifier.
"""

import base64
import hashlib
import re
import time
from datetime import datetime, timezone

import httpx
from cryptography import x509
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec, utils

from .wire import decode_json, unbase64

NRAS = "https://nras.attestation.nvidia.com"


def _url64(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", value):
        raise ValueError("invalid JWT encoding")
    raw = base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    if base64.urlsafe_b64encode(raw).decode().rstrip("=") != value:
        raise ValueError("noncanonical JWT")
    return raw


def verify_jwt(token, jwks, anchors, nonce, now):
    if not isinstance(token, str) or len(token) > 2 * 1024 * 1024:
        raise ValueError("invalid NVIDIA JWT")
    parts = token.split(".")
    if len(parts) != 3:
        raise ValueError("invalid NVIDIA JWT")
    header = decode_json(_url64(parts[0]))
    if (not isinstance(header, dict) or set(header) - {"alg", "kid", "typ"}
            or header.get("alg") != "ES384" or not isinstance(header.get("kid"), str) or not header["kid"]):
        raise ValueError("unsupported NVIDIA JWT header")
    matching = [key for key in jwks["keys"] if key.get("kid") == header["kid"]]
    if len(matching) != 1 or not 2 <= len(matching[0]["x5c"]) <= 4:
        raise ValueError("ambiguous NVIDIA signing chain")
    ders = [unbase64(v, 64 * 1024) for v in matching[0]["x5c"]]
    if hashlib.sha256(ders[-1]).hexdigest() not in anchors:
        raise ValueError("NVIDIA signing anchor is not approved")
    certs = [x509.load_der_x509_certificate(raw) for raw in ders]
    instant = datetime.fromtimestamp(now, timezone.utc)
    for cert in certs:
        if not cert.not_valid_before_utc <= instant < cert.not_valid_after_utc:
            raise ValueError("expired NVIDIA signing certificate")
    for index, (cert, issuer) in enumerate(zip(certs, certs[1:])):
        constraints = issuer.extensions.get_extension_for_class(x509.BasicConstraints).value
        if not constraints.ca or (constraints.path_length is not None and constraints.path_length < index):
            raise ValueError("invalid NVIDIA certificate chain")
        cert.verify_directly_issued_by(issuer)
    public = certs[0].public_key()
    if not isinstance(public, ec.EllipticCurvePublicKey) or public.curve.name != "secp384r1":
        raise ValueError("NVIDIA signing key must use P-384")
    signature = _url64(parts[2])
    if len(signature) != 96:
        raise ValueError("invalid NVIDIA JWT signature")
    der = utils.encode_dss_signature(int.from_bytes(signature[:48], "big"),
                                     int.from_bytes(signature[48:], "big"))
    public.verify(der, (parts[0] + "." + parts[1]).encode(), ec.ECDSA(hashes.SHA384()))
    claims = decode_json(_url64(parts[1]))
    if claims.get("iss") != NRAS or claims.get("eat_nonce") != nonce:
        raise ValueError("NVIDIA issuer/nonce mismatch")
    for name, limit, lower in (("exp", now, True), ("iat", now + 30, False), ("nbf", now + 30, False)):
        if name in claims and (type(claims[name]) is not int or
                               (claims[name] <= limit if lower else claims[name] > limit)):
            raise ValueError("NVIDIA JWT validity mismatch")
    return claims


def verify_bundle(bundle, jwks, anchors, nonce, now=None):
    now = int(time.time()) if now is None else now
    if (not isinstance(bundle, list) or len(bundle) != 2
            or not isinstance(bundle[0], list) or len(bundle[0]) != 2 or bundle[0][0] != "JWT"
            or not isinstance(bundle[1], dict) or len(bundle[1]) != 1):
        raise ValueError("one complete NVIDIA detached EAT required")
    overall = verify_jwt(bundle[0][1], jwks, anchors, nonce, now)
    if (overall.get("x-nvidia-overall-att-result") is not True or overall.get("x-nvidia-ver") != "3.0"
            or type(overall.get("iat")) is not int or type(overall.get("exp")) is not int
            or not now - 120 <= overall["iat"] <= now + 30 or overall["exp"] <= overall["iat"]):
        raise ValueError("NVIDIA overall appraisal failed")
    submods = overall.get("submods")
    if not isinstance(submods, dict) or set(submods) != set(bundle[1]):
        raise ValueError("NVIDIA detached device set mismatch")
    expires_at = overall["exp"]
    for label, token in bundle[1].items():
        if not isinstance(token, str):
            raise ValueError("invalid NVIDIA detached token")
        expected = ["DIGEST", ["SHA-256", hashlib.sha256(token.encode()).hexdigest()]]
        if submods[label] != expected:
            raise ValueError("NVIDIA detached token digest mismatch")
        claims = verify_jwt(token, jwks, anchors, nonce, now)
        expires_at = min(expires_at, claims.get("exp", expires_at))
        if claims.get("secboot") is not True or claims.get("dbgstat") != "disabled" or claims.get("measres") != "success":
            raise ValueError("GPU secure-boot/debug/measurement policy failed")
        for name in ("x-nvidia-gpu-attestation-report-parsed", "x-nvidia-gpu-attestation-report-signature-verified",
                     "x-nvidia-gpu-attestation-report-nonce-match", "x-nvidia-gpu-attestation-report-cert-chain-fwid-match",
                     "x-nvidia-gpu-driver-rim-signature-verified", "x-nvidia-gpu-vbios-rim-signature-verified"):
            if claims.get(name) is not True:
                raise ValueError("GPU evidence or RIM verification failed")
        for name in ("x-nvidia-error-details", "x-nvidia-attestation-warning"):
            if claims.get(name) not in (None, False, "", [], {}):
                raise ValueError("GPU warning/error is forbidden")
        for name in ("x-nvidia-gpu-attestation-report-cert-chain", "x-nvidia-gpu-driver-rim-cert-chain",
                     "x-nvidia-gpu-vbios-rim-cert-chain"):
            chain = claims.get(name, {})
            if (chain.get("x-nvidia-cert-status") != "valid" or chain.get("x-nvidia-cert-ocsp-status") != "good"
                    or chain.get("x-nvidia-cert-ocsp-nonce-matches") is not True
                    or chain.get("x-nvidia-cert-ocsp-response-valid") is not True):
                raise ValueError("GPU revocation freshness policy failed")
    return expires_at


async def appraise(evidence, nonce, anchors):
    if (not isinstance(evidence, list) or len(evidence) != 1
            or not isinstance(anchors, list) or not 1 <= len(anchors) <= 8
            or any(not isinstance(v, str) or not re.fullmatch(r"[0-9a-f]{64}", v) for v in anchors)):
        raise ValueError("one GPU and explicit NVIDIA signing anchors required")
    device = evidence[0]
    if not isinstance(device, dict) or not isinstance(device.get("arch"), str) or not device["arch"]:
        raise ValueError("invalid GPU evidence")
    payload = {"nonce": nonce, "arch": device["arch"], "claims_version": "3.0",
               "evidence_list": [{"evidence": device["evidence"], "certificate": device["certificate"]}]}
    from .client import read_limited
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=20) as http:
        async with http.stream("GET", NRAS + "/.well-known/jwks.json") as response:
            if response.status_code != 200:
                raise RuntimeError("NVIDIA signing keys unavailable")
            jwks = decode_json(await read_limited(response, 2 * 1024 * 1024))
        # NRAS v3 silently returns claims v2 despite claims_version=3.0.
        # The v4 API supports the granular claims required by this policy.
        async with http.stream("POST", NRAS + "/v4/attest/gpu", json=payload) as response:
            if response.status_code != 200:
                raise RuntimeError("NVIDIA appraisal unavailable")
            bundle = decode_json(await read_limited(response, 2 * 1024 * 1024))
    return verify_bundle(bundle, jwks, anchors, nonce)

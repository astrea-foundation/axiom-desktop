#!/usr/bin/env python3
"""Standalone native helper entry; never accepts messages in CLI arguments."""
import argparse
import asyncio
import io
import json
import os
import ssl
import tempfile
from pathlib import Path
import sys

from chutes_desktop_bridge import run


def self_test():
    """Exercise bundled native extensions without an account or network."""
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    from dcap_qvl import get_collateral_and_verify
    from PIL import Image
    from pqcrypto.kem import ml_kem_768
    import certifi
    from axiom_chutes import gpu, publisher
    from axiom_chutes.runtime_policy import default_runtime, validate_runtime, aspect_dimensions
    from chutes_native_install import bridge_config, hosted_release_config
    from axiom_chutes.private_state import atomic_state, check_private, state_lock

    public, private = ml_kem_768.keygen()
    ciphertext, shared = ml_kem_768.encaps(public)
    if ml_kem_768.decaps(private, ciphertext) != shared:
        raise ValueError("ML-KEM self test failed")
    nonce, message = os.urandom(12), b"Axiom helper self test"
    aead = AESGCM(shared)
    if aead.decrypt(nonce, aead.encrypt(nonce, message, None), None) != message:
        raise ValueError("AEAD self test failed")
    signing = Ed25519PrivateKey.generate()
    signing.public_key().verify(signing.sign(message), message)
    image = io.BytesIO()
    Image.new("RGB", (1, 1)).save(image, format="PNG")
    image.seek(0)
    with Image.open(image) as decoded:
        if decoded.size != (1, 1):
            raise ValueError("image decoder self test failed")
    if not callable(get_collateral_and_verify) or not callable(gpu.appraise) or not callable(publisher.verify_metadata):
        raise ValueError("hardware verifier or publisher module missing")
    if ssl.create_default_context(cafile=certifi.where()).cert_store_stats()["x509_ca"] == 0:
        raise ValueError("bundled TLS trust roots missing")
    runtime = validate_runtime(default_runtime())
    if aspect_dimensions(runtime["image"]["default_aspect_ratio"]) != (1216, 1824):
        raise ValueError("runtime policy self test failed")
    with tempfile.TemporaryDirectory() as temporary:
        state = Path(temporary) / "private/state.json"
        with state_lock(state.with_suffix(".lock")):
            atomic_state(state, b'{"self_test":true}')
            check_private(state)
            if state.read_bytes() != b'{"self_test":true}':
                raise ValueError("private state self test failed")
    report = {"schema": "axiom-chutes-helper-self-test-v1", "passed": True,
              "components": ["ml-kem-768", "aes-gcm", "ed25519", "tdx-verifier", "nvidia-verifier", "pillow", "tls-roots", "runtime-policy-v1", "private-state-v1"],
              "live_attestation_verified": False, "frozen": bool(getattr(sys, "frozen", False))}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        if args.config:
            parser.error("self-test does not use account or relay configuration")
        print(json.dumps(self_test()), flush=True)
        return 0
    if args.config is None:
        parser.error("--config is required")
    try:
        asyncio.run(run(args.config))
    except Exception:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

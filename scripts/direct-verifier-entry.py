"""Frozen entry point for the independently verified AstreaAI protocol."""
import json
import sys

from axiom_direct.verify import main

if sys.argv[1:] == ["--self-test"]:
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    key = Ed25519PrivateKey.generate()
    message = b"axiom-direct-verifier-self-test-v1"
    key.public_key().verify(key.sign(message), message)
    print(json.dumps({"schema": 1, "passed": True, "frozen": bool(getattr(sys, "frozen", False))}))
else:
    main()

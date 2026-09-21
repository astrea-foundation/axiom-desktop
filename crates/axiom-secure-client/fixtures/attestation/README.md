# Attestation trust fixtures

`nvidia-nras-intermediate-004.pem` is the pinned NVIDIA Attestation Service GPU
Intermediate 004 certificate. It was obtained from `x5c[1]` of NVIDIA's NRAS
JWKS at `https://nras.attestation.nvidia.com/.well-known/jwks.json` and matches
the first-party Axiom frontend snapshot at commit `114be9a4061e`.

- SHA-256 of DER: `2df8907cf4d6c277b855d407ec178a649930a8e73f7294e8fce66e6ee27c5175`
- Validity: 2025-12-08 through 2029-12-08

Rotation is fail-closed. Accepted NVIDIA intermediate fingerprints come from
the verified signed trust policy; a JWKS response cannot add an anchor on its
own. A rotation may require fixture changes as well as a signed policy update.
Follow [trust-policy operations](../../../../docs/threat-model.md#trust-policy-operations).

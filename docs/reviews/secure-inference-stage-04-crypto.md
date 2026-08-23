# Secure inference Stage 04 review — NEAR v2 cryptography

Decision: **PASS (reviewed 2026-08-23)**

## Delivered

- New provider-private `near-v2` implementation of Ed25519-to-X25519 public and
  secret conversion, ephemeral X25519 ECDH, HKDF-SHA256, and
  XChaCha20-Poly1305-IETF.
- Exact wire layout and lowercase hex encoding from the protocol specification.
- Fresh per-request client Ed25519 identity and fresh per-field ephemeral
  X25519 key/nonce.
- Secret buffers use zeroizing wrappers or secret-key types with zeroization;
  debug output redacts the client secret and does not print the public identity.
- Strict key, hex, ciphertext, plaintext, and authentication limits.
- Selected backend-generated compatibility vectors with source commit and
  original fixture SHA-256 provenance.

## Verification

```text
cargo fmt --all -- --check
cargo clippy -p axiom-secure-client --all-targets --all-features -- -D warnings
cargo test -p axiom-secure-client
AXIOM_BACKEND_SOURCE=/home/salt/clone/axiom/backend \
  cargo test -p axiom-secure-client python_decrypts_rust_ciphertext -- --ignored
```

Nine ordinary tests pass and the explicit Python cross-language test passes.
Evidence includes:

- authoritative public/secret conversion and HKDF outputs;
- Python-encrypt/Rust-decrypt for empty, ASCII, and multibyte UTF-8 data;
- Rust-encrypt/Python-decrypt using the backend implementation;
- empty, 4,096-byte, and 262,000-byte round trips;
- fresh identity separation and wrong-recipient rejection;
- bit-flip, truncation, invalid hex, and oversize rejection;
- property-generated arbitrary ciphertext inputs do not panic;
- report-data binding vectors match the backend.

## Findings

- Blocker: none.
- High: none.
- Medium: none.
- Low: provider entry points are temporarily allowed as dead code until the next
  stages connect attestation and relay. The allowance is scoped to the private
  NEAR module and must be removed before the final gate.

## Exit criteria

- Cross-language byte compatibility: **met**.
- Fresh session identity and authenticated encryption: **met**.
- Tamper/wrong-key/truncation/limit failures close safely: **met**.
- No secret-bearing debug output: **met for implemented types**.

Stage 05 may begin.

# Secure inference Stage 05 review — local NEAR attestation

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `146cd78` (`feat: verify near attestations
locally`)

## Delivered

- A production NEAR provider driver selected only for the exact compiled
  provider, E2EE version, and attestation protocol tuple.
- Fresh 32-byte challenges and bounded report parsing with redirects disabled.
- DNS resolution validation and address pinning before provider, NVIDIA, and
  Intel collateral requests, preventing a second DNS lookup from pivoting to a
  private address.
- SHA-256 of the leaf SPKI from the exact TLS connection that delivered the
  attestation report.
- Local Intel DCAP quote/collateral verification using the ring-backed QVL,
  including model-key/TLS/nonce report-data binding and workload-manifest
  `mr_config_id` binding.
- Local NVIDIA NRAS verification: pinned intermediate fingerprint, leaf
  certificate signature and validity, ES384 JWT signature, pass verdict, and
  fresh `eat_nonce`.
- Normalized metadata-only evidence. The model key and TLS key are represented
  by fingerprints. The raw quote, NVIDIA payload/JWT, workload manifest, and
  collateral are not exposed through `SecurityEvidence`.
- A deliberately false `approved_workload_identity` claim until an explicit
  measurement allowlist is configured. Vendor evidence and quote-bound
  workload integrity are reported separately.

## Verification

The final gate was run after all corrective changes:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
AXIOM_NEAR_MODEL_BASE_URL=https://dsv4-flash.completions.near.ai/v1 \
  AXIOM_NEAR_UPSTREAM_MODEL=deepseek-ai/DeepSeek-V4-Flash \
  cargo test -p axiom-secure-client \
  live_provider_attestation_verifies_locally -- --ignored
```

All commands passed. The workspace suite includes 164 AxiomCLI unit tests and
all ordinary ACP, agent, MCP, persistence, PTY, policy, release, and soak
integration tests. The secure-client suite passed 15 deterministic tests; its
two opt-in tests remained ignored in the ordinary gate. The explicit live test
then fetched and locally verified a current DeepSeek V4 Flash report in 2.20
seconds.

Concrete adversarial evidence includes:

- report algorithm, challenge, model identity, and exact-connection TLS
  mutations are rejected;
- model-key/TLS/challenge report-data mutation is rejected;
- ambiguous or empty multi-model report wrappers are rejected;
- byte limits apply before JSON parsing and before quote, GPU evidence, JWT,
  JWKS, certificate, or collateral processing;
- a generated P-384 CA/leaf/JWT fixture proves the valid NVIDIA path;
- changed JWT signatures, non-pass verdicts, replayed nonces, and substituted
  anchors are rejected;
- provider protocol/version mismatches never begin attestation;
- malformed quotes and invalid/expired collateral are rejected by the local
  DCAP QVL before a verified session can exist.

## Findings and corrections

- High, corrected: the old Rust proxy decoded the NRAS JWT without verifying
  its signature or certificate chain. The new implementation does not reuse
  that behavior and authenticates the ES384 verdict against a reviewed pinned
  NVIDIA intermediate.
- High, corrected: enabling both `dcap-qvl` crypto backends introduced the
  RustCrypto `rsa` dependency affected by `RUSTSEC-2023-0071`. The dependency is
  now built only with its ring verification backend; `cargo deny` passes and
  `rsa` is absent from the secure-client dependency tree.
- Low, accepted: NVIDIA intermediate rotation requires a reviewed code update.
  This is intentional fail-closed behavior and the fixture records its source,
  fingerprint, and validity.
- Informational: the default policy has no approved workload measurement
  allowlist. The live result therefore proves vendor evidence, endpoint/model
  key binding, and quote-bound workload integrity, but does not claim an
  Axiom-approved software build.
- Blocker: none.

## Exit criteria

- Fresh live report verifies completely on the user's machine: **met**.
- Exact TLS connection, model key, model identity, and nonce are bound:
  **met**.
- Intel TDX and signed NVIDIA evidence verify locally: **met**.
- Binding and signature mutations fail closed: **met**.
- Evidence claims are no stronger than configured policy: **met**.
- Secret/plaintext material is absent from normalized evidence: **met**.

Stage 06 may begin.

# Secure inference Stage 13 review — deterministic proxy process fixture

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `6ddb920` (`test: verify proxy with encrypted
fixture`)

## Evidence

- The harness starts an adversarial encrypted provider/relay on an ephemeral
  TCP port, the actual built `axiom-proxy` child process, and an independent
  Reqwest client.
- The fixture is gated by both the non-default `test-fixture` Cargo feature and
  `AXIOM_TEST_FIXTURE=1`. Normal builds do not contain its provider or
  test-support API; production configuration still requires HTTPS and rejects
  local provider addresses.
- Establishment uses a fresh 256-bit nonce and verifies a domain-separated
  Ed25519 fixture signature over the model, nonce, exact provider URL, and model
  encryption key. Evidence labels state that it is process-test evidence, not
  TEE attestation.
- Requests and responses use the production `near-v2` conversion, ECDH, HKDF,
  XChaCha20-Poly1305, relay DTO, bounded SSE, secure-client, OpenAI adapter, and
  proxy route code.
- The actual-process test covers authentication-before-upstream, catalog,
  non-stream text, streamed content/reasoning/usage, single and parallel tools,
  fragmented streamed tool calls, encrypted assistant/tool history and result
  follow-up, credit exhaustion, tampered ciphertext, abrupt EOF, client
  disconnect, graceful shutdown, and parallel session isolation.
- Relay-body capture scans every secret-bearing request field. Proxy-log scans
  cover the Axiom credential, local token, message content, names, reasoning,
  refusal, tool names/descriptions/schemas/arguments/results, and paths.

## Commands and findings

All passed:

```text
cargo test -p axiom-proxy --all-features --test process_secure_fixture -- --nocapture
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

- High, corrected: a local fixture reachable through ordinary configuration
  would create a production bypass. Selection now requires compile-time and
  runtime opt-ins, while ordinary configuration remains fail-closed.
- High, corrected: the first recursive confidentiality inspector mistook the
  plaintext `required` tool-choice mode for ciphertext. The inspector now uses
  the protocol's explicit ciphertext-field allowlist, so a field cannot be
  silently skipped or misclassified by a naming heuristic.
- Medium, corrected: streaming disconnect needed proof at the remote body, not
  merely at the local response. A drop guard now proves the upstream response
  stream is released after the local client disconnects.
- Informational: deterministic signed fixture evidence deliberately does not
  prove Intel, NVIDIA, TLS SPKI, or production workload identity. Those claims
  remain exclusive to Lane C.
- Blocker/high unresolved: none in deterministic Lane B scope.

## Exit criteria

Lane B is green through TCP and the real executable, including confidentiality,
fault injection, tools, streaming, cancellation, and cross-session isolation.
Stage 14 may proceed to protected live-service verification.

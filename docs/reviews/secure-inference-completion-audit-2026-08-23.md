# Secure inference and proxy completion audit

Decision: **PASS (audited 2026-08-23)**

Implementation tip audited: `7abdf55a45aa641ac564a5b4218f8b78a99710a2`

Claimed scope: the early Linux x86-64 GNU AxiomCLI and `axiom-proxy` release
candidate described by the plan. This is not an Electron, wallet, multi-user,
LAN proxy, or approved-workload-identity claim.

## Definition-of-complete mapping

| # | Requirement | Evidence | Result |
|---:|---|---|:---:|
| 1 | One production Axiom transport | AxiomCLI and the proxy depend on `axiom-secure-client`; the legacy adapter is build- and runtime-quarantined. | PASS |
| 2 | Provider-modular boundary | The sealed provider registry owns session establishment and application crates contain no NEAR protocol branches. | PASS |
| 3 | Authoritative vectors and live attestation | Stages 04–05 prove Python/Rust `near-v2` compatibility; Stage 14 verifies live TDX, NVIDIA, nonce, model/key, and exact TLS-SPKI bindings. | PASS |
| 4 | Real authenticated loopback proxy | Stages 12–14 prove models plus streaming and non-streaming chat through the actual process with a separate local credential. | PASS |
| 5 | Confidential secret-bearing fields | Stage 09 and the Stage 13 relay capture assert ciphertext for messages, tool definitions, choices, arguments, results, reasoning, and response fields. | PASS |
| 6 | Fail closed | Crypto, attestation, process, malformed SSE, missing terminal, contract mismatch, network, and cancellation negatives reject without downgrade. | PASS |
| 7 | Real OpenCode workflow | Stage 15 repeats read/edit/read/final, parallel reads, and cancellation through the live proxy. | PASS |
| 8 | Direct AxiomCLI workflow | Stage 16 repeats live observe/read and confirm/edit/read/final ACP loops with no proxy hop. | PASS |
| 9 | No secret leakage | Exact runtime-credential scans cover the repository and isolated artifacts; relay captures/logs pass plaintext-sentinel scans; the release archive rejects credential-shaped data. Synthetic redaction-test values are not packaged. | PASS |
| 10 | Reviewed stages and final audit | Stages 00–16 all have PASS records with exact implementation commits; the independent clean-checkout gate passes. | PASS |

## Stage record

| Stage | Review | Decision |
|---:|---|:---:|
| 00 | `secure-inference-stage-00-baseline.md` | PASS |
| 01 | `secure-inference-stage-01-protocol.md` | PASS |
| 02 | `secure-inference-stage-02-contracts.md` | PASS |
| 03 | `secure-inference-stage-03-lifecycle.md` | PASS |
| 04 | `secure-inference-stage-04-crypto.md` | PASS |
| 05 | `secure-inference-stage-05-attestation.md` | PASS |
| 06 | `secure-inference-stage-06-relay.md` | PASS |
| 07 | `secure-inference-stage-07-nonstream.md` | PASS |
| 08 | `secure-inference-stage-08-streaming.md` | PASS |
| 09 | `secure-inference-stage-09-tools.md` | PASS |
| 10 | `secure-inference-stage-10-axiomcli.md` | PASS |
| 11 | `secure-inference-stage-11-openai-adapter.md` | PASS |
| 12 | `secure-inference-stage-12-proxy.md` | PASS |
| 13 | `secure-inference-stage-13-process-fixture.md` | PASS |
| 14 | `secure-inference-stage-14-live.md` | PASS |
| 15 | `secure-inference-stage-15-opencode-tools.md` | PASS |
| 16 | `secure-inference-stage-16-hardening.md` | PASS |

## Acceptance-lane result

- Lane A: deterministic protocol/crypto/attestation/relay suites — PASS.
- Lane B: actual proxy process with encrypted fixture and adverse responses —
  PASS.
- Lanes C/D: live local attestation plus streaming/non-streaming chat,
  cancellation, and unavailable-network behavior for two models — PASS.
- Lane E: unmodified OpenCode through the live proxy with ordered and parallel
  tools — PASS twice.
- Lane F: AxiomCLI through the direct secure client with read and approved edit
  loops — PASS twice.

The normal Rust gate, root and fuzz Cargo Deny checks, release lifecycle,
credential/plaintext scans, and a clean-checkout deterministic rerun also pass.

## Explicit deferrals and residual risk

- A provider-approved workload measurement allowlist is not available. Local
  verification proves the stated hardware, key, model, freshness, and endpoint
  properties only, and reports the missing approval explicitly.
- Only Linux x86-64 GNU is claimed. macOS, Windows, and other Linux targets need
  their own TLS trust-store, loopback, lifecycle, packaging, and cancellation
  gates before support is claimed.
- Attestation caching, a second provider, `/v1/responses`, embeddings,
  Anthropic compatibility, LAN exposure, persisted receipts, Electron
  supervision/keychain/update handling, and wallet authority remain deferred as
  the plan specifies.
- Live behavior depends on provider availability and compatible current model
  capabilities. Deterministic fixtures remain the regression baseline.

These items are visible scope limits, not unresolved blocker/high findings in
the claimed release-candidate scope. All ten completion conditions are met.

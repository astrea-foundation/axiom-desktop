# Secure inference Stage 01 review — protocol and threat specification

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `b6aebc1` (`docs: specify secure inference
protocol`)

## Evidence reviewed

- Axiom backend commit `114be9a4061e8b1a9049f1e31b7b05c52a694c50`.
- Axiom proxy commit `29efe06c6b6598ee94b81d47c5b0409d99974c15`.
- Backend Python E2EE primitives, vector generator, relay schemas/service, NEAR
  verifier, and provider contract tests.
- Working Rust proxy E2EE, attestation, relay, OpenAI validation, streaming, and
  encrypted-tool integration tests.
- `docs/protocols/near-v2.md`, ADR 0010, and the updated threat model.

## Contract decisions

- Backend snapshot `114be9a` is authoritative for wire compatibility.
- `near-v2` is provider-specific; it is not embedded in provider-neutral domain
  types.
- A provider owns establishment through decryption and yields an opaque verified
  session.
- Attestation redirects are disabled and TLS binding comes from the exact report
  connection.
- Initial trust policy accepts Intel `UpToDate` and `SWHardeningNeeded`, plus
  NVIDIA `verified`, while explicitly not claiming an approved workload
  measurement.
- Production has no plaintext or TLS-only fallback.
- The proxy transports tools but never authorizes or executes them.

## Coverage

The specification defines key formats, conversions, HKDF, AEAD, wire layout,
report-data binding, attestation checks, catalog fields, relay request fields,
SSE events, confidentiality classification, request limits, compatibility
vectors, and terminal failure semantics.

Every secret-bearing field listed in the implementation plan has an encrypted
wire location and a required sentinel test. Every permitted plaintext field is
routing, cryptographic-public, parameter, usage, or terminal metadata.

## Findings

- Blocker: none.
- High: none.
- Medium: approved workload measurements are not presently supplied as a
  maintained client allowlist. This is recorded as a deliberately narrower
  security claim, not silently treated as verified code identity.
- Low: relay-level remote cancellation semantics require live confirmation; the
  local client must still stop/deallocate promptly and record cancellation.

## Exit criteria

- A compatible client can be implemented from the local protocol document and
  versioned vectors: **met**.
- Backend/proxy precedence is explicit: **met**.
- Provider-specific and application-neutral boundaries are decided: **met**.
- Security-critical ambiguity is either resolved or represented as a narrower
  claim: **met**.

Stage 02 may begin.

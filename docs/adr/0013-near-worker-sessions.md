# ADR 0013: Single-key NEAR worker sessions

Status: native implementation accepted; deployment must be checked for the release

Date: 2026-09-19

## Context

Independent attestation requests can reach different NEAR workers. Even two reports
from one connection do not coordinate native verification with the relay's separate
model-wide key lease and inference connection. NEAR supplies Ed25519 receipts signed
by the same identity used for its encryption-key conversion.

## Decision

Register `near-v3` with `near-tdx-nvidia-v2`, retaining encryption wire version 2.
The native client sends a fresh challenge to the backend, which selects one direct
worker and returns its raw evidence plus an account-scoped session lease. The native
client independently verifies Intel, NVIDIA, nonce, key, and measured-manifest
bindings before encrypting. The backend's verification assertion is not trusted.

The client verifies one Ed25519 identity for encryption and response signatures.
Every receipt binds the exact model and request/response hashes; streamed transcripts
must authenticate ordering and completion. Receipt lookup does not replay inference.

The backend persists immutable worker-session metadata independently of the latest
model lease. Its connection cache is optional and every new upstream TLS connection
must match the attested SPKI before request bytes are sent. Session expiry, account,
model, protocol and key are validated on each request.

The client reports an **attested worker TLS key**, not an independently observed live
TLS connection. Local attestation and Ed25519 receipt verification establish the
client's cryptographic trust across the ciphertext relay. The backend's connection
pin is an additional routing check. This supersedes the NEAR-specific establishment
details in ADR 0012; other providers are unchanged.

## Consequences

Multiple legitimate workers may coexist without invalidating each other's sessions.
Connection rotation before dispatch triggers the existing one-time re-attestation
flow. After potentially dispatched inference, errors remain incomplete; no automatic
generation replay is introduced.

The backend includes worker-session support in its `platform_20260921` baseline. Native/backend releases must be coordinated;
old NEAR protocol offers are rejected by the new driver. Provider hardware-policy
rejections and unavailable endpoints remain real failures, with specific diagnostics.

See [the wire contract](../protocols/near-v3.md).

## Validation recorded 2026-09-19

These results describe the original batch and are not a current production or
release-acceptance claim.

The Rust workspace passed 508 tests (six existing ignores), with a subsequent
additional retry-classification regression passing too. Desktop proof rendering
and TypeScript checks passed. Twelve live requests used real vendor verification,
encryption, Ed25519 receipts, streaming and tool continuations. Axiom debited
$0.001999 against $0.001998230 in NEAR's per-request billing records; the difference
is rounding. The final five requests exercised the selectable GLM 5.3 Flash offering.

The backend suite passed 606 tests, including real TLS rotation with zero POST bytes
sent to the wrong worker and retries that cannot duplicate potentially sent inference.
Strict Clippy retains pre-existing warnings outside the new provider implementation.
See the [sanitized validation and billing report](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/testing/near-v3-2026-09-19/README.md).

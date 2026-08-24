# Secure inference Stage 06 review — catalog and encrypted relay

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `ac1a83d` (`feat: implement encrypted relay
inference`)

## Evidence

- The authenticated catalog is bounded before parsing, rejects duplicate IDs,
  impossible limits, unsupported reasoning levels, driver mismatches, malformed
  URLs, and provider hosts resolving to non-public addresses.
- Relay DTOs expose ciphertext wrappers and allowed metadata only. There is no
  plaintext message/tool relay request type.
- Relay POSTs have bounded serialized bodies, bounded responses/errors, stable
  status mapping, cancellation, no redirects, DNS pinning, and no ambiguous
  POST retry.
- The SSE decoder handles arbitrary byte fragmentation, CRLF/LF, comments,
  Unicode split across chunks, the six allowed events, size limits, unknown
  events, invalid UTF-8, and abrupt EOF.
- Credential debug output and upstream error-body reflection are redacted.

## Commands

All passed:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
git diff --check
credential/plaintext pattern scan outside target/.git
```

## Findings

- High, corrected: verified sessions initially had no credential path to the
  relay. Credentials are now reference-counted secret values owned only by the
  secure client/provider/session boundary.
- Medium, corrected: the provider-neutral role vocabulary omitted the relay's
  `developer` role. It is now represented without leaking provider terms.
- Informational: the authoritative relay has no idempotency contract, so
  inference POSTs are deliberately never retried.
- Blocker/high unresolved: none.

## Exit criteria

Authenticated discovery, strict provider resolution, typed ciphertext-only
transport, bounded SSE parsing, stable safe errors, and SSRF resistance are
all met. Stage 07 may proceed.


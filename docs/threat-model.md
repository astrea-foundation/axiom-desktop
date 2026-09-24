# Security and provider trust operations

This guide describes the native implementation's trust boundaries. The
[privacy guide](privacy.md) explains user-visible data handling.
[Release acceptance](validation.md) defines checks for actual deployed artifacts.

## Assets and authority

Protect conversation/tool plaintext, credentials, inference key material,
workspace integrity, process authority, account isolation, billing projections
and truthful verification status. Models, repository input, fetched content,
MCP protocol data, catalogs, attestation payloads and external ACP clients are
untrusted. The native policy engine validates normalized tool effects;
model output never grants authority.

Electron renderer isolation uses sandboxing, context isolation, disabled Node
integration and a typed preload API. Main validates IPC; AxiomCLI validates
again. Packaged sidecars use fixed application-resource paths. Only unpackaged
development accepts the sidecar override. Local ACP legitimately carries
conversation content between native runtime and UI; hosted APIs do not.

## Provider contracts

Intel `OutOfDate` is blocked by default. A user can explicitly accept this single
TCB posture for NEAR until the native process restarts. Consent is held in memory,
shared across that provider's models, and sent explicitly when establishing each
relay worker session. It does not alter signed policy or carry into the separate
local proxy. The UI labels accepted evidence **degraded**, retains its advisories,
and shows a yellow warning even after an authenticated reply. No other rejected
TCB status, stale proof, binding failure, GPU failure or response-authentication
failure is overridable. See the [NEAR contract](protocols/near-v3.md).

The [compiled registry](../crates/axiom-secure-client/src/provider.rs) selects
these exact contracts. Unknown provider offerings can be quarantined; a catalog
cannot authorize an unsupported protocol or unverified public key.

| Provider | Registered contract | Authentication boundary |
|---|---|---|
| NEAR | `near-v3`, encryption `2`, `near-tdx-nvidia-v2` | Fresh challenged direct-worker evidence, locally verified Intel/NVIDIA and key/manifest bindings; Ed25519 receipt for exact request/response/model |
| Tinfoil | `tinfoil-ehbp-v1`, encryption `1`, `tinfoil-snp-sigstore-v1` | Pinned verifier checks SNP router/build/TLS/HPKE binding; request-bound encrypted response and authenticated completion |

Provider-specific wire details live in
[NEAR v3](protocols/near-v3.md) and [Tinfoil EHBP](protocols/tinfoil-ehbp-v1.md).

NEAR v3's client verifies an **attested worker TLS key**. The backend pins its
own live worker connection; the native client does not claim to have observed
that socket. Tinfoil verifies the measured router and its worker-verification
chain, not independent local quotes for every GPU. Neither claim is independent
proof of arbitrary model weights/source code beyond the accepted protocol.

## Freshness, streaming and failure

Only public verified material is cached. Production defaults bound it to four
minutes and additionally to applicable provider evidence, lease and policy expiry.
Every request creates fresh private contexts/nonces and request bindings.
Security rejection invalidates the usable path. No plaintext, unauthenticated-key
or TLS-only inference mode is available.

A protocol-authenticated delta may be displayed as provisional. Success requires
authenticated order and completion to the request and preverified environment.
NEAR use terminal signatures/receipts; Tinfoil requires authenticated frames,
valid terminal application fields, an encrypted `[DONE]` and matching final
accounting. Independent AEAD chunks plus an unsigned end marker are insufficient.
Missing, reordered, truncated or mismatched responses fail closed. Accounting
alone never proves response authenticity.

NEAR public evidence includes a positive relay attestation generation and
hard expiry. Tinfoil omits the generation because it has no relay lease generation;
its registered protocol, verified identity and expiry remain required. UI evidence
must describe the checks actually made.

A typed, proven pre-dispatch `ATTESTATION_KEY_CHANGED` can establish a new session
and re-encrypt once. Ambiguous dispatch, missing receipts or partial output cannot
authorize replay of a possibly billable inference.

## Trust-policy operations

The signed envelope is bundled at
[axiom-production-v2.json](../crates/axiom-secure-client/policy/axiom-production-v2.json)
and distributed by the authenticated relay. [security.rs](../crates/axiom-secure-client/src/security.rs)
validates the signature, bounded validity, mandatory vendor rules, sequence and
certificate anchors. CLI/Desktop share an atomic rollback journal. A failed
refresh leaves the still-valid last-known-good policy in use. Invalid signatures,
rollback and equivocation cannot replace that policy; an expired policy cannot
authorize inference.

NEAR uses the verified policy's NVIDIA intermediate fingerprints. Empty,
malformed or unlisted anchors are rejected; signed overlap permits rotation.
Certificate validity/signatures, verdict nonce, TDX and protocol-specific response
authentication remain required.

The current envelope uses schema 2 and sequence 3 with a new Ed25519 verification
key. Its payload contains vendor requirements, certificate anchors and validity
metadata. Publish the matching native and backend artifacts together. The bundled
sequence supersedes older rollback-journal entries without parsing their policy;
an equal or higher cached sequence must still verify. A higher sequence must be
preserved during future signing-key changes.

Clients released with schema 1 reject schema 2 refreshes and retain their existing
policy until it expires. Release the paired native update to move those clients
to the current policy. The new signing key must remain in controlled release
storage outside either repository; the relay receives only the signed envelope.

For a policy update:

1. Increase the sequence and set a bounded validity window with rollout overlap.
2. Preserve mandatory vendor/freshness/key checks. Overlap old/new NVIDIA anchors
   during certificate rotation; an unauthenticated JWKS change cannot add trust.
3. Sign using the controlled offline/HSM-backed release signer. Copy the same
   envelope to the native/backend repositories and run signature/rollback/tamper tests.
4. Publish through the backend release process, then release compatible clients;
   verify the new sequence and persisted rollback state with an attested canary.

Never store the private signer in source, runtime configuration or logs. A signer
compromise requires a client trust-root replacement; backend distribution alone
cannot replace the compiled trust root. During provider outages, restore the
service within accepted expiry bounds rather than extending trust lifetimes or
marking security failures retryable. Backend lease and deployment operations are
maintained separately by the platform team.

Canary each selected provider/model with discovery, fresh local attestation,
an encrypted completion and authenticated stream termination. Check the specific
worker/lease binding where applicable; Tinfoil has no relay lease generation.
Monitor policy expiry/rollback failures, unsupported catalog entries, repeated
pre-send key changes, rejected NVIDIA anchors and receipt timeout. The default
native catalog cache permits at most 30 minutes of stale metadata after transient
transport failures. Authentication/schema failures and withdrawals cannot use that
fallback. Catalog freshness never bypasses attestation or endpoint validation. See
[secure-client configuration](../crates/axiom-secure-client/src/config.rs).

## Local tools and processes

Native file tools canonicalize/recheck workspace paths and bound their input and
output. Permissions combine profile rules, normalized effects and explicit deny
rules. Confirm asks for each invocation. Full access skips routine prompts but
does not bypass structural validation. Repository configuration cannot launch
MCP servers or grant broader authority.

[workspace.rs](../apps/axiomcli/src/workspace.rs) has different process paths:

- **Linux:** `/usr/bin/bwrap` is mandatory. Commands run in isolated namespaces,
  with the workspace writable, system/toolchain mounts read-only, a temporary
  home, a cleared/allowlisted environment and no host network namespace.
- **macOS:** commands use the host user's authority with a sanitized environment
  and canonical working directory. No equivalent OS sandbox is implemented.
- **Windows:** programs are resolved through the sanitized path. `.cmd`, `.bat`
  and `.ps1` require the explicit shell tool. Process groups and a kill-on-close
  Job Object support descendant cleanup; Job Object setup failure emits a warning.
  This is lifecycle control, not an OS sandbox.

Processes have bounded output/time and cancellation cleanup; background tasks
have four slots. An approved host-authority program on macOS/Windows can access
resources beyond its cwd. The Web toggle only gates built-in search/fetch tools.
MCP executables are trusted user-selected host code at startup; returned schemas,
text and side-effect claims stay untrusted. Ambiguous side-effecting calls are
never automatically replayed. Read-only reconnect retry requires unchanged schema.

## Accounts, local state and recovery

Opaque account paths, store closure and account-generation checks prevent stale
operations from crossing login boundaries. Native access/refresh tokens never
enter renderer state. The separate API-key creation operation intentionally returns
one new automation key for display; its Debug/state paths redact or omit the token.
There is no automation-key submission path for Desktop native sign-in.

Conversation databases are not encrypted at rest. Startup repairs unfinished
turns to interrupted state without replay. Delete confirmations bind the exact
selection, expire after 60 seconds and are one-use/process-local. IPC framing is
bounded to 64 MiB (including escaped text and inline image payloads); retained sidecar stderr is capped at 64 KiB. Sequence gaps require
snapshot replacement, not assumed success.

Account credit/deposit/gift-redemption operations require a native account session,
validate bounded amounts and identities, and discard results after an account
change. They do not grant wallet or model transaction authority. See
[architecture](architecture.md#account-credit-and-integrations).
Gift codes cross the authenticated HTTPS account boundary only from a private
input control. They are omitted from conversation persistence, activity events
and debug output. Interrupted redemption does not establish failure to credit;
the backend makes same-account retries financially idempotent.

# ADR 0012: Expiring attestation leases and signed trust policy

> Partially superseded. [ADR 0013](0013-near-worker-sessions.md) replaces NEAR
> model-wide establishment with challenged worker sessions. Tinfoil has no relay
> lease generation. Phala verifies exact measured-workflow provenance and the
> complete worker/receipt chain without repository/commit allowlists or manual
> approval of gateway updates. The original signed-policy/source-approval wording
> below must not be used to introduce such pins. See [current trust operations](../threat-model.md).

Status: accepted

Date: 2026-08-28

## Context

Provider attestation has two independent consumers: the Axiom relay must avoid
forwarding ciphertext to a stale recipient key, and desktop/CLI must verify the
provider locally before encrypting. Historical verification rows, per-request
verification, hardcoded certificate intermediates, and all-or-nothing catalogs
are poor durability boundaries. They either remain trusted indefinitely or turn
a routine provider rotation/outage into a complete product outage.

## Decision

1. The backend owns a short provider/model attestation lease. It binds the full
   routing and protocol identity, key generation, raw-key fingerprint, optional
   keyset digest, verification record, soft expiry, and hard expiry.
2. PostgreSQL performs refresh singleflight across workers. Only a retryable
   verifier-availability failure may use the previous hard-valid generation.
   Security rejection and hard expiry never fall back.
3. Every new client request binds its ciphertext to the lease generation,
   fingerprint, and keyset. A mismatch is rejected before provider I/O with a
   typed `ATTESTATION_KEY_CHANGED` response.
4. Desktop/CLI re-attest, create new client crypto, and re-encrypt the original
   request once for that exact typed pre-send response. No ambiguous error is
   retried.
5. Clients cache verified public material per exact model and trust policy for
   at most four minutes, bounded by every upstream expiry. Per-request secrets
   and nonces are never cached.
6. The model catalog negotiates an explicit secure contract and quarantines a
   malformed/unsupported entry without hiding healthy providers.
7. Mandatory TCB rules, NVIDIA intermediate fingerprints, workload identities,
   version, validity, and a monotonic sequence live in an Ed25519-signed policy.
   The authenticated API distributes it but cannot forge it. CLI and desktop
   share an atomic rollback journal and use a still-valid last-known-good policy
   during transport outages.
8. The Phala verifier is an owned in-tree fork. It exposes the full quote-bound
   repository and commit for signed-policy evaluation and retains receipt-gated
   output.

## Consequences

Normal attestation work is amortized without weakening per-request encryption.
Key and certificate rotations have an overlap path, while compromised or
expired evidence fails closed. Operations must publish policy updates before
the previous policy expires and must overlap old/new NVIDIA anchors. Workload
enforcement progresses from observe to warn to enforce only after canaries show
complete approved identities; an empty allowlist never silently approves a
workload.

NVIDIA NRAS, Intel collateral services, NEAR, and Phala remain external
availability dependencies. The design bounds their outage effect but cannot
make an independent attestation service available when that service is down.

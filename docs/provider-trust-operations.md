# Provider trust operations

This runbook covers the security state shared by Axiom Desktop, AxiomCLI, and
the Axiom API. It is intentionally separate from provider credentials and
billing configuration.

## Normal behavior

NEAR now uses [near-v3 worker sessions](protocols/near-v3.md), with fresh client
challenges and an exact worker/session binding. The generic lease/cache rules
below do not permit a client to skip those checks or switch workers on retry.

- Backend leases refresh after 240 seconds and hard-expire after 300 seconds.
- Only retryable attestation-service unavailability may bridge the soft-to-hard
  interval. A TCB, signature, model, endpoint, key, keyset, or workload rejection
  revokes the usable path immediately.
- Clients cache only verified public material for at most 240 seconds. The
  provider evidence, backend lease, and signed policy can shorten that bound.
- A client that locally sees a key rotation before the API does receives a typed
  pre-send rejection, invalidates its cache, re-attests, and re-encrypts once.
- Catalog failure uses a provider-isolated last-known-good catalog for at most
  24 hours; it does not bypass endpoint validation or attestation.

## Signed-policy release

The checked-in policy is
`crates/axiom-secure-client/policy/axiom-production-v1.json`; the API serves the
same signed envelope from `backend/src/axiom_api/trust/`. The private Ed25519
policy signer must live in an offline/HSM-backed release secret and must never
be present in source, a developer `.env`, a runtime container, logs, or release
artifacts.

For every policy update:

1. Start from the last released envelope and increase `sequence`; never reuse a
   sequence for different content.
2. Set a bounded validity window with enough overlap for older desktop builds.
3. Keep `UpToDate`, NVIDIA verification, model binding, and live TLS binding
   mandatory. The parser rejects a manifest that weakens them.
4. For an NVIDIA rotation, add the new intermediate fingerprint while the old
   one remains valid, deploy and canary it, then remove the old fingerprint in
   a later sequence.
5. Review any NEAR workload-policy changes against its current attestation contract.
   Phala source updates use exact quote-bound workflow provenance, not repository
   or commit allowlists. The legacy Phala fields remain in signed v1 documents
   for signature compatibility; they do not authorize or reject a gateway update.
6. Sign the canonical policy with the enrolled Ed25519 signer, copy the exact
   envelope to both repositories, run signature/rollback/tamper tests, deploy
   the API first, and then release clients.
7. Confirm `/api/v1/relay/trust-policy` returns the new sequence and that a
   restarted CLI retains it in the shared rollback journal.

The bundled policy uses `observe` for NEAR workload measurements. The UI must
not label an observed workload as approved. Phala still verifies the exact
TEE-measured source/workflow and complete worker/request receipt chain; source
provenance does not require manual approval of each gateway revision.

## Canary and alerts

Canary every provider/model contract for: catalog discovery, fresh local
attestation, backend key-lease equality, one encrypted completion, stream
termination, and (for Phala) signed receipt verification. Alert on:

- lease refresh failures or a lease within 60 seconds of hard expiry;
- repeated `ATTESTATION_KEY_CHANGED` after the one allowed refresh;
- unknown/unsupported catalog entries increasing unexpectedly;
- policy signature/rollback/expiry failures;
- NVIDIA `kid` or intermediate fingerprints outside the overlap set;
- Phala receipt latency approaching the 25-second verification window;
- NEAR workload-policy observations and any Phala quote/workflow/source binding mismatch.

## Incident response

- Provider evidence unavailable: keep the last generation only until hard
  expiry; do not extend TTLs during the incident.
- Security rejection: disable the affected offering or publish a reviewed
  signed policy update. Never mark the error retryable to restore traffic.
- Key rotation loop: disable the one offering, compare local evidence with the
  backend lease, and check that catalog identity did not change.
- Policy endpoint unavailable: clients use their still-valid last-known-good
  policy. Restore distribution before its expiry.
- Signer compromise: stop policy publication, ship a client release with a new
  trust root, and increase the sequence. A backend-only change cannot replace a
  compromised compiled policy key.

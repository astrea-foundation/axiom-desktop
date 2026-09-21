# Axiom-owned Phala verifier fork

This crate retains the Phala verifier and receipt-gated cipher from
`axiom-core` 0.1.6, originally from
`https://github.com/astrea-foundation/axiomio`, pinned at commit
`f7b871ac2214becb12e541ab2ac5c5fd963c6e8c`.

The upstream repository was archived. Keeping the verifier in this repository
removes a build-time dependency on an archived Git host and lets Axiom review,
test, and update provider evidence formats atomically with the desktop and CLI.
The upstream MIT license is preserved in `LICENSE`.

Only the Phala-specific verifier/cipher boundary is retained. The upstream
NEAR implementation, generic proxy/UI types, permissive compatibility aliases,
and provider fallbacks are neither compiled nor exported. Axiom's current NEAR
implementation lives independently in `axiom-secure-client`.

Local security changes from the pinned source:

- implement two deliberately disjoint gateway artifact formats:
  `aci-v1-08d550` for standard ACI and `dstack-aci-draft-d567f6` for draft ACI;
  these stable format IDs retain historical suffixes, not source-revision pins;
- require each format's evidence shape, model mapping, key algorithms, key
  identifiers, receipt shape, and signed event sequence so formats cannot be mixed;
- verify the reported source repository and commit against the exact TEE-measured
  workflow, without a repository/commit allowlist or manual gateway-update approval;
- bind the encrypted wire request and the byte-exact restored plaintext request
  independently, then require the terminal receipt to commit to both hashes;
- recover standard-profile secp256k1 receipt signers from the 65-byte
  recoverable signature and compare them to the attested receipt key;
- expose the full quote-bound gateway repository and commit in verified provider
  state for inspection without claiming Axiom code approval;
- accept NVIDIA intermediate fingerprints from Axiom’s locally verified signed
  trust policy, including overlap during certificate rotation; and
- retain fail-closed TDX, measured compose, keyset, worker-session,
  transparency-event, response-hash, and terminal-receipt verification.

Compatible gateway source updates need no Axiom release. A new evidence format
still requires an explicit verifier implementation and cross-language vectors.

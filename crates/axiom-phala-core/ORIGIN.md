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

- implement two deliberately disjoint, content-addressed gateway profiles:
  `aci-v1-08d550` for the standard ACI artifact at commit
  `08d550817ee36eeadea01689b134e46791216b66`, and
  `dstack-aci-draft-d567f6` for the deployed draft artifact at commit
  `d567f6b4c4d93e33037ea202db04db86a8ad881c`;
- require each profile's exact evidence shape, source identity, model mapping,
  key algorithms, key identifiers, receipt shape, and signed event sequence so
  fields from the two protocol revisions cannot be mixed;
- bind the encrypted wire request and the byte-exact restored plaintext request
  independently, then require the terminal receipt to commit to both hashes;
- recover standard-profile secp256k1 receipt signers from the 65-byte
  recoverable signature and compare them to the attested receipt key;
- expose the full quote-bound gateway repository and commit in verified provider
  state so the signed Axiom trust policy can evaluate workload identity; and
- retain fail-closed TDX, measured compose, keyset, worker-session,
  transparency-event, response-hash, and terminal-receipt verification.

The profile names above are content-addressed evidence profile identifiers.
Adding or changing a profile requires a reviewed source pin and corresponding
cross-language vectors; the verifier does not infer protocol versions from
permissive evidence fields.

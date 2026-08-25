# Secure provider default migration

AxiomCLI and `axiom-proxy` now use `axiom-secure-client` as their only default
production inference transport. AxiomCLI establishes verified sessions
in-process; it does not use the localhost proxy. The proxy independently uses
the same crate for OpenAI-compatible clients.

The earlier plaintext OpenAI-compatible `AxiomProvider` remains only as a
quarantined protocol-development fixture:

- its implementation is compiled only for library tests or the explicit
  `development-provider` Cargo feature;
- the AxiomCLI binary can select it only when that feature was compiled and
  `AXIOMCLI_UNATTESTED_DEV=1` is set;
- the default workspace and release package do not enable that feature;
- its state remains visibly `UNATTESTED DEV` and has no automatic fallback
  path from the secure provider.

Remove development overrides before testing a release candidate. Existing
`AXIOM_BASE_URL` configuration remains valid, but production endpoints must
satisfy the secure client's HTTPS/public-address and provider-evidence policy.

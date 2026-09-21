# Axiom native documentation

Use the [root quickstart](../README.md) for builds and
[CONTRIBUTING.md](../CONTRIBUTING.md) for development/release branches.
The companion [platform documentation](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/README.md)
owns hosted API, billing integration and website operations.

## Current guides

- [Fresh storage, deposit billing and reset procedure](adr/0014-clean-storage-and-deposit-billing.md).
- [Configuration](configuration.md), [privacy](privacy.md), [threat model](threat-model.md)
  and [troubleshooting](troubleshooting.md).
- [Installing Desktop](installing-desktop.md), [platform support](platform-support.md),
  [releasing](releasing.md) and [versioning](versioning.md).
- [ACP compatibility](acp-compatibility.md), [extension contract](axiom-acp-extension.md)
  and [desktop parity](acp-desktop-parity-matrix.md).
- [Provider trust operations](provider-trust-operations.md),
  [NEAR v3](protocols/near-v3.md) and [Tinfoil EHBP](protocols/tinfoil-ehbp-v1.md).
- [Benchmarks](benchmarks.md) and [evaluations](evaluations.md).

Architecture decisions are in `docs/adr/` and scoped validation evidence is in
`docs/testing/`. The old near-v2 protocol is historical;
use current provider contracts. Superseded clean-slate plans and invoice QA were
removed; their remaining release gates are recorded in ADR 0014.

Keep new guides under `docs/` and update the applicable guide with every change
to the behavior it describes. Component READMEs remain setup entry points.

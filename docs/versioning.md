# Versioning and compatibility

Tagged releases use Semantic Versioning. Before 1.0, a minor release may change
CLI, library or configuration contracts; patch releases preserve compatibility
except for explicitly documented security fixes. Public package tags must be
stable `vMAJOR.MINOR.PATCH`, match AxiomCLI's manifest and point to a promoted
`main` revision. [Releasing](releasing.md) owns the packaging procedure.

## Local database

The current SQLite schema is **7**, defined in
[session.rs](../apps/axiomcli/src/session.rs). Each authenticated account and
frontend has a separate database. Runtime event envelopes are translated into
materialized records; they are not the on-disk format.

| Schema | Change |
|---|---|
| 1 | Threads, turns, timeline items, profile preferences, collections and assignments |
| 2 | Last message activity timestamp |
| 3 | Last provider-reported conversation usage |
| 4 | Per-thread Desktop Agent settings |
| 5 | Request-level usage/accounting records |
| 6 | Narrow repair of missing assistant request bindings using existing local receipt verification |
| 7 | Last user submission timestamp for stable sidebar ordering |

New databases migrate from empty state to the current version. Each recognized
migration runs transactionally. Schema 6 repairs only completed, unambiguous
single-request turns with recorded local response verification; billing or
attestation metadata alone cannot retroactively verify an answer.

Unknown, malformed, newer or unsupported pre-release databases fail closed.
There is no automatic migration from the old installation-wide pre-release
store. `reset-local-state --confirm` handles only the legacy reset path; it is
not a general account-data repair command. Preserve/export account data before
investigation. See [configuration paths](configuration.md#paths-and-local-state)
and [recovery](troubleshooting.md#session-recovery).

On restart, incomplete turns become interrupted and tools are never replayed.
Downgrading to a binary that does not recognize a newer schema is unsupported;
installers preserving data does not make a downgrade compatible.

## ACP and provider contracts

Standard ACP uses protocol v1 via `agent-client-protocol` 2.0.0 and the pinned
schema dependency. The negotiated Axiom extension is 0.2 with separately
versioned features. See [the ACP contract](acp-compatibility.md) for exact
versions, methods, bounds and generated files. Changes to advertised behavior,
session identity, cancellation or wire mapping require corresponding contract
and transcript validation. Unstable form elicitation is opt-in.

Provider protocol IDs, encryption versions and attestation profiles select
explicitly compiled drivers. Unsupported offers fail closed; replacing a
provider protocol requires coordinated native/backend changes. The registered
contracts are listed in [the threat model](threat-model.md#provider-contracts).

## Packages and acceptance

Desktop ships a matching-version AxiomCLI binary for its OS and architecture;
the standalone Linux archive also contains the local proxy. Checksums, signing
and build metadata describe the produced artifact. Supported build targets and
real-machine acceptance are separate claims; see
[supported platforms](releasing.md#supported-platforms). No document or green
build alone establishes that an artifact has been published or accepted.

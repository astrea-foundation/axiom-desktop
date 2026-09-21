# Versioning and compatibility

Tagged releases use Semantic Versioning. Before 1.0, a minor release may change
CLI, library or configuration contracts; patch releases preserve compatibility
except for explicitly documented security fixes. Public package tags must be
stable `vMAJOR.MINOR.PATCH`, match AxiomCLI's manifest and point to a promoted
`main` revision. [Releasing](releasing.md) owns the packaging procedure.

## Local database

The current SQLite schema is **2**, application ID **`0x41584133`**,
with the schema 1 baseline in [schema.sql](../apps/axiomcli/src/session/schema.sql)
and the forward migration in [the initializer](../apps/axiomcli/src/session/schema.rs).
Each authenticated account
and frontend has a separate database. It contains threads, turns, timeline items,
collections, preferences, both activity clocks, Agent settings, request usage,
and full local prompt attachments in a separate table.
Runtime event envelopes are translated into materialized records.
The explicit timeline sequence index was removed because the unique constraint
already supplies the same index. Existing schema 1 records remain compatible.

Pre-baseline application IDs and unsupported versions fail closed. Historical
receipt repairs and activity backfills have been removed; local response
verification remains required before content can be marked verified. Schema 1 upgrades to schema 2 in a transaction without resetting history, adding
a payload table with cascading deletion from its user timeline row. Future schema
changes must continue adding transactional forward migrations.

For the authorized development reset, stop all processes using the account and run
`axiomcli reset-local-state --account ACCOUNT_ID --frontend desktop-chat --confirm`
(or `--frontend cli`). It deletes only the selected database and SQLite sidecars.
See [ADR 0014](adr/0014-clean-storage-and-deposit-billing.md) for the coordinated
native/backend reset and required release gates.

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

Desktop and AxiomCLI use product version **0.1.2** for the next release. Desktop
ships a matching-version AxiomCLI binary for its OS and architecture and enforces
that exact match during ACP initialization, before account/chat bootstrap. ACP's
extension version remains independent of the product version.

The standalone Linux archive also contains the local proxy. Checksums, signing
and build metadata describe the produced artifact. Supported build targets and
real-machine acceptance are separate claims; see
[supported platforms](releasing.md#supported-platforms). No document or green
build alone establishes that an artifact has been published or accepted.

# Versioning and compatibility

Tagged releases use Semantic Versioning. Before 1.0, a minor release may change
CLI, library or configuration contracts; patch releases preserve compatibility
except for explicitly documented security fixes. Public package tags must be
stable `vMAJOR.MINOR.PATCH`, match AxiomCLI's manifest and point to a promoted
`main` revision. [Releasing](releasing.md) owns the packaging procedure.

## Local database

The current SQLCipher-backed SQLite schema is **7**, application ID **`0x41584133`**,
with the schema 1 baseline in [schema.sql](../apps/axiomcli/src/session/schema.sql)
and the forward migration in [the initializer](../apps/axiomcli/src/session/schema.rs).
Each authenticated account
and frontend has a separate database. It contains threads, turns, timeline items,
collections, preferences, both activity clocks, Agent settings, request usage,
and asset references in local prompt attachments. Managed image/file bytes are
authenticated encrypted external files, deduplicated within the account store.
Runtime event envelopes are translated into materialized records.
Unknown application IDs and unsupported versions fail closed. Local response
verification remains required before content can be marked verified. Schema 1 upgrades to schema 2 in a transaction without resetting history, adding
a payload table with cascading deletion from its user timeline row. Schema 2
upgrades transactionally to schema 3 by adding a nullable title-generation
reservation to threads. Background title results can replace only their own
reserved fallback; a manual rename clears the reservation. Existing titles and
conversation data are preserved. Schema 3 upgrades transactionally to schema 4
by adding account-scoped Desktop MCP configuration and a thread-tool selection
table with cascading thread deletion. Environment secrets stay in the OS
credential store. Future schema
changes must continue adding transactional forward migrations.

The [local-state reset command](configuration.md#paths-and-local-state) deletes
only the selected account/frontend database, external assets and migration
sidecars. Stop processes using that store
before resetting it. Hosted account and payment data are managed separately.

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

Desktop and AxiomCLI use the same product version, declared in their package
manifests. Desktop
ships a matching-version AxiomCLI binary for its OS and architecture and enforces
that exact match during ACP initialization, before account/chat bootstrap. ACP's
extension version remains independent of the product version.

The standalone Linux archive also contains the local proxy. Checksums, signing
and build metadata describe the produced artifact. Supported build targets and
real-machine acceptance are separate claims; see
[supported platforms](releasing.md#supported-platforms). No document or green
build alone establishes that an artifact has been published or accepted.

Automatic updates accept stable product versions and remember the highest signed
release sequence/version per installation. A release version and its signed
inventory are immutable. Desktop, bundled CLI and the standalone CLI/proxy pair
are installed together. Storage compatibility must be checked before any rollback.

Local account conversation storage schema 6 adds durable execution owners and
version-1 authoritative model history. Schema upgrades run in one immediate
transaction. Existing schema-4 conversations remain readable through the legacy
projection until their next atomic write adopts canonical history. Newer or
invalid canonical versions are rejected explicitly; downgrade tools must not
reconstruct inference history from display rows.

Schema 7 adds binary asset metadata/owners, native draft/outbox payloads and a
retryable payload-purge marker. Known media slots in older attachments, generated
timeline content, authoritative events and checkpoints migrate to external
ciphertext references without changing runtime message hashes or IDs. The
independent file-format migration exports plaintext SQLite into SQLCipher and
verifies content before replacement. Both migrations preserve existing data on
failure and require exclusive maintenance when rewriting older stores. See
[encrypted local storage](local-storage.md) for the key lifecycle, migration and
portable recovery format. An older unencrypted binary cannot open the new store.

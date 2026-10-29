# ADR 0015: Focused native persistence modules

Status: implemented on the development branch.

The account-scoped `SessionStore` previously contained its schema, connection
routing, record types, event translation, reads, collections and recovery in one
module. Those responsibilities now live in focused child modules behind the
existing public API. Connection ownership and generation-bound account handles
remain central; child modules receive the existing connection/transaction rather
than creating independent transactions during one event commit. Helpers remain
private to the storage module.

The single native baseline is a SQL asset alongside the schema initializer.
Fresh stores omit the explicit timeline sequence index duplicated by its unique
constraint. SQLite schema version 1/application ID `0x41584133`, logical records,
wire contracts, account isolation and response-verification rules remain intact.
Existing schema 1 databases remain compatible and may retain that redundant
index until reset. Existing integration tests validate behavior across the
module boundary; a new regression checks timeline uniqueness and indexed reads.

The [platform persistence plan](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/architecture/persistence-refactor-plan.md)
tracks the coordinated constraint, redundancy, storage and SQL organization work.
This decision extends [ADR 0014](0014-clean-storage-and-deposit-billing.md) without
performing a production reset or changing the release workflow.

The later attachment change extends this baseline with a transactional schema 2
migration; see [versioning](../versioning.md). It keeps these module boundaries.

# ADR 0015: Focused native persistence modules

Status: accepted for implementation on the development branch.

The account-scoped `SessionStore` currently contains its schema, connection
routing, record types, event translation, reads, collections and recovery in one
module. Split those responsibilities behind the existing public API. Connection
ownership and generation-bound account handles remain central; child modules
receive the existing connection/transaction rather than creating independent
transactions during one event commit. Keep helpers private to the storage module.

Put the single native baseline in a SQL asset alongside the schema initializer.
Remove only the explicit timeline sequence index duplicated by its unique
constraint. SQLite schema version 1/application ID `0x41584133`, logical records,
wire contracts, account isolation and response-verification rules remain intact.
Use existing integration tests to validate behavior across the module boundary.

The [platform persistence plan](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/architecture/persistence-refactor-plan.md)
tracks the coordinated constraint, redundancy, storage and SQL organization work.
This decision extends [ADR 0014](0014-clean-storage-and-deposit-billing.md) without
performing a production reset or changing the release workflow.

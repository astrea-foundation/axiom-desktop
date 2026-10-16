# ADR 0005: Materialized SQLite thread state and non-replay recovery

> The materialized-state and no-replay decisions remain accepted. The schema-1
> description below is historical: [current versioning](../versioning.md) documents
> the fresh baseline and rejection of incompatible databases.

Status: accepted for v1.

AxiomCLI stores user-domain records in SQLite: threads, turns, ordered timeline
items, collections, assignments, and profile preferences. Runtime command/event
envelopes are translated transactionally and are not the durable schema.

Turn admission writes the turn, user item, and assistant placeholder atomically.
Terminal persistence writes the assembled assistant/reasoning state and turn
status atomically. On startup, any nonterminal turn and timeline item becomes
interrupted; recovery never replays a tool, process, network request, or other
side effect.

TUI resume and ACP `session/load` read the same authoritative materialized
snapshot and require the canonical workspace identity to match. Every thread
has a monotonically increasing revision and every timeline item has a unique
thread-local sequence, so ACP clients can detect gaps and replace local state.

The first supported database has SQLite `user_version=1` and a fixed application
identifier. Empty databases are initialized; pre-release or unknown schemas are
rejected with an explicit reset instruction rather than migrated implicitly.
The database file is owner-only where Unix permissions are available. Local OS
account and filesystem security remain part of the trust boundary.

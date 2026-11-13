# ADR 0001: Shared command/event kernel

> The shared runtime decision remains accepted. The early event-persistence
> description below predates materialized SQLite storage; see
> [ADR 0005](0005-storage-and-recovery.md) and [versioning](../versioning.md).

Status: accepted for the early implementation

## Decision

The TUI, ACP adapter, and deterministic test harness submit typed commands to
one session runtime and consume typed semantic events. No adapter owns agent
state or embeds agent-loop decisions.

Bounded Tokio channels carry live events. Persisted events use explicit schema
versions. UI formatting and ACP schema types do not enter the core domain.

## Consequences

- ACP can be proven before the full TUI exists.
- TUI and ACP behavior can be compared using the same event fixtures.
- Adapter-specific conveniences require translation instead of changing core
  semantics.
- Early event types may evolve until persistence is introduced and versioned.

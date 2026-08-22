# ADR 0005: SQLite event journal and non-replay recovery

Status: accepted for the early implementation.

AxiomCLI stores redacted semantic event envelopes in a versioned SQLite schema.
Session metadata and append-only events are written transactionally. On startup
or inspection, an unmatched started turn is reported as interrupted; no tool,
process, network request, or other side effect is automatically replayed.

SQLite keeps the implementation local and inspectable while schemas are still
changing. Versioned, forward-only migrations are transactional. The append-only
journal is authoritative; bounded recovery snapshots are rebuildable indexes,
so a snapshot failure never creates an ambiguous journal append result.

Resume restores the kernel and a bounded reconstruction of provider-visible
conversation history. It marks unmatched turns, tools, and background tasks as
interrupted/unknown, exposes recorded workspace changes, and never invokes an
executor during restoration. TUI `--resume` and ACP `session/load` use the same
internal session UUID and require the canonical workspace identity to match.

Retention is explicit. It can remove old events while preserving the creation
record and newest bounded tail, marking the session as history-truncated; it can
also permanently remove archived sessions after a requested age. JSON export
can omit prompts and/or tool output, is event-bounded, and passes through the
same structured redaction layer as stored records.

The storage API remains deliberately small so encryption-at-rest or a different
backend can replace it later. The database file is owner-only on Unix. The early
schema does not claim encryption at rest, so local OS account and filesystem
security remain part of the trust boundary.

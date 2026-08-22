# ADR 0005: SQLite event journal and non-replay recovery

Status: accepted for the early implementation.

AxiomCLI stores redacted semantic event envelopes in a versioned SQLite schema.
Session metadata and append-only events are written transactionally. On startup
or inspection, an unmatched started turn is reported as interrupted; no tool,
process, network request, or other side effect is automatically replayed.

SQLite keeps the implementation local and inspectable while schemas are still
changing. The storage API is deliberately small so encryption-at-rest or a
different backend can replace it later. Full inference-history restoration and
ACP `session/load` are deferred and therefore not advertised.

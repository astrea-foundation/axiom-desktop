# Stage 10 Review — Durable sessions and recovery

Decision: **PASS (journal milestone)**

- A versioned SQLite migration creates transactional session metadata and event
  journals under the platform data directory.
- Structured fields and recognizable credentials are redacted before storage.
  Adversarial storage tests verify token prefixes are absent.
- CLI list/show/rename/archive/unarchive operations are implemented.
- Recovery identifies unmatched started turns as interrupted and never replays
  unknown side effects. Corrupt JSON produces a bounded diagnostic.

Full conversational resume and ACP `session/load` remain unadvertised; this is
documented rather than represented as a completed compatibility promise.

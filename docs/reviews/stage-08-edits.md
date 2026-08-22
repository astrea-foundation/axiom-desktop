# Stage 08 Review — Transactional edits

Decision: **PASS (initial edit primitive)**

- `replace_text` requires exactly one match, supports guarded create/update,
  rejects stale hashes, writes a sibling temporary file, preserves permissions,
  and atomically renames on the supported filesystem.
- Tests prove old-or-new content, stale-write rejection, Unicode-safe hashes,
  symlink/root confinement, and generated diff agreement.
- Changed paths and bounded diffs flow through shared semantic events.

Multi-file all-or-nothing transactions and delete intents are not exposed yet;
the early tool surface does not pretend to provide them.

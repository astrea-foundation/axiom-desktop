# Stage 10 Review — Durable sessions and recovery

Decision: **PASS**

- Schema v2 has forward-only transactional v0→v1→v2 migrations, rejects future
  schemas, and retains the old version when a migration fixture fails. Fixtures
  exercise every schema version that has existed.
- The append-only redacted event journal is authoritative. Bounded, redacted
  recovery snapshots are created at terminal/checkpoint boundaries and retained
  as rebuildable indexes; snapshot failure cannot make a committed append look
  uncommitted.
- Prefix restart tests kill/reopen at every semantic event boundary. Restored
  kernel state is always ready, while unmatched turns, started tools, and live
  background tasks are explicitly `unknown_after_restart` and never executed.
- Agent context reconstruction restores user/assistant/tool history under a
  context budget. Restored tool content keeps its untrusted-data wrapper, and an
  interrupted-operation warning becomes provider-visible context.
- TUI `--resume` restores the transcript and displays interrupted operations,
  prior workspace change paths, corruption warnings, and retention truncation.
  A PTY restart test proves the flow and normal terminal cleanup.
- ACP now advertises and implements `session/load` when durable storage is
  present. A two-process transcript test proves the same ACP/internal UUID is
  loaded, prior transcript updates are replayed, and a new prompt continues.
  Canonical cwd mismatch and corrupted history fail closed.
- Session list/show/rename/archive/unarchive remain available. Explicit export
  controls omit prompts/tool output and bound event count; retention preserves
  the creation record plus newest tail and marks truncation, while archived-age
  deletion is opt-in.
- Structured fields and recognizable credentials are redacted before journal,
  snapshot, and export serialization. A real SQLite/WAL checkpoint scan proves
  an adversarial credential is absent. Unix database permissions are owner-only.
- Corrupt records produce bounded diagnostics, preserve the valid prefix, and
  are never deleted as a recovery side effect.

Validation evidence:

```text
cargo clippy -p axiomcli --all-targets -- -D warnings    PASS
cargo test -p axiomcli                                  PASS
cargo test -p axiomcli --test acp_transcript             PASS (4/4)
cargo test -p axiomcli --test tui_pty                    PASS (4/4)
cargo deny check                                        PASS
```

## Final revalidation

Reopened and passed against implementation `f9fcedf` on 2026-08-22 by Codex.
The criterion mapping, exact commands/test totals, architecture/security/front-
end/dependency/performance findings, deferrals, and final rationale are
incorporated from the [final Stage 00–15 revalidation](final-revalidation-2026-08-22.md).
Final decision: **PASS**.

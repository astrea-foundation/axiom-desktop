# Stage 08 Review — Transactional edits

Decision: **PASS**

- `apply_patch` is AxiomCLI's own strict JSON create/update/delete format.
  Unknown fields, duplicate targets, out-of-root paths, invalid modes, oversized
  transactions, missing versions, and stale SHA-256 identities fail before the
  first target changes.
- Every create/update is staged as a sibling file. Existing targets are moved to
  unique sibling backups immediately before installation/deletion. If any later
  commit step fails, completed steps roll back in reverse order; unrecoverable
  cleanup errors identify the exact retained backup for manual recovery.
- The Linux supported path revalidates opened targets after policy-time path
  resolution. Symlinks cannot redirect edits outside the workspace.
- Updates preserve existing permissions and, by default, an existing consistent
  CRLF convention. Creates accept an explicit portable numeric mode on Unix.
- Golden fixture tests cover Unicode, LF/CRLF, modes, create/update/delete,
  multi-file success, stale prevalidation, duplicate targets, partial-commit
  rollback, unknown fields, symlink escape, and diff truncation.
- The legacy `replace_text` primitive now delegates its commit to the same
  transaction engine rather than retaining a weaker write path.
- Bounded diff previews and changed paths become shared semantic events. TUI
  renders them inline/expanded/in a viewport, ACP attaches them to the same tool
  call, and the agent keeps a per-session sorted change set without committing
  to Git.
- The deterministic coding evaluation uses `apply_patch`, observes the diff
  event and change set, runs a real test command, and verifies that structured
  and libgit2 diffs agree on removed/added content.

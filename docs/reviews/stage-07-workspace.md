# Stage 07 Review — Read-only workspace intelligence

Decision: **PASS**

- Real list, ignored-aware text search, glob, bounded ranged read, metadata,
  relevant-range context selection, and Git branch/HEAD/status/diff/log tools
  are available. Git inspection uses libgit2 and does not spawn a process.
- Results are stable, bounded, workspace-relative, and include SHA-256 version
  identities for source ranges. Context selection scores matching ranges and
  transmits only the retained byte budget, rather than whole repositories.
- Binary, non-UTF-8, large, likely-generated, symlink, and special files receive
  explicit classifications or fail safely. UTF-8 truncation is boundary-safe.
- Canonical root checks reject parent traversal, absolute escape, and symlink
  escape. Linux file reads use `O_NOFOLLOW` plus post-open `/proc/self/fd`
  verification, and the deterministic symlink-swap test proves execution
  revalidates after policy-time resolution.
- Ignore files are honored by default. `include_ignored` creates a separate
  scope-expansion effect which always requires a one-time approval and cannot
  become a session grant.
- `observe` permits all normalized intelligence effects and denies edits,
  processes, network, scope expansion, and MCP side effects before their
  executors run; the fixture test confirms the workspace remains unchanged.
- Purpose-built Rust and Python fixture sources, Git modifications, huge sparse
  files, invalid encodings, a Unix socket, binary content, and ignored files are
  exercised without external services.

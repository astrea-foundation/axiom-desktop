# Stage 15 Review — Early development hardening

Decision: **PASS (reviewed 2026-08-22)**

The original release-candidate gate was rerun after the corrective completion
audit. The supported scope is the early Linux x86-64 GNU AxiomCLI release; this
decision does not claim macOS/Windows support or production TEE/E2EE.

## Daily-use and documentation evidence

- The keyboard reference, native terminal text selection, transcript search/folding,
  entry and diff viewers, sticky task/security state, approval language, and
  error recovery are exercised by unit snapshots and real PTY tests.
- The TUI fills only the terminal's current viewport, responds to resize, has an
  actionable 42x10 fallback, restores the terminal after quit/cancellation/panic,
  and supports no-color, reduced-motion, and ASCII modes.
- README, command help, configuration reference, ACP matrix, privacy behavior,
  troubleshooting, versioning/storage policy, and known limitations match the
  tested behavior.

## Release evidence

- `scripts/package-release` creates a locked optimized GNU tar/gzip artifact
  with fixed timestamps, stable ordering, SHA-256, build metadata, project
  license, generated third-party license inventory, and dependency-shipped
  license/copying/notice files with a deterministic manifest. Fixed-input
  archives compare byte-for-byte.
- Detached GPG signing and verification were exercised with an ephemeral release
  test key. Tagged-release automation accepts only the separately provisioned
  maintainer key; publishing an officially signed release remains an operational
  maintainer action, not a source-tree claim.
- Credential-shaped values, unsafe archive paths, multiple roots, missing
  notices, checksum/signature failures, and a non-starting binary fail package
  verification.
- An actual optimized artifact installed into an isolated prefix, started both
  normally and inside a fresh Bubblewrap mount/network namespace, and uninstalled
  only its manifest-owned program files while preserving user state.

## Complete-system smoke evidence

- The optimized binary completed a disposable Rust coding task: it inspected the
  repository, corrected the defect, ran the real test suite, and reported the
  passing result.
- The installed ACP server initialized, created a session, completed a prompt
  through a real local OpenAI-compatible HTTP stream, stopped, reloaded the same
  durable session after restart, replayed history, and completed another prompt.
- The installed TUI resumed that session in a PTY, rendered help, handled input,
  and restored the alternate screen on exit.
- This smoke exposed a real persistence defect: secret redaction confused usage
  fields such as `input_tokens` with credential tokens. Exact/suffix secret-key
  classification fixed it, and a typed-usage resume regression test now covers
  the boundary.

## Independent clean-checkout gate

Commit `f9fcedf3125c83e1b2082da7cee422ad94142f10` was cloned into a new
directory and passed:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
scripts/axiomcli-eval deterministic
AXIOMCLI_SEARXNG_STATE_DIR=... scripts/searxng-dev integration-test
SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD) scripts/package-release
scripts/verify-release ARCHIVE
scripts/install-release ARCHIVE PREFIX
scripts/uninstall-release PREFIX
```

The SearXNG gate started the real pinned service and exercised lifecycle,
failure, and citation behavior; it did not use a search mock. The clean release
binary also started inside a fresh Bubblewrap namespace. Two fixed-input
optimized archives were byte-identical; the 7,666,205-byte archive contained
745 dependency license/notice files and an 809-line manifest.

The release-candidate threat-model audit has no open blocker or high-severity
item inside the stated platform and development-inference scope. Dependencies
are locked, `unsafe` Rust is forbidden, Cargo Deny passes (with reviewed
duplicate-version warnings), and neither repository nor artifact contains an
Axiom credential.

## Final revalidation

Reopened and passed against implementation `f9fcedf` on 2026-08-22 by Codex.
The criterion mapping, exact commands/test totals, architecture/security/front-
end/dependency/performance findings, deferrals, and final rationale are
incorporated from the [final Stage 00–15 revalidation](final-revalidation-2026-08-22.md).
Final decision: **PASS**.

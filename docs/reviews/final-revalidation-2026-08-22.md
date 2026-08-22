# Final Stage 00–15 Revalidation — 2026-08-22

Decision: **PASS for the stated early Linux x86-64 GNU scope**

Reviewed implementation: `f9fcedf3125c83e1b2082da7cee422ad94142f10`
(the complete range through Stage 15). The commit containing this record changes
only review documentation and CI package prerequisites; no reviewed Rust,
packaging, or runtime behavior follows `f9fcedf`.

Reviewer: Codex, acceptance-criteria-first revalidation from the written plan

This record is incorporated into every stage review. It reopens all earlier
gates after the final implementation audit and supersedes evidence tied only to
older release-candidate commits. The supported binary scope is Linux x86-64 GNU.
macOS and Windows have source-portability CI lanes but are not advertised as
supported releases. Production attestation/E2EE, the desktop application,
wallets, remote ACP, hosted SearXNG operations, and subagents remain explicitly
outside this plan.

## Exact final verification

The source tree was clean at `f9fcedf`, cloned with `git clone --no-local`, and
checked out detached at the full hash above. The following ran in that isolated
checkout:

```text
cargo fmt --all -- --check                                  PASS
cargo check --workspace --all-targets --all-features        PASS
cargo clippy --workspace --all-targets --all-features \
  -- -D warnings                                             PASS
cargo test --workspace --all-features                       PASS
cargo deny check                                             PASS
scripts/axiomcli-eval deterministic                          PASS, twice by script
AXIOMCLI_SEARXNG_STATE_DIR=<fresh temp> \
  scripts/searxng-dev integration-test                       PASS, real SearXNG
SOURCE_DATE_EPOCH=<commit time> scripts/package-release      PASS, optimized
scripts/verify-release <archive>                             PASS
scripts/install-release <archive> <isolated prefix>          PASS
Bubblewrap isolated installed-binary start                   PASS
installed coding/ACP/TUI/resume/uninstall smoke              PASS
```

The ordinary suite passed 126 tests with three deliberate opt-in tests ignored:
two live-model tests and the real-SearXNG test. The ignored SearXNG test then
passed separately against a freshly cloned pinned SearXNG revision and real
upstream engines. The deterministic lane passed 30 tests per repetition, 60 in
total. The full eight-test PTY suite also passed ten consecutive times before
the implementation commit was frozen, then passed in the clean checkout.

The last recorded live-model baseline remains the three-run, read-only
`glm-5-2` baseline in `docs/evaluations.md` (all correct, zero mutations, 1–2
tools, 5,970–8,252 ms). A credential was intentionally not copied into this
checkout. Scheduled and tagged-release workflows now fail when their live
endpoint/model/key secrets are absent, so a release cannot turn missing live
evidence into a false pass.

## Acceptance criterion mapping

| Stage | Final concrete evidence |
|---:|---|
| 00 | The plan, eight ADRs, threat model, vocabulary, module boundaries, tracing/redaction rules, CI, platform scope, and baseline budgets are committed. Scope remains CLI/TUI plus ACP; `--help`/`--version` remain independent of product services. |
| 01 | `app` owns typed IDs, commands/events, state transitions, injectable clock/ID sources, terminal-turn identity, replay, cancellation, and bounded publication. Transition/property/golden/backpressure tests pass. |
| 02 | The pinned official ACP crate drives protocol-only stdio. Real subprocess transcripts cover initialize/new/prompt/update/cancel/EOF, malformed JSON, unknown methods, duplicate IDs, stdout purity, and stable `session/load`; the adapter uses the shared runtime. |
| 03 | Ratatui/Crossterm render only semantic events. Snapshot/unit matrices cover sizes, theme/color/ASCII, all categories, scrolling/search/selection/folding/viewers, and control stripping. Real PTYs cover resize, quit, panic, cancellation, questions, plan review, resume, edits, and restoration. |
| 04 | Provider-neutral types and the Axiom development adapter implement bounded model catalogs, bounded SSE parsing, cancellation, retry classification, stable idempotency, usage, and honest `UNATTESTED DEV` status. Real local HTTP tests cover valid streams and failure modes; the opt-in live baseline is documented and release-gated. |
| 05 | The shared agent loop validates typed tools, limits steps/context/output/time, detects repetition, returns repairable malformed/failed calls, emits structured progress/checklists, and uses bounded event queues. Scripted zero/one/many/failure/cancellation cases and TUI/ACP semantics pass. |
| 06 | Deny-first policy covers every profile/effect/rule/scope combination, scoped grants, timeout/disconnect, repository mutations, credential access, and approval explanations. Project rules can only append explicit `ask`/`deny` restrictions; no prompt is treated as authority. |
| 07 | Canonical workspace list/glob/search/read/metadata/context and libgit2 inspection are bounded. Traversal, alternate roots, symlinks/TOCTOU, special/binary/large/invalid files, ignores, and observe-mode side effects fail safely. |
| 08 | Original structured patches and exact replacement implement hashes, prevalidation, rollback, create/update/delete, newline/mode preservation, bounded diffs, and changed paths. `view_session_diff` compares bounded session-scoped text snapshots, including process-created files; TUI/ACP map the same changes. |
| 09 | Structured commands precede separately classified shell execution. Cwd/environment allowlists, Linux Bubblewrap isolation, process groups, time/output limits, foreground/background lifecycle, cancellation, and credential non-inheritance pass adversarial and repair-loop tests. Git mutation is separately one-shot confirmation-gated. |
| 10 | SQLite journals, snapshots, transactional v0/v1 migrations, bounded corruption recovery, retention/export/redaction, unknown-after-restart behavior, headless journaling, TUI resume, and stable ACP identity/load all pass. Unknown side effects are never replayed. |
| 11 | `scripts/searxng-dev` freshly cloned and ran pinned real SearXNG. Real engine search, empty/disabled/timeout/shutdown paths, citations, local fetch, URL normalization, response bounds, DNS rebinding, alternate IPs, metadata targets, and redirect pivots pass. No normal search path is mocked. |
| 12 | Real stdio MCP fixtures cover discovery, stable namespaces/provenance, misleading annotations, schema drift, collision, malformed/long results, cancellation, crash/reconnect, and read-only-only retry. Configured executables are explicit trusted-user programs; protocol data remains untrusted and policy-controlled. |
| 13 | Durable plan state, revisions, line/range comments, approval/abandonment, deterministic plan read-only policy, up-to-three structured questions, stable ACP plan updates, TUI answer composer, attention ordering, and bounded parent-owned background work pass. Subagents remain intentionally absent under the plan’s optional clause. |
| 14 | Original Rust/TypeScript/Python/config/docs/investigation fixtures, repair loops, denied escape, MCP, resume/cancel, real web, TUI/ACP equivalence, score budgets, and soak tests pass twice from clean fixtures. Live variance is documented and mandatory for tag packaging. |
| 15 | Documentation/help match behavior. Reproducible optimized archives compare byte-for-byte; SHA-256, ephemeral GPG signing tests, path/credential checks, project license, Cargo Deny inventory, and 745 dependency license/notice files pass. The installed optimized binary starts normally and in Bubblewrap, completes a multi-step coding task and real `cargo test`, serves/restarts/loads/continues ACP, resumes in a real PTY, then uninstalls while preserving journals. |

## Architecture and dependency-direction findings

- `app` remains the source-of-truth state machine. TUI, ACP, and headless modes
  translate or present shared commands/events; headless execution no longer
  bypasses the durable runtime.
- Provider, search, MCP, workspace, process, policy, storage, and plan boundaries
  are explicit. Model output only proposes typed operations. Authorization and
  path/process/network validation remain outside the model.
- Event delivery and provider/search inputs are bounded before parsing. No
  unbounded channel remains in source or tests.
- Platform-specific `nix` usage is Unix-targeted. CI performs native all-target
  source checks on macOS and Windows, while release packaging correctly refuses
  non-Linux/non-GNU hosts.
- The final audit added no production dependency and removed
  `eventsource-stream` in favor of the bounded in-house SSE boundary parser.
  Cargo Deny reports accepted duplicate-version warnings but passes advisories,
  bans, licenses, and sources.

## Security, privacy, and resource findings

- Credential-shaped values are absent from the source diff and optimized
  archive. Error bodies, provider/search responses, SSE events/streams, tool
  output, session exports, context, and task counts all have explicit caps.
- Credential/keychain access is hard-denied. Git mutations, shell execution,
  deletion, scope expansion, and other high-risk effects cannot inherit a broad
  autonomous grant. Project configuration cannot broaden authority or launch MCP.
- Filesystem canonicalization/revalidation, transactional writes, Linux process
  sandboxing, per-hop SSRF checks, fail-closed approvals, secret redaction, and
  no-replay recovery each have adversarial regression coverage.
- Idle TUI measurement was 26,808 KiB RSS and zero scheduler ticks over one
  second. Ten CLI startups took 43 ms total. Twenty-four background tasks took
  95 ms. The long-session soak handled 10,000 stream chunks, 5,000 transcript
  entries, and a 2,781,184-byte journal in 2,124 ms within recorded gates.

## Front-end and contract findings

- TUI snapshots changed for task/checklist state and plan review; real PTY
  coverage proves the terminal viewport is used without resizing the terminal.
- ACP transcripts changed to use stable standard `plan` updates, form
  elicitation for review, durable line comments, bounded queues, and stable
  session load. Stdout remains JSON-RPC only.
- A final installed-binary smoke used the same OpenAI-compatible HTTP boundary
  as development inference: the agent read a defective Rust file, replaced the
  exact expression, ran `cargo test` inside policy/sandbox controls, and reported
  success. A separate installed ACP process repeated the task, restarted,
  replayed the journal under the same session ID, accepted another prompt, and
  that session resumed in the installed TUI.

## Clean-room, licenses, and notices

The reference repositories listed in the plan were used only for behavioral
questions: Axiom/AxiomIO and Grok Build for identity/provider/UI constraints;
Codex, OpenCode, Goose, Gemini CLI, Aider, and mini-SWE-agent for failure modes
and architectural comparisons; and the official ACP source as a protocol
authority. No reference source, prompt, benchmark fixture, branding, or UI code
was copied or mechanically ported. AxiomCLI’s implementations and fixtures are
original and contract-driven.

The 7,666,205-byte optimized archive includes the project Apache-2.0 license,
the Cargo Deny locked SPDX inventory, and a deterministic manifest plus every
root `LICENSE*`, `COPYING*`, and `NOTICE*` file shipped by resolved dependencies.
The inspected archive contained 745 such files and an 809-line manifest.
Dependencies publishing metadata without a file are explicitly marked rather
than silently omitted.

## Unresolved items and final decision

No blocker or high-severity issue remains inside the advertised early Linux
development-inference scope. The following are safely deferred because the
product and documentation do not claim them:

- production TEE attestation/E2EE and encrypted local journals
- official release signing with the maintainer key and the mandatory tag-time
  live evaluation (the mechanics and ephemeral signature path pass)
- supported macOS/Windows packages and equivalent OS sandbox/PTY gates
- hosted SearXNG operations/abuse controls, remote ACP, desktop UI, wallets,
  blockchain functions, and capability-bearing subagents

Stages 00–15 therefore receive a final **PASS**. This is an early, deliberately
changeable AxiomCLI release candidate, not a claim that deferred production
security or platform programs are complete.

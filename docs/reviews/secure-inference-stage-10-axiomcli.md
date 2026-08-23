# Secure inference Stage 10 review — AxiomCLI direct integration

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `6bb193c` (`feat: use secure inference in
axiomcli`)

## Delivered and verified

- The default provider is `SecureAxiomProvider`, which directly discovers
  models, establishes a locally attested session, and streams E2EE inference in
  process. The default relay is `https://api.axiom.stream`.
- AxiomCLI no longer requires or traverses a localhost inference proxy.
- Model, thinking, compaction, title, partial-response recovery, usage,
  cancellation, tool loops, permissions, persistence, resume, TUI, headless,
  and ACP paths continue through the unchanged agent/runtime boundaries.
- Typed security states drive `NOT VERIFIED`, `VERIFYING`, `SECURE`,
  `SECURITY FAILED`, and the explicit development-only `UNATTESTED DEV` badge.
- The old OpenAI development adapter requires both the non-default
  `development-provider` build feature and `AXIOMCLI_UNATTESTED_DEV=1`.
- Secure-client errors remain classified and redacted; credit exhaustion maps
  to the existing actionable top-up UI.

The full format/check/Clippy/workspace-test/Cargo-Deny gate passed. The AxiomCLI
library gate passed 165 tests with two intentional live/visual tests ignored.
Existing ACP, agent, MCP, persistence, PTY, policy, release, and workspace
integration suites remained green.

## Findings

- High, corrected: legacy UI and doctor paths hard-coded an unattested claim
  independent of the selected provider. Security status is now typed and
  emitted by the actual provider lifecycle.
- High, corrected: the old development adapter was production-default. It is
  now excluded from the default composition and requires two explicit opt-ins.
- Medium, corrected: thinking-level UI still claimed transport was pending.
  The secure path now sends and capability-validates the explicit level.
- Blocker/high unresolved: none in deterministic coverage.
- Informational: protected live direct-agent loops remain Lane F release gates;
  deterministic provider tests do not make a live-service claim.

## Exit criteria

AxiomCLI uses the shared secure client directly while local tool policy remains
the sole source of execution authority. Stage 11 may proceed.


# Native maintenance implementation plan

> September 21, 2026: Phala is removed. Completed Phala work below is historical;
> the remaining open maintenance tasks are unchanged. See [provider catalogs](provider-catalogs.md).

Status: complete. Initial baseline: `0eec7cd` on `refactor/clean-baseline`.
Final branch: `refactor/native-maintenance`, targeting `dev`.

This work completes the cleanup identified by the source audit. It began on the
native cleanup branch; PR #3 was merged into `dev` during implementation, including
this plan’s initial commit. The remaining changes continue in a follow-up PR to
`dev`. The earlier invoice removal and fresh-storage baseline remain intact.
This work does not promote `main` or deploy a service.

## Intended changes

1. **Remove unused product scaffolding.** Delete the unimplemented TUI Crypto
   permissions tab, its commands/help wording and its dedicated tests. Remove
   the dormant aggregate token counter and its event bookkeeping while retaining
   request accounting and displayed usage. Remove the three unused duplicate
   Desktop fonts, unreferenced CSS selectors/animations and the reserved empty
   sidecar-protocol directory. Update current documentation and references.
2. **Make titles a local operation.** Replace the asynchronous runner title API
   with one synchronous, bounded title helper. Persist an initial title as part
   of frontend prompt admission, keeping account binding and set-if-absent
   semantics. Remove background title tasks, cancellation/accounting plumbing
   and title-result messages. Retain compatibility values required to read
   existing accounting contracts.
3. **Use signed NVIDIA policy in both providers.** Pass the verified policy's
   accepted intermediate fingerprints into the Phala verifier. Reject absent,
   malformed or unapproved anchors; allow signed overlap during rotation.
   Preserve certificate signatures/validity, nonce binding, TDX, measured source,
   worker identity and receipt checks. No gateway repository/commit allowlist or
   manual approval of provider source updates is introduced.
4. **Test the production NEAR report verifier.** Eliminate the duplicate
   report-binding helper and its blanket dead-code suppression. Run the existing
   cross-language report vectors through the verifier used during attestation,
   with explicit rejection of malformed lengths and mismatched bindings.
5. **Separate ACP responsibilities.** Introduce a shared typed server context and
   feature-specific handlers for sessions/prompts, accounts/billing, catalog and
   preferences, collections, and security/compaction. Keep stdio setup and typed
   handler registration readable. Preserve request ordering, responder failures,
   cancellation, account transitions, persistence and extension negotiation.
6. **Separate TUI responsibilities.** Move state/reduction, rendering/screens and
   event-loop concerns into focused modules. Remove obsolete state instead of
   relocating it. Keep the existing public TUI API, terminal cleanup, approval,
   steering, resume and account-isolation behavior.
7. **Separate Desktop controllers.** Extract account/login, model catalog and
   conversation/submission lifecycle from `App.tsx` into cohesive hooks or
   controllers. Leave application layout/composition in `App`. Preserve stable
   queue IDs, account/runtime generations, draft ownership, settings revisions,
   selection and edit/regenerate behavior.

## Validation

- Run relevant Rust unit and integration suites after behavior changes. Add
  meaningful trust-policy rotation/rejection coverage and retain production
  report-vector coverage.
- Run workspace formatting, checks and strict Clippy; run the full workspace
  tests and the pinned ACP SDK stack regression after the controller splits.
- Run deterministic evaluations, ACP transcripts and TUI PTY tests to cover
  protocol ordering, approvals, cancellation, recovery and frontend parity.
- Regenerate/check the ACP contract; run TypeScript checks, client/Desktop tests,
  browser regressions, callback asset verification and the Desktop build.
- Check moved documentation links and confirm removed artifacts have no remaining
  active references. Record actual results and unresolved failures here.

## Completion record

- [x] Unused UI, counters, assets and placeholder directory removed.
- [x] Titles use shared synchronous generation and account-bound persistence.
- [x] Phala uses verified signed NVIDIA anchor policy with rejection/rotation tests.
- [x] NEAR report vectors exercise the production verifier.
- [x] ACP registration delegates to feature handlers using typed context.
- [x] TUI state, screens and runtime are separated.
- [x] Desktop account, model and conversation lifecycle are separated from layout.
- [x] Integration checks pass and current guides reflect the final implementation.
- [x] Commits are pushed and a follow-up `dev` pull request describes the maintenance work.

Final validation on 2026-09-21:

- `cargo fmt --all -- --check`, workspace checks and strict Clippy across all
  targets/features pass.
- `cargo test --workspace --all-features`: 510 passed, six opt-in/manual tests
  ignored. This includes title persistence/notification ordering, trust-policy
  rotation/rejection, production report vectors, account isolation, ACP, TUI PTY,
  recovery and reproducible signed release archives.
- Pinned ACP SDK stack regression: one passed.
- `scripts/axiomcli-eval deterministic`: both repetitions passed; each covers
  62 autonomous/multilanguage, soak, ACP, real-terminal and MCP tests.
- TypeScript checks and all 209 JavaScript/TypeScript tests pass; all 127 Desktop
  browser regressions pass across the platform fixtures.
- Desktop production build and compiled callback asset verification pass.
  ACP protocol regeneration produces no drift.
- All 393 authored Markdown links/anchors resolve. All 93 Desktop/client source
  modules are reachable from the application entry points. Removed artifacts
  have no active references; dated historical evidence remains labeled.

Strict-lint follow-up also boxes the large accounting event payload, removes
redundant resumed-model reconciliation, and fixes routine clone, conversion,
formatting and import warnings. Model capability booleans are intentionally
independent catalog fields and retain a narrowly documented lint expectation.

No live inference, funded payment, native OS acceptance or production deployment
was performed. Those release gates remain in [validation](validation.md).

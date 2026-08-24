# Stage 03 Review — TUI foundation driven by fixtures

Decision: **PASS**

Reviewed commit: `8ced0d7`

Date: 2026-08-22

Reviewer: Codex, acceptance-criteria-first re-review

## Acceptance evidence

| Criterion | Evidence |
|---|---|
| Terminal guard/input/resize | `TerminalGuard` owns raw mode and the alternate screen without capturing pointer input; `EventStream` handles keys/paste/resize; real PTY tests resize 60×14 → 100×30 |
| Operational-document layout | Header, semantic transcript, status, persistent composer, shortcuts, viewer and help overlay are fixture-driven and contain no provider/agent behavior |
| Navigation and follow behavior | Composer/transcript focus, j/k/arrows, stable selection, deliberate follow stop and End-to-follow are implemented and tested during streaming; native terminal text selection remains available |
| Search/fold/view | `/` live transcript/tool-output search, n/N matches, Enter fold/unfold, and `v` terminal-viewport viewer are operational tests, not placeholder text |
| Accessible theme matrix | True color, 256, 16 and no-color render at 42×10, 60×14, 100×30 and 160×50; no-color asserts every cell uses reset foreground/background |
| ASCII/reduced motion | `AXIOMCLI_ASCII`, dumb-terminal fallback and `AXIOMCLI_REDUCED_MOTION` remove decorative Unicode/animation while retaining textual meaning |
| Semantic coverage | Every Stage 01 event variant is applied in the renderer fixture; representative operational-document output is committed as an Insta snapshot |
| Restoration/security | Real PTY tests cover normal quit after resize, active cancellation and panic unwinding; all assert alternate-screen leave. ANSI/control input is sanitized before rendering |

## Verification

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
                                                                 PASS
cargo test --workspace --all-targets --locked                   PASS
cargo deny check                                                PASS
```

Totals at gate: 42 library tests passed, 1 opt-in live-network test ignored, 3
ACP subprocess tests, 2 agent evaluations and 3 real PTY tests passed.

## Review findings

- View state alone owns focus, search, scroll, selection, expansion, overlays
  and composer contents. Streamed semantic events never mutate the composer.
- Pending permission handling does not discard the user's draft.
- Minimum size is documented and rendered safely at 42×10. Larger layouts use
  the current terminal viewport only and never alter the OS window state.
- Status semantics retain glyph plus text; pink remains identity color while
  warning/error/success retain distinct text and palette roles.
- Untrusted terminal text cannot emit ESC/BEL/control sequences. The PTY output
  confirms terminal restoration on all tested exit paths.
- `portable-pty` is a dev-only MIT dependency recorded in `THIRD_PARTY.md`; its
  addition passes the dependency policy. No application source was copied from
  a reference agent.
- Resource behavior stayed within the Stage 00 baseline; the largest snapshot
  matrix completes in milliseconds and the PTY suite in under one second.

Unresolved items: inline patch-specific navigation will attach to the generic
viewer in Stage 08; session selection/resume attaches in Stage 10. Neither is a
missing Stage 03 shell behavior.

## Final revalidation

Reopened and passed against implementation `f9fcedf` on 2026-08-22 by Codex.
The criterion mapping, exact commands/test totals, architecture/security/front-
end/dependency/performance findings, deferrals, and final rationale are
incorporated from the [final Stage 00–15 revalidation](final-revalidation-2026-08-22.md).
Final decision: **PASS**.

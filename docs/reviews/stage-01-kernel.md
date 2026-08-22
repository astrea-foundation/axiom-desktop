# Stage 01 Review — Core command/event kernel

Decision: **PASS**

Reviewed commit: `a25dd27` (parent baseline `e40fdca`)

Date: 2026-08-22

Reviewer: Codex, acceptance-criteria-first re-review

## Acceptance evidence

| Criterion | Evidence |
|---|---|
| Typed IDs, commands, events, states and transitions | `app.rs`: `SessionId`, `TurnId`, `CorrelationId`, `CommandEnvelope`, `AppCommand`, `AppEvent`, `SessionState`, `Kernel` |
| Every command carries correlation, origin and cancellation | `CommandEnvelope`; `Runtime::dispatch_request`; cancelled-command test |
| Injectable clock/ID source | `RuntimeServices`, `SystemServices`, and deterministic counter/clock implementation |
| Valid/invalid transition coverage | `command_state_matrix_is_fail_closed` covers ready/running/approval/question/closed × every stateful command; focused transition tests cover duplicate create, empty/overlapping prompts and mismatched IDs |
| Terminal-state monotonicity | Property test `terminal_turn_id_never_becomes_active_again` |
| Slow/disconnected subscriber behavior | Capacity-one undrained subscriber timeout test proves dispatch does not block |
| Golden semantic replay | Fixed clock, UUIDs and command stream produce byte-equivalent event structures; restore test marks in-flight work ready without replay |

## Verification

```text
cargo fmt --all -- --check                                      PASS
cargo check --workspace --all-targets --locked                  PASS
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
                                                                 PASS
cargo test --workspace --all-targets --locked                   PASS
```

Totals at gate: 38 unit tests passed, 1 opt-in live-network test ignored, and 2
deterministic agent evaluations passed.

## Review findings

- Architecture: the kernel has no TUI, ACP, provider, storage, or tool types.
  Adapters submit command envelopes and receive structured event envelopes.
- Ordering: committed events receive monotonically increasing sequence numbers
  and deterministic timestamps under test services.
- Cancellation: a pre-cancelled command has no state or event effect. Terminal
  turn IDs cannot be completed or cancelled twice.
- Recovery: replay reconstructs state but deliberately converts incomplete work
  into ready/unknown state; it does not re-execute anything.
- Security: no model or external text can choose sequence, time, correlation, or
  transition validity. Subscriber lag is bounded by Tokio broadcast semantics.
- Clean room: the implementation uses original AxiomCLI contracts and test
  fixtures. No reference implementation identifiers or code were introduced.
- Dependencies/licenses: no new dependency was added.
- Performance: the capacity-one subscriber test completes within 100 ms; no
  regression was observed against the baseline startup/memory record.

Unresolved items: none for Stage 01. Storage migration behavior and user-facing
resume remain owned by Stage 10.

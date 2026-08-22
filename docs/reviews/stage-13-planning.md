# Stage 13 Review — Plans, questions, and controlled concurrency

Decision: **PASS (reviewed 2026-08-22)**

- `plan` is a deterministic policy profile, not a prompt convention. It permits
  workspace reads and `PlanWrite` only beneath `.axiomcli/plans`; edits,
  processes, network, MCP, deletion, and scope changes are denied.
- Agents can create, revise with optimistic revision checks, and propose plans.
  Humans can comment on line ranges, request revision, approve, or abandon via
  `axiomcli plans`. Artifacts use atomic owner-only writes, survive restarts, and
  approval never starts execution.
- One-to-three structured questions flow through shared application events. The
  TUI preserves the message draft in a separate answer composer. ACP maps to
  capability-negotiated form elicitation and fails closed for unsupported,
  declined, cancelled, incomplete, or wrong-typed answers.
- Background processes have independent UUIDs, sequence order, command/cwd and
  permission-profile provenance, bounded output, per-task timeouts, and four
  concurrent slots. Parent cancellation propagates to the child process group;
  a fifth concurrent task is rejected.
- Pending permissions, questions, plan reviews, and background work use stable
  ordered attention keys in the TUI without replacing the composer draft.
- Subagents remain intentionally absent under ADR 0007. Adding them requires a
  separate authority/cancellation/audit design review.

Acceptance evidence includes policy table tests, durable plan and CLI restart
tests, shared agent question tests, a real two-sided ACP elicitation transcript,
TUI PTY question input, cancellation/concurrency process tests, and deterministic
attention ordering.

Verification:

```text
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

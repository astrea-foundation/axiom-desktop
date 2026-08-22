# Stage 13 Review — Plans and controlled concurrency

Decision: **PASS (no-subagent scope)**

- Plans use a tested state machine with revision checks, terminal states,
  line-range comments, and atomic workspace-local persistence.
- Background processes provide bounded concurrency with task IDs, independent
  state/output, timeouts, and cancellation.
- ADR 0007 explicitly forbids subagents until identity, inherited capability,
  budget, audit attribution, concurrency, and cancellation-tree semantics are
  designed. No partial or unbounded subagent feature is hidden behind prompts.

Rich question overlays and ACP plan operations are pre-1.0 UX work; their core
semantic event types are reserved without advertising unsupported ACP features.

# Contributing to AxiomAI

This is an early codebase. Prefer the smallest change that establishes a clear
contract and leaves room to revise it.

1. Work only in the active stage from the implementation plan.
2. Add or update tests for externally observable behavior.
3. Keep terminal rendering, ACP translation, and persistence out of the agent
   domain model.
4. Treat model, network, tool, terminal, and ACP input as untrusted.
5. Do not copy implementation code, prompts, comments, names, or tests from a
   reference agent.
6. Run formatting, Clippy, tests, and the current stage's review checklist.
7. Do not start the next stage until the current review record is `PASS`.

Application-specific presentation and lifecycle code belongs under `apps/`.
Code belongs under `crates/` only when it has a narrow contract and more than
one application-level consumer. Shared crates must not read environment
variables or choose application policy on behalf of their callers.

Public APIs can change before `1.0`, but changes should still be intentional and
documented in an ADR when they affect architecture or persisted data.

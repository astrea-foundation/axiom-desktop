# Stage 02 Review — ACP Skeleton

Decision: **PASS**

## Acceptance evidence

- Uses the official Apache-2.0 `agent-client-protocol` 2.0.0 crate.
- Supports v1 initialization, absolute/canonical session cwd, text prompts,
  streamed text updates, cancellation tokens, and EOF shutdown over stdio.
- ACP stdout is protocol-only and tracing is configured for stderr.
- A real subprocess transcript initialized AxiomCLI, created a session, sent
  `contract probe`, observed `session/update`, received `end_turn`, and exited
  cleanly on EOF.
- Compatibility matrix: `docs/acp-compatibility.md`.
- Formatting, check, Clippy with warnings denied, and unit tests pass.

## Review findings

- ACP and future TUI turns share `TurnRunner` and semantic `AppEvent` types.
- Long prompt work is spawned through the ACP connection so cancellation
  notifications can continue to be processed.
- Unknown and invalid sessions fail without creating authority.
- Only implemented capabilities are advertised.
- The temporary echo runner is a deterministic contract fixture, not a product
  provider; Stage 04 replaces it in normal execution.
- Clean-room review found no copied adapter source or sample fixture text.

# Stage 14 Review — Autonomous evaluations

Decision: **PASS (reviewed 2026-08-22)**

- Fresh fixture repositories cover Rust, TypeScript, Python, configuration, and
  documentation tasks. They include investigation-only work, targeted edits,
  failed commands followed by repair, multi-file refactors, and real verification
  processes. The multi-language set repeats twice internally from clean fixtures.
- Per-task scorecards gate completion, exact workspace outcome, test/verification
  results, policy escapes, tool efficiency, expected failures, provider retries,
  latency, and context bytes.
- Adversarial policy/path, web, and MCP suites cover denial, traversal, symlink
  swaps, SSRF/rebinding, ambiguous MCP disconnects, malformed results,
  cancellation, and resume. The real SearXNG integration exercises research and
  citations rather than replacing search with a mock.
- Matching deterministic workspace-edit runners pass through an actual TUI PTY
  and actual ACP stdio client. Both reach `frontend.txt = "after\n"` and expose
  the same tool identity, completion, diff, and final-message semantics.
- Soak gates cover 10,000 streamed deltas, 5,000 durable transcript entries, 24
  isolated background processes, repeated startup, and an idle PTY. Measured
  development-host budgets are recorded in `docs/benchmarks.md`.
- A real local AxiomIO/`glm-5-2` baseline passed three read-only investigation
  runs in 5,970–8,252 ms with one or two tool calls, correct answers, and zero
  mutations. Its accepted variance and failure conditions are documented in
  `docs/evaluations.md`; the opt-in scheduled job has a 15-minute ceiling.

Verification includes `scripts/axiomcli-eval deterministic`, the opt-in live
evaluation, the real SearXNG lifecycle suite, strict Clippy, and dependency
policy checks. Live credentials are never persisted or printed.

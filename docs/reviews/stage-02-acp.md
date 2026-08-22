# Stage 02 Review — ACP skeleton and conformance harness

Decision: **PASS**

Reviewed commit: `cb74793`

Date: 2026-08-22

Reviewer: Codex, acceptance-criteria-first re-review

## Acceptance evidence

| Criterion | Evidence |
|---|---|
| Official schema/SDK pinned | Apache-2.0 `agent-client-protocol` 2.0.0 in `Cargo.lock`; ADR and compatibility matrix record the choice |
| Protocol-only stdio lifecycle | `axiomcli acp`; tracing is stderr-only; three real subprocess tests parse every stdout line as JSON |
| Initialize/new/prompt/stream/cancel/EOF | `acp_transcript.rs` drives all operations against the built binary and verifies `session/update`, `end_turn`, `cancelled`, and clean EOF exit |
| Malformed and unknown input | Raw malformed JSON followed by a valid unknown method proves the connection remains usable; unknown method returns JSON-RPC `-32601` |
| Duplicate IDs | Two in-flight `session/new` requests with ID 9 are exercised and produce bounded protocol responses without panic or cross-session state corruption |
| Stable semantics | ACP calls the same `Runtime`, `TurnRunner`, `AppCommand`, and `AppEvent` contracts as the TUI/test front ends; the adapter only translates protocol objects |
| Capability honesty | Only ACP v1 and the implemented baseline content types are accepted; unadvertised image/audio/embedded-resource blocks fail explicitly; resource links are supported as required by ACP |

## Verification

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
                                                                 PASS
cargo test --workspace --all-targets --locked                   PASS
```

Totals at gate: 38 library tests passed, 1 opt-in network test ignored, 3 ACP
subprocess tests passed, and 2 deterministic agent evaluations passed.

## Review findings

- Cancellation is accepted while the prompt request is in flight and produces
  exactly one terminal prompt response.
- Unknown sessions and overlapping turns fail without affecting other sessions.
- ACP stdout contains only JSON-RPC; error and trace output remain on stderr.
- The echo and blocking runners are deterministic debug-build fixtures. Release
  builds ignore `AXIOMCLI_TEST_RUNNER`, preventing the test path from becoming a
  provider bypass.
- The adapter owns connection/request correlations only. Core session state and
  lifecycle remain in the Stage 01 runtime.
- Resource links are represented as untrusted references in the prompt rather
  than dereferenced implicitly.
- Clean-room review found only official schema/API usage and original AxiomCLI
  adapter/test code. No new dependency was added.

Unresolved items: ACP `session/load`, durable identity, richer plans/questions,
and per-session MCP belong to Stages 10–13 and are not advertised at this gate.

## Final revalidation

Reopened and passed against implementation `f9fcedf` on 2026-08-22 by Codex.
The criterion mapping, exact commands/test totals, architecture/security/front-
end/dependency/performance findings, deferrals, and final rationale are
incorporated from the [final Stage 00–15 revalidation](final-revalidation-2026-08-22.md).
Final decision: **PASS**.

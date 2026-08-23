# Secure inference Stage 02 review — provider-neutral contracts

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `635244b` (`feat: expand shared inference
contracts`)

## Delivered

- `axiom-inference` now represents optional message reasoning/name/refusal,
  strict tool definitions, tool choice, parallel-tool preference, explicit
  reasoning effort, sampling, response format, output limits, usage, finish
  reasons, refusal/tool stream deltas, and model capabilities.
- Shared request validation enforces message/tool/count/text/schema/argument
  bounds without echoing untrusted values into errors.
- `ToolCallAccumulator` assembles independent streamed calls, rejects conflicting
  stable metadata, requires contiguous indices, and validates completed calls.
- AxiomCLI constructs the expanded requests with its explicit session thinking
  level and preserves its existing event behavior.

## Boundary review

`axiom-inference` depends only on Serde and Serde JSON. It contains no HTTP,
OpenAI, TEE, cryptographic, environment, filesystem, terminal, ACP, persistence,
or permission code. Provider IDs/protocol strings are data supplied in model
descriptors rather than compiled NEAR branches.

## Verification

```text
cargo fmt --all -- --check
cargo clippy -p axiom-inference --all-targets --all-features -- -D warnings
cargo test -p axiom-inference
cargo check --workspace --all-targets --all-features
git diff --check
```

Six inference-contract tests pass, including property-generated argument
fragment assembly, stable metadata conflict rejection, redacted validation
errors, explicit defaults, and capability-safe catalog defaults. The complete
workspace builds against the new types.

## Findings

- Blocker: none.
- High: none.
- Medium: none.
- Low: temperature/top-p capability ranges remain provider/model policy and are
  intentionally not globally restricted beyond requiring finite values.

## Exit criteria

- Contracts express secure client, proxy, and AxiomCLI needs: **met**.
- Tool fragment handling is bounded and transport-neutral: **met**.
- No provider or presentation dependencies leaked into the crate: **met**.
- Existing AxiomCLI compiles with explicit reasoning settings: **met**.

Stage 03 may begin.

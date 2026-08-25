# Secure inference Stage 11 review — OpenAI compatibility adapter

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `4c7291e` (`feat: add openai compatibility
adapter`)

## Evidence

- `axiom-openai-compat` depends only on domain types, Serde, and its error
  helper. It has no HTTP, crypto, attestation, environment, server, filesystem,
  or tool-execution capability.
- Strict request translation covers system/developer/user/assistant/tool
  messages, assistant call history, tool results, tools, all tool choices,
  parallel calls, reasoning effort, output limits, sampling, response formats,
  streaming, and usage options.
- Conflicting output limits, stream options without streaming, unknown fields,
  unknown roles/types, and legacy prompt shapes fail explicitly.
- Response translation covers non-stream text/reasoning/refusal/tools/usage and
  finish reasons, plus role/content/reasoning/refusal/tool/usage/terminal SSE
  chunks and `[DONE]`.
- Tests serialize the produced response/SSE forms and prove the supported
  request forms survive translation without loss.

## Commands and findings

All passed:

```text
cargo fmt --all -- --check
cargo check -p axiom-openai-compat --all-targets
cargo clippy -p axiom-openai-compat --all-targets -- -D warnings
cargo test -p axiom-openai-compat
```

Medium, corrected: `max_tokens` and `max_completion_tokens` could have created
ambiguous limits; equal values coalesce and conflicting values are rejected.
No blocker or high-severity finding remains.

## Exit criteria

The adapter is a pure, strict data boundary with no route around secure
inference. Stage 12 may proceed.


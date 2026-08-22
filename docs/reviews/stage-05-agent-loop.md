# Stage 05 Review — Agent loop and typed tools

Decision: **PASS**

- Tool schemas, strict JSON argument decoding, typed results, stable call IDs,
  bounded output, repeated-call detection, a step cap, a context-byte cap, and a
  wall-clock cap are enforced outside the model.
- Zero-tool, text, repeated, unknown, failed, and multi-step paths are exercised
  by scripted providers and deterministic evaluations.
- Tool output is wrapped as untrusted data before returning to inference.
- TUI and ACP consume the same proposal/start/output/completion events.

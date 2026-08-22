# Stage 04 Review — Development inference provider

Decision: **PASS**

- The provider-neutral stream maps text, reasoning, usage, and indexed tool-call
  deltas from an OpenAI-compatible development adapter.
- A real loopback HTTP service streams SSE in the test suite; malformed and
  prematurely closed streams fail rather than silently becoming complete. The
  suite also covers cancellation, retryable rate limits with a stable
  idempotency key, non-retryable authentication failures, redaction, bounded
  stream events, and model discovery.
- Requests are cancellable and time-bounded. API keys use a secret wrapper and
  provider debug output contains only `[REDACTED]`.
- All front ends display/advertise unattested development status; no insecure
  fallback is called secure.
- On 2026-08-22, `axiomcli models` discovered five models through a real local
  AxiomIO proxy and the ignored live smoke completed a streamed `glm-5-2` text
  turn. The command used only process-local environment variables and emitted
  no credential material.

The live test remains opt-in and is not run with credentials in CI. It fails
closed unless all three explicit live-test environment variables are present.

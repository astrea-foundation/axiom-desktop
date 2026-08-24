# Secure inference Stage 14 review — live Axiom attestation and chat

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `04bd35d` (`feat: verify live secure proxy
inference`)

Live service: `https://api.axiom.stream`

## Live evidence

The secret-safe `scripts/smoke-live-proxy` harness started the actual proxy on
an ephemeral loopback port and passed against the current production catalog.
It selected `glm-5-2` for explicit `medium` reasoning and independently tested
`deepseek-v4-flash` as a second model.

For both models, local evidence verification reported:

- provider `near`, E2EE `near-v2` version 2, and attestation
  `near-tdx-nvidia-v1`;
- fresh nonce, exact model, model-key, and live TLS SPKI binding passed;
- Intel TDX status `UpToDate` and NVIDIA evidence verified;
- distinct model-key and TLS-SPKI fingerprints remained bound to the selected
  model;
- `approved_workload_identity=false`, so the evidence does not overclaim an
  approved Axiom software build.

The same run passed encrypted non-streaming output, streamed text and reasoning,
stream usage, `[DONE]`, a ten-second client disconnect recorded as `cancelled`,
prompt-sentinel log scanning, credential scanning, and a separate real proxy
pointed at an unresolvable relay that returned `502` without reporting
verification or falling back.

Sanitized run summary:

```json
{
  "status": "pass",
  "model": "glm-5-2",
  "second_model": "deepseek-v4-flash",
  "nonstream": true,
  "stream": true,
  "cancellation": true,
  "attestation": "verified locally per request",
  "network_failure": "failed closed",
  "plaintext_log_scan": "pass",
  "credential_scan": "pass"
}
```

## Commands

All passed:

```text
scripts/smoke-live-proxy
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
bash -n scripts/smoke-live-proxy
```

The harness requires `AXIOM_API_KEY` in process environment and accepts neither
the upstream credential nor local token through argv. Successful runs delete
plaintext outputs; retained failure artifacts discard response text and retain
only catalog and metadata-safe logs.

## Findings

- High, corrected: current live usage includes `cached_prompt_tokens`; the
  strict decoder rejected the otherwise valid completion. The field is now
  explicitly modeled and rejected when it exceeds prompt usage.
- Medium, corrected: the OpenAI model-list adapter hid capability information,
  causing the smoke to choose a model without reasoning support. It now exposes
  bounded model capabilities as compatible extensions.
- Medium, corrected: models with no reasoning capability were unusable because
  the local explicit level was always sent. They now retain the explicit local
  setting but omit the unsupported provider parameter.
- Medium, corrected: an early disconnect occurred during per-request
  attestation and therefore had no stream terminal event. The live check now
  waits long enough to disconnect an established remote stream and proves the
  upstream body is cancelled rather than completed.
- Informational: a real out-of-credit account was not manufactured or drained;
  the exact 402 mapping is covered through the actual process in Lane B.
- Informational: Intel/NVIDIA evidence currently proves hardware and bindings,
  while the lack of an approved workload allowlist is represented accurately.
- Blocker/high unresolved: none in the Stage 14 exit scope.

## Exit criteria

Current real attestation and both response modes pass through the actual proxy
for two model identities. Cancellation and unavailable-network paths fail
closed without a plaintext fallback. Stage 15 may proceed.

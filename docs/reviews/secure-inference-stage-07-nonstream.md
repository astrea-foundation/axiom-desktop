# Secure inference Stage 07 review — non-streaming inference

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `ac1a83d` (`feat: implement encrypted relay
inference`)

## Evidence

- A session can only be returned after provider-owned local attestation.
- Each request creates a new OS-random client identity and encrypts content,
  reasoning, refusal, names, tool definitions, schemas, choices, assistant
  tool history, arguments, and tool results to the attested model key.
- The relay request carries only catalog routing, protocol/version, public
  keys, role/call metadata, sampling/limits, and ciphertext.
- Completion ciphertext is authenticated/decrypted locally. Model identity,
  run ID, output presence, tool IDs/counts, JSON arguments, usage, and finish
  reason are validated before an `InferenceResponse` exists.
- A sentinel test scans the actual serialized relay JSON, and tampered
  ciphertext fails authentication.

## Commands

The full Stage 06 gate was rerun and passed. Secure-client tests include
non-stream round trips, sentinel absence, tamper rejection, protocol vectors,
and all prior attestation adversarial cases.

## Findings

- High, corrected: the original response contract discarded reasoning,
  refusal, usage, and finish reason. A provider-neutral `InferenceResponse`
  now retains them while AxiomCLI can still consume its `AssistantTurn`.
- Medium, corrected: invalid tool choices and output limits could otherwise be
  rejected only by the relay. They are now capability-checked locally.
- Blocker/high unresolved: none in deterministic coverage.
- Informational: live Axiom inference remains a protected Stage 14 gate; this
  review does not substitute fixtures for that claim.

## Exit criteria

The deterministic full encrypted request/decrypt path and fail-closed mutation
cases pass. Stage 08 may proceed.


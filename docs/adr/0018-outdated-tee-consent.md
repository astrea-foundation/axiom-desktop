# 0018: Explicit consent for outdated provider security updates

Status: accepted, September 22, 2026.

Intel may authenticate a TDX quote while reporting that its environment needs
security updates. Previously this blocked generation without a user choice.
A local user may now accept the exact `OutOfDate` verdict for NEAR. Consent lasts
for that provider until the native runtime restarts and is never persisted or
written into the signed trust policy. A local proxy or headless request remains
strict unless its caller explicitly provides this narrow runtime consent.

Missing updates remain a security risk. Desktop and TUI show accepted evidence
as degraded, with a yellow warning; authenticating a reply does not turn that
environment green. Fresh quote verification, nonce, key, model, workload, TLS,
GPU and response authentication remain mandatory. Revocation, configuration
faults and every other failed verification remain blocking errors.

The native verifier is authoritative. The backend needs an explicit
`allow_outdated_tcb` worker-establishment option so it can relay the original
evidence for local verification. It retains the actual status in the
account-bound worker snapshot. This does not authorize plaintext inference.

`securityEvidence@3` adds `outdated` and `degraded` states and explicit consent
on security verification, bound to the selected model. Desktop and its sidecar
must ship together. Desktop retries a paused message through the existing local
edit flow, preserving its attachments; TUI exposes `/security accept-outdated`.

Current contracts live in [security](../threat-model.md),
[ACP compatibility](../acp-compatibility.md) and [NEAR v3](../protocols/near-v3.md).
The [platform NEAR contract](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/architecture/near-v3.md)
owns backend behavior. Compatible native and backend revisions must be released
together; merging development code alone does not enable production consent.

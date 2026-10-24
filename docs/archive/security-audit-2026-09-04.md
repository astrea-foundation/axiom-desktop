# Security audit — 2026-09-04

> Historical evidence copied from the former threat model. Conclusions below
> apply only to the recorded scope and date, not current release acceptance.
> Current boundaries and limitations are in [the security guide](../threat-model.md).


Reviewed 2026-09-04 for the Linux x86-64 GNU AxiomCLI coding-agent candidate and
the headless macOS Intel/Apple-Silicon and Windows x64/ARM64 Desktop packaging
lanes. Severity refers to impact inside the advertised scope; a passing row
means the control and its regression evidence are present, not that the
underlying model or external service is trusted. Interactive target-machine
Desktop acceptance remains a separate release gate.

| Boundary | Severity | Status and evidence |
|---|---:|---|
| Workspace traversal/symlink/transaction escape | blocker | closed: canonicalization, descriptor rechecks, rollback, adversarial/property tests |
| Process host/network/credential escape | blocker | closed on Linux: mandatory Bubblewrap, isolated network/environment, process groups, cancellation/soak tests |
| Policy or session-grant authority escalation | blocker | closed: deny-first normalization, hard bounds, exhaustive profile/effect/rule tests |
| Web SSRF, redirect, DNS rebinding | blocker | closed: per-hop public-address validation and adversarial local-service tests |
| Ambiguous MCP side-effect replay | blocker | closed: never replayed; read-only retry requires unchanged schema; real stdio crash fixtures |
| ACP disconnect grants permission or leaves work running | blocker | closed: disconnect/cancellation fail closed and parent cancellation reaches children |
| Durable recovery replays an unknown operation | blocker | closed: interrupted work is marked unknown, surfaced, and never replayed |
| Credential in logs, journals, child env, renderer IPC, or package | blocker | closed in headless scope: native token isolation, secret wrappers/redaction/environment allowlist plus persistence/package scans |
| Terminal control injection | high | closed: content sanitization, no raw model ANSI, snapshots and PTY restoration tests |
| Resource exhaustion from streams/tools/journals | high | closed to recorded bounds: step/context/output/time caps, bounded exports, soak gates |
| Dependency/advisory/license or artifact tampering | high | closed: lockfile, Cargo Deny, reproducible tar, SHA-256, detached signature verification, license inventory |
| Malformed secure-provider wire input | high | closed to recorded bounds: parser validation, property tests, negative fixtures |
| Proxy stream/cancellation resource leak | high | closed on Linux: process soak checks bounded file descriptors and resident memory after completed/cancelled streams |

No blocker/high-severity item remains open within the audited Linux coding-agent
scope. Desktop platform support remains conditional on the interactive release
checks documented in `platform-support.md`. Local journals are not encrypted by
AxiomCLI and are documented as relying on OS/disk protection.

# Early AxiomCLI Threat Model

## Assets

- user source code and uncommitted changes
- credentials and process environment
- integrity of local files and repositories
- local process and network authority
- session history and audit information
- correctness of permission decisions

## Trust boundaries

The model, fetched web pages, MCP protocol data, ACP clients, repository contents,
tool output, and terminal input are untrusted. A configured MCP executable is
separately treated as trusted user-selected local code at process startup; it
cannot be introduced by repository configuration. The Rust policy engine decides
whether normalized effects may run. A user approval is authority only for the
operation and scope shown in the approval request.

Development Axiom inference is unattested. The implementation must never label
it secure or silently imply that later TEE/E2EE guarantees already apply.

## Principal threats and early mitigations

| Threat | Early mitigation |
|---|---|
| Model requests an unsafe operation | Typed tools, effect normalization, deny-first policy, bounded grants |
| Path traversal or symlink escape | Canonical workspace roots, parent validation, revalidation immediately before mutation |
| Shell/argument injection | Structured program tool; shell is separate and higher risk |
| Process survives cancellation | Process groups, bounded shutdown, explicit interrupted state |
| Tool output controls terminal | Strip control characters and never render raw ANSI from untrusted content |
| Web SSRF or redirect pivot | Resolve/revalidate each hop; deny non-public addresses; byte/time/content limits |
| Web/MCP prompt injection | Delimit as untrusted tool data; never interpret it as policy |
| ACP client disconnects during approval | Fail closed; do not convert disconnection into approval |
| Credential exposure | Environment/secret wrapper, redacted tracing, deny child inheritance by default |
| Event/output memory exhaustion | Bounded channels, transcript/tool limits, backpressure and truncation markers |
| Crash after side effect | Journal start/completion; unknown operations are not replayed |
| Persistent rule broadens authority | Explicit scope and confirmation; deny outranks all allows |
| Configured MCP executable has host authority | Only trusted user config may launch it; protocol data and side-effect claims remain untrusted and calls stay policy-controlled |

## Deferred threats

- Production attestation and E2EE require a separate protocol-specific review.
- OS-level sandbox availability differs by platform and will supplement, not
  replace, application policy.
- Multi-user remote ACP transports are not supported by the first version.
- Wallet and blockchain authority are out of scope for this repository.

## Release-candidate security audit

Reviewed 2026-08-22 for the supported Linux x86-64 GNU development release.
Severity refers to impact inside the advertised scope; a passing row means the
control and its regression evidence are present, not that the underlying model
or external service is trusted.

| Boundary | Severity | Status and evidence |
|---|---:|---|
| Workspace traversal/symlink/transaction escape | blocker | closed: canonicalization, descriptor rechecks, rollback, adversarial/property tests |
| Process host/network/credential escape | blocker | closed on Linux: mandatory Bubblewrap, isolated network/environment, process groups, cancellation/soak tests |
| Policy or session-grant authority escalation | blocker | closed: deny-first normalization, hard bounds, exhaustive profile/effect/rule tests |
| Web SSRF, redirect, DNS rebinding | blocker | closed: per-hop public-address validation and adversarial local-service tests |
| Ambiguous MCP side-effect replay | blocker | closed: never replayed; read-only retry requires unchanged schema; real stdio crash fixtures |
| ACP disconnect grants permission or leaves work running | blocker | closed: disconnect/cancellation fail closed and parent cancellation reaches children |
| Durable recovery replays an unknown operation | blocker | closed: interrupted work is marked unknown, surfaced, and never replayed |
| Credential in logs, journals, child env, or package | blocker | closed: secret wrappers/redaction/environment allowlist plus persistence/package scans |
| Terminal control injection | high | closed: content sanitization, no raw model ANSI, snapshots and PTY restoration tests |
| Resource exhaustion from streams/tools/journals | high | closed to recorded bounds: step/context/output/time caps, bounded exports, soak gates |
| Dependency/advisory/license or artifact tampering | high | closed: lockfile, Cargo Deny, reproducible tar, SHA-256, detached signature verification, license inventory |

No blocker/high-severity item remains open within this release’s stated scope.
Unattested plaintext inference is an explicit product limitation rather than a
silently accepted secure path: this build says `UNATTESTED DEV`, and production
TEE/E2EE remains a separate future security program. Local journals are not
encrypted by AxiomCLI and are documented as relying on OS/disk protection.

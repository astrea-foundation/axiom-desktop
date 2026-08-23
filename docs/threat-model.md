# Early AxiomCLI Threat Model

## Assets

- user source code and uncommitted changes
- credentials and process environment
- integrity of local files and repositories
- local process and network authority
- session history and audit information
- correctness of permission decisions
- plaintext prompts, conversation history, model responses, and tool context
- Axiom API credentials and ephemeral inference key material
- integrity of model selection and provider security evidence

## Trust boundaries

The model, fetched web pages, MCP protocol data, ACP clients, repository contents,
tool output, and terminal input are untrusted. A configured MCP executable is
separately treated as trusted user-selected local code at process startup; it
cannot be introduced by repository configuration. The Rust policy engine decides
whether normalized effects may run. A user approval is authority only for the
operation and scope shown in the approval request.

The default inference implementation is the secure provider. The old plaintext
adapter is absent from default/package selection and requires both a deliberate
Cargo feature and runtime opt-in; that mode must always report
`UNATTESTED DEV`.

## Production inference trust boundary

The Axiom relay, model catalog, provider endpoints, DNS, TLS peers, attestation
payloads, Intel/NVIDIA collateral services, and encrypted stream metadata are
untrusted inputs. Production inference accepts a provider key only after the
local secure client verifies fresh evidence, the selected model, and the exact
live TLS SPKI binding. The provider-specific implementation owns this complete
chain and returns only an opaque verified session.

The Axiom relay is trusted for authentication, billing, routing, and
availability, but not with message plaintext. It may receive ciphertext,
provider/model identifiers, public keys, protocol metadata, request parameters,
usage, and terminal status. It must not receive plaintext messages, reasoning,
refusals, tool names/descriptions/schemas/arguments/results, or model output.

Valid vendor evidence and endpoint binding do not by themselves prove an
Axiom-approved software measurement. Until an approved workload allowlist is
enforced, security evidence must describe the narrower verified claims rather
than implying approved-code identity.

## Local proxy trust boundary

`axiom-proxy` is a user-local plaintext endpoint because OpenAI-compatible
clients do not implement Axiom E2EE. It binds only to loopback and requires a
separate local credential. Any local process holding that credential can submit
plaintext to the proxy, so process isolation and credential distribution remain
part of the desktop threat model. The proxy never gives its Axiom credential to
the client and never executes requested tools.

Tool definitions, model-proposed calls, and tool results are encrypted by the
secure transport. Tool execution authority remains with AxiomCLI or the
external coding client and its deterministic permission system.

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
| Relay observes prompts or tool context | Encrypt every secret-bearing field locally; sentinel scans over serialized relay requests and logs |
| Malicious catalog chooses wrong provider/endpoint | Match compiled provider/protocol versions; HTTPS/public-address policy; verify fresh evidence rather than trusting catalog keys |
| Replayed or substituted attestation | Fresh nonce, expected model, signing-key/report-data binding, exact live TLS SPKI binding |
| Genuine hardware runs unapproved workload | Expose the distinction; add an approved measurement/manifest allowlist before claiming approved-runtime identity |
| Ciphertext/stream tampering | AEAD authentication, bounded typed parsing, strict event ordering, fail closed on missing terminal event |
| Local client steals Axiom credential | Separate proxy credential; never inherit or expose the upstream credential to clients |
| Proxy exposed beyond the machine | Loopback-only binding in the initial product; non-loopback configuration rejected |
| Event/output memory exhaustion | Bounded channels, transcript/tool limits, backpressure and truncation markers |
| Crash after side effect | Journal start/completion; unknown operations are not replayed |
| Persistent rule broadens authority | Explicit scope and confirmation; deny outranks all allows |
| Configured MCP executable has host authority | Only trusted user config may launch it; protocol data and side-effect claims remain untrusted and calls stay policy-controlled |

## Deferred threats

- Approved Axiom workload measurement/manifest allowlisting is not enforced
  yet. Current evidence must not be described as approved-code identity.
- OS-level sandbox availability differs by platform and will supplement, not
  replace, application policy.
- Multi-user remote ACP transports are not supported by the first version.
- Wallet and blockchain authority are out of scope for this repository.

## Release-candidate security audit

Reviewed 2026-08-23 for the supported Linux x86-64 GNU early release candidate.
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
| Malformed secure-provider wire input | high | closed to recorded bounds: ciphertext/evidence/SSE/request/tool-fragment fuzz entry points, properties, fixture negatives |
| Proxy stream/cancellation resource leak | high | closed on Linux: process soak checks bounded file descriptors and resident memory after completed/cancelled streams |

No blocker/high-severity item remains open within this release’s stated scope.
The remaining workload-allowlist limitation is disclosed independently of the
verified vendor evidence and endpoint/key bindings. Local journals are not
encrypted by AxiomCLI and are documented as relying on OS/disk protection.

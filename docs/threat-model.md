# Early AxiomCLI Threat Model

## Assets

- user source code and uncommitted changes
- credentials and process environment
- integrity of local files and repositories
- local process and network authority
- session history and audit information
- correctness of permission decisions
- plaintext prompts, conversation history, model responses, and tool context
- native account-session credentials, automation API keys, and ephemeral
  inference key material
- integrity of account credit/invoice state and short-lived invoice payment
  URLs
- integrity of model selection and provider security evidence

## Trust boundaries

The model, fetched web pages, MCP protocol data, ACP clients, repository contents,
tool output, and terminal input are untrusted. A configured MCP executable is
separately treated as trusted user-selected local code at process startup; it
cannot be introduced by repository configuration. The Rust policy engine decides
whether normalized effects may run. A user approval is authority only for the
operation and scope shown in the approval request.

Production/provider E2EE is the only inference path reachable from AxiomCLI and
the desktop sidecar. No Cargo feature, environment variable, or runtime option
can select a plaintext provider adapter.

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
Axiom-approved software measurement. Phala source provenance is verified against
the TEE-measured workflow and exposed for inspection, without manual approval of
source revisions. Security evidence describes these verified claims and does not
imply Axiom-approved code identity.

## Local proxy trust boundary

`axiom-proxy` is a separate user-local plaintext endpoint because
OpenAI-compatible clients do not implement Axiom E2EE. It binds only to loopback
and requires a separate local credential. Any local process holding that
credential can submit plaintext to the proxy, so process isolation and
credential distribution remain part of its local-integration threat model.
Axiom Desktop does not route chat through this proxy. The proxy never gives its
Axiom credential to the client and never executes requested tools.

Tool definitions, model-proposed calls, and tool results are encrypted by the
secure transport. Tool execution authority remains with AxiomCLI or the
external coding client and its deterministic permission system.

## Desktop and ACP trust boundary

The Electron renderer and every external ACP client are untrusted. The desktop
renderer is sandboxed and context-isolated, has no Node or child-process access,
and receives only a domain-specific preload API. It cannot send arbitrary ACP
method names. Electron main validates every argument before forwarding it, and
AxiomCLI validates it again before applying application or policy semantics.

Only Electron main spawns AxiomCLI. Packaged builds resolve a fixed binary under
the application resources; an environment override is accepted only in an
unpackaged development build. ACP stdout contains only bounded JSON-RPC frames,
stderr diagnostics are capped in the desktop process, and sidecar exit rejects
all pending requests. A sidecar restart changes the runtime instance ID, making
old approval, elicitation, and deletion handles stale.

The Axiom extension is capability-negotiated. Its event sequence lets a client
detect lost notifications and repair state from an authoritative snapshot.
Session deletion uses a short-lived single-use token bound to an exact set of
session IDs. Native sign-in passes only a method hint and opaque login handle
through the renderer boundary. Passwords, passkey assertions, OAuth codes,
wallet signatures, access tokens, and refresh tokens remain outside renderer
state and IPC. The renderer cannot approve an option that AxiomCLI did not
offer.

Desktop Agent mode is off by default. An explicit per-thread configuration can
enable confirmation-gated local tools or Full access, using a native-picked
directory or a distinct managed thread directory. Permissions and directory
persist atomically in the account-local store. Active work blocks changes;
queued prompts and steering bind to the saved configuration revision. Changing
configuration clears cached policy grants and file-change tracking. Legacy mode
setters cannot bypass this desktop configuration flow. A working directory is
not an OS sandbox: programs use the OS user's permissions and can contact the
network, even though the Web toggle independently gates Axiom's search/fetch tools.

AxiomCLI stores a rotating native refresh token only in the operating-system
credential vault and keeps access tokens in memory. Credential-vault failure
leaves the process signed out; there is no plaintext file fallback. Once signed
in, each opaque account ID selects separate CLI and Desktop SQLite databases.
Signing out closes the active store, and an account change invalidates pending
Desktop operations before another account's state opens.

Billing is an account-control boundary, not wallet authority. AxiomCLI permits
status and Shieldz invoice operations only with a current native account access
token; the automation API-key path cannot authorize them. It validates integer
amount/balance bounds, invoice identity/state, timestamps, and a bounded HTTPS
payment URL, then rejects an in-flight result if the account context changed.
The typed Desktop state may contain pending invoice metadata and its payment
URL, but never Zcash keys, an Ethereum signature, or a transaction request.

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
| Compromised renderer calls privileged IPC | Narrow preload API, context isolation/sandbox, main-process validation, Rust revalidation; no raw ACP escape hatch |
| Forged or replayed interaction response | Client-side offered-option check plus one active Rust request; stale IDs fail closed |
| Replayed/altered delete confirmation | Random process-local token, exact normalized selection, 60-second expiry, single use |
| Sidecar replacement | Fixed packaged resource path; development override is unavailable in packaged mode |
| Malformed/oversized ACP output | Incremental 4 MiB frame bound, fatal malformed JSON handling, bounded diagnostics and request map |
| Lost/reordered extension activity | Runtime instance and monotonic sequence trigger authoritative snapshot repair |
| Native credential leaked through desktop state | No token/API-key IPC method; browser handles method credentials; only bounded account metadata and opaque login handle reach renderer |
| Credential exposure | OS credential vault for rotating refresh token, memory-only access token, secret wrappers/redacted tracing, deny child inheritance by default, no file fallback |
| Account-local thread crossed into another account | Opaque validated account path, separate stores, account-context revision checks, close/invalidate on logout or account change |
| Stale or forged invoice grants credit/payment authority | Native-account-only billing client, bounded strict invoice DTOs, idempotent client request ID, account-generation recheck, monotonic revisions; payment URL grants no wallet/model authority |
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
- Ethereum wallet signatures may authenticate an account in the hosted auth
  surface. Wallet key custody and transaction authority remain out of scope and
  are never granted to Desktop, AxiomCLI, ACP, or the model.

## Release-candidate security audit

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
| Malformed secure-provider wire input | high | closed to recorded bounds: ciphertext/evidence/SSE/request/tool-fragment fuzz entry points, properties, fixture negatives |
| Proxy stream/cancellation resource leak | high | closed on Linux: process soak checks bounded file descriptors and resident memory after completed/cancelled streams |

No blocker/high-severity item remains open within the audited Linux coding-agent
scope. Desktop platform support remains conditional on the interactive release
checks documented in `platform-support.md`. The remaining workload-allowlist
limitation is disclosed independently of the verified vendor evidence and
endpoint/key bindings. Local journals are not encrypted by AxiomCLI and are
documented as relying on OS/disk protection.

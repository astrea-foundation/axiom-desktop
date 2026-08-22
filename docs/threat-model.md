# Early AxiomCLI Threat Model

## Assets

- user source code and uncommitted changes
- credentials and process environment
- integrity of local files and repositories
- local process and network authority
- session history and audit information
- correctness of permission decisions

## Trust boundaries

The model, fetched web pages, MCP servers, ACP clients, repository contents,
tool output, and terminal input are untrusted. The Rust policy engine decides
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

## Deferred threats

- Production attestation and E2EE require a separate protocol-specific review.
- OS-level sandbox availability differs by platform and will supplement, not
  replace, application policy.
- Multi-user remote ACP transports are not supported by the first version.
- Wallet and blockchain authority are out of scope for this repository.

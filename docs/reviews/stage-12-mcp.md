# Stage 12 Review — MCP

Decision: **PASS (configured stdio scope, reviewed 2026-08-22)**

- Configured stdio servers are started with a cleared environment through the
  official Rust MCP SDK, initialized, and queried for real tool discovery.
- Tools receive stable `mcp__server__tool` namespaces and preserve server/tool
  provenance in output and policy effects. Namespace collisions fail startup.
- Every MCP invocation passes through AxiomCLI policy. Server annotations are
  never authorization; only explicit user `read_only_tools` configuration can
  classify a tool as read-only.
- Each configured server has one shared connection manager. A closed process is
  restarted on demand. Read-only calls may be retried once after a transport
  disconnect, but only after rediscovery proves the tool and exact input schema
  are unchanged. Side-effecting calls with an ambiguous outcome are never
  replayed.
- Arguments must be objects. Schemas and descriptions are size checked; results
  and errors are control-cleaned, redacted, bounded, and labeled as untrusted.
  Result envelopes identify the server, remote tool, and connection generation.
- MCP calls enter the same `ToolProposed` → permission → `ToolStarted` →
  `ToolOutput` → `ToolCompleted` event path used by built-in tools. TUI and ACP
  therefore receive the same lifecycle and stable call IDs; MCP wire messages
  never leak into ACP framing.
- A real Rust MCP stdio fixture covers discovery, misleading annotations,
  crashes, reconnect, schema drift, duplicate names, namespace collisions,
  long/secret-bearing output, malformed protocol results, and cancellation.

Verification:

```text
cargo test --workspace --all-targets
cargo test -p axiomcli --test mcp_integration -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

HTTP MCP transport and ACP per-session MCP declarations are not advertised in
this early version. Reconnect is intentionally bounded rather than an
unbounded background retry loop.

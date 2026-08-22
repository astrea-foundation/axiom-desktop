# Stage 12 Review — MCP

Decision: **PASS (configured stdio scope)**

- Configured stdio servers are started with a cleared environment through the
  official Rust MCP SDK, initialized, and queried for real tool discovery.
- Tools receive stable `mcp__server__tool` namespaces and preserve server/tool
  provenance in output and policy effects. Namespace collisions fail startup.
- Every MCP invocation passes through AxiomCLI policy. Server annotations are
  never authorization; only explicit user `read_only_tools` configuration can
  classify a tool as read-only.
- Arguments must be objects, results are bounded/labeled untrusted, and dropping
  the connection cancels its owned child process.

HTTP MCP transport, reconnect loops, and ACP per-session MCP declarations are
not advertised in this early version.

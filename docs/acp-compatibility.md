# ACP Compatibility Matrix

Target: stable ACP v1 through `agent-client-protocol` 2.0.0.

This matrix describes the early implementation. Unsupported capabilities are
not advertised.

| ACP operation | Status | AxiomCLI mapping | Cancellation/error behavior |
|---|---|---|---|
| `initialize` | supported | identifies AxiomCLI and negotiates v1 | unsupported versions negotiate to v1 for client validation |
| `session/new` | supported | canonical absolute cwd → `CreateSession` | invalid/nonexistent cwd returns JSON-RPC error |
| `session/prompt` text | supported | `SubmitPrompt` → shared `TurnRunner` | completion returns an ACP stop reason |
| `session/update` agent text | supported | `TextDelta` | ordered per connection |
| `session/update` thoughts | supported | `ReasoningDelta` | omitted when provider exposes none |
| `session/cancel` | supported | cancels the active turn token | fail-closed; returns `cancelled` from prompt |
| `session/request_permission` | supported | normalized policy effect → allow-once/deny options | cancellation, timeout, and disconnect fail closed |
| tool calls and updates | supported | typed proposal, pending/running, output, and terminal status | IDs remain stable for a call |
| structured questions | supported when client advertises unstable form elicitation | one AxiomCLI multi-question request → one `elicitation/create` form plus attention updates | decline, cancellation, invalid shape, or unsupported capability fails closed |
| plan activity | supported as shared updates | proposed/reviewed plans → labeled thought updates; plan tools retain normal tool lifecycle | approval is artifact state, never execution authority |
| `session/load` | supported and advertised when durable storage opens | stable ACP ID → journal restore, transcript update replay, provider-context reconstruction | cwd identity mismatch/corruption fails closed; interrupted side effects are reported unknown and never replayed |
| MCP servers in session | not advertised | trusted-user-configured stdio MCP servers are namespaced and policy-controlled; their calls use the normal ACP tool lifecycle | per-session ACP MCP configuration is deferred; transport messages never enter ACP stdout |
| custom extensions | unsupported | none | unknown requests receive standard protocol errors |

ACP mode reserves stdout for protocol frames. All logs are routed to stderr.

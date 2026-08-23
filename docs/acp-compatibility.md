# ACP Compatibility Matrix

Target: stable ACP v1 through `agent-client-protocol` 2.0.0.

This matrix describes the early implementation. Unsupported capabilities are
not advertised.

| ACP operation | Status | AxiomCLI mapping | Cancellation/error behavior |
|---|---|---|---|
| `initialize` | supported | identifies AxiomCLI and negotiates v1 | unsupported versions negotiate to v1 for client validation |
| `session/new` | supported | canonical absolute cwd → `CreateSession` | invalid/nonexistent cwd returns JSON-RPC error |
| `session/prompt` text | supported | ordinary text → `SubmitPrompt`; advertised `/permissions`, `/model`, `/resume`, `/delete`, `/thinking`, `/compact`, and `/help` input → typed local control | completion returns an ACP stop reason; slash text is never submitted to the model; `/resume` directs ACP hosts to native loading; `/delete` directs users to the TUI until ACP defines native transcript deletion |
| `session/update` available commands | supported | new/load advertises the shared TUI command vocabulary and argument hints | unsupported/invalid command input ends explicitly without starting a model turn |
| `session/set_config_option` model selection | supported after `/model` requests discovery | `/model` publishes the provider catalog as an ACP model select; selecting a listed value changes and journals the session model | arbitrary values and changes during active work fail closed |
| `session/update` agent text | supported | `TextDelta` | ordered per connection |
| `session/update` thoughts | supported | `ReasoningDelta` | omitted when provider exposes none |
| `session/cancel` | supported | cancels the active turn or compaction token | fail-closed; returns `cancelled` from prompt |
| `session/request_permission` | supported | normalized policy effect → deny/allow-once plus eligible exact-operation and safe-prefix session grants | cancellation and disconnect fail closed; an optional whole-turn limit may cancel the enclosing turn |
| tool calls and updates | supported | typed proposal, pending/running, output, and terminal status | IDs remain stable for a call |
| structured questions | supported when client advertises unstable form elicitation | one AxiomCLI multi-question request → one `elicitation/create` form plus attention updates; required and optional fields remain distinct | decline, cancellation, invalid shape, or unsupported capability emits a terminal failed-question event and fails closed |
| task/checklist and plan activity | supported as shared updates | task lists and proposed/reviewed plans → stable ACP `plan` updates; review input uses one structured elicitation form; plan tools retain normal tool lifecycle | review comments are persisted; approval is artifact state, never execution authority |
| `session/load` | supported and advertised when durable storage opens | stable ACP ID → journal restore, transcript update replay, provider-context reconstruction; ACP hosts own their session-selection UI | cwd identity mismatch/corruption fails closed; interrupted side effects are reported unknown and never replayed |
| MCP servers in session | not advertised | trusted-user-configured stdio MCP servers are namespaced and policy-controlled; their calls use the normal ACP tool lifecycle | per-session ACP MCP configuration is deferred; transport messages never enter ACP stdout |
| custom extensions | unsupported | none | unknown requests receive standard protocol errors |

ACP mode reserves stdout for protocol frames. All logs are routed to stderr.
Each new session resolves its permission profile and restrictive policy rules
from that session's canonical `cwd`; an ACP server launched elsewhere cannot
accidentally use the launch directory's project policy. Provider/model/tool
construction remains process-wide in this early ACP implementation.
Model selection is session-scoped inside the shared runner. In ACP, `/model`
requests real provider discovery and exposes the result through ACP's native
session configuration selector rather than accepting a typed ID. Thinking
preference is explicit (default `medium`), session-scoped, durable, and reused
for future chats after a prompt runs, but remains a development-proxy
transport stub until direct Axiom model metadata defines its wire contract.
Versioning, storage migration, and ACP change rules are defined in
[`versioning.md`](versioning.md).

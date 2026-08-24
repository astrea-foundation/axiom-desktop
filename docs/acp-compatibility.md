# ACP compatibility

AxiomCLI targets ACP protocol v1 using the latest published
`agent-client-protocol` runtime available during this implementation: crate
2.0.0, which pins its runtime schema to 1.5.0. The ACP protocol number, Rust
crate release, schema crate release, and Axiom extension version are separate
numbers and must not be compared as if they were one version.

## Standard ACP

| Operation | Support and mapping |
|---|---|
| `initialize` | ACP v1, implementation metadata, load-session capability, and optional `_meta.axiom` negotiation |
| `session/new` | canonical absolute cwd, durable `CreateSession`, native modes and config options |
| `session/load` | journal recovery/replay, exact cwd identity, restored model/thinking/profile |
| `session/prompt` | text/resource-link prompts, ordered message/reasoning chunks, structured tool activity and stop reason |
| `session/cancel` | cancels the session's active prompt or compaction |
| `$/cancel_request` | cooperatively cancels prompt, security, compaction, and browser-login completion requests |
| `session/set_mode` | permission profile; policy remains enforced inside AxiomCLI |
| `session/set_config_option` | model and thinking level |
| permission requests | deny, allow once, eligible exact-session, and eligible safe-prefix-session choices |
| form elicitation | required/optional text, enum, and multiple-choice questions when advertised by the client |
| plans | native plan updates for tasks and plan review |
| tools | stable IDs, kinds, pending/running/final states, raw input, bounded output, and native before/after file diffs |
| session info | generated title updates after the first prompt |
| available commands | slash-command fallback for interactive generic clients |

Prompt image/audio support, client-managed terminals/filesystems, and
per-session MCP injection are not advertised. Local tool and MCP execution must
remain behind AxiomCLI's policy boundary. Unknown standard methods receive
normal JSON-RPC method errors.

## Negotiated Axiom extension

An Axiom-capable client sends compatible capabilities under
`clientCapabilities._meta.axiom`. AxiomCLI always describes its extension in
the initialize response so the client can select a compatible feature set, but
it sends `_axiom/event` only after successful negotiation. Custom calls made
without negotiation fail safely and do not weaken standard behavior.

Version 0.1 adds:

- searchable/paginated session catalog, authoritative snapshots, safe batch
  deletion, title/settings/outcome metadata;
- typed model catalog and atomic session settings;
- browser/API-key account flow and logout/status;
- fresh security verification and bounded attestation/workload evidence;
- cancellable compaction phases and structured progress/task/workspace events.

Permission and question answering deliberately remain standard ACP. See
[`axiom-acp-extension.md`](axiom-acp-extension.md) for method names and schema
generation, and [`acp-desktop-parity-matrix.md`](acp-desktop-parity-matrix.md)
for semantic coverage.

## First-party process rules

ACP uses newline-delimited JSON-RPC. AxiomCLI reserves stdout for protocol
frames and sends diagnostics to stderr. The TypeScript client incrementally
bounds incoming/outgoing frames to 4 MiB, caps retained stderr at 64 KiB,
honors stdin backpressure, rejects pending work on process exit, and detects
extension sequence gaps. Renderer code never sees a raw-request method.

The TypeScript package is tested against the actual `axiomcli acp` binary with
deterministic runners. Rust transcript tests separately prove a standard-only
client and a negotiated first-party client.

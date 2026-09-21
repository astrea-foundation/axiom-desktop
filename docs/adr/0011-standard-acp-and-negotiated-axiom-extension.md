# ADR 0011: Standard ACP with a negotiated Axiom extension

Status: accepted (early implementation)

## Context

AxiomCLI must remain useful to generic ACP clients while giving the first-party
desktop application the account, billing, attestation, session-catalog,
deletion, and task-state controls the product needs. Flattening those controls
into slash-command text loses state, cancellation, and security meaning.

## Decision

Standard ACP v1 is the primary coding-agent contract. AxiomCLI uses native ACP
prompts, cancellation, permission requests, elicitation, modes, configuration
options, plans, tool kinds, native file diffs, and session metadata. The first
party client advertises `_meta.axiom` during `initialize`; only a compatible
client may call `_axiom/*` methods or receive `_axiom/event` notifications.

The Rust types in `axiom-acp-extension` are authoritative. Their deterministic
JSON Schema is checked in under `protocol/axiom-acp-extension/v0.2/`, and the
TypeScript DTOs are generated from that schema. Additive fields are allowed
within a feature version. Removing, renaming, or changing the meaning of a
field requires a new protocol/feature version.

Extension state contains a runtime instance ID and a process-wide monotonic
sequence. A gap or runtime change causes the desktop client to retrieve an
authoritative session snapshot. Delete confirmations are exact-selection,
single-use, process-local, and expire after 60 seconds.

AxiomCLI—not Electron—owns authentication storage, secure transport,
attestation, policy decisions, tool execution, journals, and destructive
session operations. Electron main owns subprocess supervision. The sandboxed
renderer receives a narrow preload API and cannot submit raw ACP methods.

The first-party chat client launches the explicit
`axiomcli acp --frontend desktop-chat` profile and negotiates
`desktopChat@1`. `_axiom/desktop/bootstrap` returns the canonical app-owned chat
cwd and effective Web policy. AxiomCLI rejects other cwd or permission values,
does not connect configured MCP servers, and uses an embedded desktop product
prompt. Generic `axiomcli acp` behavior remains workspace-oriented.

Both frontends use a platform-native `axiom` root. AxiomCLI owns native session
storage in the operating-system credential vault and selects account-scoped,
frontend-specific journals only after authentication. It also owns Desktop
collection metadata; renderer folder labels do not influence filesystem paths.
Electron and ACP never receive access or refresh tokens. Extension 0.2 replaces
the old browser/API-key account requests with native login
start/complete/cancel, raises the account feature to version 2, and adds
`billing@2` for bounded credit and mainnet deposit status. The former invoice
methods are superseded by [ADR 0014](0014-clean-storage-and-deposit-billing.md).
Wallet custody and transaction authority never cross the extension.

## Bounds and failure behavior

- ACP is newline-delimited JSON-RPC on stdout; diagnostics use stderr.
- The first-party process client rejects frames larger than 4 MiB and keeps at
  most 64 KiB of stderr diagnostics.
- Extension identifiers, queries, native-login handles, evidence, manifests,
  selections, and compaction focus have explicit bounds.
- Event queues use existing bounded Rust channels; Node writes honor stream
  backpressure.
- Prompt, compaction, security, and native-login completion support
  cancellation. Disconnect and stale interaction responses fail closed.
- A production package resolves a fixed resource path for AxiomCLI. A sidecar
  environment override exists only in unpackaged development builds.

## Consequences

Generic ACP clients get a standards-based agent without custom messages.
First-party clients obtain recoverable desktop parity without treating UI
types or `AppEvent` as a public wire format. The extension remains deliberately
small and versioned while AxiomCLI is early and changing.

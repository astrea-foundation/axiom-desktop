# Native architecture

AxiomCLI owns model-message construction, the selected system prompt, tool
execution, provider verification/encryption/decryption, and durable conversations.
Desktop presents and controls that runtime over local ACP. The proxy is a separate
integration for clients that speak OpenAI-compatible HTTP.

## Components

| Component | Contract |
|---|---|
| `apps/axiomcli` | TUI, one-shot `exec`, ACP server, account authentication, tools and storage |
| `apps/desktop` | Electron main supervises the sidecar; sandboxed renderer uses typed preload methods |
| `apps/axiom-proxy` | Standalone authenticated loopback server and library used by `axiomcli desktop-proxy` |
| `crates/axiom-inference` | Transport-independent model, request, tool, usage and event types |
| `crates/axiom-secure-client` | Catalog, registered providers, local attestation and authenticated E2EE |
| `crates/axiom-phala-core` | Owned Phala verifier/cipher fork; see its [provenance](../crates/axiom-phala-core/ORIGIN.md) |
| `crates/axiom-openai-compat` | Pure OpenAI wire translation |
| `crates/axiom-acp-extension` | Authoritative first-party Rust DTOs and schema |
| `packages/axiom-acp-client` | Node-side process framing, negotiation, state reduction and recovery |
| `packages/brand` | Native fonts/artwork and compiled local login callback page |
| `packages/desktop-releases` | Release-manifest validation shared by native packaging/update code |

The TUI and proxy call the secure client directly. Desktop chat uses ACP; its
optional Proxy screen supervises a separate account-bound loopback process.
The [ACP guide](acp-compatibility.md) owns the control contract.

## Frontend code organization

[ACP registration](../apps/axiomcli/src/acp/server.rs) creates one typed server
context. Handlers in `apps/axiomcli/src/acp/` own sessions, prompts, account and
billing operations, thread history, collections, model preferences, settings and
security/compaction. Account transition barriers, responder errors and spawned
work retain the same connection ownership.

The [TUI entry point](../apps/axiomcli/src/tui.rs) preserves the public interface.
Its `state` module owns event reduction and selection; `runtime` owns terminal
lifecycle, input and asynchronous work; `screens` and `transcript` render those
states. Attestation export and terminal-safe text helpers have separate modules.

Desktop's [App](../apps/desktop/src/renderer/src/App.tsx) composes layout with
`useDesktopState`, `useNativeAccount`, `useModelCatalog`, `useConversationNavigation`
and `useConversations`. These hooks own the IPC subscription, login lifecycle,
model preferences, selection, and durable submission/queue lifecycle respectively.
Account/runtime generations and stable submission IDs remain the guards against
late work affecting a different account or conversation.

## Account and credential ownership

Native login starts a short-lived browser authorization bound to PKCE and a
one-use device secret. The browser handles passkey, Google, password or wallet
credentials. Polling completes authorization even if the loopback notification
cannot reach the client.

AxiomCLI holds access tokens in memory and rotating refresh tokens in the OS
credential store. Desktop and CLI share installation identity and native login;
token rotation and account transitions are coordinated. ACP and Electron receive
bounded account metadata and opaque login handles, never native access/refresh
tokens. Desktop removes `AXIOM_API_KEY` from its sidecar environment; standalone
CLI/proxy automation may use that explicit environment override.

The hosted account ID is an opaque identifier, independent of email, wallet or
installation identity. Login identities and wallet signatures are never provider
E2EE key material. Implementation: [auth.rs](../apps/axiomcli/src/auth.rs),
[account_store.rs](../apps/axiomcli/src/account_store.rs), and
[Desktop sidecar](../apps/desktop/src/main/agent-sidecar.ts).

## Local state

Each signed-in account has separate CLI and Desktop SQLite databases. The
[configuration guide](configuration.md#paths-and-local-state) owns their paths.
Signed-out operations cannot open a fallback conversation database. Account
changes close the old store and invalidate pending operations.

[session.rs](../apps/axiomcli/src/session.rs) stores threads, turns, timeline
items, collections, preferences, Agent settings, context reports and request
accounting. Runtime events are translated into materialized records. Startup
marks unfinished work interrupted; it never replays side effects. Supported
schema upgrades run transactionally; see [versioning](versioning.md).

Conversation content and drafts stay local. Desktop drafts, pending submissions,
Web preferences and queues use account-scoped local state. Deleting a thread
removes its conversation records but does not delete files in its working directory.
Local conversation databases are not encrypted at rest by AxiomCLI.

## Inference and operational metadata

The compiled registry supports NEAR v3, Phala ACI v2 and Tinfoil EHBP v1.
The catalog cannot expand that registry or substitute an unauthenticated key.
The native client verifies each protocol's complete trust chain and authenticates
completion before persisting a successful verified outcome. See
[security](threat-model.md#provider-contracts).

Axiom-platform authenticates, routes ciphertext, accounts for usage and serves
public metadata. It must not receive model-message plaintext or build model
messages. Its authenticated search endpoint is the explicit non-inference
exception: it accepts a bounded search query and returns results. Search queries
and fetched URLs are not provider E2EE; the native client owns tool execution
and encrypts tool results in subsequent model requests.

Request accounting is separate from response verification. A completed billing
record cannot turn cancelled or unauthenticated output into a verified reply.
Automatic conversation titles are derived synchronously from the first accepted
prompt by the shared [title helper](../apps/axiomcli/src/session_title.rs). They
are bounded to eight words and 80 UTF-8 bytes, persisted only if absent in the
account-bound store, and require no inference request or background task.

## Account credit and integrations

The native billing client accepts account-bound status and deposit metadata
from the backend. The Axiom Payments path uses persistent **mainnet ZEC** addresses,
exact integer receipt amounts, and optional backend-reported USD valuation.
The client validates the valuation fields and does not perform a currency swap
or hold payment-provider credentials. Testnet data is rejected in this path.
The native and backend billing contracts must be released together. See [billing.rs](../apps/axiomcli/src/billing.rs)
and the [Desktop guide](desktop.md#account-credit-and-usage).

Hosted website code, payment reconciliation and deployment tooling belong to
axiom-platform. Desktop consumes its public release API, and native packaging
hands installer files/manifests to its publisher. Neither checkout imports
source or fixtures from the other to build or test.

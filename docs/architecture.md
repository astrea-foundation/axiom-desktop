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

[SessionStore](../apps/axiomcli/src/session.rs) keeps one account-generation-bound
connection behind a stable public API. Its [storage modules](../apps/axiomcli/src/session)
separate records, initialization, connection routing, event/timeline writes,
request accounting, thread queries, collections, preferences, Agent settings,
recovery, projections and value encoding. Helpers receive the existing connection
or transaction; moving a method does not create an extra commit boundary.
Runtime events become materialized records rather than stored event envelopes.
Startup marks unfinished work interrupted and never replays side effects.
The inspectable [SQL baseline](../apps/axiomcli/src/session/schema.sql) and
[versioning policy](versioning.md) define compatibility.

Conversation content and drafts stay local. Desktop drafts, pending submissions,
Web preferences and queues use account-scoped local state. Deleting a thread
removes its conversation records but does not delete files in its working directory.
Local conversation databases are not encrypted at rest by AxiomCLI.

## Runtime boundaries

TUI and ACP submit typed commands to the shared runtime and translate semantic
events for their own presentation. Tokio tasks use explicit cancellation tokens;
bounded channels and task ownership prevent abandoned work from outliving its
account or parent operation. Domain errors remain typed; diagnostics are redacted.

Each secure provider owns evidence retrieval, local verification, key/endpoint
binding, encryption and response authentication as one operation. It returns an
opaque verified session rather than allowing callers to assemble trust from
arbitrary public-key bytes. Shared crates receive typed configuration; application
composition roots own environment settings and tool permission profiles.

## Inference and operational metadata

The compiled registry supports NEAR v3 and Tinfoil EHBP v1.
Models and capabilities refresh through [provider discovery](provider-catalogs.md).
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
The shared [title helper](../apps/axiomcli/src/session_title.rs) stores a bounded
local fallback when a thread begins. A background attested E2EE request can
replace only its own reserved fallback; manual renaming prevents replacement.
Title work has separate usage accounting and cannot update another account or
thread. Failure preserves the local fallback.

## Account credit and integrations

The native billing client accepts account-bound status and deposit metadata
from the backend. The Axiom Payments path uses persistent **mainnet ZEC** addresses,
exact integer receipt amounts, and optional backend-reported USD valuation.
The client validates the valuation fields and does not perform a currency swap
or hold payment-provider credentials. Testnet data is rejected in this path.
Gift redemption uses the same interactive account authority. TUI and Desktop
send codes through the native account client, receive a validated credit receipt,
and never store the code as chat/history. Platform issues the card and owns its
atomic ledger credit. The [ACP contract](acp-compatibility.md#accounts-billing-and-public-evidence)
defines the client boundary.
The native and backend billing contracts must be released together. See [billing.rs](../apps/axiomcli/src/billing.rs)
and the [Desktop guide](desktop.md#account-credit-and-usage).

Hosted website code, payment reconciliation and deployment tooling belong to
axiom-platform. Desktop consumes its public release API, and native packaging
hands installer files/manifests to its publisher. Neither checkout imports
source or fixtures from the other to build or test.

### Local attachment storage

Schema v2 upgrades v1 in a transaction by adding `prompt_attachments`, keyed to
the owning user timeline row with cascading deletion. Timeline metadata retains
only filename/type/size summaries; full inputs are fetched on demand or restored
into native model history. Revision replaces payloads in the same transaction as
the prompt; account guards apply to reads. Missing/corrupt payloads fail closed.
Older binaries reject v2 instead of silently losing attachments.

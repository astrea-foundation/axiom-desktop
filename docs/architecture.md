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

Desktop sends generated PNG bytes once per document-local image resource,
separately from streaming state. Main replaces duplicate inline blocks with
opaque references; preload creates local Blob URLs before state crosses
`contextBridge`. Token updates carry metadata and references, not image base64.
Each window has its own resource context. Logout, account/runtime changes,
disconnection and document unload revoke resources; deleted timeline resources
are released. A full bootstrap resends assets, and late same-context bootstrap
assets can repair references without rolling back newer conversation state.
Native conversation storage, image hashes, E2EE verification and resolution
limits are unchanged. Browser inline-image compatibility decoding yields in
bounded chunks outside React rendering.

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

On macOS, startup checks for the origin-scoped Keychain item by reference without
decrypting its password. The first restore reads the refresh token once while
holding the CLI/Desktop coordination lock continuously through server rotation
and persistence. Commit reuses that locked observation, and the macOS writer
updates the existing item in place without reading the old password again.
The item keeps its existing access controls and shared service/account identity.
The protected entry is an `axc1_` envelope containing the rotating refresh token
and an independently random 32-byte recovery attempt. It is still one OS
credential item, using the existing origin/profile scope. Legacy bare tokens
upgrade under the same process-shared lock before a request is dispatched.
Transport timeouts, 5xx and malformed responses retain the same durable attempt;
a different CLI/Desktop process reloads it under that lock. The API may return
the exact original rotation for two minutes while all session/token bounds are
valid and the successor remains unused. Retries never extend expiration or
create another family branch. A different attempt, late retry or successor reuse
retains family revocation; HTTP 401 clears the matching local credential.
Successful rotation replaces the entry with the successor and a fresh attempt.
Install compatible platform migration/API support before shipping this native
client; an old API rejects the new request field and the native client keeps its
entry for a later compatible retry. This envelope is opaque to older native
versions, so do not downgrade a profile with a saved current session.

A denied read remains unavailable and is not retried by account-status polling.

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
Local conversation databases use SQLCipher with independent OS-protected keys.
Managed image/file bytes are encrypted external assets; authoritative records
and checkpoints keep their references. Drafts and the outbox use native encrypted
persistence with acknowledged forward import from legacy browser storage. See
[encrypted local storage](local-storage.md). Authoritative prompts, assistant
messages, tool results and metadata retain their exact runtime content; only
diagnostics and explicit exports apply credential redaction. MCP adapters still
filter exact secrets supplied in their configured environment before establishing
a tool result; generic diagnostic heuristics never rewrite successful results. If automatic
compaction fails before a turn, original context and media remain recoverable.
The turn fails with an explicit retry/choose-larger-context message; it never
substitutes a local excerpt for the source history.

Compaction first checks whether the native conversation and summary output fit
one request, using the current context count when available. Historical system
notes are demoted to untrusted context and reasoning/refusal text is preserved
even when the selected model cannot replay its original fields. Fallback grouping
keeps consecutive serialized fragments and native attachments together until a
token or provider request limit is reached. All passes must succeed before the
successor context replaces the original; partial summaries never commit.

Account databases migrate transactionally to schema 7. Each thread has a
versioned authoritative semantic history independent of presentation rows.
Exact provider-neutral model-message checkpoints preserve raw tool argument
strings, grouped tool calls, tool results, reasoning, images, files and context
boundaries. Live attestation state and evidence remain ephemeral; historical
response verification never restores a live lease. Resume uses this source; timeline edits cannot rewrite inference
context. Display remains a projection of the same semantic events. The latest
exact checkpoint for each turn is retained alongside the full semantic source
history, so earlier message revisions preserve exact model structure. Checkpoints
reference deduplicated model messages, avoiding a new copy of every earlier
image or file at every checkpoint.
Legacy threads without canonical records use their existing compatible timeline
projection until the next write adopts it atomically. A partial legacy load
refuses migration with an explicit error and retains the original records. Unsupported canonical
versions or corrupt authoritative records fail explicitly instead of silently
rebuilding a different conversation. Content already lost by older versions
cannot be recovered by migration.

Execution ownership combines a persisted owner ID and an OS lock per active
thread. Opening a database preserves live work owned by another client. A new
claim rejects concurrent execution transactionally; process exit releases the
lock, allowing recovery to mark only abandoned turns interrupted. Recovery also
runs before a new claim so an already-open observer can recover a later crash.
These local lock files are retained to preserve inode identity. Delayed native
output carries its originating turn ID and cannot attach to another turn.
Account switching invalidates old handles and releases their local execution
locks. Interrupted tool operations remain unknown and are never replayed.

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
Ready deposit accounts without confirming receipts use the read-only billing
projection. Non-ready accounts and confirming receipts use an optional refresh
bounded to five seconds; transient deposit transport/service failures preserve
the validated ledger and previous deposit state. Malformed
financial/address data, access rejection, cancellation and account transitions
still reject the operation. The ACP client shares overlapping balance reads only
within the same account generation and never caches a settled result.
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

# Axiom clean-slate v1 refactor plan

Status: superseded historical record

This document describes the retired wallet-root/browser-storage account design. It is retained only
as implementation history and is not a current product or security contract. The authoritative
account, authentication, local-storage, and invoice-billing design is
[`account-auth-billing-clean-slate-plan.md`](./account-auth-billing-clean-slate-plan.md).

Scope: `axiomai` desktop/TUI repository and `~/clone/axiom` web/backend repository

Data policy: all pre-release local and deployed development databases may be destroyed and recreated

## 1. Objective

Establish the first supported Axiom data model and protocol contract without carrying compatibility
paths for unreleased installations. AxiomCLI remains the trusted agent runtime and the sole durable
state owner for Axiom Desktop. The web backend continues to persist only encrypted conversation
payloads and operational metadata, while every inference path remains bound to locally verified TEE
evidence and provider E2EE.

This refactor is complete only when the Desktop, TUI, web client, API relay, NEAR provider, and Phala
provider all use the same strict model identity, attestation, and secure-inference concepts.

## 2. Non-negotiable invariants

1. No plaintext prompt, history, assistant delta, tool argument, or completion may pass through or be
   persisted by the web backend.
2. A model encryption key is usable only after the client locally verifies fresh TEE evidence and
   binds the key to the selected provider and model.
3. A successful streamed response requires its authenticated terminal event and final signed receipt.
4. Axiom Desktop is a Web-profile ACP client. It cannot broaden its permissions or install MCP
   servers.
5. AxiomCLI owns Desktop threads, transcript state, folders, model preferences, and recovery state.
   Electron owns only presentation state such as window geometry.
6. One semantic update has one protocol owner. Standard ACP owns conversation content and tool
   activity; the Axiom extension owns Axiom-specific account, security, catalog, and collection data.
7. First-release schemas reject unknown newer versions. They do not migrate any pre-release schema.
8. Provider identity is explicit. No client infers a provider from a model ID, label, suffix, or URL.
9. Every stored AES-GCM envelope uses purpose- and row-bound AAD. A valid ciphertext cannot be
   replayed as another thread title, message, reasoning item, profile field, or device field.
10. Both browser and native clients independently verify the provider's terminal receipt against
    signing material from the same accepted attestation snapshot; backend verification alone is
    never a success condition.

## 3. Target local layout

Use the operating system's platform configuration and local-data roots rather than assuming Unix
home-directory conventions.

```text
<config>/axiom/
  config.toml
  cli/config.toml
  desktop/config.toml

<data>/axiom/
  shared/identity/client-id
  shared/credentials/
  shared/trust-policy.json
  cli/state.sqlite3
  desktop/state.sqlite3
  desktop/chat/
```

The fixed Desktop chat directory exists to satisfy ACP's absolute-cwd contract. It is safe to share
between Desktop threads because the Desktop profile exposes only web tools. If filesystem tools are
ever added to Desktop, that later feature must introduce isolated per-thread workspaces.

## 4. Target local state model

Replace the raw serialized-event database and Electron-owned collection database with one schema per
frontend profile:

- `threads`: identity, title, cwd, profile, selected model, thinking level, lifecycle, archive flag,
  monotonically increasing revision, and activity ordering.
- `turns`: one user submission and its resulting agent execution, with explicit running, completed,
  cancelled, failed, or interrupted status.
- `timeline_items`: ordered durable user messages, assistant messages, reasoning, tool calls, plans,
  and notices. Streaming updates mutate an in-progress item rather than appending one row per token.
- `collections`: Desktop-only folder identity, name, collapsed state, and position.
- `thread_collections`: Desktop thread-to-folder assignment.
- `profile_preferences`: last selected model and thinking level for new threads.

The runtime may continue to use `AppCommand` and `AppEvent` internally. Persistence must translate
runtime events into stable user-domain records and must not serialize transient delivery envelopes as
the database contract.

Turn creation, the user timeline item, and the initial assistant item are one transaction. Terminal
turn state and final assembled assistant/reasoning content are one transaction. During a stream,
bounded checkpoints may update the in-progress item to preserve useful partial output after a crash.
On startup, any nonterminal turn becomes interrupted.

Client-selected thread, turn, and message IDs exist before encryption so stored ciphertext AAD can
bind the exact thread, turn, message, role, and field. The backend validates those bindings before
write and the client supplies the same expected AAD when decrypting; authentication failure is shown
as corrupted data rather than silently rendered as an empty message.

## 5. Target ACP contract

### Standard ACP ownership

- user and assistant message chunks
- reasoning chunks
- tool proposal, progress, output, and completion
- plans
- standard mode/config changes

### Axiom extension ownership

- desktop bootstrap and fixed profile declaration
- thread catalog and authoritative paginated transcript snapshots
- profile preferences
- collection CRUD and assignment
- account/login state
- provider/model catalog metadata
- TEE verification state and evidence
- compaction and background-task metadata that standard ACP cannot represent

Every thread snapshot carries a durable thread revision and last timeline sequence. Notifications
carry the resulting revision. A gap causes the TypeScript client to replace its thread state from the
authoritative transcript endpoint; metadata-only refresh is insufficient.

Provider-model entries require `providerId`, `providerLabel`, `shortLabel`, `upstreamModel`, and
supported thinking levels. Generated Rust, JSON Schema, and TypeScript types must agree.

## 6. Electron boundary

Remove Electron's direct use of `node:sqlite` for collections. Add collection operations to the ACP
client and sidecar wrapper. The renderer consumes one immutable client-state snapshot and issues
intent-based operations; it does not maintain a second durable copy.

Optimistic user-message rendering remains, but it is reconciled by stable client item IDs rather than
message counts. A pending message stays visible during TEE verification and disappears only when the
authoritative timeline contains the same client item ID or the submission fails visibly.

## 7. Target backend identity and encryption model

The launch authentication flow is a client-side wallet-derived pseudonymous account key followed by
Ed25519 challenge authentication. Direct SIWE backend authentication is not part of v1.

Replace the dual wallet/anonymous principal model with:

- `accounts`: pseudonymous account identity, authentication public key, encrypted profile fields,
  encrypted system prompt, disabled flag, explicit trial eligibility state, and next activity sequence.
- `auth_challenges`: hashed nonce and bounded expiry for the account or a registered device.
- `sessions`: account, optional device, token hash, expiry, and revoked flag.
- `devices`: account-scoped authentication keys and encrypted presentation metadata.
- `account_keyrings`: account-wrapped root keys.
- `device_keyrings`: device-wrapped root keys.
- `link_sessions`: short-lived encrypted device-link handshakes.

Remove direct wallet addresses, SIWE challenges, wallet-auth branches, plaintext display names,
generic user metadata used for trial state, and duplicate crypto-version columns already present in
encrypted envelopes. Rename wallet-specific keyring identifiers to account-specific identifiers.

## 8. Target backend chat model

- `threads`: account, encrypted title, selected provider offering, archived flag, per-thread next item
  sequence, and account activity sequence.
- `messages`: thread-local sequence, turn, role, encrypted content/reasoning envelope, state, encrypted
  provider identifiers where required, and aggregate token counts.
- `runs`: turn identity, provider/model/protocol snapshot, status, usage/billing state, client/model key
  fingerprints, and required attestation snapshot identity.

Conversation rows are encrypted directly with the account root key and their exact row AAD. There
is no per-thread key table in the first-release schema: it added another mutable key hierarchy
without providing isolation the current clients could actually use.

Creating a run atomically creates the user message, assistant placeholder, run, thread ordering, and
billing admission record. Editing a message and truncating its descendants is also atomic.

For streaming runs, the provider ciphertext request is durable and atomically claimed for dispatch.
It is not kept in a process-local map. A pre-send `claimed` dispatch may be safely reclaimed after
its short lease, while concurrent stream consumers cannot send the same request twice. Immediately
before provider I/O, the worker irreversibly advances the record to `sending`. Because neither launch
provider exposes a durable idempotency/result-lookup contract, an expired post-send dispatch fails
visibly and is never replayed. Only ciphertext, public keys, and non-secret protocol metadata are
persisted in this dispatch record.

The browser's ephemeral provider session is also restart-safe without disclosing it to Axiom. Before
run admission, the client serializes the minimum provider-specific recovery state and encrypts it
under the account root key. The backend requires and stores that opaque envelope with AAD bound to
the account, thread, turn, provider, model, E2EE protocol, client-key fingerprint, and revision. Both
streaming and non-streaming terminal proofs remain recoverable through an authenticated run-read
contract. A reloaded browser unwraps the state locally, re-verifies the exact terminal receipt, and
submits the row-bound assistant ciphertext; a missing or corrupt recovery envelope is a visible
failed/interrupted run, never a blank assistant message.

Fields that can only have one legal launch value are constraints or code invariants, not per-row
booleans pretending multiple modes exist. In particular, plaintext-transient and unencrypted-output
states do not belong in the v1 schema.

## 9. Shared secure-inference execution

Introduce one backend secure-inference executor used by persistent web chat and the stateless
OpenAI-compatible relay. It owns:

1. stable provider offering resolution;
2. capability and reasoning validation;
3. billing admission and stream metering;
4. attestation lease lookup/refresh;
5. client-supplied generation and fingerprint binding;
6. provider-E2EE request validation;
7. provider invocation and normalized encrypted output;
8. final receipt/proof validation;
9. usage accounting and terminal failure handling.

Persistent chat supplies a database sink. Relay supplies a stateless response sink. Provider and
security behavior must not be independently reimplemented in those adapters.

Billing holds are also database-backed and expiry-bounded so multiple API workers share one view of
in-flight spend and a process restart cannot erase active reservations.

## 10. Provider-neutral attestation model

Use two complementary records:

- immutable attestation snapshots containing provider, offering, protocol, generation, model-key
  fingerprint, keyset digest, generic evidence/checks, verification instant, and expiry;
- a mutable current lease keyed by provider and offering, pointing to its active snapshot and carrying
  refresh ownership, soft expiry, hard expiry, and failure state.

Every run references the exact snapshot accepted for that request. The request must include the
generation and fingerprints verified by the client. The backend rejects stale or mismatched bindings.
This backend check supplements, and never replaces, browser or AxiomCLI verification.

## 11. Compatibility removal

Remove before declaring the schemas complete:

- old AxiomCLI platform-directory migration and SQLite backup feature;
- local schema v1-to-v3 and journal-version compatibility;
- permission aliases and `auto` thinking;
- provider inference from model names;
- duplicate `/api` mounting;
- SIWE API and repository paths;
- disabled legacy regenerate endpoint;
- raw-message, legacy-AAD, funding, verification-shape, and encrypted-field aliases;
- CLI authorization without installation identity;
- optional attestation binding;
- deprecated proxy submodule and legacy R2 rollback configuration;
- migration-only documentation and compatibility tests.

Keep negative tests that prove plaintext inference is rejected.

## 12. Destructive reset and migration policy

1. Replace AxiomCLI's schema history with local schema version `1`.
2. Replace all backend Alembic revisions with one `0001` baseline representing the final schema.
3. Provide explicit development reset commands/scripts; never silently erase an unknown database in
   a released binary.
4. Recreate development and deployed pre-release databases only after all application code targets
   the new baseline.
5. The first post-release schema change becomes migration `0002` and must preserve released data.

## 13. Workstreams and ownership

- Local-state agent: Rust schema/store, path cleanup, collection ownership, and local tests.
- ACP/Desktop agent: extension schema, generated TypeScript, state reconciliation, Electron removal of
  SQLite, renderer integration, and frontend tests.
- Backend agent: identity/schema baseline, strict APIs, atomic chat persistence, shared executor,
  attestation records, web-client changes, and backend/frontend tests.
- Root integrator: resolves cross-workstream contracts, removes repository-level legacy assets,
  performs security review, runs complete verification, and fixes integration failures.

Agents must not reset or overwrite unrelated dirty work. Each agent records commands and outcomes and
reviews at least one boundary owned by another workstream.

## 14. Verification matrix

### Local Rust

- formatting and workspace compilation;
- AxiomCLI unit and integration tests;
- ACP transcript tests;
- new-database creation and future-version rejection;
- interrupted-stream recovery and atomic turn writes;
- CLI and Desktop profile separation with shared identity/trust state.

### Desktop and ACP client

- schema generation produces no diff after regeneration;
- TypeScript type checking and unit tests;
- one visible item per user message, reasoning stream, tool call, progress event, and error;
- sequence-gap transcript replacement;
- collection persistence through sidecar restart;
- optimistic prompt remains visible during TEE verification.

### Backend and web

- one clean Alembic upgrade succeeds on an empty PostgreSQL database;
- authentication, device linking, keyrings, account deletion, and trial-abuse tombstones work with the
  pseudonymous account model;
- a run is either fully admitted with both message records or leaves no partial rows;
- stored ciphertext fails authentication when replayed into another row or semantic field;
- concurrent stream consumers yield one provider dispatch; an expired pre-send claim can recover
  after a process restart, while an expired post-send claim fails durably without an ambiguous
  duplicate provider request;
- web chat and relay exercise the same secure executor;
- NEAR and Phala reject stale/mismatched attestation generations and fingerprints;
- browser and native NEAR clients reject missing, truncated, mismatched, or wrong-signer terminal
  receipts, even when the backend reports success;
- plaintext and unauthenticated provider paths fail closed;
- frontend type checking/build and backend test suite pass.

### Repository audit

- searches find no retired route, alias, migration marker, legacy provider inference, SIWE path,
  direct Desktop SQLite access, or optional attestation binding;
- canonical documentation matches the implemented storage and security design;
- package/dependency manifests no longer include dependencies used only by deleted compatibility code.

## 15. Completion gate

The refactor is complete when fresh local and backend databases support all current Desktop, TUI, web
chat, device-linking, account, billing, NEAR, and Phala flows; all strict contracts are generated and
checked in; all component and integration tests pass; and no pre-release migration or compatibility
path remains in production code.

## 16. Implementation record

Completed on 2026-08-28 as the clean-slate first-release contract across both repositories.

- AxiomCLI is the authoritative Desktop runtime and durable state owner. Desktop is a restricted Web
  ACP profile with a fixed platform data-directory cwd, a Desktop-specific system prompt, stable
  optimistic IDs, transcript-gap recovery, and durable messages, reasoning, plans, tools, interaction
  requests, collections, preferences, and terminal failures.
- Desktop readiness is now a single-flight boundary: Electron exposes the ACP client only after local
  bootstrap and transcript hydration complete, while account validation and model discovery remain
  non-fatal background work. Sidecar restart attempts are bounded and reset only after a stable ready
  window; stale initial renderer snapshots cannot overwrite a newer live ready event.
- Local storage is schema version `1`; backend storage is one Alembic `0001` baseline. Pre-release
  migration, backup, platform-path, SIWE, anonymous-auth, provider-inference, and Desktop SQLite paths
  were removed. Explicit reset commands remain for intentional development resets.
- Shared identity, credentials, and trust policy live below the platform Axiom data root; CLI and
  Desktop state remain isolated. Windows shell execution uses PowerShell or `cmd`, while macOS and
  Linux use `/bin/sh`; the model prompt receives the actual operating system, architecture, shell,
  profile, and cwd. Linux process isolation admits only canonical executable paths from recognized
  user toolchains and mounts those roots read-only without exposing the user's home or credentials.
- `auto` reasoning was removed. New threads start at `medium`, then inherit the profile's last valid
  selection. Catalog reconciliation atomically repairs a retired model or reasoning level.
- ACP request execution uses canonical per-thread work leases with exact-token cleanup. Prompt,
  slash-command, mode, configuration, security, compaction, load, and delete operations cannot race
  one another; cancellation is acquired before spawned work, maps to JSON-RPC request cancellation,
  and cannot split a durable auth or settings commit.
- Authentication mutations are serialized and OS credential access is offloaded and startup-bounded.
  Every validation and mutation receives a monotonic account publication revision, so delayed account
  responses cannot overwrite newer login/logout state. Unavailable platform keyrings leave AxiomCLI
  usable and explicitly signed out instead of preventing Desktop from starting.
- Settings persistence is atomic from the client's perspective. Injected SQLite failures roll the
  runner, Runtime projection, ACP mirror, security state, and profile preferences back before an error
  response is returned.
- Large transcript restoration is cooperative: history and Runtime projection yield in bounded
  batches, shared Runtime state is locked only for atomic installation, and context trimming is exact
  linear-time work rather than repeated full serialization and front removal.
- The TypeScript transport treats closed stdin/EPIPE as one terminal process failure and drains all
  pending work without unhandled rejections. AxiomCLI registers every POSIX tool and MCP process
  group before its first await, drains them at ACP EOF, and kills them on leader exit, future abort,
  or owner drop; a Windows kill-on-close Job Object provides the equivalent process-tree boundary.
- The crates.io ACP SDK 2.0.0 source is vendored with only upstream rust-sdk PR #306's boxed handler
  dispatch fix. A 512 KiB stack regression runs on Linux, macOS, and Windows until a later published
  SDK release contains that upstream fix.
- The Desktop renderer shows exactly one optimistic user item while verification is pending, renders
  structured timeline activity instead of flattening ACP events, uses provider-tab model search with
  normalized brand icons and counts, and carries TEE state in the composer badge.
- NEAR and Phala are strict E2EE adapters. Both buffer provisional plaintext until independently
  verified terminal evidence succeeds, bind the request and terminal receipt to the exact model,
  attestation generation, key fingerprint, request bytes, and response bytes, and revoke a green
  session after a security failure. Plaintext development adapters are build-time gated.
- The browser, native client, and backend now share the same explicit provider/model/protocol and
  immutable attestation-snapshot vocabulary. The browser validates the complete backend key binding
  before independently checking fresh evidence and the terminal receipt.
- The backend uses pseudonymous Ed25519 account/device authentication, row- and purpose-bound AAD,
  atomic run admission, durable dispatch claims, non-replayable post-send failure, shared secure
  execution for chat and relay, database-backed billing holds, bounded provider I/O, and exact usage
  settlement.

Validation evidence:

- Rust workspace: formatting, all-target/all-feature check and Clippy passed; 400 tests passed and 6
  explicitly ignored live/manual tests remained ignored.
- ACP TypeScript client and Desktop renderer: generation/type checks and production builds passed;
  32 tests passed. The vendored ACP SDK small-stack regression passed separately.
- Backend: compile check, Ruff, and 333 tests passed. A fresh PostgreSQL database upgraded from the
  sole `0001` revision, schema checks passed, and the OpenAPI artifact reproduced byte-for-byte.
- Browser: 257 tests passed and 2 environment-specific tests were skipped; lint, TypeScript, verified
  production build, and bundle-size gates passed. The TDX verifier WASM passed 11 tests.
- Landing: TypeScript and production build passed.
- Live NEAR contract checks and cross-stack Phala standard/draft profile vectors passed. Independent
  agents audited native lifecycle/cancellation, browser/backend verification shape, provider
  correlation, portability, legacy residue, and deployment safety.

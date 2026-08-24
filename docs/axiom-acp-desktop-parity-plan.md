# Axiom ACP desktop-parity implementation plan

Status: **implemented on the working tree; deterministic release gates pass**

Implementation note (2026-08-24): the protocol crate, standard/extension ACP
adapter, generated TypeScript client, Electron main/preload sidecar boundary,
recovery logic, safety controls, and deterministic parity tests described below
are present. The desktop renderer remains intentionally visual-only because its
product design is a separate workstream, as defined in section 2.2. A packaged
artifact and live Axiom-service smoke test remain deployment release activities;
they are not represented as deterministic local test results.

Primary objective: expose the full semantic functionality of AxiomCLI's TUI to
the Electron desktop application without making the desktop renderer a trusted
execution or security boundary.

The implementation must preserve ordinary ACP interoperability while allowing
the first-party desktop client to negotiate richer, versioned Axiom behavior.
It must not stream terminal pixels, emulate TUI keystrokes, or make `tui.rs`
the source of application behavior.

## 1. Desired outcome

The TUI and desktop application should be two presentations of the same agent
runtime:

```text
                              AxiomCLI core
                     commands, events, policy, tools,
                    sessions, authentication, TEE/E2EE
                              /          \
                             /            \
                    Ratatui adapter     ACP adapter
                                              |
                              stable standard ACP behavior
                                              +
                              negotiated _axiom extensions
                                              |
                                  Electron main process
                                              |
                                    restricted preload API
                                              |
                                      React renderer
```

At completion, everything a user can meaningfully do in the TUI must either:

1. use a standard ACP operation;
2. use a negotiated `_axiom/*` operation; or
3. be explicitly classified as terminal-only presentation behavior with no
   missing application capability.

Full parity means semantic parity, not visual identity. Cherry-blossom
animations, terminal wrapping, and key bindings belong to their respective
presentation layers. Session control, settings, security evidence, approvals,
questions, compaction, tool activity, and durable state do not.

## 2. Scope

### 2.1 In scope

- Current stable ACP support and capability negotiation.
- A versioned Axiom extension carried on the same local ACP JSON-RPC channel.
- A typed, UI-neutral application service layer behind both adapters.
- Full session lifecycle and transcript management.
- Model discovery and selection.
- Thinking-level and permission-profile selection.
- Prompting, streaming, cancellation, and compaction.
- Structured tool calls, terminal activity, diffs, workspace changes, tasks,
  plans, background work, warnings, and errors.
- Structured approvals and questions.
- Axiom account login, logout, validation, and browser authorization progress.
- TEE/E2EE status, verification, evidence, and workload inspection.
- State snapshots, ordered notifications, gap detection, and reconnection.
- Generated TypeScript types and a desktop-side ACP client library.
- Electron main/preload integration with a narrow renderer API.
- Compatibility, parity, security, recovery, and performance tests.

### 2.2 Explicitly out of scope

- Designing or finishing the desktop visual experience.
- Reproducing Ratatui widgets in React.
- Proxy sidecar lifecycle, which remains a separate versioned control protocol.
- Wallet and financial authorization implementation. The extension must reserve
  a safe future boundary for these features but must not invent them now.
- Remote ACP listeners or network-exposed agent control.
- Treating an Electron renderer decision as security authorization.
- Stabilizing a public protocol before the pre-1.0 implementation has real
  desktop usage.

## 3. Existing foundation

The repository already contains the right architectural seed:

- `apps/axiomcli/src/app.rs` defines typed `AppCommand`, `AppEvent`, and
  versioned event envelopes.
- ADR 0001 requires the TUI, ACP adapter, and test harness to share the same
  command/event kernel.
- `apps/axiomcli/src/acp.rs` implements a real ACP v1 adapter with prompting,
  streaming, cancellation, session loading, permissions, tool updates, plans,
  model configuration, and negotiated form elicitation.
- `apps/axiomcli/src/tui.rs` consumes the same events, but still directly owns
  some orchestration for overlays, model/session pickers, authentication,
  security verification, and compaction.
- `apps/desktop` is currently a visual concept. It does not spawn AxiomCLI or
  communicate with inference.
- `packages/sidecar-protocol` correctly excludes ACP; proxy lifecycle control
  and agent control must remain separate.
- ADR 0009 already specifies that Electron spawns `axiomcli acp` for agent work
  and `axiom-proxy` independently for proxy work.

### 3.1 Current fidelity gaps

Before extension work, create a checked-in matrix covering every `AppCommand`,
`AppEvent`, slash command, TUI overlay, and persistent setting. The initial
inventory must include at least these known gaps:

| Domain | Current ACP behavior | Required direction |
|---|---|---|
| permission profile | slash command and text update | standard mode/config when suitable, typed Axiom metadata where needed |
| thinking level | slash command and text update | native session config option |
| model | native config only after `/model` discovery | advertise and update predictably without command side effects |
| diffs | flattened into text | native ACP diff content |
| tools | lifecycle present, limited kind/location/content metadata | fully typed standard tool calls and updates |
| local terminal | executed inside AxiomCLI | expose structured activity; evaluate ACP-managed terminal use without moving policy to the client |
| progress/background tasks | thought text | typed progress/activity updates |
| compaction | command plus text completion | typed start/progress/complete/cancel lifecycle |
| account/login | slash commands and text | native auth where possible plus Axiom browser-flow state |
| security evidence | bounded text | typed, redacted status and evidence |
| session picker | host-owned load instruction | typed session catalog with titles and settings |
| transcript deletion | TUI only | safe batch preview and confirmed deletion |
| prompt content | text and resource links | honest capability advertisement; add types only when provider/runtime support exists |
| MCP session configuration | not advertised | implement or explicitly defer with a documented reason |
| unmatched internal events | silently ignored | explicit mapped/desktop-irrelevant/sensitive classification |

The matrix is a living artifact. Adding a command or event without updating it
must fail CI.

## 4. Architectural rules

### 4.1 Standard ACP first

Use standard ACP whenever its semantics are sufficient. In particular, prefer
standard session operations, configuration options, modes, authentication,
elicitation, permissions, plans, tool calls, diffs, terminal references,
content blocks, cancellation, and session updates.

Do not create `_axiom/model/set` if the current standard configuration API can
express model selection correctly. Do not wrap a native ACP diff in a custom
event merely to keep the adapter easy.

### 4.2 A negotiated Axiom extension second

Use the protocol's extension mechanisms for Axiom-specific behavior:

- custom capabilities advertised during `initialize`;
- custom method names beginning with `_`;
- `_meta` on standard objects when supplemental data belongs to that object.

The first-party extension namespace is `_axiom`. No extension request may be
sent unless both peers negotiated the corresponding feature and version.
Unknown extensions must produce ordinary method-not-found or unsupported
feature errors without damaging the session.

### 4.3 The wire contract is not `AppEvent`

`AppEvent` remains the canonical source of application semantics, but its
persisted Rust representation must not be serialized directly as the public
desktop wire protocol. A dedicated outward DTO layer is required because:

- internal events can evolve independently;
- not every event is safe or useful to expose;
- wire fields require explicit size, redaction, and compatibility rules;
- state snapshots need derived state rather than event replay alone;
- the TypeScript client needs a stable discriminated contract.

Every internal-to-wire mapping must be exhaustive. A wildcard that silently
drops new `AppEvent` variants is prohibited in the parity mapper.

### 4.4 AxiomCLI remains the trusted principal

The desktop may request actions and render choices, but AxiomCLI owns:

- permission and tool policy evaluation;
- grant validation and expiry;
- filesystem/shell/MCP execution;
- session persistence and destructive transcript operations;
- credential storage;
- attestation verification and encryption state;
- cancellation state and side-effect recovery;
- future wallet authorization enforcement.

An extension is a control and presentation mechanism, never an authorization
bypass. All model output, renderer input, extension input, and ACP client claims
remain untrusted.

### 4.5 Renderer isolation

Only the Electron main process may spawn or communicate directly with
`axiomcli acp`. The renderer receives a narrow, typed API through the preload
bridge. It must not receive raw subprocess handles, arbitrary JSON-RPC access,
environment variables, credentials, encryption keys, or unrestricted local
paths.

### 4.6 Early-version changeability

The first extension is explicitly pre-1.0. Prefer small domain messages,
additive schemas, fixtures, and clear conversion code over a large framework.
Do not create compatibility guarantees that cannot yet be tested with the real
desktop consumer.

## 5. Target protocol shape

### 5.1 Capability negotiation

The exact schema will be settled against the upgraded ACP dependency, but the
semantic shape should resemble:

```json
{
  "_axiom": {
    "protocolVersion": "0.1",
    "features": {
      "sessionCatalog": 1,
      "globalSettings": 1,
      "account": 1,
      "securityEvidence": 1,
      "compaction": 1,
      "activity": 1
    }
  }
}
```

Capabilities are granular. A client that supports security status but not raw
evidence must not be sent raw evidence. Version negotiation chooses the highest
mutually supported version per feature, not merely one process-wide boolean.

The initialization result must also include:

- AxiomCLI implementation name and version;
- extension protocol version range;
- runtime instance ID;
- stable standard ACP capabilities;
- development/production transport classification without secret data.

### 5.2 State and ordering

Live events alone are insufficient for a desktop application. Define:

- a global state snapshot;
- a per-session state snapshot;
- monotonically increasing per-session event sequence numbers;
- a runtime instance ID to detect sidecar restarts;
- correlation IDs joining commands, activities, approvals, and responses;
- a resynchronization request when the client observes a sequence gap.

The desktop must be able to reconstruct correct state after:

- opening after AxiomCLI has initialized;
- loading a durable session;
- a renderer reload;
- an Electron window recreation;
- a temporary main-to-renderer subscription gap;
- an AxiomCLI crash and supervised restart.

Snapshots must include durable truth, not transient widget state:

```text
session identity and title
cwd identity appropriate for display
lifecycle and active turn
model, thinking level, permission profile
security state and evidence summary
conversation/activity references
pending approval or question, if recoverable
compaction state
last terminal outcome
```

### 5.3 Proposed extension domains

Names below are planning names, not frozen wire APIs.

#### Session catalog

```text
_axiom/session/list
_axiom/session/get_state
_axiom/session/delete_preview
_axiom/session/delete_confirm
_axiom/session/title_changed            notification
_axiom/session/state_changed            notification
```

Deletion is a two-step destructive operation. `delete_preview` resolves exact
session IDs, reports what will be deleted, and returns a short-lived opaque
confirmation token. `delete_confirm` must use that token and exact IDs. The
runtime rejects stale, altered, active, or unknown targets. Multi-select and
select-all are desktop/TUI presentation behaviors over this same service.

#### Global and session settings

Use ACP configuration and mode mechanisms for session model, thinking, and
permission profile wherever the standard supports the required semantics.
Reserve custom operations for global defaults and Axiom-specific constraints:

```text
_axiom/settings/get_defaults
_axiom/settings/set_defaults
_axiom/settings/defaults_changed         notification
```

Setting changes must return the accepted authoritative value. The desktop must
never optimistically assume a permission-profile change succeeded.

#### Account and browser authorization

Use stable ACP authentication/logout operations where possible, supplemented
with Axiom browser-flow progress where the standard cannot represent it:

```text
_axiom/account/status
_axiom/account/browser_login_start
_axiom/account/browser_login_cancel
_axiom/account/use_api_key
_axiom/account/status_changed             notification
```

Browser-login responses may expose an authorization URL, user code, expiry,
and non-secret progress. API keys are accepted only by the Electron main
process and sent once to AxiomCLI over the local child-process pipe. They are
stored by AxiomCLI, redacted from logs/errors/events, and never sent back.

#### Security, attestation, and workload evidence

```text
_axiom/security/get_status
_axiom/security/get_evidence
_axiom/security/verify
_axiom/security/cancel_verification
_axiom/security/status_changed            notification
```

Separate summary, workload, and raw-evidence views in the DTO. Raw evidence
must be bounded and reviewed for identifiers or secrets before exposure.
Verification reports stages and terminal outcomes. Failure remains fail-closed;
the desktop cannot acknowledge a failure into an insecure inference mode.

#### Compaction

```text
_axiom/compaction/start
_axiom/compaction/cancel
_axiom/compaction/progress                 notification
_axiom/compaction/completed                notification
_axiom/compaction/failed                   notification
```

The request carries an optional preservation focus. Progress reports phase,
not fabricated percentages, unless the runtime has real quantitative progress.
The final event reports the resulting context boundary and message count but
does not automatically expose the private summary to UI telemetry.

#### Rich activity

Use standard ACP plans, tool calls, tool updates, diffs, and terminal content
first. Use Axiom metadata or extensions only for information without a native
home:

```text
_axiom/activity/snapshot
_axiom/activity/progress_changed           notification
_axiom/activity/background_task_changed    notification
_axiom/activity/workspace_changed          notification
```

The activity model must distinguish model text, reasoning, tool activity,
background tasks, plans, warnings, and final outcomes so the desktop can render
a task running to completion instead of a raw transcript.

#### Approvals and questions

Continue using standard `session/request_permission` and stable elicitation.
Attach negotiated `_meta._axiom` fields only when needed to represent normalized
policy effects, exact-operation scope, safe-prefix scope, risk explanation, or
confirmation semantics that standard fields cannot carry.

The selected response returns to the same `ApprovalHandler` or question handler
used by the TUI. Disconnect, cancellation, malformed choice IDs, stale request
IDs, and renderer closure all fail closed.

### 5.4 Error contract

Define stable error categories independent of presentation strings:

```text
unsupported_feature
invalid_request
invalid_state
not_found
conflict
permission_denied
authentication_required
security_verification_failed
cancelled
expired
rate_limited
provider_unavailable
internal
```

Errors may include a safe user-facing message, retryability, correlation ID,
and bounded structured details. They must never include API keys, encrypted key
material, raw secrets, unrestricted provider bodies, or sensitive tool output.

## 6. Repository changes proposed by this plan

Do not perform these changes until their stage begins.

```text
crates/
  axiom-acp-extension/
    src/
      capabilities.rs
      methods.rs
      notifications.rs
      state.rs
      errors.rs
      redaction.rs
    tests/
      fixtures.rs

protocol/
  axiom-acp-extension/
    v0.1/
      schema.json
      examples/

packages/
  axiom-acp-client/
    src/
      generated/
      process.ts
      client.ts
      state.ts
      errors.ts
    tests/

apps/axiomcli/src/
  application/
    account.rs
    catalog.rs
    compaction.rs
    security.rs
    sessions.rs
  acp/
    standard.rs
    extension.rs
    mapping.rs
    server.rs

apps/desktop/src/
  main/agent-sidecar.ts
  preload/agent-api.ts
```

The exact split may change during implementation review. The important
boundaries are:

- extension DTOs and validation are not embedded in `tui.rs`;
- TypeScript types are generated from one checked-in contract, not handwritten
  separately from Rust;
- ACP framing and Axiom semantics remain separable;
- proxy sidecar types remain outside the agent ACP package;
- React types do not become the transport schema.

## 7. Stage governance

Stages are sequential. A later stage must not begin until the current stage has
a review record marked `PASS`.

Every stage review must:

1. identify the exact commit or working-tree state reviewed;
2. list every command run and its result;
3. map each exit criterion to concrete evidence;
4. review correctness, security, privacy, compatibility, recovery, performance,
   and maintainability as applicable;
5. classify findings as blocker, high, medium, low, or informational;
6. remain `FAIL` while a blocker or high finding is unresolved;
7. fix failures in the stage that introduced them;
8. rerun the complete stage gate after fixes;
9. save the review under `docs/reviews/acp-desktop-stage-NN-*.md`.

The default Rust gate for implementation stages is:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

Once the TypeScript package exists, also run:

```text
pnpm install --frozen-lockfile
pnpm --recursive typecheck
pnpm --recursive test
pnpm --filter axiom-desktop build
```

Network/live-service tests remain opt-in except at designated integration and
release gates. All protocol behavior must have deterministic local coverage.

## 8. Implementation stages

### Stage 00 — Freeze and inventory the baseline

**Goal:** establish exactly what the TUI and ACP adapter do before changing
either one.

**Work:**

1. Record the current ACP dependency, wire version, schema version, enabled
   unstable features, and upstream stable version.
2. Enumerate every `AppCommand`, `AppEvent`, slash command, TUI overlay,
   approval outcome, question shape, session field, and durable setting.
3. Create `docs/acp-desktop-parity-matrix.md` with columns for core semantic,
   TUI behavior, standard ACP mapping, Axiom-extension mapping, desktop API,
   persistence, cancellation, sensitivity, and test coverage.
4. Add tests that capture current ACP initialization capabilities and complete
   representative TUI and ACP transcripts.
5. Record known bugs and intentional omissions without fixing them in this
   stage.

**Exit criteria:**

- No existing command/event/overlay lacks a matrix row.
- Existing tests pass without changed user behavior.
- The review can explain every current wildcard or dropped ACP event.

### Stage 01 — Protocol and threat-model decisions

**Goal:** settle the contract rules before adding messages.

**Work:**

1. Add an ADR for standard-first ACP plus negotiated `_axiom` extensions.
2. Define protocol/source-of-truth ownership between Rust DTOs, checked-in JSON
   Schema, and generated TypeScript.
3. Define extension version negotiation, feature negotiation, method naming,
   `_meta` use, additive evolution, deprecation, and unknown-field behavior.
4. Define stdout/stderr separation, maximum message size, maximum field sizes,
   stream backpressure, timeouts, and cancellation.
5. Extend the threat model for a compromised renderer, malicious ACP client,
   forged permission response, replayed extension request, sidecar replacement,
   malformed JSON, oversized output, and secret leakage.
6. Decide which path information may cross to the renderer and how it is
   presented or redacted.

**Exit criteria:**

- ADR and threat-model changes are approved.
- Every proposed extension domain has an owner, trust boundary, and version.
- No authorization decision is assigned to Electron.

### Stage 02 — Upgrade and prove the standard ACP baseline

**Goal:** avoid inventing extensions for capabilities now present in stable
ACP.

**Work:**

1. Upgrade to the selected current stable ACP Rust/schema release.
2. Remove obsolete unstable feature gates when their behavior is stable.
3. Reconcile changed authentication, elicitation, configuration, modes,
   terminals, compaction updates, and content types.
4. Update `docs/acp-compatibility.md` to distinguish ACP protocol version from
   Rust crate version.
5. Run upstream conformance/schema fixtures where available.
6. Preserve fallback behavior for a minimally capable standard ACP client.

**Exit criteria:**

- All existing ACP transcript tests pass or have reviewed protocol-correct
  replacements.
- Initialization advertises exactly what is implemented.
- No custom feature exists where stable ACP now suffices.
- A generic ACP client can create, prompt, cancel, approve, and load a session.

### Stage 03 — Move TUI-owned orchestration into application services

**Goal:** ensure adapters request semantics instead of implementing workflows.

**Work:**

1. Extract UI-neutral services for session catalog/deletion, model catalog,
   account state, browser login, security verification/evidence, and compaction.
2. Add typed commands/results/events for operations currently performed
   directly in the TUI event loop.
3. Keep overlays, input buffers, selections, animations, and formatting in the
   TUI.
4. Make TUI behavior call the new services without visual regression.
5. Ensure the same service operations are cancellable and carry correlation
   IDs.
6. Add exhaustive event mapping helpers and remove silent adapter drops.

**Exit criteria:**

- `tui.rs` no longer owns account, security, compaction, catalog, or destructive
  persistence policy.
- TUI snapshots and PTY tests remain equivalent.
- A headless Rust harness can execute every semantic TUI operation.

### Stage 04 — Saturate standard ACP fidelity

**Goal:** make the ordinary ACP representation as rich as the standard permits.

**Work:**

1. Represent model and thinking level as native session config options.
2. Represent permission profile as a native mode or config option after a
   security review of its semantics.
3. Send native diff content, tool kinds, tool locations, raw input/output, and
   terminal references where appropriate.
4. Send plans, commands, mode changes, configuration changes, and elicitation
   using their native updates.
5. Advertise prompt image/audio/resource capabilities only when the complete
   provider path supports them.
6. Decide whether client-managed ACP terminal/filesystem operations are
   compatible with Axiom's policy boundary. If not, document why execution
   remains internal while still exposing structured results.
7. Implement per-session MCP configuration only if it can pass through the
   existing policy and namespace controls without weakening them.

**Exit criteria:**

- Diffs are no longer plain text.
- Settings no longer require slash-command parsing for first-party control.
- Tool activity has stable IDs, kinds, statuses, locations, and structured
  content.
- Generic clients receive the best possible experience without `_axiom`.

### Stage 05 — Create the extension schema and Rust protocol crate

**Goal:** establish the minimal negotiated extension foundation.

**Work:**

1. Create `axiom-acp-extension` with capabilities, common envelopes, feature
   versions, state versions, safe errors, and size validation.
2. Implement checked-in schema generation and golden JSON fixtures.
3. Define JSON-RPC method/notification registries so method strings and DTOs do
   not drift.
4. Add redaction-aware `Debug` implementations for sensitive types.
5. Add initialize negotiation to AxiomCLI without enabling domain features yet.
6. Verify that non-Axiom clients see no unsolicited custom methods.

**Exit criteria:**

- Rust fixtures validate against the checked-in schema.
- Schema regeneration is deterministic and CI detects drift.
- Unsupported/unnegotiated extension calls fail safely.
- Logs and error formatting reveal no fixture secrets.

### Stage 06 — State snapshots, sequencing, and recovery

**Goal:** make a long-lived desktop UI recoverable instead of dependent on
perfect notification delivery.

**Work:**

1. Define global and per-session snapshot DTOs.
2. Add runtime instance IDs, event sequences, correlation IDs, and occurred-at
   timestamps to outward events.
3. Implement session state retrieval and resynchronization after gaps.
4. Specify active-turn and pending-interaction recovery after `session/load`.
5. Handle renderer reload independently of the AxiomCLI process.
6. Handle AxiomCLI restart by invalidating stale request IDs and rebuilding
   state from durable storage.

**Exit criteria:**

- Dropping an arbitrary notification causes detectable resynchronization, not
  silent state corruption.
- A renderer reload reconstructs the same authoritative state.
- Stale approval, question, and delete tokens cannot be reused after restart.

### Stage 07 — Session catalog, resume, titles, and safe deletion

**Goal:** expose the complete `/resume` and `/delete` semantics.

**Work:**

1. Implement paginated/searchable session summaries with stable IDs, titles,
   timestamps, cwd display identity, model, thinking, profile, and last outcome.
2. Use standard `session/load` for the actual resume operation.
3. Ensure resumed sessions restore model, thinking level, and permission profile
   exactly as the TUI does.
4. Implement two-step batch deletion with preview and expiring confirmation.
5. Reject deletion of active sessions or make the required close/cancel action
   explicit.
6. Send title changes generated after the first prompt as notifications.

**Exit criteria:**

- Desktop harness can list, search, resume, select multiple, select all from a
  resolved result set, preview deletion, confirm it, and observe durable removal.
- A forged, expired, or altered delete confirmation fails.
- TUI and ACP use the same catalog and deletion service.

### Stage 08 — Model, thinking, permissions, and defaults

**Goal:** expose all runtime settings without text command emulation.

**Work:**

1. Publish model choices and supported thinking levels as typed configuration.
2. Expose all permission profiles with stable IDs and user-facing descriptions.
3. Distinguish session settings from last-used global defaults.
4. Return authoritative accepted values and notifications after changes.
5. Reject changes that are unsafe during active work rather than racing them.
6. Verify settings are journaled and restored identically through TUI and ACP.

**Exit criteria:**

- No first-party client needs `/model`, `/thinking`, or `/permissions` text.
- New sessions use saved defaults; resumed sessions use their durable settings.
- Permission changes remain enforced in AxiomCLI.

### Stage 09 — Authentication and account lifecycle

**Goal:** reproduce the TUI account flow safely in desktop-native UI.

**Work:**

1. Implement current stable ACP authentication/logout semantics where possible.
2. Add negotiated browser login start, progress, cancellation, expiry, and
   completion state.
3. Add one-way API-key submission through the Electron main process.
4. Store and validate credentials only through `AuthManager`/system keyring.
5. Make invalid/revoked credentials return to the required sign-in state.
6. Ensure production launch can never enter unattested-development mode through
   logout/login or extension input.

**Exit criteria:**

- Browser and API-key flows work through a headless desktop client.
- Cancellation and window closure leave no hanging login task.
- API keys never appear in stdout, stderr, snapshots, fixtures, errors, or the
  renderer after submission.

### Stage 10 — Security and workload evidence

**Goal:** give the desktop the same security inspection capability as the TUI.

**Work:**

1. Expose structured unverified/verifying/verified/failed/development states.
2. Expose typed provider, model, attestation, encryption, verification time,
   workload identity, and bounded evidence claims.
3. Add fresh verification and cancellation operations with progress stages.
4. Separate summary, workload, and raw views.
5. Redact or omit claims unsuitable for a renderer.
6. Preserve automatic startup verification and visible failure behavior.

**Exit criteria:**

- Desktop harness observes startup verification and its terminal result.
- Attestation failure prevents inference and cannot be downgraded by the client.
- TUI and desktop render from the same verified evidence object.

### Stage 11 — Compaction and rich task activity

**Goal:** expose the complete task-running experience rather than a flattened
transcript.

**Work:**

1. Implement cancellable compaction lifecycle messages with truthful phases.
2. Map task lists, progress, background tasks, workspace changes, warnings, and
   terminal outcomes to structured state.
3. Preserve temporal placement of tool calls among response segments.
4. Add bounded output/diff pagination or artifact references for large data.
5. Make completion, cancellation, failure, and partial-output outcomes explicit.
6. Keep token accounting internal unless a later product decision exposes it.

**Exit criteria:**

- A desktop harness can reconstruct a complete ordered task timeline.
- Compaction never looks frozen and can be cancelled.
- Large tool output cannot exhaust the ACP queue or renderer memory.

### Stage 12 — Approvals, questions, and interaction races

**Goal:** reach full fidelity for all human-in-the-loop controls.

**Work:**

1. Use stable ACP permission and elicitation primitives.
2. Attach structured Axiom policy scope metadata only when standard fields are
   insufficient.
3. Expose allow-once, exact-session, safe-prefix-session, and deny choices with
   stable meanings.
4. Support required/optional, single/multiple-choice, and free-text questions.
5. Define one active owner for each interaction and deterministic behavior for
   cancel/answer/disconnect races.
6. Journal outcomes without persisting secrets or UI-only state.

**Exit criteria:**

- TUI and ACP decisions produce identical policy outcomes.
- Duplicate, stale, malformed, or reordered answers fail closed.
- Client disconnect while waiting cannot become implicit approval.

### Stage 13 — Generate the TypeScript client and wire Electron main/preload

**Goal:** provide the desktop developer a safe, ergonomic API rather than raw
JSON-RPC.

**Work:**

1. Generate TypeScript DTOs from the checked-in extension schema.
2. Build `axiom-acp-client` with subprocess supervision, ACP framing,
   capability negotiation, request cancellation, notification ordering, state
   reduction, and typed errors.
3. Spawn `axiomcli acp` only from Electron main.
4. Resolve the packaged sidecar path without relying on the shell or user PATH.
5. Route stdout exclusively to protocol parsing and stderr to bounded local
   diagnostics.
6. Expose a narrow context-isolated preload API grouped by session, settings,
   account, security, activity, approval, and question domains.
7. Validate every renderer argument again in main and in Rust.
8. Add sidecar crash/restart/backoff behavior and clean application shutdown.

**Exit criteria:**

- The renderer has no Node or child-process access.
- A mocked React consumer can perform every parity-matrix operation through the
  preload API.
- Sidecar crash, renderer reload, and application shutdown are deterministic.
- No raw ACP method escape hatch is exposed to the renderer.

### Stage 14 — End-to-end parity and compatibility suite

**Goal:** prove behavior rather than infer it from unit tests.

**Work:**

1. Build a deterministic fake Electron-main client using the production
   TypeScript package.
2. Run scenario tests for new session, streaming prompt, cancellation, tool
   approval, questions, edits, diffs, plans, compaction, model/thinking/profile
   changes, security inspection, login/logout, resume, and deletion.
3. Replay the same semantic scenario through the TUI harness and compare
   authoritative final state.
4. Test a generic standard-only ACP client and an Axiom-capable client.
5. Test version skew: older client/newer agent and newer client/older agent.
6. Test actual Electron main/preload/renderer boundaries without requiring the
   final visual implementation.

**Exit criteria:**

- Every parity-matrix row has deterministic automated evidence.
- Standard-only clients retain the documented baseline.
- Extension negotiation never makes standard behavior worse.
- The first-party desktop harness reaches full semantic TUI parity.

### Stage 15 — Adversarial hardening and performance

**Goal:** make the local protocol safe under hostile or pathological input.

**Work:**

1. Fuzz JSON-RPC framing, extension deserialization, size limits, unknown
   fields, invalid UTF-8 boundaries, and event ordering.
2. Property-test capability negotiation and schema round trips.
3. Test oversized prompts, tool output, diffs, evidence, and session catalogs.
4. Test cancellation at every await boundary and all approval/question races.
5. Test malicious renderer IPC, path traversal, forged session IDs, duplicate
   request IDs, stale tokens, and sidecar replacement assumptions.
6. Measure streaming latency, notification queue growth, snapshot time, catalog
   pagination, and memory during long agent runs.
7. Verify log redaction and perform a fixture secret scan.

**Exit criteria:**

- No unbounded protocol field or queue remains.
- No high-severity threat-model finding remains.
- Performance budgets are documented and met on the supported Linux target.
- All fuzz/property regressions have durable seeds or fixtures.

### Stage 16 — Documentation, migration, and release gate

**Goal:** make the feature maintainable by both Rust and desktop teams.

**Work:**

1. Finalize the compatibility matrix and extension reference.
2. Document how Electron locates, launches, negotiates, monitors, and shuts down
   AxiomCLI.
3. Document standard-client fallbacks and unsupported capabilities.
4. Document schema generation and the breaking-change procedure.
5. Add release notes and migration guidance for older desktop/CLI pairings.
6. Run a clean packaged desktop smoke test against the packaged AxiomCLI
   sidecar, including real secure inference at the release gate.

**Exit criteria:**

- A desktop developer can integrate without reading `tui.rs` or `acp.rs`.
- Packaged binary/version mismatch errors are understandable and recoverable.
- Clean-build, full test, security, and live-service gates pass.
- Final review explicitly signs off standard ACP compatibility and first-party
  desktop parity separately.

## 9. Required test layers

The completed implementation needs all of these layers:

1. **Domain unit tests:** command validation, service behavior, state reduction,
   redaction, and policy outcomes.
2. **Protocol unit tests:** every request, response, notification, error, and
   version negotiation path.
3. **Schema fixtures:** Rust serialization, JSON Schema validation, and generated
   TypeScript compatibility.
4. **ACP transcript tests:** exact JSON-RPC sequences, cancellation, malformed
   frames, duplicate IDs, and capability fallbacks.
5. **Parity tests:** identical semantic operations through TUI and desktop
   adapters result in identical authoritative state.
6. **Process tests:** real `axiomcli acp` subprocess over stdin/stdout.
7. **Electron boundary tests:** main/preload/renderer isolation and IPC
   validation.
8. **Persistence/recovery tests:** resume, title generation, defaults, interrupted
   work, restart, sequence gaps, and deletion.
9. **Security tests:** fail-closed approvals, authentication redaction,
   attestation failure, malicious renderer input, and no development downgrade.
10. **Live tests:** real Axiom login, attestation, encrypted inference, streaming,
    tool loop, cancellation, and resume at the designated release gate.

## 10. Definition of full TUI parity

The work is complete only when all of the following are true:

- The desktop can start, load, inspect, and close agent sessions.
- It can list, search, resume, title, and safely delete transcripts.
- It can select the model, explicit thinking level, and permission profile.
- It can submit long multiline prompts and receive ordered Markdown/reasoning
  streams.
- It can observe and cancel active inference and compaction.
- It can render tool calls at the point they occur, including structured output,
  diffs, locations, and terminal state.
- It can render plans, task lists, progress, background work, warnings, errors,
  cancellations, and completion.
- It can answer structured questions and resolve every approval scope supported
  by the runtime.
- It can complete browser login, submit an API key through main, inspect account
  state, and log out.
- It can observe automatic attestation verification, inspect workload evidence,
  request fresh verification, and clearly surface failure.
- It can recover state after renderer reload, session load, dropped notification,
  and supervised sidecar restart.
- A generic ACP client still receives a correct standard experience.
- No desktop code becomes responsible for enforcing tool, filesystem, security,
  or future financial permissions.

## 11. Deferred extension points

The initial contract should reserve versioned domains, not fake methods, for:

- wallet capability requests and revocation;
- deterministic transaction policy results;
- audit-event queries and export;
- subagent topology and delegation;
- remote workspace identity;
- additional secure inference providers;
- desktop-controlled MCP server configuration;
- proxy lifecycle status through its separate sidecar protocol.

These must use the same principles: standard protocol first, negotiated
Axiom-specific data second, and enforcement inside the trusted Rust runtime.

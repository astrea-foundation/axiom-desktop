# AxiomCLI TUI Implementation Plan

Status: **complete for the stated early-version Linux x86-64 GNU scope**. All
applicable Stage 00–15 gates pass; see the 2026-08-22 completion audit and stage
review records. Explicitly deferred and out-of-scope work remains deferred.

Product: `axiomcli`

Primary implementation language: Rust

Primary user interface: terminal user interface (TUI)

External control protocol: Agent Client Protocol (ACP)

## 1. Purpose

Build a dependable local coding agent named `axiomcli`. Its first-party user
experience is a Rust TUI. The same agent must also be controllable by an ACP
client without duplicating the agent loop or making the TUI the source of truth.

The goal of this plan is to build the system in reviewable vertical stages. No
stage may begin until the previous stage has passed its review gate. A failed
gate sends work back into the same stage; it does not create a follow-up task
for a later stage.

## 2. Hard scope boundary

### In scope

- A Rust executable named `axiomcli`.
- A keyboard-first TUI that fills the terminal's current viewport, adapts to
  terminal resizing, and leaves pointer input to native terminal selection. It never changes the
  operating system's window/fullscreen state.
- Interactive and non-interactive test entry points for the same agent runtime.
- An ACP server mode, initially over standard input/output.
- Streaming AxiomAI inference in an explicitly unattested development mode.
- Codebase inspection, safe file changes, commands, diffs, web research, MCP,
  task progress, permissions, persistence, and recovery.
- Deterministic policy checks outside the model.
- A pink/cherry visual identity appropriate for terminal capabilities.
- Automated tests capable of exercising agent runs without a human present.

### Out of scope

- Electron, browser, or desktop application code.
- Designing the future desktop application.
- Embedding a web renderer into the TUI.
- Wallets, transactions, Astrea, or financial permissions.
- Production TEE attestation in the first implementation.
- A hosted SearXNG deployment or other production infrastructure.
- Copying source code from any reference agent.
- Protocol extensions that are only useful to a hypothetical desktop client.

The repository may document how an ACP client observes and controls AxiomCLI,
but it must not contain that client's UI or lifecycle implementation.

## 3. Clean-room reference policy

Codex, Grok Build, OpenCode, Goose, Gemini CLI, Aider, mini-SWE-agent, and the
ACP examples are behavioral and architectural references only.

For every feature informed by a reference:

1. Record the user problem and the observable behavior being learned from.
2. Write an AxiomCLI-specific contract before implementation.
3. Implement from that contract using AxiomCLI names, types, state machines,
   error handling, tests, and visual language.
4. Do not paste, transliterate, mechanically port, or lightly rename reference
   implementation code unless it is ours (Axiom's)
5. Prefer public specifications and official protocol crates for standards such
   as ACP. A public dependency may be used after license and security review;
   its use does not justify copying an application's adapter code.
6. Preserve dependency licenses and notices for libraries actually consumed by
   AxiomCLI.
7. During review, search for suspiciously similar identifiers, comments, test
   text, and control flow. Any questionable implementation is rewritten.

Reference repositories must remain outside the AxiomCLI source tree and must
never be added as Git submodules or vendored source.

### 3.1 Reference repository inventory

These are the reference checkouts available when this plan was written. Commit
IDs record the inspected snapshot; the temporary directory may be deleted by the
operating system and is not a durable dependency of AxiomCLI.

| Repository | Local reference checkout | Inspected snapshot |
|---|---|---|
| [Axiom](https://github.com/astrea-foundation/axiom) | `/home/salt/clone/axiom` | `db9e498b82aa` |
| AxiomIO | `/home/salt/clone/axiomio` | `8ef49ef98a74` |
| [Agent Client Protocol](https://github.com/agentclientprotocol/agent-client-protocol) | `/tmp/axiom-agent-references.YVt8ZR/agent-client-protocol` | `a5ad78b82070` |
| [Aider](https://github.com/Aider-AI/aider) | `/tmp/axiom-agent-references.YVt8ZR/aider` | `5dc9490bb35f` |
| [Codex](https://github.com/openai/codex) | `/tmp/axiom-agent-references.YVt8ZR/codex` | `16e2722c50b7` |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | `/tmp/axiom-agent-references.YVt8ZR/gemini-cli` | `5411f113cafa` |
| [Goose](https://github.com/aaif-goose/goose) | `/tmp/axiom-agent-references.YVt8ZR/goose` | `45b322c1df30` |
| [Grok Build](https://github.com/xai-org/grok-build) | `/tmp/axiom-agent-references.YVt8ZR/grok-build` | `19d42e35c07a` |
| [mini-SWE-agent](https://github.com/SWE-agent/mini-swe-agent) | `/tmp/axiom-agent-references.YVt8ZR/mini-swe-agent` | `25941c89cfbc` |
| [OpenCode](https://github.com/anomalyco/opencode) | `/tmp/axiom-agent-references.YVt8ZR/opencode` | `1b937c860b6f` |
| [SearXNG](https://github.com/searxng/searxng) | `/tmp/axiom-searxng-reference` and reproducible `.cache/searxng/source` | `5f4005996e38` |

SearXNG was added during Stage 11. Its official source is used as a separately
running development service under the AGPL; no SearXNG source is copied or
linked into the AxiomCLI binary.

### 3.2 What AxiomCLI learns from each repository

#### Axiom

**Primary lessons:**

- Axiom's existing visual identity: near-black surfaces, cherry pink accents,
  restrained glow, sakura imagery, typography hierarchy, and security status.
- Product vocabulary for models, conversations, inference status, and explicit
  verification state.
- How successful security information can remain visible without overwhelming
  the normal interaction.
- Existing Axiom API concepts that should feel consistent when represented in a
  terminal.

**AxiomCLI interpretation:** carry the identity and terminology into a
terminal-capable theme, welcome view, status bar, and errors. Rebuild the
components for Ratatui rather than porting React components or CSS. The TUI
transcript does not need to imitate the website's chat bubbles.

**Do not inherit:** browser-specific state, React component structure, frontend
network orchestration, or assumptions that visual security status is itself a
security control.

#### AxiomIO

**Primary lessons:**

- Separation between inference providers, attestation, encryption, session
  transport, and presentation.
- The intended Axiom secure-session lifecycle and the points at which later TEE
  verification and E2EE will enter the system.
- Local OpenAI-compatible proxy behavior that may clarify existing API semantics,
  even though proxy implementation is not part of AxiomCLI's current scope.
- Error and event information needed to explain secure-transport failures.

**AxiomCLI interpretation:** define new provider and secure-transport interfaces
that preserve these boundaries while implementing only honest unattested
development inference initially.

**Do not inherit:** the existing application shell, UI framework, concrete type
layout, or a fake attestation implementation that reports success during
development.

#### Grok Build

**Primary lessons:**

- The main TUI interaction reference: a work transcript rather than chat bubbles,
  progressive folding, stable scrollback, terminal-viewport viewers, sparse
  semantic color, edge-aligned metadata, and a persistent composer/control area.
- Inline presentation of streamed text, tool calls, thinking/progress, diffs,
  terminal output, questions, and completion state.
- Plan review with line-level comments and revisions rather than a binary modal.
- Permission prompts that help users understand and narrow the requested scope.
- Background-task visibility and attention-oriented multi-session organization.
- Practical separation between a Rust TUI, agent runtime, tools, workspace, and
  ACP adapter.
- Theme degradation across terminal color capabilities.

**AxiomCLI interpretation:** use the same class of interaction problems as input
to AxiomCLI's own view contracts and state machines, then express them with
Axiom's cherry identity and terminology.

**Do not inherit:** Grok/XAI branding, command names, data types, shortcuts by
default, account/provider assumptions, or permissive “always approve” semantics.
Grok Build's notices identify some in-tree Codex and OpenCode ports, so apparent
overlap must be traced to its origin before deciding whether an idea is actually
distinctive. No implementation is copied from any layer.

#### Codex

**Primary lessons:**

- Large Rust workspace organization and dependency direction for an agentic CLI.
- An event-oriented agent loop, provider boundaries, tool-call lifecycle, and
  cancellation propagation.
- Filesystem tooling, patch application behavior, child-process management,
  output bounding, sandbox integration, and approval boundaries.
- Separating protocol messages from terminal presentation.
- Testing strategies for tool execution, policy-sensitive actions, and complex
  asynchronous behavior.
- Questions to ask when distinguishing local tools from provider-hosted
  capabilities such as search.

**AxiomCLI interpretation:** use Codex to enumerate failure modes and boundary
conditions, then create smaller Axiom-specific contracts and implementations.

**Do not inherit:** OpenAI authentication/provider assumptions, product-specific
protocols, internal naming, patches, prompt text, approval rules, or a workspace
layout merely because it exists at larger scale.

#### OpenCode

**Primary lessons:**

- Session, project, message-part, and tool-state modeling in a multi-provider
  coding agent.
- Representing streamed activity as structured parts rather than one mutable
  assistant string.
- Provider isolation, tool registration, configuration layering, and MCP-facing
  concepts.
- Terminal information architecture, navigation density, and ways to move
  between sessions and projects.
- Coordinating state across interactive and externally controlled modes.

**AxiomCLI interpretation:** compare its domain model against the AxiomCLI
command/event design and ensure AxiomCLI can represent partial and concurrent
work without coupling state to a renderer.

**Do not inherit:** its TypeScript architecture, web/desktop components, global
state choices, provider-specific compatibility behavior, styling, or directory
structure.

#### Goose

**Primary lessons:**

- A Rust agent architecture with strong MCP integration and provider extension
  points.
- Tool discovery, MCP server lifecycle, configuration, and tool provenance.
- ACP as a boundary between an agent process and a separate controlling client.
- Session recipes/configuration and the practical lifecycle of a long-running
  local agent process.
- Keeping protocol adapters distinct from the agent's core decisions.

**AxiomCLI interpretation:** use Goose to identify ACP/MCP lifecycle cases and
cross-process failure modes that need tests, while keeping the Axiom runtime and
event model authoritative.

**Do not inherit:** Goose's desktop UI, branding, recipes, provider types, ACP
adapter implementation, or an MCP trust decision based only on server metadata.

#### Gemini CLI

**Primary lessons:**

- Permission modes, policy rules, trusted-workspace concepts, and approval UX.
- Tool scheduling, cancellation, headless operation, and MCP integration.
- Configuration precedence and the distinction between user, project, and
  managed restrictions.
- Broad automated tests for interactive and non-interactive agent behavior.
- Handling tool confirmation consistently across front ends.

**AxiomCLI interpretation:** use its policy surface to build a threat-case matrix,
then implement AxiomCLI's own normalized effect system and permission profiles.

**Do not inherit:** Google authentication, telemetry, model/provider contracts,
TypeScript implementation patterns, exact policy names, or rules that are only
prompt instructions.

#### Aider

**Primary lessons:**

- Repository-map construction, relevance selection, and operation under a fixed
  context/token budget.
- Selecting files and ranges rather than sending an entire repository.
- Edit-format tradeoffs, failed-edit repair, diff presentation, and validation.
- Tight edit → lint/test → inspect failure → repair loops.
- Useful Git-aware workflows and user control over changed files.

**AxiomCLI interpretation:** define measurable context-selection and editing
requirements, create original fixture repositories, and compare strategies using
outcomes rather than copying prompts or algorithms.

**Do not inherit:** prompt templates, repository-map algorithms, edit parsers,
automatic Git commits, model-specific heuristics, or the assumption that Git is
always present.

#### mini-SWE-agent

**Primary lessons:**

- The minimum understandable agent loop and the value of a small environment
  interface.
- Evaluation-driven development with reproducible repository tasks.
- Scriptable/headless execution suitable for unattended testing.
- Keeping trajectories legible enough to diagnose why an agent failed.

**AxiomCLI interpretation:** use its simplicity as a check against accidental
complexity and build original deterministic task fixtures around AxiomCLI's
command/event model.

**Do not inherit:** its source, prompts, benchmark fixtures, unconstrained
execution assumptions, or minimal safety model. Simplicity does not override
AxiomCLI's permission and recovery requirements.

#### Agent Client Protocol

**Primary lessons and authority:**

- ACP's official schema, initialization, capability negotiation, sessions,
  prompting, streaming updates, permissions, cancellation, and error semantics.
- Expected JSON-RPC framing and compatibility behavior.
- Which concepts belong in the interoperable protocol and which should remain
  private AxiomCLI state.
- Conformance examples against which AxiomCLI can build original transcript
  tests.

**AxiomCLI interpretation:** ACP is a specification dependency, not a product UI
reference. Use the official schema or reviewed official Rust crate and write the
Axiom adapter around the core command/event model.

**Do not inherit:** sample-agent architecture as AxiomCLI architecture, example
names or fixtures, speculative extensions, or client-specific behavior outside
the negotiated standard.

### 3.3 Reference routing by implementation area

No stage should browse every reference repository. Research is routed narrowly
to avoid cargo-cult architecture and accidental imitation.

| Implementation area | Primary references | Specific question being answered |
|---|---|---|
| Product identity and TUI theme | Axiom, Grok Build | How should Axiom identity survive terminal constraints while keeping semantic states distinct? |
| Core event/session model | Codex, OpenCode, mini-SWE-agent | What states and failure cases must the original AxiomCLI command/event contract represent? |
| ACP interoperability | Agent Client Protocol, Goose, Grok Build | How does a controller create, drive, observe, approve, cancel, and close an agent session? |
| TUI scrollback and controls | Grok Build, OpenCode, Axiom | How can dense agent activity remain readable, navigable, and recognizably Axiom? |
| Axiom inference boundary | AxiomIO, Axiom, Codex | Where do provider, transport, security state, streaming, and agent orchestration separate? |
| Agent loop and tool lifecycle | Codex, mini-SWE-agent, Gemini CLI | How do we bound turns, validate tool calls, cancel work, and keep failures diagnosable? |
| Permissions and approvals | Gemini CLI, Codex, Grok Build | Which effects, scopes, precedence rules, and user explanations must AxiomCLI support? |
| Repository context | Aider, Codex, OpenCode | How do we select relevant code under a budget without reading or transmitting everything? |
| File editing and diffs | Aider, Codex, Grok Build | What conflicts, partial failures, repair cases, and review views must our own edit format handle? |
| Processes/background work | Codex, Grok Build, Goose | How should process trees, streaming output, cancellation, and background status behave? |
| Persistence and recovery | OpenCode, Grok Build, Codex | What must survive restart, and how are incomplete side effects represented without replay? |
| Web research UX | Grok Build, Codex; later SearXNG sources | How are search and local fetch distinguished, permissioned, bounded, and cited? |
| MCP | Goose, Gemini CLI, Codex | How are servers discovered, tools namespaced, effects authorized, and failures isolated? |
| Plan/questions/concurrency | Grok Build, Codex, Gemini CLI | How are plans reviewed and concurrent work kept attributable, bounded, and cancellable? |
| Automated evaluations | mini-SWE-agent, Aider, Codex | How do we create original repeatable tasks that measure outcome, safety, and efficiency? |

### 3.4 Required reference-study workflow

When a stage author consults reference code:

1. Start with the stage's AxiomCLI acceptance criteria and write the research
   questions before opening a reference repository.
2. Inspect only the repositories and subsystems routed to that question.
3. Record observations as behavior, invariants, tradeoffs, and failure cases;
   avoid retaining implementation-shaped pseudocode.
4. Close the reference material and write or revise the AxiomCLI contract.
5. Implement against that contract using AxiomCLI's domain model.
6. Create original tests from the contract, including cases where the chosen
   AxiomCLI behavior intentionally differs from the references.
7. During the stage gate, document which repositories informed the work and
   complete the clean-room similarity review.

Conflicting reference designs are treated as evidence of a tradeoff requiring an
AxiomCLI decision or ADR, not as a reason to combine every approach.

## 4. Product principles

1. **One runtime, multiple front ends.** The TUI, ACP, and test harness submit
   commands to the same runtime and consume the same event stream.
2. **The model proposes; Rust decides.** Tool arguments, paths, permissions,
   network access, and process execution are validated outside the model.
3. **Fail closed.** Ambiguous permissions, disconnected approval clients,
   invalid paths, malformed tool calls, and unavailable secure transport do not
   silently become allowed operations.
4. **Useful before clever.** Repository inspection, edits, tests, cancellation,
   and understandable diffs matter before subagents or elaborate automation.
5. **Observable behavior.** Every meaningful transition produces a typed event
   suitable for the TUI, ACP, audit log, and deterministic tests.
6. **Local authority.** AxiomCLI owns local files and processes. Remote model
   output never executes directly.
7. **Honest security state.** Development inference is visibly marked
   `Unattested`; the UI must never show `Secure` until verification exists.
8. **Bounded output.** Tool output, network content, model context, and terminal
   rendering all have explicit size and time limits.
9. **Recoverability.** Interrupted sessions preserve enough state to explain
   what completed, what did not, and whether the workspace changed.
10. **Terminal-native, not terminal-hostile.** Good keyboard navigation,
    readable density, Unicode fallbacks, reduced animation, and limited color
    assumptions are baseline requirements.

## 5. Target system architecture

```text
                         +----------------------+
                         |     Axiom API        |
                         +----------+-----------+
                                    |
                          InferenceProvider
                                    |
+-------------+    commands   +-----v----------------+    requests   +-------------+
| Axiom TUI   +-------------->|                      +-------------->| Tool runtime|
+------+------+               |  Agent/session core |               +-------------+
       |       events          |  (source of truth)  |                      |
       +<----------------------+                      |               +------v------+
                               +---+----------+-------+               | OS/workspace|
+-------------+    commands        |          |                       +-------------+
| ACP adapter +--------------------+          |
+------+------+       events                  |
       +<-------------------------------------+
                                               v
                                      Policy + audit journal
```

The architecture is intentionally centered on an application command bus and a
typed event stream. Rendering, JSON-RPC translation, storage, and model-provider
details are adapters around the core.

### 5.1 Initial crate strategy

Do not begin with a large collection of tiny crates. Start with explicit Rust
modules inside `apps/axiomcli`, enforce dependency directions, and extract crates
only when the boundaries have tests and at least two real consumers.

Expected modules:

```text
apps/axiomcli/src/
  main.rs                 CLI parsing and composition root only
  app/                    Runtime commands, events, IDs, state machine
  agent/                  Turn loop, context assembly, tool-call orchestration
  acp/                    ACP transport and protocol mapping
  tui/                    Terminal input, views, rendering, theme
  provider/               Inference traits and Axiom adapter
  tools/                  Tool specifications and implementations
  policy/                 Permission evaluation and approval lifecycle
  workspace/              Paths, edits, diffs, processes, repository metadata
  session/                Journal, snapshots, recovery, migrations
  web/                    Search and fetch provider implementations
  mcp/                    MCP connection and tool adapters
  audit/                  Redaction and user-readable security/action records
  config/                 Layered configuration and validation
  test_support/           Fake model, fake tools, fixtures, scripted runs
```

Likely extractions after the interfaces stabilize:

- `axiom-agent-core`: application, agent, and provider-neutral domain types.
- `axiom-acp`: ACP adapter and conformance harness.
- `axiom-tui`: Ratatui presentation.
- `axiom-policy`: effect classification and deterministic decisions.

Extraction is not itself a milestone. It occurs only when it reduces coupling.

### 5.2 Core command model

Front ends submit typed commands such as:

- `CreateSession`
- `ResumeSession`
- `SubmitPrompt`
- `CancelTurn`
- `FinishTurn`
- `FailTurn`
- `ChangePermissionProfile`
- `CloseSession`

Every command carries a session ID, origin, correlation ID, and cancellation
context. Commands that mutate state are idempotent where practical.

Approval decisions and structured answers are asynchronous responses to typed
`ApprovalHandler` and `QuestionHandler` requests, not independent kernel
commands. Their request and terminal outcome still pass through the shared
event journal, so the kernel can enforce one foreground interaction at a time
without letting a front end manufacture an unrelated answer.

### 5.3 Core event model

The core publishes versioned events such as:

- `SessionCreated`, `SessionResumed`, `SessionClosed`
- `PromptAccepted`, `TurnStarted`, `TurnCancelled`, `TurnCompleted`
- `TextDelta`, `ReasoningDelta`, `UsageUpdated`
- `ToolProposed`, `PermissionRequired` (normalized effect and offered choices),
  `PermissionResolved`
- `ToolStarted`, `ToolOutput`, `ToolCompleted`, `ToolFailed`
- `WorkspaceChanged`, `DiffAvailable`
- `QuestionsAsked`, `QuestionsAnswered`, `QuestionsFailed`
- `PlanProposed`, `PlanReviewed`
- `BackgroundTaskChanged`
- `ProviderStatusChanged`, `SecurityStatusChanged`
- `WarningRaised`, `ErrorRaised`

Events contain structured data, not preformatted terminal strings. Secrets and
unbounded raw output are prohibited. Each adapter decides how to render or map
an event while preserving its semantic identity.

### 5.4 State ownership

- The agent/session core owns the authoritative session state.
- The TUI owns only ephemeral view state: focus, scroll position, expansion,
  selection, and local composer contents.
- The ACP adapter owns connection state and request/response correlations, not
  agent state.
- Tools do not update session state directly; they return typed outcomes to the
  core.
- Persistence consumes committed events and produces recovery snapshots.
- Permission decisions are produced by the policy engine and cannot be
  overridden by presentation code or model text.

## 6. Target TUI experience

### 6.1 Screen anatomy

```text
project / branch        Unattested development       context usage
-------------------------------------------------------------------
scrollable work history
  user prompt
  assistant progress
  tool proposal / execution / output
  file changes and diffs
  questions and plan revisions
  final response
-------------------------------------------------------------------
active task, permission, question, or provider status
-------------------------------------------------------------------
composer                         model · permission profile
contextual shortcuts
```

The transcript behaves like an operational document, not a collection of chat
bubbles. Tool calls, reasoning summaries, command output, and diffs start
compact and expand in place. Terminal-viewport viewers are available for long
output and diffs; they fill only the space currently provided by the terminal.

### 6.2 Visual identity

Default dark palette:

- base: near-black neutral (`#0e0e10` target)
- surface: charcoal (`#18181b` target)
- primary accent: cherry (`#ff4d6d` target)
- active accent: bright cherry (`#ff6b85` target)
- restrained/depth accent: deep cherry (`#8a1f30` target)
- primary text: soft white
- secondary text: neutral gray

Pink is an identity color, not a replacement for semantics. Success/verified,
warning, error, diff addition, and diff deletion remain distinguishable without
color. Every status also has text and/or a glyph.

Cherry blossom motifs are limited to the welcome/empty view, tasteful loading
states, and small identity marks. Animation is sparse, configurable, and off
when reduced-motion/animation configuration is set. Active code and diff views
have no decorative background.

The renderer must support true color, 256-color, 16-color, and no-color modes,
with ASCII fallbacks for important Unicode glyphs.

### 6.3 Required interaction behaviors

- Keyboard-first navigation with discoverable context-sensitive shortcuts.
- Native terminal text selection and copying without application pointer capture.
- Stable selection when streamed content arrives above or below the viewport.
- A follow-output mode that stops when the user deliberately scrolls away.
- Search within transcript and tool output.
- Collapse, expand, and terminal-viewport actions for semantic transcript
  entries.
- Visible cancellation with immediate input feedback.
- Permission requests that retain the pending user draft.
- Resize-safe layouts down to a documented minimum terminal size.
- No raw model or tool ANSI escape sequence may reach the terminal renderer.

## 7. Permission model

Permission profiles are named presets over a rule engine. They are not separate
execution paths and they are not model instructions.

### 7.1 Initial profiles

| Profile | Automatic behavior | Requires approval or is denied |
|---|---|---|
| `observe` | Read files in declared roots, list/search, inspect repository metadata | Writes, processes, network, MCP side effects |
| `confirm` | Same safe reads as `observe` | Every write, process, network request, or externally visible action |
| `workspace` | Safe reads and edits inside declared roots; explicitly allowlisted build/test programs | Shell interpretation, new network destinations, destructive actions, scope expansion |
| `autonomous` | Operations allowed by explicit filesystem, command, and network rules | Hard-denied classes and any operation outside configured capabilities |

`autonomous` does not mean unrestricted. Hard policy remains active. A future
automation flag may reduce prompts, but it cannot disable hard denials.

### 7.2 Effect classification

Before execution, every tool request is normalized into effects:

- filesystem read/write/create/delete
- process spawn, shell interpretation, or background process
- repository mutation
- network connection and destination
- MCP invocation and declared side-effect level
- credential/keychain access
- scope change

Policy evaluates normalized, canonical resources—not the model's description of
the action. Decision precedence is:

```text
hard deny > configured deny > explicit ask > scoped grant > profile allow > ask
```

Approval choices must be narrow and understandable:

- deny once
- allow once
- allow this exact normalized operation for the session
- allow a user-selected safe prefix/scope for the session
- add a project rule through an explicit configuration flow

Persistent grants are never created by a single default keystroke. ACP approval
requests use the same normalized operation and choices as the TUI. If an ACP
client disconnects during approval, the operation is denied or remains paused
under a bounded timeout; it is never automatically approved.

## 8. Initial tool surface

### Repository and files

- list directory
- glob paths
- search file content
- read bounded file ranges
- inspect file metadata
- apply a structured patch
- create a file through the edit transaction system
- view the current session diff

### Processes and repository operations

- execute a program using explicit executable and argument fields
- execute a shell script as a separately classified, higher-risk tool
- stream bounded stdout/stderr
- start, inspect, wait for, and terminate background processes
- read Git status, diff, log, and repository metadata

Git mutations initially go through normal process permissions. There is no
special commit, push, reset, or checkout tool in the MVP.

### Agent interaction

- ask a structured user question
- update task/checklist state
- propose or revise a plan
- report progress

### Network

- search through a configurable `SearchProvider`
- fetch a public web document locally through a constrained `WebFetcher`

The first search adapter targets a configured SearXNG JSON endpoint. Axiom may
later provide the default hosted endpoint, while users can supply their own.
The search request is initiated by AxiomCLI; upstream search-engine traffic from
a hosted SearXNG instance originates at that host. Direct page retrieval is a
separate local tool so the UI and policy can state exactly where traffic runs.

Web fetching blocks private, loopback, link-local, multicast, and cloud metadata
targets by default; revalidates every redirect; limits bytes, duration, and
content types; and labels fetched text as untrusted data. Network tools cannot
read local credentials or silently transmit workspace content.

### MCP

- list and connect configured servers
- discover tools and schemas
- invoke tools through normalized permission effects
- expose server/tool provenance in the transcript and audit record

MCP tools are never considered safe merely because a server describes them as
read-only.

## 9. Axiom inference boundary

Define an `InferenceProvider` abstraction that supports:

- model discovery or configured model identifiers
- streaming assistant text
- structured tool calls
- usage/context reporting
- cancellation
- retry classification
- provider and transport status

The development Axiom adapter reads credentials from an environment variable or
approved OS secret-store adapter. Keys are never stored in repository config,
session events, prompts, crash dumps, or debug logs.

TEE support is deferred behind explicit interfaces such as `SecureTransport`
and `AttestationVerifier`. The development implementation reports
`UnattestedDevelopment`; it does not implement a verification stub that returns
success. Adding attestation later must not require changing the agent loop,
tools, TUI event types, or ACP session semantics.

## 10. ACP contract

The first ACP transport is JSON-RPC over stdio:

```text
axiomcli acp
```

ACP stdout is protocol-only. Diagnostics go to stderr or the structured log.
No terminal UI is initialized in ACP mode.

Initial compatibility target:

- initialize and advertise only implemented capabilities
- create/load a session rooted at a declared working directory
- submit prompts and stream updates
- map assistant content and tool activity to ACP session updates
- request client permission for gated operations
- propagate cancellation in both directions
- report terminal/process activity where the stable ACP schema supports it
- preserve stable session and tool-call IDs
- return structured protocol errors without terminating unrelated sessions
- cleanly shut down on EOF

A conformance matrix maps each ACP method and notification to:

- support status
- Axiom command/event mapping
- cancellation behavior
- error behavior
- test transcript
- protocol version

No custom ACP extension is introduced until the standard path is complete and a
written requirement cannot be represented by the protocol. Extension fields,
if ever needed, are namespaced and optional.

## 11. Persistence and audit design

Use a versioned local store under the operating system's standard application
data directory. The storage decision is made through an ADR after measuring the
first event model; SQLite with WAL is the default candidate.

Persist:

- session metadata and workspace identity
- committed user/assistant messages
- normalized tool requests and bounded/redacted outcomes
- permission requests and decisions
- file-change metadata and patch/diff records
- plan and task state
- provider/model identity and honest security state
- recovery markers for in-flight work

Do not persist:

- API keys, authorization headers, raw encryption material
- arbitrary environment-variable dumps
- unredacted command output known to contain secrets
- hidden reasoning that the provider does not explicitly expose for display

Large outputs are bounded and may be stored separately by content hash. Schema
migrations are forward-only, transactional, backed up before destructive
changes, and tested against fixtures from every released schema version.

On restart, any tool call that lacks a committed completion becomes
`interrupted/unknown`; it is never automatically replayed. The user sees the
workspace diff and decides what to do next.

## 12. Testing strategy

### 12.1 Deterministic test harness

A scripted fake inference provider emits text, tool calls, malformed calls,
retries, disconnects, and cancellation races. A fake clock and deterministic ID
source make event traces reproducible.

Fixture repositories are created in temporary directories. Each scenario states:

- initial files and repository state
- scripted model responses
- expected permission decisions
- expected tool effects
- expected ordered semantic events
- expected final diff and command result
- expected ACP transcript where applicable
- expected TUI snapshots at stable checkpoints

This harness is the main way to test agent behavior without a human or live
model. Live Axiom smoke tests are supplementary and never replace deterministic
tests.

### 12.2 Test layers

- unit tests for state transitions, normalization, and rendering helpers
- property tests for paths, rule precedence, truncation, and event ordering
- integration tests with temporary repositories and real local services when
  they can be started safely and reproducibly; protocol simulators are reserved
  for failures that cannot be produced safely on demand
- golden event-trace tests
- Ratatui `TestBackend` snapshots across terminal sizes and color modes
- pseudo-terminal tests for raw input, resize, cancellation, and clean exit
- ACP JSON-RPC transcript and malformed-client tests
- crash/restart recovery tests
- adversarial security tests for symlinks, traversal, redirects, ANSI/control
  sequences, output floods, shell injection, and secret redaction
- opt-in live provider smoke tests with strict cost and timeout limits
- platform CI on Linux, macOS, and Windows before release readiness

### 12.3 Quality commands

Every stage eventually runs the subset that exists at that point, converging on:

```text
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

Additional snapshot, PTY, fuzz, and live smoke suites have named commands in
`justfile` or `cargo xtask`; CI does not depend on undocumented shell sequences.

## 13. Mandatory stage-gate procedure

Every stage ends with a review record in `docs/reviews/stage-NN-*.md`. The next
stage remains untouched until that record is marked `PASS`.

### Required review passes

1. **Requirement review:** verify every stage acceptance criterion with linked
   evidence and identify scope that was accidentally added or omitted.
2. **Architecture review:** confirm dependency direction, state ownership,
   cancellation, and error behavior remain consistent with the core contracts.
3. **Behavior review:** run deterministic unit/integration/end-to-end scenarios
   and inspect user-visible errors, not only exit codes.
4. **Security review:** examine new trust boundaries, permissions, secrets,
   untrusted input, resource bounds, and fail-closed behavior.
5. **TUI/ACP review:** whenever affected, inspect terminal snapshots and ACP
   transcripts for equivalent semantics.
6. **Regression review:** run all tests from completed stages and compare
   performance/resource baselines where relevant.
7. **Clean-room review:** confirm reference code was not copied and record any
   new dependencies and licenses.
8. **Documentation review:** update contracts, ADRs, help output, and known
   limitations before declaring the stage complete.

### Gate outcomes

- `PASS`: all required criteria and reviews succeeded; next stage may begin.
- `FAIL`: remain in the current stage, fix the defects, and rerun the full gate.
- `BLOCKED`: stop and document the missing external decision or dependency.

`PASS WITH KNOWN CORRECTNESS ISSUE` is not an allowed state. Cosmetic or future
enhancement work may be deferred only when it cannot invalidate later
assumptions, and it must be written into the review record with an owner/stage.

Review should use a fresh-context pass where possible. The reviewer starts from
the stage contract and evidence rather than the implementation narrative.

### Review record template

Each stage review records:

- stage name, reviewed commit/range, date, and reviewer
- acceptance criterion → concrete evidence mapping
- exact verification commands and their results
- automated test totals and any platform-specific exclusions
- architecture/dependency-direction findings
- permission, secret, untrusted-input, and resource-bound findings
- TUI snapshots and ACP transcripts changed by the stage
- full regression result for all completed stages
- new dependencies, licenses, and clean-room review result
- performance measurements relevant to the stage
- unresolved items, each classified as blocking or safely deferred
- final `PASS`, `FAIL`, or `BLOCKED` decision with rationale

A `PASS` record is committed before work begins on the next stage. If later work
invalidates evidence from an earlier gate, that earlier gate is reopened and
must pass again before forward development resumes.

## 14. Staged implementation roadmap

### Stage 00 — Charter, threat model, and engineering baseline

**Objective:** Make the scope, trust boundaries, vocabulary, and quality process
executable before feature work begins.

**Work:**

- Adopt this plan and add contribution/build instructions.
- Record ADRs for the command/event kernel, async runtime, error taxonomy,
  configuration precedence, and initial crate/module boundaries.
- Create a data-flow threat model covering model output, ACP input, terminal
  input, tools, filesystem, child processes, network, storage, and secrets.
- Define domain vocabulary and stable identifiers.
- Establish formatting, linting, dependency audit, unit-test, and CI commands.
- Add a structured tracing policy with redaction and stdout/stderr rules.
- Define platform support and minimum terminal capabilities.
- Establish benchmarks for idle CPU, startup time, and baseline memory without
  choosing aggressive targets prematurely.

**Acceptance evidence:**

- Clean checkout passes baseline CI.
- Threat model includes abuse cases and explicit mitigations/deferred risks.
- ADRs resolve all decisions needed by Stage 01.
- No product behavior beyond a version/help command is introduced.

**Gate focus:** scope leakage, security assumptions, dependency hygiene, and
whether later stages have testable contracts.

---

### Stage 01 — Core command/event kernel

**Objective:** Create the source-of-truth state machine without TUI, ACP,
network, storage, or real tools.

**Work:**

- Implement typed IDs, commands, events, session/turn states, and transitions.
- Define ordered event publication, bounded queues, backpressure policy, and
  subscriber lifecycle.
- Implement cancellation tokens and terminal states for success, error, and
  cancellation.
- Add a scripted in-memory front end and fake clock/ID source.
- Define public error categories and user-safe error details.

**Acceptance evidence:**

- Transition-table tests cover every valid and invalid command/state pair.
- Property tests demonstrate no completed/cancelled turn returns to running.
- Slow/disconnected subscribers cannot deadlock the runtime.
- Replaying a golden command stream produces an identical semantic event trace.

**Gate focus:** race conditions, event ordering, ownership, cancellation, and
whether adapters can remain presentation-only.

---

### Stage 02 — ACP skeleton and conformance harness

**Objective:** Prove early that the core is externally controllable through ACP.

**Work:**

- Evaluate and pin the official ACP Rust schema/SDK or generate types from the
  public schema; record the choice in an ADR.
- Implement protocol-only stdio framing and lifecycle.
- Map initialize, session creation, prompt, streamed text updates, cancellation,
  and shutdown to Stage 01 commands/events using a fake agent.
- Create a protocol conformance matrix and transcript-based harness.
- Enforce stdout purity and bounded malformed-message handling.

**Acceptance evidence:**

- A test client creates a session, submits a prompt, observes streamed updates,
  cancels a turn, and exits cleanly.
- Invalid JSON-RPC, unknown methods, duplicate IDs, and EOF are tested.
- ACP and the in-memory front end produce equivalent core event semantics.
- No ACP-specific state leaks into the core.

**Gate focus:** standard compliance, transport isolation, cancellation races,
stdout contamination, and honest capability advertisement.

---

### Stage 03 — TUI foundation driven by fixtures

**Objective:** Build the terminal interaction shell before connecting a model.

**Work:**

- Add terminal setup/restore guards, input decoding, resize handling, and panic
  restoration.
- Implement the project/status header, semantic scrollback, composer, active
  status line, shortcut bar, overlays, and terminal-viewport viewer shell.
- Implement focus, scrolling, follow-output, entry expansion, selection, and
  transcript search.
- Add Axiom cherry theme, terminal color quantization, no-color/ASCII modes, and
  configurable animation.
- Render scripted Stage 01 events only; do not add agent logic to view code.

**Acceptance evidence:**

- Snapshot matrix covers minimum/typical/wide sizes, long paths, Unicode,
  no-color, 16/256/true-color, and every core event category.
- PTY tests verify clean restoration after quit, panic, cancellation, and resize.
- Streamed events do not steal scroll position after the user scrolls away.
- Raw ANSI/control characters in content cannot alter terminal state.

**Gate focus:** renderer purity, accessibility, information density, input loss,
terminal cleanup, and whether the visual hierarchy works without decoration.

---

### Stage 04 — Axiom inference adapter and streaming turns

**Objective:** Complete a text-only agent turn through the development Axiom
endpoint with explicit unattested status.

**Work:**

- Define provider-neutral request, stream, tool-call, usage, and error types.
- Implement the development Axiom adapter and bounded streaming parser.
- Load credentials through the approved secret configuration path.
- Add timeouts, cancellation, retry classification, jitter, and rate-limit
  handling without duplicating user prompts.
- Surface model, connection, and `UnattestedDevelopment` status in TUI and ACP.
- Add a real local HTTP streaming test service for ordinary integration paths;
  use deliberately malformed fixtures only for wire failures a conforming
  service cannot produce.

**Acceptance evidence:**

- TUI and ACP both stream a text-only turn from the mock provider.
- An opt-in live smoke test succeeds without printing or persisting credentials.
- Cancellation closes the provider request and completes the turn exactly once.
- Truncated streams, invalid payloads, rate limits, and disconnects produce clear
  recoverable errors.

**Gate focus:** secret leakage, retry duplication, stream correctness, provider
coupling, and honest security labeling.

---

### Stage 05 — Agent loop, context builder, and typed tool protocol

**Objective:** Let the model propose typed tool operations without executing real
side effects.

**Work:**

- Implement turn context assembly with explicit token/byte budgets.
- Add tool schemas, registry, argument validation, result envelopes, and output
  truncation contracts.
- Implement the model/tool continuation loop with maximum steps and repeated-call
  detection.
- Classify malformed/unknown tool calls and report repairable errors to the model.
- Add progress/task events and loop-level cancellation.
- Use fake tools only.

**Acceptance evidence:**

- Scripted runs cover zero, one, many, malformed, repeated, failed, and cancelled
  tool calls.
- The agent cannot exceed step, output, context, or wall-clock budgets.
- Tool results cannot impersonate system/developer instructions in context.
- TUI and ACP display the same tool identity, lifecycle, and outcome.

**Gate focus:** infinite loops, context poisoning, schema validation, budget
enforcement, and front-end semantic parity.

---

### Stage 06 — Policy kernel and approval lifecycle

**Objective:** Implement deterministic authorization before any mutating tool is
available.

**Work:**

- Implement effect normalization, permission profiles, rule precedence, scoped
  grants, and hard denials.
- Define global/project/session configuration precedence and validation.
- Add pending-approval lifecycle, timeout/disconnection behavior, and audit events.
- Build the TUI approval surface and ACP permission mapping.
- Add a policy explanation object suitable for both users and tests.
- Keep all real operations mocked.

**Acceptance evidence:**

- Table/property tests cover profile × effect × rule × scope combinations.
- `deny` wins under every ordering and source combination.
- ACP disconnect, TUI quit, cancellation, and timeout never approve an operation.
- Approval text identifies the actual normalized effect and resource.
- Grants cannot broaden themselves or change the active policy.

**Gate focus:** fail-closed paths, precedence ambiguities, confused-deputy risks,
grant scope, and understandable approval language.

---

### Stage 07 — Read-only workspace intelligence

**Objective:** Give the agent safe, bounded understanding of a repository.

**Work:**

- Implement workspace-root discovery and canonical path handling.
- Add bounded list, glob, search, ranged-read, metadata, and Git-inspection tools.
- Respect ignore rules while allowing explicit overrides through policy.
- Detect binary/large/generated files and return useful summaries.
- Add context selection that prefers relevant ranges rather than whole files.
- Defend against symlink escapes and time-of-check/time-of-use changes.

**Acceptance evidence:**

- The agent answers and plans changes in several purpose-built fixture repositories.
- Traversal, absolute-path escape, symlink swap, special files, huge files, and
  encoding edge cases fail safely.
- Search/read results are stable, bounded, and cite workspace-relative paths.
- `observe` mode cannot modify the fixture or spawn a process.

**Gate focus:** path safety, resource bounds, repository correctness, context
quality, and cross-platform path behavior.

---

### Stage 08 — Transactional edits and diff review

**Objective:** Allow understandable, recoverable file changes inside authorized
workspace roots.

**Work:**

- Implement AxiomCLI's own structured patch representation and parser.
- Validate expected file versions/hashes to prevent stale edits.
- Stage edits in memory/temp files, validate all targets, then commit atomically
  per operation where the platform permits.
- Preserve line endings and permissions deliberately.
- Produce semantic edit and diff events with bounded previews.
- Add TUI inline/expanded/terminal-viewport diff views and ACP content mapping.
- Track a session change set without automatically committing to Git.

**Acceptance evidence:**

- Golden tests cover create/update/delete intents, multi-file patches, conflicts,
  Unicode, newline styles, permissions, and rollback on partial failure.
- No target outside authorized roots changes under adversarial path tests.
- A failed edit leaves either the complete old state or documented recoverable
  state, never an unexplained partial patch.
- TUI and `git diff` agree on changed content.

**Gate focus:** data loss, atomicity, stale writes, symlinks, diff accuracy, and
whether users can understand every proposed/finished change.

---

### Stage 09 — Processes, tests, and background tasks

**Objective:** Execute authorized development commands with reliable streaming,
cancellation, and cleanup.

**Work:**

- Implement structured program execution before adding explicit shell execution.
- Add cwd/environment allowlists, timeouts, output caps, process-group control,
  and terminal escape sanitization.
- Add background start/list/wait/output/terminate lifecycle.
- Normalize command effects for Stage 06 policy.
- Show compact live tasks and expandable output in TUI and ACP.
- Ensure child processes do not inherit API credentials unless explicitly needed.

**Acceptance evidence:**

- Fixture agents edit code, run tests, interpret failure, repair, and rerun.
- Cancellation terminates process trees on every supported platform.
- Output floods, invalid bytes, ANSI sequences, hanging processes, and closed
  clients remain bounded and recoverable.
- Shell execution always appears as higher-risk than structured program execution.

**Gate focus:** command injection, child cleanup, credential inheritance,
platform differences, output backpressure, and background-state accuracy.

---

### Stage 10 — Session persistence, resume, and crash recovery

**Objective:** Make agent work durable without replaying unknown side effects.

**Work:**

- Finalize and implement the storage ADR, schema, migrations, and data paths.
- Journal committed events and create bounded snapshots.
- Add session list, resume, rename, archive, and inspect commands/views.
- Reconcile interrupted tool/turn states and display workspace changes on resume.
- Add retention, export, and redaction controls.
- Keep ACP session identity stable across supported resume flows.

**Acceptance evidence:**

- Kill/restart tests at every event boundary recover to a valid explainable state.
- In-flight side effects are marked unknown/interrupted and never auto-replayed.
- Migration fixtures upgrade transactionally from every test schema.
- Corrupted records produce bounded recovery diagnostics rather than data loss.
- Secrets are absent from a storage scan of adversarial sessions.

**Gate focus:** replay safety, schema durability, privacy, corruption behavior,
concurrency, and ACP/TUI identity consistency.

---

### Stage 11 — Web search and local fetch

**Objective:** Give the agent evidence-gathering tools with explicit network
provenance and policy.

**Work:**

- Provision and run a real local SearXNG server with an AxiomCLI-owned development
  configuration, then implement `SearchProvider` and its JSON adapter against
  that server. The service lifecycle must be scripted and reproducible rather
  than depending on a developer's unrelated SearXNG installation.
- Implement constrained local HTTP fetch with DNS/IP/redirect revalidation.
- Normalize URLs, deduplicate results, preserve citations, and bound extracted
  content.
- Show whether each operation used the configured search service or local fetch.
- Add destination-aware permission rules and audit data.
- Treat all retrieved content as untrusted and isolate it in model context.

**Acceptance evidence:**

- Integration tests start or connect to the real development SearXNG service,
  wait for its health check, issue real searches, validate result normalization,
  and stop it cleanly when the test owns the process.
- Empty results, disabled/unavailable engines, timeouts, and service shutdown are
  exercised through real SearXNG configuration and lifecycle controls. Static
  payloads may test parser corruption, but they cannot stand in for the normal
  integration path.
- SSRF suite blocks alternate IP encodings, DNS rebinding simulations, redirect
  pivots, local services, and cloud metadata endpoints.
- Agent research scenarios cite retrieved sources and never execute page text.
- Offline and denied-network behavior remains useful and understandable.

**Gate focus:** SSRF, privacy/provenance, prompt injection, rate limiting,
citation fidelity, content bounds, and hosted-vs-local wording.

---

### Stage 12 — MCP integration

**Objective:** Expose configured MCP tools without weakening AxiomCLI policy.

**Work:**

- Add server configuration, lifecycle, health, discovery, and reconnect behavior.
- Adapt MCP schemas/results into the typed tool protocol.
- Assign stable server/tool provenance and normalized effects.
- Route every call through policy regardless of server annotations.
- Bound, redact, and render MCP content and errors.
- Map MCP activity through ACP without confusing the two protocols.

**Acceptance evidence:**

- Mock MCP servers test discovery, schema changes, disconnects, malformed results,
  duplicate names, long output, and declared side effects.
- A server cannot bypass policy through naming or misleading annotations.
- Users can identify exactly which server handled a call.
- ACP and TUI observe identical MCP tool lifecycles.

**Gate focus:** protocol confusion, provenance, server trust, schema validation,
reconnect behavior, secret boundaries, and namespace collisions.

---

### Stage 13 — Plan mode, questions, and controlled concurrency

**Objective:** Support deliberate long tasks without hiding decisions or spawning
unbounded work.

**Work:**

- Implement a persisted plan artifact and explicit plan-state machine.
- In plan mode, enforce read-only workspace behavior through policy, not prompts.
- Add line/range plan comments, revision requests, approval, and abandonment.
- Add structured multi-question UI and ACP mappings.
- Add bounded concurrent background work first; introduce subagents only after a
  separate ADR defines identity, budgets, inherited capabilities, cancellation,
  and audit attribution.
- Make pending attention visible without taking over the composer.

**Acceptance evidence:**

- Plan-mode writes outside the plan artifact are rejected by deterministic policy.
- Comments and revisions survive restart and map cleanly through ACP.
- Concurrent tasks have independent IDs, budgets, output, cancellation, and
  permission provenance.
- Parent cancellation cannot leave uncontrolled child work.
- Attention ordering is deterministic and tested.

**Gate focus:** capability inheritance, concurrency races, plan enforcement,
attention UX, cancellation trees, and session recovery.

---

### Stage 14 — Integrated autonomous evaluations

**Objective:** Demonstrate that the complete system reliably solves coding tasks
without relying on manual testing.

**Work:**

- Build an original AxiomCLI evaluation set with small repositories spanning
  Rust, TypeScript, Python, configuration, and documentation tasks.
- Cover investigation-only, targeted edits, test repair, multi-file refactors,
  command failure, denied permissions, web research, MCP, cancellation, and resume.
- Score task completion, expected diff, tests, policy violations, tool efficiency,
  retries, latency, and context usage.
- Run deterministic scripted-provider evaluations on every change.
- Run opt-in live-model evaluations on a controlled schedule with budgets.
- Add soak tests for long streams, many transcript entries, and background tasks.

**Acceptance evidence:**

- Zero policy escapes or out-of-root modifications across the adversarial suite.
- All deterministic tasks pass repeatedly from clean fixtures.
- Live evaluation baseline and acceptable variance are documented; regressions
  block release candidates.
- TUI and ACP runs reach equivalent final workspace and semantic event outcomes.
- Resource usage remains within recorded startup/idle/long-session budgets.

**Gate focus:** outcome quality, nondeterminism, security regression, cross-front-
end equivalence, performance, and whether failures are diagnosable.

---

### Stage 15 — TUI refinement and release hardening

**Objective:** Produce a release candidate suitable for daily coding work and ACP
embedding.

**Work:**

- Conduct complete keyboard, native-selection, small-terminal, low-color, and accessibility
  passes.
- Refine density, sticky context, tool folding, diff navigation, task visibility,
  permission wording, help, onboarding, and error recovery.
- Finalize configuration reference, ACP compatibility matrix, privacy behavior,
  troubleshooting, and known limitations.
- Audit dependencies, licenses, packaging, updates, release signatures, and
  reproducibility expectations.
- Run platform installers/binaries in clean machines or containers.
- Define semantic versioning and storage/ACP compatibility policy.

**Acceptance evidence:**

- All prior stage gates rerun successfully from a clean checkout.
- A release candidate installs, starts, completes a fixture coding task, resumes,
  serves ACP, and uninstalls/cleans up predictably on supported platforms.
- No blocker/high-severity threat-model item remains unresolved.
- Help and documentation match observed behavior.
- Release artifacts contain the correct licenses/notices and no credentials.

**Gate focus:** complete-system regression, packaging, upgrade/recovery, security,
documentation truthfulness, and daily-use ergonomics.

## 15. Explicit sequencing rules

- ACP Stage 02 must pass before substantial TUI or agent behavior is added.
- Policy Stage 06 must pass before any mutating workspace/process/network tool
  becomes executable.
- Read-only workspace support must pass before edits.
- Transactional edits must pass before process-driven repair loops.
- Process cleanup must pass before background concurrency.
- Persistence must pass before plan state or subagent work is made durable.
- Web and MCP each receive independent security gates; neither is bundled into a
  generic “tools” milestone.
- Subagents are optional and cannot delay the first useful single-agent release.
- Production attestation is a future program with its own threat model and gates;
  the development label cannot be renamed into a secure claim.

## 16. Definition of MVP and release readiness

### Functional MVP

Stages 00 through 10 produce the first useful local coding agent:

- TUI and ACP entry points
- Axiom streaming inference in development mode
- repository inspection
- deterministic permissions
- transactional edits and diffs
- commands/tests and cancellation
- durable sessions and recovery

Web search, MCP, plan review, concurrency, broad evaluations, and release polish
remain staged additions rather than being smuggled into the MVP.

### Public release candidate

Requires all applicable stages through 15. A release can omit subagents if their
Stage 13 criteria are not met, but it may not partially ship them behind an
undocumented flag.

## 17. Questions that must be resolved through ADRs, not assumptions

- Exact development Axiom inference protocol and streaming/tool-call schema.
- Official ACP protocol version and Rust dependency choice at implementation time.
- Supported operating-system versions and terminal minimums.
- Storage backend after the Stage 01 event-volume measurement.
- Default SearXNG endpoint policy and authentication/rate-limit contract.
- Whether shell interpretation is enabled in the first MVP or deferred.
- Whether OS-level sandboxing is required for MVP on each platform.
- Which MCP transports are initially supported.
- Whether any live reasoning content is available and appropriate to display.
- Distribution/update/signing approach.

These questions do not authorize desktop application design or implementation.

## 18. First action when implementation is authorized

Start only Stage 00. Do not scaffold later modules or pull a large dependency set
“for later.” Complete its artifacts, run its review procedure, and create the
Stage 00 review record. Stage 01 begins only after that record is `PASS`.

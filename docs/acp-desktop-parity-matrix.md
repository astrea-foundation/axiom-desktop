# ACP desktop parity matrix

Reviewed against the working tree on 2026-08-24. `standard` means a generic ACP
client can use the feature. `extension` means the first-party client must
negotiate `_meta.axiom.protocolVersion = 0.1`.

## Commands and controls

| Semantic | TUI behavior | Standard ACP | Axiom extension / desktop API | Durable | Cancellation / sensitivity | Evidence |
|---|---|---|---|---|---|---|
| Create session | startup/new task | `session/new` with modes/config | `agent.newSession` + snapshot | yes | absolute canonical cwd | ACP transcript + TS client |
| Resume session | `/resume` picker | `session/load` and replay | paginated `_axiom/session/list`; `agent.loadSession` | yes | interrupted side effects remain unknown | durable ACP test |
| Delete sessions | `/delete`, multi-select, confirm | none | preview + exact single-use confirm | yes | active work rejected; 60 s token | replay/delete test |
| Submit prompt | composer | `session/prompt` | `agent.prompt` | yes | `session/cancel` and `$/cancel_request` | cancellation tests |
| Model | `/model` picker | `session/set_config_option` | model catalog + typed settings | yes | only catalog IDs; no active turn | settings tests |
| Thinking | `/thinking` | native config option | typed settings | yes | fixed enum; no `auto` | restore test |
| Permission profile | `/permissions` | native session mode | typed settings | yes | enforced in Rust, never renderer | restore/policy tests |
| Compaction | `/compact`, animation | cancellation + standard progress text | typed phase events + `compact` | summary is journaled | prompt tools disabled; cancellable | extension test |
| Help | `/help` overlay | available commands | renderer-owned help | no | no authority | transcript test |
| Login | `/login` menu | command fallback | browser start/complete/cancel; API-key submission | keyring | key never returned; cancellable | redaction/property tests |
| Logout/account | overlays/status | command fallback | typed account status/logout | keyring | renderer receives metadata only | extension/TS types |
| Security | `/security` summary/workload/raw | command fallback | typed verify/evidence/status | process state | failure blocks secure provider; evidence bounded | ACP + secure-client tests |

## Application events

| `AppEvent` family | TUI | Standard ACP mapping | Extension activity | Persistence |
|---|---|---|---|---|
| `SessionCreated`, `SessionResumed`, `SessionClosed` | task lifecycle | session responses/replay | snapshot lifecycle | journal |
| `PromptAccepted` | user card | prompt request/replay update | snapshot `hasPrompt` | journal |
| `TurnStarted`, `TurnCompleted`, `TurnCancelled`, `ErrorRaised` | task card/status | prompt stop reason and updates | lifecycle/error activity; catalog last outcome | journal |
| `TextDelta`, `ReasoningDelta` | ordered Markdown segments | agent message/thought chunks | standard is authoritative | journal |
| `UsageUpdated` | intentionally hidden | native usage update when available | not exposed | journal |
| `ProgressUpdated` | live task status | thought/progress fallback | structured progress | journal |
| `TaskListUpdated`, `PlanProposed`, `PlanReviewed` | checklist/plan | native ACP plan | structured task list where needed | journal/artifact |
| `ToolProposed`, `ToolStarted`, `ToolOutput`, `ToolCompleted` | in-place tool card | stable native tool call/update, kind, raw input/output | standard is authoritative | journal |
| `WorkspaceChanged` | change summary | tool locations/diff context | structured paths | journal |
| `DiffAvailable` | unified diff details | native ACP before/after file diff | standard is authoritative | journal |
| `PermissionRequired`, `PermissionResolved` | approval overlay | native permission request | policy details remain in normal request metadata | journal, no secrets |
| question events | question overlay | native form elicitation | standard is authoritative | outcomes journaled |
| `BackgroundTaskChanged` | task activity | thought fallback | structured background task | journal |
| setting-change events | header/status | mode/config/session-info updates | structured settings/title | journal |
| `ContextCompacted` | compacted context card | progress/text fallback | structured phases | journal |
| provider/security/warning events | header/status | bounded thought fallback | structured security/warning/error | security evidence is process state |

## TUI-only presentation state

`Entry`, `Help`, `ModelPicker`, `ResumePicker`, `DeletePicker`, `Security`, and
`Auth` overlays; composer buffers; selected rows; animations; scrolling; and
theme state remain presentation-owned. The desktop recreates these surfaces
from typed catalog, settings, account, security, activity, permission, and
elicitation APIs rather than transporting TUI widgets.

## Known deliberate limits

- ACP crate 2.0.0 is the newest published runtime and currently carries schema
  1.5.0; the separately published newer schema is not mixed into the runtime.
- Per-session ACP MCP server injection and client-owned terminals/filesystems
  remain disabled because execution must stay behind AxiomCLI policy.
- The existing desktop renderer remains a visual prototype. Electron
  main/preload and `@axiom/axiom-acp-client` now expose the functional API for
  the separate desktop UI implementation.

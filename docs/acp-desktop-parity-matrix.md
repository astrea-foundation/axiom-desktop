# ACP desktop parity matrix

Reviewed against the working tree on 2026-09-04. `standard` means a generic ACP
client can use the feature. `extension` means the first-party client must
negotiate `_meta.axiom.protocolVersion = 0.2`, including `account@2` and
`billing@1`.

## Commands and controls

| Semantic | TUI behavior | Standard ACP | Axiom extension / desktop API | Durable | Cancellation / sensitivity | Evidence |
|---|---|---|---|---|---|---|
| Create session | startup/new task | `session/new` with modes/config | `agent.newSession` + snapshot | yes | absolute canonical cwd | ACP transcript + TS client |
| Resume thread | `/resume` picker | `session/load` and replay | paginated `_axiom/thread/list`; `agent.loadChat` | yes | interrupted side effects remain unknown | durable ACP test |
| Delete sessions | `/delete`, multi-select, confirm | none | preview + exact single-use confirm | yes | active work rejected; 60 s token | replay/delete test |
| Submit prompt | composer | `session/prompt` | `agent.prompt` | yes | `session/cancel` and `$/cancel_request` | cancellation tests |
| Model | `/model` picker | `session/set_config_option` | model catalog + typed settings | yes | only catalog IDs; no active turn | settings tests |
| Thinking | `/thinking` | native config option | typed settings | yes | fixed enum; no `auto` | restore test |
| Permission profile / working directory | `/permissions`, launch cwd | native session mode for CLI | `desktopAgent@1`: atomic per-thread Agent enablement, approval level, directory | yes | idle-only; native validation; queued input bound to settings revision | workspace, restart, queue, policy and browser tests |
| Compaction | `/compact`, animation | cancellation + standard progress text | typed phase events + `compact` | summary is journaled | prompt tools disabled; cancellable | extension test |
| Help | `/help` overlay | available commands | renderer-owned help | no | no authority | transcript test |
| Login | `/login` menu | command fallback | native start/complete/cancel with passkey, Google, password, or Ethereum-wallet hint | OS credential store | no method credential or bearer token crosses ACP/IPC; cancellable | redaction/property tests |
| Logout/account | overlays/status | command fallback | typed account status/logout | OS credential store | renderer receives bounded metadata only; account transition closes local store | extension/TS types |
| Credit/invoices | insufficient-credit action | none | `billing@1` status and idempotent Shieldz invoice create/status | remote immutable ledger; process-local projection | native account only; bounded pending HTTPS payment URL; no wallet or transaction authority | extension/client/billing tests |
| Security | `/security` summary/workload/raw | command fallback | typed verify/evidence/status | process state | failure blocks secure provider; evidence bounded | ACP + secure-client tests |

## Application events

| Runtime/event family | TUI | Standard ACP mapping | Extension activity | Persistence |
|---|---|---|---|---|
| `SessionCreated`, `SessionResumed`, `SessionClosed` | task lifecycle | session responses/replay | thread catalog/timeline resync | thread tables |
| `PromptAccepted` | user card | user-message update with stable client item ID | no duplicate delivery | timeline |
| `TurnStarted`, `TurnCompleted`, `TurnCancelled`, `ErrorRaised` | task card/status | prompt stop reason and message updates | no duplicate delivery | turns/timeline |
| `TextDelta`, `ReasoningDelta` | ordered Markdown segments | agent message/thought chunks | standard is authoritative | journal |
| `UsageUpdated` | intentionally hidden | native usage update when available | not exposed | journal |
| `ProgressUpdated` | live task status | thought/progress update | no duplicate delivery | timeline |
| `TaskListUpdated`, `PlanProposed`, `PlanReviewed` | checklist/plan | native ACP plan | no duplicate delivery | timeline/artifact |
| `ToolProposed`, `ToolStarted`, `ToolOutput`, `ToolCompleted` | in-place tool card | stable native tool call/update, kind, raw input/output | standard is authoritative | journal |
| `WorkspaceChanged` | change summary | tool locations/diff context | structured paths | journal |
| `DiffAvailable` | unified diff details | native ACP before/after file diff | standard is authoritative | journal |
| `PermissionRequired`, `PermissionResolved` | approval overlay | native permission request | policy details remain in normal request metadata | journal, no secrets |
| question events | question overlay | native form elicitation | standard is authoritative | outcomes journaled |
| `BackgroundTaskChanged` | task activity | thought fallback | structured background task | journal |
| setting-change events | header/status | mode/config/session-info updates | profile preferences only | thread/profile tables |
| `BillingChanged`, `InvoiceChanged` | insufficient-credit action | none | typed account credit/invoice state | remote ledger; process state |
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
- Desktop's renderer consumes the typed main/preload API and account-scoped
  catalog. It never receives a raw-request escape hatch, native token, or manual
  API-key submission method.

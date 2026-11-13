# Axiom ACP extension 0.2

The extension adds first-party desktop controls that ACP v1 does not represent.
Clients negotiate it with `clientCapabilities._meta.axiom`; non-negotiating
clients receive no custom notifications and custom requests fail.
Each feature version is negotiated independently. Advertising protocol 0.2
with a feature version of `0` does not enable that feature. The desktop client
requires its core feature set and rejects an older or incomplete sidecar with
an actionable compatibility error. Additive features such as `steering@1` are
negotiated separately.

Protocol 0.2 raises the required `account` feature to version 2. Other feature
versions remain at 1. An otherwise compatible 0.x sidecar that advertises
`account@1` is therefore insufficient for current Desktop native sign-in.

Methods:

- `_axiom/desktop/bootstrap`
- `_axiom/thread/list`, `_axiom/thread/timeline`, `_axiom/thread/rename`
- `_axiom/thread/delete_preview`, `_axiom/thread/delete_confirm`
- `_axiom/models/list`
- `_axiom/profile/preferences`, `_axiom/profile/preferences/set`
- `_axiom/collection/list`, `_axiom/collection/create`,
  `_axiom/collection/rename`, `_axiom/collection/set_collapsed`,
  `_axiom/collection/move`, `_axiom/collection/delete`,
  `_axiom/collection/assign`
- `_axiom/account/status`, `_axiom/account/native_login_start`,
  `_axiom/account/native_login_complete`, `_axiom/account/native_login_cancel`,
  `_axiom/account/logout`
- `_axiom/billing/status`, `_axiom/billing/invoice_create`,
  `_axiom/billing/invoice_status`
- `_axiom/security/verify`
- `_axiom/compaction/start`
- `_axiom/turn/steer` (`steering@1`)

`_axiom/event` carries only Axiom-specific activity with `runtimeInstanceId`, a
monotonic `sequence`, RFC 3339 `occurredAt`, optional thread/correlation IDs,
and a tagged event. Standard ACP is the sole live owner for conversation,
reasoning, tools, plans, modes, and configuration. The extension must never
send a second presentation of those events.

Every thread summary and timeline page carries the authoritative durable
`revision` and `lastTimelineSequence`. `_axiom/thread/timeline` pages forward
from an exclusive sequence cursor. A sequence/revision gap or runtime change
requires fetching every page from the beginning and replacing the local
timeline; a metadata-only refresh is not recovery. Standard ACP notifications
may include `_meta.axiom` delivery identity, and Desktop prompts include a
stable `_meta.axiom.clientItemId` for pending-message reconciliation.

`steering@1` adds `_axiom/turn/steer` with
`{ threadId, expectedTurnId, clientItemId, text, webEnabled }`. It targets the
existing active turn, rejects stale IDs, and cannot change the turn's model or
Web permission. Text is bounded to 64 KiB; duplicate client IDs with identical
text join the existing submission instead of applying it twice. ID reuse with
different text fails. It waits asynchronously for the current model request,
tool call, or compaction to finish; the current provider stream is not canceled.
Unstarted tool calls are closed with explicit skipped results before continuing.

The response `{ turnId, clientItemId }` means the user message has been applied
at a safe boundary and committed to the account-local transcript, not that the
whole turn has ended. The applied message uses the standard ACP user-message
notification and a new durable row. Following deltas use their own segment IDs
within the same turn. `active_turn_changed` activity and timeline snapshots'
optional `activeTurnId` expose the exact target, including after missed events.
Disconnect/cancellation may make delivery uncertain: reconcile client IDs with
the authoritative local timeline before a user-driven retry. Never automatically
promote an uncertain submission into another turn. These are local IPC messages;
all subsequent inference remains native-constructed, attested provider E2EE.

Desktop also requires `webConsent@1` and sends `_meta.axiom.webEnabled` on every
prompt. Only explicit boolean `true` opts that message into external search and
page fetching; false or absent metadata leaves Web off, irrespective of thread
history. Malformed values are rejected. The native engine withholds Web tool
definitions and rejects attempted execution while off. Non-desktop CLI/TUI
behavior is unchanged. A remembered warning preference is renderer-local and is
never treated as a persistent native authorization.

Thread summaries and `_meta.axiom` delivery metadata also expose optional
`lastMessageAt`: the native, persisted UTC timestamp of the latest nonempty
user-message or assistant text/reasoning event. It is absent for message-free
threads and never advances for operational notices, settings, opening, renaming,
or collection changes. Clients must not substitute `updatedAt` or their local
arrival time. The catalog is ordered by this timestamp descending (empty last),
then thread ID ascending; its cursor follows the same ordering. Desktop applies
live message metadata immediately and ranks folders by the maximum timestamp
among all their member threads, independently of sidebar search.

The canonical schema is
`protocol/axiom-acp-extension/v0.2/schema.json`. Regenerate both language
artifacts after Rust DTO changes:

```text
cargo run -p axiom-acp-extension --example export_schema -- protocol/axiom-acp-extension/v0.2/schema.json
pnpm --filter @axiom/axiom-acp-client generate
```

`_axiom/thread/rename` takes `{ threadId, title }` and returns `{ thread }` with
the updated summary. It is gated by `threadCatalog`, uses the active account's
local store, trims and validates a nonempty title (at most 512 UTF-8 bytes), and
does not load the thread or change its last-message timestamp. Desktop exposes
Rename and Delete in the right-click menus on thread and folder rows; dialogs
stay inside the renderer so native OS prompts cannot disrupt composer focus.

The Rust schema drift test fails if the checked-in contract is stale. Generated
TypeScript is never edited manually.

`native_login_start` accepts only an optional method hint: `passkey`, `google`,
`password`, or `ethereum_wallet`. It returns a bounded login handle and hosted
authorization URL, never method credentials or a bearer token. Completion and
cancellation refer to that handle. Account snapshots expose bounded profile and
session metadata only. Passwords, OAuth codes, passkey assertions, wallet
signatures, access tokens, and refresh tokens must not appear in ACP, snapshots,
events, renderer IPC, errors, diagnostics, or debug output. Permission and
elicitation responses are standard ACP messages; the extension does not create
a second tool-authorization protocol.

The `billing@1` feature exposes account-ledger totals and idempotent Shieldz
Zcash invoice creation/status. Invoice amounts use integer USD cents; account
balances use integer microUSD. A pending invoice may include a bounded HTTPS
payment URL, but never a wallet key, transaction request, or payment authority.
Billing calls require the current native account session and discard results if
the account changes while a request is in flight. Revisions prevent an older
status or invoice response from replacing newer state.

Extension failures retain ordinary JSON-RPC codes and include a bounded typed
`data` object with `code`, `message`, `retryable`, and optional
`correlationId`. Unnegotiated feature calls return `unsupported_feature`;
clients must not infer behavior from presentation text.

Compatibility is negotiated at two levels. `protocolVersion` values in the
same leading-number family are compatible (for example `0.1` and `0.2`), while
each feature advertises the maximum schema version understood by that peer.
Missing feature keys mean version zero and unknown feature keys are ignored.
Desktop checks only the features required for its core chat and verification
experience; adding an optional capability does not turn it into a new minimum
sidecar requirement. Unknown durable timeline kinds are retained as generic
activity until the client learns their richer presentation.

Model catalog entries require stable model and provider IDs, display and short
labels, the exact upstream model name, supported thinking levels, the live
context/output token limits, and the effective automatic-compaction token
threshold (the derived provider threshold capped by the native runtime's configured
context limit). When live or stale provider pricing is available, entries also carry
paired `inputPriceMicrousdPerMillionTokens` and
`outputPriceMicrousdPerMillionTokens` integers derived from the exact per-token
prices used for billing. Both fields are absent together during a cold pricing
catalog outage. Missing or empty identity fields fail the request; clients
never infer a provider from a model ID, label, URL, or suffix.

`_axiom/thread/timeline` optionally includes a durable `contextUsage` report with
`inputTokens`, `outputTokens`, `modelId`, `reportedAt`, and optional pinned
`contextWindowTokens`/`autoCompactThresholdTokens`. The `context_usage_changed`
activity carries the same report as `usage` after each completed conversation
request, including intermediate tool rounds. These are replacement counts for
the last request, not cumulative usage or a measurement of the next request.

Schema v3 persists the report with its thread revision; timeline snapshots read
both atomically, even if the thread has not been loaded into the native engine.
No transcript rows, message activity timestamps, or billing changes are produced.
Tool results, edits, model changes, and automatic/manual compaction retain the
last report until another conversation request completes. Auxiliary title and
compaction usage cannot overwrite it. The model's pinned capacity prevents an
old report from being rescaled after a model switch. Clients reject the earlier
`usedTokens`/`estimated` shape rather than presenting it as provider-reported.
Legacy threads have no report until their next completed conversation request;
their existing turn totals and transcripts are preserved without inference or
fabricated backfill.

Verified security evidence requires a nonzero attestation generation and hard
expiry in addition to provider/model identity and key fingerprints. A sidecar
must fail verification rather than serialize an incomplete verified badge.

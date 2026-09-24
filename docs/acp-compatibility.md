# ACP and the Axiom extension

This is the current compatibility, extension and frontend-parity reference.
AxiomCLI targets **ACP protocol v1**, using the pinned `agent-client-protocol`
**2.0.0** runtime and its **1.5.0** schema. The local vendor patch fixes deep-handler
stack growth; these version numbers are independent of Axiom extension **0.2**.
No claim about the latest upstream release is needed to implement this contract.

Sources: [Rust adapter](../apps/axiomcli/src/acp.rs),
[extension DTOs](../crates/axiom-acp-extension/src/lib.rs),
[JSON Schema](../protocol/axiom-acp-extension/v0.2/schema.json), and
[Node client](../packages/axiom-acp-client/src/client.ts).

Desktop passes its product version to the client SDK's `initialize` call and
requires an exact match with `agentInfo.version` before reporting a connected
state or bootstrapping account/chat data. Missing or mismatched versions stop
startup with reinstall/rebuild guidance. This product pairing check is separate
from ACP protocol/extension negotiation; the SDK's argument is optional for
non-Desktop consumers.

## Standard ACP

| Operation | Implemented behavior |
|---|---|
| `initialize` | ACP v1, metadata, load-session support and optional Axiom negotiation |
| `session/new`, `session/load` | Canonical workspace, durable state and restored model/thinking/profile |
| `session/prompt` | Text/resource links, ordered user/assistant/reasoning chunks, tools and stop reason |
| `session/cancel`, `$/cancel_request` | Cooperative cancellation; supported long-running extension requests also cancel. A cancelled permission response also cancels its prompt, even if it arrives before the cancellation notification. |
| `session/set_mode` | CLI permission profile; Desktop Agent changes use the dedicated revisioned method |
| `session/set_config_option` | Model/thinking selection validated by native policy and the catalog |
| Permission requests | Only offered deny/once/eligible session-grant choices are valid |
| Form elicitation | Text/enum/multiple-choice questions when the unstable client capability is advertised |
| Plans, tools and diffs | Standard plan updates, stable tool IDs/states, bounded output and before/after diffs |
| Session info/commands | Local title updates and slash-command discovery |

Prompt images are supported and advertised. Image inputs require a model/protocol
with encrypted image support. File inputs use direct file metadata and original
bytes rather than extracted content. Audio, binary
embedded documents, client-owned terminals/filesystems and per-session MCP
injection are not advertised. Tools execute behind AxiomCLI policy. Unknown
standard methods receive normal JSON-RPC method errors.

## Extension negotiation

Clients advertise `clientCapabilities._meta.axiom`. AxiomCLI advertises its
extension at initialization but sends custom notifications and accepts custom
calls only after compatible negotiation. `protocolVersion` shares a leading-number
family; each named feature negotiates its own version. Missing/zero versions
mean unsupported. Desktop checks its required feature set and negotiates optional
features separately.

`account`, `timeline`, and `attachments` are version **2**;
`billing` is version **3** and `securityEvidence` is version **4**.
Other advertised features are version **1**:
`desktopChat`, `desktopAgent`, `threadCatalog`, `modelCatalog`,
`profilePreferences`, `collections`, `usage`,
`webConsent`, `steering`, `messageRevision`, `compaction`, and `activity`.
The source `FeatureVersions` and generated schema are authoritative.

`securityEvidence@4` carries verified hardware, key, freshness and provenance
evidence plus `outdated` (awaiting consent) and `degraded` (explicitly accepted)
states. Desktop requires this version before accepting verification reports.
`security/verify` accepts `acceptOutdatedTee: true` only with the current `modelId`;
the native runtime resolves the provider and remembers consent until restart.
Normal verification requests never grant consent. Degraded reports retain the
failed Intel `OutOfDate` posture check; other checks must still pass.

`security/prewarm` accepts only `modelId` and returns the same public report as
`security/verify`, without creating a thread or invalidating valid native keys.
It never grants outdated-TEE consent. Composition calls are coalesced by account
and model, with expiry-aware reuse and bounded failure cooldowns. Native provider
caches additionally enforce model identity, trust policy and expiry and share
work with inference. Account transitions cancel native warmups; stale reports
cannot replace evidence after account, runtime or model changes. Manual
`security/verify` still requests fresh attestation.


## Methods and ownership

All methods below are local control-plane operations. Their plaintext payloads
are never an instruction to send conversation plaintext through a hosted endpoint.

| Group | Method names following `_axiom/` |
|---|---|
| Desktop | `desktop/bootstrap`, `desktop/agent/configure` |
| Threads | `thread/list`, `thread/timeline`, `thread/attachments`, `thread/rename`, `thread/delete_preview`, `thread/delete_confirm` |
| Models/preferences | `models/list`, `profile/preferences`, `profile/preferences/set` |
| Collections | `collection/list`, `collection/create`, `collection/rename`, `collection/set_collapsed`, `collection/move`, `collection/delete`, `collection/assign` |
| Native account | `account/status`, `account/native_login_start`, `account/native_login_complete`, `account/native_login_cancel`, `account/logout` |
| Automation-key management | `account/api_keys`, `account/api_key_create`, `account/api_key_revoke` |
| Billing | `billing/status`, `billing/redeem_gift_code` |
| Usage | `usage/summary` (optional `period`: `week`/`month`/`all_time`, IANA `timezone`; defaults: all time/UTC) |
| Evidence/compaction/steering | `security/verify`, `security/prewarm`, `compaction/start`, `turn/steer` |

Permission and question responses remain standard ACP. Standard notifications
are the sole live representation of user/assistant/reasoning content, tools,
plans, modes and configuration. `_axiom/event` carries additional typed account,
security, context/accounting and activity state with `runtimeInstanceId`, monotonic
`sequence`, RFC 3339 `occurredAt`, and optional thread/correlation IDs.

## Durable timeline and recovery

Thread summaries/pages include `revision` and `lastTimelineSequence`. Timeline
pagination uses an exclusive sequence cursor. On gaps/runtime changes, fetch all
pages from the beginning and atomically replace the projection. Metadata-only
refresh is insufficient. Standard notifications can include `_meta.axiom`
delivery identity; stable prompt `clientItemId` values reconcile optimistic
messages with durable rows.

`lastMessageAt` describes nonempty user/assistant/reasoning activity.
`lastUserMessageAt` advances on user submissions and is the current catalog sort
key, descending with empty threads last and ID as tie-breaker. Clients must not
substitute metadata `updatedAt` or arrival time. Unknown timeline kinds remain
bounded generic activity until a client understands them.

Context usage is a replacement report from the last completed conversation
request, with input/output tokens, model, time and optional pinned capacity/
compaction threshold. It is persisted atomically with thread revision. It does
not estimate unsent drafts, sum billing, or get overwritten by compaction/title
work. Request-accounting reconciliation never marks partial output verified.

## Desktop authority and revisions

Bootstrap starts new Desktop threads with Agent off. `desktopAgent@1` atomically
configures enablement, approval level and directory with `expectedRevision`.
Changes require idle work; queued prompts/steering carry `agentRevision` to
reject stale authority. Generic mode setters cannot bypass this path. Desktop
keeps its embedded system prompt and does not start configured MCP servers.

Every Desktop prompt supplies `_meta.axiom.webEnabled`; only explicit boolean
`true` permits built-in Web tools. Missing consent is false, and malformed
values fail. Remembering a warning in the renderer is not native consent.

`messageRevision@1` adds `PromptMetadata.revision = { userItemId,
expectedRevision }` on standard `session/prompt`. The native store truncates
local history starting at the selected turn, retains accounting and executes a
normal E2EE run. It rejects active/stale/cross-account edits. This is the edit
and regenerate flow; it does not undo prior tool side effects.

`steering@1` takes `{ threadId, expectedTurnId, clientItemId, text, webEnabled }`
plus the applicable Agent revision. Text is limited to 64 KiB. It waits for a safe
boundary, skips unstarted tools and cannot change active-turn model/Web authority.
Identical IDs/text join an existing submission; conflicting ID reuse fails.
The response acknowledges application to the local transcript, not completion of
the whole turn. Uncertain delivery requires timeline reconciliation before retry.

`attachments@2` adds original inline file bytes to `_meta.axiom.attachments`:
`{kind: "file", name, file: {name, mimeType, data}}`. Images retain
`{kind: "image", name, image: {mimeType, data}}`. Data is canonical base64;
outer/inner names must agree. Count, size, format, role and model capabilities are
validated before committing a new prompt. New extracted-text uploads are rejected;
legacy text attachments remain readable in local history. Standard ACP embedded
resources are not advertised or converted to text.

Desktop requires attachment version 2 from its sidecar. This prevents a new
renderer from sending a file shape to an older process. Timeline metadata contains
only summaries; `thread/attachments` reads original inputs under the active account
guard. Revisions preserve attachments and recheck model support. Model discovery
adds `fileMimeTypes`; `supportsImages` remains separate. See [direct uploads](desktop.md#images-and-files).

## Accounts, billing and public evidence

Login-start accepts a method hint and returns a hosted URL and opaque handle.
Completion/cancellation use that handle. Native access/refresh tokens and browser
method credentials never appear in ACP/renderer state. API-key **creation** is an
explicit exception for a newly issued automation token shown once; account
snapshots and Debug output omit it. It is not a native-login credential.

`billing@2` carries account-ledger status with required trial/paid buckets,
nullable mainnet deposit metadata and a current ZEC/USD quote. Credit uses integer
microUSD; deposit/quote amounts use exact decimal strings. Calls require a current
native account and reject stale account/revision results. `usage@1` carries posted
spend by model.

`billing@3` adds private gift redemption. Its bounded `code` request returns
`creditedMicrousd`, `alreadyRedeemed` and `status`; only the updated billing status
is published as activity. Never log or persist the code. Desktop requires version
3 and carries its captured account ID through the IPC boundary. New sidecars still
serve `billing/status` to negotiated version 2 clients, but reject redemption until
version 3 is negotiated. Retrying the same gift code after an uncertain response
uses backend exact-once redemption; cancellation does not prove that credit failed.

Security projections require the registered provider identity, accepted key
fingerprints and hard expiry. NEAR require positive lease generation;
Tinfoil intentionally omits it. The projection must preserve router-versus-worker
and observed-versus-approved distinctions from [security](threat-model.md).

## Process bounds and contract generation

ACP is newline-delimited JSON-RPC. Stdout is protocol-only. The Node client bounds
frames to 64 MiB and retained stderr to 64 KiB, honors stdin backpressure, rejects
pending work on process exit, and never exposes raw method dispatch to the renderer.
Typed extension failures include a bounded code/message/retryability object;
clients must not infer authority or retry behavior from display text.

```sh
pnpm generate:protocol
pnpm --filter @axiom/axiom-acp-client typecheck
pnpm --filter @axiom/axiom-acp-client test
```

The generator exports Rust DTOs to JSON Schema and TypeScript; never hand-edit
generated TypeScript. Build `cargo build --locked -p axiomcli` first for real-process
client tests. The Rust schema-drift and ACP transcript tests, Node client/state tests
and Desktop IPC/browser tests cover the same semantic contract.

## Local history pagination

Timeline v2 accepts independent `afterSequence` and `afterRequestId` cursors and
returns `nextCursor` and `nextRequestUsageCursor`. Continue until both are absent,
keeping each finished stream's last cursor while reading the other. Request usage
is ordered by request ID, limited to 100 records and 512 KiB per page. Timeline
items are limited by row count and 32 MiB of serialized JSON (including escaping);
the final wire response is also checked below the 64 MiB transport ceiling.
Oversized individual records fail explicitly without truncating stored content.
Only the active turn is queried for a wire page. Accounting recovery runs before
the first page, at most 100 pending requests per refresh, to avoid changing the
snapshot revision during pagination. Desktop validates cursor progress and
assembles both streams atomically at one revision and account generation.
These exchanges use the native local pipe and never upload conversation history.

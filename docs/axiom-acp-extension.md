# Axiom ACP extension 0.1

The extension adds first-party desktop controls that ACP v1 does not represent.
Clients negotiate it with `clientCapabilities._meta.axiom`; non-negotiating
clients receive no custom notifications and custom requests fail.
Each feature version is negotiated independently. Advertising protocol 0.1
with a feature version of `0` does not enable that feature. The desktop client
requires every feature below and rejects an older or incomplete sidecar with an
actionable compatibility error.

Methods:

- `_axiom/desktop/bootstrap`
- `_axiom/thread/list`, `_axiom/thread/timeline`
- `_axiom/thread/delete_preview`, `_axiom/thread/delete_confirm`
- `_axiom/models/list`
- `_axiom/profile/preferences`, `_axiom/profile/preferences/set`
- `_axiom/collection/list`, `_axiom/collection/create`,
  `_axiom/collection/rename`, `_axiom/collection/set_collapsed`,
  `_axiom/collection/move`, `_axiom/collection/delete`,
  `_axiom/collection/assign`
- `_axiom/account/status`, `_axiom/account/browser_login_start`,
  `_axiom/account/browser_login_complete`, `_axiom/account/use_api_key`,
  `_axiom/account/logout`
- `_axiom/security/verify`
- `_axiom/compaction/start`

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
stable `_meta.axiom.clientItemId` for optimistic-message reconciliation.

The canonical schema is
`protocol/axiom-acp-extension/v0.1/schema.json`. Regenerate both language
artifacts after Rust DTO changes:

```text
cargo run -p axiom-acp-extension --example export_schema -- protocol/axiom-acp-extension/v0.1/schema.json
pnpm --filter @axiom/axiom-acp-client generate
```

The Rust schema drift test fails if the checked-in contract is stale. Generated
TypeScript is never edited manually.

Account API keys are one-way input. They must not appear in snapshots, events,
errors, diagnostics, or debug output. Permission and elicitation responses are
standard ACP messages; the extension does not create a second authorization
protocol.

Extension failures retain ordinary JSON-RPC codes and include a bounded typed
`data` object with `code`, `message`, `retryable`, and optional
`correlationId`. Unnegotiated feature calls return `unsupported_feature`;
clients must not infer behavior from presentation text.

Model catalog entries require stable model and provider IDs, display and short
labels, the exact upstream model name, and supported thinking levels. Missing
or empty identity fields fail the request; clients never infer a provider from
a model ID, label, URL, or suffix.

Verified security evidence requires a nonzero attestation generation and hard
expiry in addition to provider/model identity and key fingerprints. A sidecar
must fail verification rather than serialize an incomplete verified badge.

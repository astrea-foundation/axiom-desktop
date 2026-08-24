# Axiom ACP extension 0.1

The extension adds first-party desktop controls that ACP v1 does not represent.
Clients negotiate it with `clientCapabilities._meta.axiom`; non-negotiating
clients receive no custom notifications and custom requests fail.
Each feature version is negotiated independently. Advertising protocol 0.1
with a feature version of `0` does not enable that feature. The desktop client
requires every feature below and rejects an older or incomplete sidecar with an
actionable compatibility error.

Methods:

- `_axiom/session/list`, `_axiom/session/get_state`
- `_axiom/session/delete_preview`, `_axiom/session/delete_confirm`
- `_axiom/models/list`, `_axiom/session/set_settings`
- `_axiom/account/status`, `_axiom/account/browser_login_start`,
  `_axiom/account/browser_login_complete`, `_axiom/account/use_api_key`,
  `_axiom/account/logout`
- `_axiom/security/verify`
- `_axiom/compaction/start`

`_axiom/event` carries typed activity with `runtimeInstanceId`, a monotonic
`sequence`, RFC 3339 `occurredAt`, optional session/correlation IDs, and a tagged
event. On a sequence gap or runtime change, retrieve session state rather than
guessing what was missed.

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

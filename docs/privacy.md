# Privacy behavior

## Inference

Desktop, TUI, CLI and the local proxy use the same registered provider-E2EE
client. Model messages, system instructions, history and tool definitions/results
are constructed and encrypted on the device. Recipient keys are accepted only
from current, locally verified TEE evidence. Responses are authenticated to the
accepted environment and request. Ordinary TLS is not an inference fallback.

The backend sees authentication, routing/provider/model identifiers, ciphertext,
public protocol metadata, aggregate usage, accounting and terminal status. It
must not receive model-message plaintext, assistant output or tool-message secrets.
Provider-specific verification and its limits are documented in
[security](threat-model.md#provider-contracts).

Authenticated streaming deltas can appear provisionally. A missing or invalid
terminal authentication fails the response; partial output may remain locally
as incomplete history. Later accounting reconciliation cannot upgrade it to a
verified reply. NEAR/Phala use receipts; Tinfoil uses request-bound AEAD and
an encrypted authenticated terminal marker, without claiming a signed receipt.

## Web tools and local integrations

Web starts off for a new conversation. Opting in permits search queries and URL
fetches to leave the device outside provider E2EE. Default hosted search sends
only a bounded query to Axiom's authenticated search endpoint and Decodo. The
hosted contract prohibits retaining/logging query or result text. Explicit
SearXNG configuration sends searches to that service instead. Page fetching
runs in the native client with public-address/redirect checks. Results are
locally encrypted when included in later inference.

The [local proxy](proxy.md) receives plaintext from authenticated loopback
clients and returns plaintext to them. A process with its local token can use
that interface; its upstream Axiom credential is separate.

Local agent tools can read files and execute approved operations. On Linux,
process tools require Bubblewrap. macOS/Windows process execution does not
provide equivalent OS isolation. The Web toggle controls Axiom's Web tools,
not networking by host-authority programs on those platforms. A configured MCP
executable is trusted local code at startup; its results and authority claims
remain untrusted. See [security](threat-model.md#local-tools-and-processes).

## Update checks

Installed Desktop and TUI processes request the public
`https://axiom.stream/api/releases/latest` feed at startup and every six hours;
`axiomcli update` checks on demand. These are ordinary HTTPS metadata requests,
not inference. They contain no account headers, cookies, API keys, conversation
content, or installed-version query parameters. The website/CDN still observes
normal network metadata such as the source IP. Update notices remain UI-only.

## Local data and credentials

Conversations live in account/frontend-specific SQLite databases. Desktop also
stores account-scoped drafts, queues and Web preferences locally. They are not
synchronized through a plaintext backend cache. AxiomCLI does not encrypt its
conversation database at rest; OS-account and disk protections apply. Sensitive
content can also appear in workspace files, exports and the system clipboard.
Signing out closes the store but does not erase all local history or workspace files.

Native method credentials remain in the browser/hosted account flow. Access
tokens remain in AxiomCLI memory; rotating refresh tokens require the OS credential
store, with no plaintext-file fallback. Desktop's normal sign-in does not accept
a manually entered automation key. Its separate API-key management UI intentionally
shows a newly created automation key once for copying; it is not the native
session token and must not be retained in account snapshots or diagnostics.

Account billing and payment-provider credentials remain backend-owned. Desktop
receives bounded credit/deposit metadata and an indicative current ZEC/USD quote. It never receives wallet custody or grants the model
transaction authority. Currency valuation is distinct from a currency conversion.

[Session commands](cli.md#saved-sessions-and-plans) support explicit export and
archiving; TUI/ACP provide confirmed deletion. Uninstall preserves local account
data and credential-store entries. See [configuration](configuration.md#paths-and-local-state)
for paths and the account-scoped reset command.

## Local attachments

Desktop extracts UTF-8/PDF text locally and keeps attachment drafts and queued
payloads in account-scoped IndexedDB. Accepted inputs are also stored in the native
account-local conversation database for resume, inspection and regeneration. File
contents, filenames included in prompts, and image bytes reach the provider only
inside its authenticated E2EE request; hosted Axiom services do not parse or extract
files. These local stores are not an additional at-rest encryption boundary.
PDF extraction includes text only; scan pages and illustrations require images.

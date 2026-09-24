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
verified reply. NEAR uses receipts; Tinfoil uses request-bound AEAD and
an encrypted authenticated terminal marker, without claiming a signed receipt.

Cached attestation reports and reply authentication have separate lifetimes.
Report expiry prevents reuse of stale evidence for new requests; it does not
by itself fail an ongoing reply or revoke a reply's authenticated completion.
Each ongoing response still requires its protocol's authentication against the
environment and request accepted at its start. See the [Desktop privacy badge](desktop.md#usage-and-verification)
for the separate reply and report displays.

## Web tools and local integrations

Web starts off for a new conversation. Opting in permits search queries and URL
fetches to leave the device outside provider E2EE. Hosted search sends
only a bounded query to Axiom's authenticated search endpoint and Decodo. The
hosted contract prohibits retaining/logging query or result text. Page fetching
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
normal network metadata such as the source IP. Update notices remain UI-only. Selecting Update downloads the signed installer
from the CDN with the same anonymous transport. Private local staging contains
installer bytes, signed release metadata, installation paths and a restart
workspace/session identifier; it contains no credentials or message text. The
helper verifies the signature and bytes before executing the installer. Public
GitHub links offer identical installers without embedding GitHub credentials.

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

Gift codes are bearer secrets entered in Desktop's Balance form or the TUI's
masked `/redeem` screen. The native client sends the code to Axiom's authenticated
HTTPS account API, outside provider-E2EE inference. The backend stores only a code
digest and financial records. Clients do not place codes in conversations, saved
drafts, activity events or diagnostics. The operator's private issuance file is
the deliberate exception: it contains the generated code for delivery.

[Session commands](cli.md#saved-sessions-and-plans) support explicit export and
archiving; TUI/ACP provide confirmed deletion. Uninstall preserves local account
data and credential-store entries. See [configuration](configuration.md#paths-and-local-state)
for paths and the account-scoped reset command.

## Local attachments

Desktop reads original file bytes and keeps attachment drafts and queued payloads
in account-scoped IndexedDB. Accepted inputs are also stored in the native
account-local conversation database for resume, inspection and regeneration.
No local PDF/text extraction or OCR is performed. Original bytes and filenames
reach the selected provider only inside its attested, authenticated E2EE request;
Tinfoil's attested services handle document processing. Hosted Axiom services
cannot parse files or see extracted content. These local stores are not an additional at-rest encryption boundary.
Upload availability follows the current model capabilities; see [Desktop](desktop.md#images-and-files).

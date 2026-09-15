# Privacy behavior

AxiomCLI's default build performs provider attestation and end-to-end
encryption directly in-process through `axiom-secure-client`; it does not route
inference through the local proxy. It reuses only bounded, locally verified
public evidence and never beyond provider evidence, backend lease, signed-policy,
or local cache expiry. Fresh client secrets, nonces, ciphertext, and receipt
bindings are created for every inference. Secret-bearing fields are encrypted
only after the selected offering's exact provider/protocol tuple verifies. NEAR verification
binds the nonce, selected model, signing key, Intel and NVIDIA evidence, and live
TLS SPKI. Phala ACI v2 verification binds the nonce-quoted gateway, measured
workflow and keyset, source-transparency values, and content-addressed eligible
worker sessions. An attestation, binding, encryption, receipt, or decryption
failure stops the request without a plaintext or TLS-only fallback.

Phala completion ciphertext is not accepted merely because it decrypts.
Non-streaming output remains untrusted until its signed receipt binds the exact
local request hash, exact encrypted response bytes, model, and a preverified
worker session. AEAD-authenticated streaming text is displayed immediately and
stored locally as provisional output while the complete transcript is retained
within bounded memory for final receipt verification. Partial replies remain
available in conversation context after interruption, with an incomplete-turn
notice; this does not claim that a final receipt was verified. Tool execution
and successful completion still require terminal verification. Missing, invalid,
reordered, truncated, or mismatched receipts fail the turn and never mark its
partial output as receipt-verified.

The Axiom relay necessarily sees authentication, billing, routing, provider and
model identifiers, public client key material, ciphertext, bounded inference
parameters, usage, and terminal status. It must not see plaintext prompts,
conversation content, reasoning, refusals, model output, tool names,
descriptions, schemas, arguments, or results. The signed policy currently
observes workload identity while production measurements and Phala commits are
collected for an enforce-mode rollout. Evidence explicitly reports whether an
allowlist was configured and whether the quote-bound identity was approved;
observation is never presented as approval.

Web search is a separate, explicitly non-E2EE tool service. By default, the
native client sends only the model-selected search query to the authenticated
Axiom `/api/v1/search` endpoint, which sends it to Decodo. Both services can see
that query; do not include secrets in searches. Axiom does not log, cache, or
persist queries/results and stores only account quota timestamps. The client
receives the results locally and encrypts them for subsequent model inference.
No full transcript or backend-generated model message enters the search path.
Selecting `web_search_provider = "searxng"` instead shares queries with that
configured service. `fetch_url` still fetches public pages from the native client.

`axiom-proxy` accepts plaintext OpenAI-compatible requests from authenticated
clients on loopback, then uses the same secure client for the remote hop. A
local process holding `AXIOM_PROXY_TOKEN` can submit and receive that plaintext;
the operating-system process boundary and careful token distribution are
therefore part of the local trust model. The sidecar never gives clients
`AXIOM_API_KEY`, never executes model-proposed tools, and rejects non-loopback
bind addresses. It emits metadata-only lifecycle records to stderr and a
bounded readiness object to stdout.

Local reads, edits, Git inspection, and process execution happen on the user’s
machine. On supported Linux builds, child commands run through Bubblewrap with
the workspace writable, system directories read-only, a new network namespace,
temporary home directories, an explicit environment, and process-group
cancellation. There is no silent unsandboxed fallback when Bubblewrap is
missing.

`web_search` contacts the configured search service. Search terms and ordinary
HTTP metadata are therefore visible to that service and to whatever upstream
engines it uses. `fetch_url` is performed locally, with local/private/metadata
destinations and redirect pivots rejected. A configured MCP executable is
trusted user-selected local code and starts with that user's host authority;
therefore MCP commands are accepted only from trusted user configuration, not
repository configuration. Once connected, server descriptions, schemas,
results, and side-effect claims remain untrusted. Tool calls receive only their
explicit protocol arguments from AxiomCLI.

Session events are stored in an account-scoped local SQLite database under the
platform data directory. AxiomCLI redacts known secret shapes before
persistence, bounds events/exports, and supports explicit export and retention
commands. Redaction is defense in depth, not permission to paste secrets into
prompts. Session data is not encrypted at rest by AxiomCLI; rely on
operating-system account and disk-encryption protections.

`axiomcli sessions prune` can delete archived sessions under an explicit
retention policy. Package uninstall intentionally preserves user configuration
and session data. Remove the config/data paths printed by `axiomcli doctor`
manually only when permanent local-data deletion is intended.

Axiom Desktop and AxiomCLI share an installation identity and native account
session, but each account and frontend has a separate conversation journal.
Desktop state lives below
`accounts/<opaque-account-id>/desktop/state.sqlite3`; its app-owned chat cwd is
the sibling `desktop/chat/` directory. Signing out closes the active account
store instead of opening an installation-wide fallback. Sidebar folders and
thread membership are local database records; their names are UI labels and
never filesystem paths. The desktop product profile allows only Web tools, and
they are off by default. Enabling the composer Web toggle requires acknowledging
that queries and page requests leave the private inference environment. Users
can remember that warning per account on this device. New chats always start
with Web off. Existing chats save their explicit on/off choice in desktop local
storage, scoped to the account and thread, and restore it across app and sidecar
restarts. The warning preference is separate and cannot enable other chats;
missing or invalid saved choices default off. Each prompt explicitly opts in; the native
engine both withholds and blocks Web tools otherwise. The desktop does not
start configured MCP servers or grant local file/process access. Web
queries and fetched URLs have the same network privacy implications described
above.

Native sign-in is completed in the system browser using passkey, Google,
email/password, or Ethereum-wallet authentication. The native authorization is
bound to the initiating process with PKCE and a one-time device secret. Polling
is authoritative when the loopback notification is blocked; the callback only
accelerates completion. Method-specific passwords, OAuth codes, passkey
assertions, and wallet signatures stay in the browser and hosted auth surface.

The resulting short-lived access token is memory-only. Its rotating refresh
token is persisted only in the operating-system credential store; if that store
is unavailable, native sign-in fails closed and no plaintext file fallback is
used. Tokens are wrapped as secrets in memory, redacted from debug output,
excluded from renderer state, child environments, session journals, and release
artifacts. `AXIOM_API_KEY` remains a separate, explicit automation override for
standalone AxiomCLI and `axiom-proxy`; Axiom Desktop removes it from the sidecar
environment. Proxy clients use their own generated `AXIOM_PROXY_TOKEN`.
Account credit and Shieldz invoice status come from the API's immutable ledger,
not a smart contract or a local chat database. Desktop may receive bounded
invoice metadata and a short-lived HTTPS payment URL for a pending invoice. It
never receives Zcash or Ethereum private keys, constructs a transaction, or
grants the model payment authority; invoice URLs and ledger projections are not
written into conversation journals.
Ordinary logs, panic messages, security receipts, and desktop control messages
must contain only bounded metadata and fingerprints—not credentials, key
pairs, shared secrets, ciphertext dumps, plaintext request/response fields, or
raw attestation artifacts.

# Privacy behavior

AxiomCLI's default build performs provider attestation and end-to-end
encryption directly in-process through `axiom-secure-client`; it does not route
inference through the local proxy. For each inference request it fetches fresh
evidence, verifies the nonce, selected model, provider signing key, Intel and
NVIDIA evidence, and exact live TLS SPKI binding, then encrypts secret-bearing
fields to the verified model key. An attestation, binding, encryption, or
decryption failure stops the request without a plaintext or TLS-only fallback.

The Axiom relay necessarily sees authentication, billing, routing, provider and
model identifiers, public client key material, ciphertext, bounded inference
parameters, usage, and terminal status. It must not see plaintext prompts,
conversation content, reasoning, refusals, model output, tool names,
descriptions, schemas, arguments, or results. Current evidence verifies genuine
hardware and the runtime-provided workload manifest, but does not yet enforce
an Axiom-approved workload measurement allowlist. Status text and audit
evidence preserve that distinction.

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

`web_search` contacts the configured SearXNG service. Search terms and ordinary
HTTP metadata are therefore visible to that service and to whatever upstream
engines it uses. `fetch_url` is performed locally, with local/private/metadata
destinations and redirect pivots rejected. A configured MCP executable is
trusted user-selected local code and starts with that user's host authority;
therefore MCP commands are accepted only from trusted user configuration, not
repository configuration. Once connected, server descriptions, schemas,
results, and side-effect claims remain untrusted. Tool calls receive only their
explicit protocol arguments from AxiomCLI.

Session events are stored in a local SQLite database under the platform data
directory. AxiomCLI redacts known secret shapes before persistence, bounds
events/exports, and supports explicit export and retention commands. Redaction
is defense in depth, not permission to paste secrets into prompts. Session data
is not encrypted at rest by AxiomCLI; rely on operating-system account and disk
encryption protections.

`axiomcli sessions prune` can delete archived sessions under an explicit
retention policy. Package uninstall intentionally preserves user configuration
and session data. Remove the config/data paths printed by `axiomcli doctor`
manually only when permanent local-data deletion is intended.

Provider credentials come from browser authorization, direct API-key entry, or
the explicit `AXIOM_API_KEY` automation override. Persisted login uses the
operating-system keyring when available and otherwise an owner-only local file.
Browser authorization uses PKCE plus a one-time device secret: the website can
approve and trigger the loopback callback, but it never receives the permanent
API key. Its displayed `axc_` fallback is single-use and short-lived. The
credential is wrapped as a secret in memory, redacted from debug output,
excluded from child environments and session journals, and absent from release
artifacts. Proxy clients use a separate generated `AXIOM_PROXY_TOKEN`.
Ordinary logs, panic messages, security receipts, and desktop control messages
must contain only bounded metadata and fingerprints—not credentials, key
pairs, shared secrets, ciphertext dumps, plaintext request/response fields, or
raw attestation artifacts.

The legacy plaintext provider is compiled only for tests or with the explicit
`development-provider` Cargo feature. Selecting it also requires
`AXIOMCLI_UNATTESTED_DEV=1`; default and packaged binaries cannot select it.

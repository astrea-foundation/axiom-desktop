# Local OpenAI-compatible proxy

`axiom-proxy` accepts authenticated plaintext HTTP from local clients, then uses
the shared secure client for attested provider E2EE. It exposes `GET /v1/models`
and `POST /v1/chat/completions`. It translates tool definitions/calls/results but
never executes or authorizes tools for the caller. Desktop chat uses ACP directly.

## Standalone setup

Provide secrets through the process environment:

| Variable | Meaning |
|---|---|
| `AXIOM_API_KEY` | Upstream Axiom automation credential |
| `AXIOM_PROXY_TOKEN` | Separate random local bearer credential, at least 32 bytes |
| `AXIOM_BASE_URL` | Relay origin; default `https://api.axiom.stream` |
| `AXIOM_PROXY_BIND` | Numeric loopback address; default `127.0.0.1:8484` |
| `AXIOM_PROXY_MAX_CONCURRENCY` | Concurrent request limit; default 8 |
| `AXIOM_PROXY_COMPAT` | `lenient` (default) or `strict` |

```sh
cargo run --locked -p axiom-proxy
```

The first stdout line is a bounded JSON readiness record. Metadata-only
structured diagnostics go to stderr. Configure the external client's base URL
as the reported address plus `/v1`, and its bearer credential as the **local**
proxy token. Do not give the client the upstream Axiom key.

Only numeric loopback binding is accepted; this is not a remote multi-user
server. Any process holding the local token can submit/read plaintext on that
local interface. OS process isolation and token distribution matter even though
the remote inference hop is encrypted.

## Desktop supervision

The Proxy screen explicitly starts `axiomcli desktop-proxy`, which hosts the
same server library using the signed-in native account. Electron pins the
expected account/service origin and generates a new local token on each start.
The native process obtains and refreshes upstream tokens through the OS account
session. Copy token writes the local token directly to the clipboard.

Stop terminates the listener; Restart applies port changes and rotates the
local credential. Sign-out, account/runtime changes, app shutdown and supervisor
stdin EOF stop the hosted process. Status and evidence are bounded metadata
for that run. The standalone executable remains usable independently.

## Request compatibility

Lenient mode drops unsupported request parameters that have no relay equivalent
(such as `stop`, `seed` and penalties). Parameters that could affect the answer
are named in `x-axiom-ignored-parameters` and the metadata-only
`request_parameters_ignored` diagnostic. `user`, `metadata`, `store` and
`service_tier` are dropped without that report.

Strict mode rejects unsupported parameters and names them in the same header.
Both modes reject `n > 1` and unsupported content parts. Text parts are joined
with newlines. Inline `image_url` data URLs for PNG/JPEG/WebP/GIF are accepted
(up to 5 MiB per image, eight total attachments per message); remote URLs are rejected.
Inline `file` parts with `filename` and `file_data` data URLs are accepted up to
10 MiB per file, subject to the selected model's file MIME capabilities and the
16 MiB combined text/encoded-attachment bound. Only user messages may carry
images or files. File IDs, remote downloads and local extraction are unsupported.
Images require an image-capable NEAR or Tinfoil model from the refreshed catalog
and use its authenticated E2EE format. Unsupported capabilities fail before sending. Image `detail` is an
unsupported parameter (reported/ignored in lenient mode, rejected in strict mode).
Requests are bounded to 64 MiB of serialized JSON, matching the native relay body
bound; each message's text is bounded to 4 MiB. These are local resource limits.
For Tinfoil, an omitted output limit stays omitted, allowing upstream defaults;
an explicit limit is checked against the catalog ceiling. Model tool/streaming capability checks
still apply; compatibility mode never relaxes E2EE authentication.

The proxy implements Chat Completions, not every OpenAI endpoint. A third-party
client requiring a different endpoint needs an explicit supported integration.
It owns its tool execution, permission prompts and conversation retention.

## Failure behavior

Local authentication failure returns 401. Upstream insufficient credit returns
402. A sanitized upstream failure may return 502 for attestation rejection,
transport failure or invalid encrypted output. Missing authenticated completion,
stream reordering, decryption failure and protocol mismatches fail closed.
Retrying a potentially dispatched inference is not a safe recovery shortcut.
See [troubleshooting](troubleshooting.md#local-proxy).

Implementation: [proxy server](../apps/axiom-proxy/src/lib.rs),
[wire translation](../crates/axiom-openai-compat/src/lib.rs),
[native Desktop host](../apps/axiomcli/src/proxy.rs), and
[Electron supervisor](../apps/desktop/src/main/proxy-supervisor.ts).

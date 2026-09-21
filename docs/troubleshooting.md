# Troubleshooting

Start with `axiomcli doctor` for configuration, selected model, account/provider
and search diagnostics. It can make network requests; `--help` and `--version`
are offline checks. See [CLI](cli.md), [configuration](configuration.md) and
[installation](installing-desktop.md) for normal setup.

## Account and provider errors

Use `/account` in the TUI, or Desktop Settings, to inspect the current account.
`/login` opens the system browser. Native sessions require the OS credential
vault; a locked or denied vault cannot fall back to a plaintext token file.
If a loopback callback is blocked, the pending native flow can still complete
through PKCE polling. Keep the login flow open until completion or cancellation.

`AXIOM_API_KEY` selects automation credentials in CLI/proxy modes and prevents
native `/login` until unset. Desktop removes this override from its sidecar.
`axiomcli models` checks model discovery separately from inference.

Attestation, nonce, model/key, TLS-binding or response-authentication failures
must fail closed. A truncated stream is incomplete even if some text appeared.
Do not disable verification to work around a failure. The
[security guide](threat-model.md) distinguishes retryable transport/pre-send
failures from ambiguous or security failures.

For insufficient credit, inspect the native account's billing status. Current
Axiom payment mode uses a mainnet ZEC deposit address and indicative valuation;
legacy Shieldz invoices are available only when advertised by the backend.
See [Desktop credit and usage](desktop.md). Payment state never substitutes for
local response verification.

## Local proxy

[The proxy guide](proxy.md) owns environment variables, endpoints and limits.
For standalone use, set the upstream automation credential and an independent
local token of at least 32 bytes. Read the first stdout line for the JSON
readiness address, and give local clients that address plus `/v1` and the local
token. Diagnostic metadata goes to stderr.

HTTP 401 indicates missing/wrong local authorization; 402 indicates insufficient
credit. A sanitized `request_failed` record helps explain 502 errors, including
network, attestation or encrypted-response failure. Only numeric loopback binds
are supported. Desktop's Proxy screen manages its own account-bound instance.

## Local tools and approval

On Linux, `/usr/bin/bwrap` is required for process execution. Missing Bubblewrap
fails closed; sandboxed processes get no host network and only the sanitized
environment/mounts. If Cargo cannot find an installed toolchain, check that the
invoking account's Cargo/Rustup directories exist for read-only mounting.

macOS and Windows use host processes with a sanitized environment, not a
Bubblewrap-equivalent sandbox. Windows batch/PowerShell scripts require the
explicit shell path supported by the tool. Desktop Agent mode must be enabled
for local tools; Web is a separate control for built-in search/fetch.

The default `confirm` profile asks before tools; headless execution cannot answer
an approval and fails it closed. `full_access` removes tool prompts but does not
remove hard workspace boundaries. A denial or restrictive project rule cannot
be overridden by a session grant. See [profiles](configuration.md#trusted-user-configuration).
Use `permissions add-project-rule --action ask|deny --effect ...` to add a
restrictive repository rule; it cannot grant authority.

## Search and MCP

Hosted Axiom search requires a native account session, not an automation key.
A 429 means the rolling account quota was reached; honor `Retry-After`. A 502/503
indicates service/provider failure. There is no automatic provider fallback.
Search queries leave the device outside inference E2EE; see [privacy](privacy.md).

For an explicitly configured SearXNG service, use the
[real-service commands](evaluations.md#real-searxng-integration). Empty results
or engine warnings can reflect upstream throttling. Rejection of loopback/private
URLs by `fetch_url` is expected.

MCP server commands belong in trusted user configuration. Desktop's chat profile
does not connect them. A disconnected read-only tool may retry once only if its
name/schema are unchanged. A side-effecting operation with an ambiguous outcome
is never replayed. Keep server logs off ACP stdout.

## Session recovery

Use `axiomcli sessions list` and `axiomcli tui --resume SESSION_ID`. The canonical
workspace must still exist and match any `--cwd`, and the active account must own
the session. Signing out closes the account store; switching accounts does not
expose another account's threads. Desktop drafts and account data persist locally.

Incomplete turns are marked interrupted after restart; tools are not replayed.
Accounting reconciliation cannot turn incomplete text into a verified response.
Export with `sessions export` while the database can still be read. Unknown,
malformed or newer schemas are rejected; there is no general `sessions prune`
command or automatic destructive repair. Preserve the database for investigation.
[Versioning](versioning.md#local-database) describes supported migrations and the
narrow legacy reset path.

## Terminal display

The minimum useful viewport is 42×10 cells. `?` opens help, `/` the command menu,
and `Ctrl+F` history search. `Tab` focuses history; `Enter` toggles task details
and `v` opens its full viewer. `Esc` clears focus/closes an overlay. Two consecutive
`Ctrl+C` presses exit; the first displays confirmation.

Use `AXIOMCLI_COLOR=none`, `NO_COLOR=1`, `AXIOMCLI_ASCII=1` or
`AXIOMCLI_REDUCED_MOTION=1` for terminal compatibility/accessibility. Terminal
restoration is guarded for normal exit and panic.

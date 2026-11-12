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

On macOS, `stream.axiom.axiomcli` is the native login's Keychain item. A saved
session restore requires one password read; startup item discovery and writing
the rotated token do not read the password again. Keychain may separately ask
to trust a changed executable and its signing partition. If **Always Allow**
does not persist, check the exact requesting helper's signature and whether a
host is timing out and replacing that process while its dialog remains open.
An ad hoc development helper has a different code identity after rebuilding;
certificate-signed previews must use a consistent signing identity. Do not
delete login credentials or grant all applications access to work around this.

Background ACP hosts must use `axiomcli acp --no-keychain-ui`. On macOS this
disables Keychain interaction before authentication initializes, for that
process only. A locked or unapproved item returns unavailable instead of opening
a dialog that can survive the host's probe timeout. Explicit user sign-in and
ordinary Desktop/CLI launches omit this flag. Older clients reject the unknown
flag before reading any credential; hosts must not retry without it.

Desktop preserves the native sign-in failure reason in its error banner, including
TLS connection causes. When starting sign-in fails with `InvalidContentType`,
the banner says **Couldn’t connect securely**, explains that browser sign-in could
not start, and suggests checking VPN, proxy or network-filter settings or trying
another network. Expand **Technical details** for the original native error.
This explanation does not identify which component sent the invalid data.
Version 0.1.7 can hide failures while starting sign-in behind the generic
`Internal error` message. A failed start can happen before a request reaches the
account service, so server logs alone cannot identify it.

If sign-in and the update check both fail to connect, compare the same check on
another network (for example, a phone hotspot). On macOS, run the bundled native
client from Terminal to check the same updater without installing anything:

```sh
/Applications/Axiom.app/Contents/Resources/bin/axiomcli update --check
```

`InvalidContentType` in the TLS error means the client encountered an invalid TLS
record type, before receiving the release HTTP response. Check proxy, VPN and
network-filter settings; that error alone does not identify which component sent
the invalid data. Keep certificate verification enabled.

`AXIOM_API_KEY` selects automation credentials in CLI/proxy modes and prevents
native `/login` until unset. Desktop removes this override from its sidecar.
`axiomcli models` checks model discovery separately from inference.

Attestation, nonce, model/key, TLS-binding or response-authentication failures
must fail closed. A truncated stream is incomplete even if some text appeared.
Do not disable verification to work around a failure. The
[security guide](threat-model.md) distinguishes retryable transport/pre-send
failures from ambiguous or security failures.

For insufficient credit, inspect the native account's billing status. Current
Axiom payment mode uses a mainnet ZEC deposit address and indicative valuation.
See [Desktop credit and usage](desktop.md). Payment state never substitutes for
local response verification.

A rate-limit error can mean temporary service throttling or too many active
requests for the account. Wait for a running reply to finish, or stop it before
trying again. Axiom does not automatically replay the inference request. A
rate-limit rejection recorded before any reply shows **No reply received**;
response-authentication failures still show **Reply check failed**.

**Balance unavailable** means the balance could not be fetched, not that the
account has no credit. Use **Refresh balance** to retry. Valid credit remains
visible if only deposit provisioning is temporarily unavailable; the address
stays pending until it can be validated. Billing and inference have separate
limits, so a balance error alone does not identify the cause of a chat rejection.

## Completely reset local data

Reinstalling keeps Axiom's account data. To erase it, open **Settings → Account →
Factory reset Axiom…**, type `RESET`, and choose **Delete everything and restart**.
The control works while signed out or disconnected. Close other AxiomCLI windows
first. See [configuration](configuration.md#paths-and-local-state) for the full
deletion scope and the offline `axiomcli factory-reset --confirm` command.

On older versions without factory reset, quit Desktop, CLI and local proxies,
then remove the application's directories and its saved credentials. This
permanently removes local history for every account. These are the default paths;
explicit XDG overrides change native config/data locations.

| Platform | App-owned directories to remove if present |
|---|---|
| Windows | `%APPDATA%\Axiom`, `%APPDATA%\axiom-desktop`, `%APPDATA%\axiom`, `%LOCALAPPDATA%\axiom`, `%LOCALAPPDATA%\Axiom`, `%LOCALAPPDATA%\axiom-desktop` |
| macOS | `~/Library/Application Support/Axiom`, `~/Library/Application Support/axiom-desktop`, `~/Library/Application Support/axiom`, `~/Library/Caches/Axiom`, `~/Library/Caches/axiom-desktop`, `~/Library/Caches/axiom`, `~/Library/Logs/Axiom`, `~/Library/Logs/axiom-desktop` |
| Linux | `${XDG_CONFIG_HOME:-~/.config}/Axiom`, `${XDG_CONFIG_HOME:-~/.config}/axiom-desktop`, `${XDG_CONFIG_HOME:-~/.config}/axiom`, `${XDG_DATA_HOME:-~/.local/share}/axiom`, `${XDG_CACHE_HOME:-~/.cache}/Axiom`, `${XDG_CACHE_HOME:-~/.cache}/axiom-desktop`, `${XDG_CACHE_HOME:-~/.cache}/axiom` |

On a case-insensitive filesystem, `Axiom` and `axiom` refer to the same directory.
In **Credential Manager → Windows Credentials**, **Keychain Access** on macOS,
or the system **Passwords and Keys** app on Linux, remove Axiom entries with
service names `stream.axiom.axiomcli` and `stream.axiom.desktop.mcp`. Windows
generic credentials include these service names after the account name in their
target. Leave other applications' credentials alone, then reopen Axiom and sign
in. The online account, balance and hosted API keys remain available; external
browser sessions and files saved outside Axiom-managed folders are kept.

If a TLS error such as `InvalidContentType` persists after a clean reset, compare
the native client's update check on another network and inspect VPN, proxy and
network-filter settings as described above. A clean profile does not repair a
broken TLS connection.

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

Only `web_search_provider = "axiom"` is supported. If startup rejects an older
provider setting, remove that override or explicitly select `axiom` after
reviewing the hosted-search privacy behavior. Rejection of loopback/private
URLs by `fetch_url` is expected.

CLI MCP server commands belong in trusted user configuration. In Desktop, add
and test [connections](mcp.md) in Settings, then choose tools in the conversation's
Tools menu. Check that the executable and its runtime are installed and reachable
on the sanitized PATH; use an absolute path if needed. Locked/missing environment
credentials require unlocking the OS credential store or saving them again. A disconnected read-only tool may retry once only if its
name/schema are unchanged. A side-effecting operation with an ambiguous outcome
is never replayed. Keep server logs off ACP stdout.

## Session recovery

Use `axiomcli sessions list` and `axiomcli tui --resume SESSION_ID`. The canonical
workspace must still exist and match any `--cwd`, and the active account must own
the session. Signing out closes the account store; switching accounts does not
expose another account's threads. Desktop drafts and account data persist locally.

Desktop holds pending messages after a reconnect rather than automatically
resending them. On the welcome screen, **Review draft** moves a saved first
message back into an empty composer; review it and press Send. In an existing
thread, check the transcript before using **Send now** when delivery is uncertain.
The composer's lock icon indicates encrypted model conversations and does not
disable typing.

Incomplete turns are marked interrupted after their owning process exits; tools
are not replayed. Recovery acquires the same per-thread OS lock as execution.
Opening the store from another client preserves live turns and compaction;
repeated probes never expire a healthy owner's lock.
An unavailable saved attachment, damaged image result, or incomplete tool reply
is omitted or marked unknown in model context so usable text can still be loaded,
edited and sent. The original saved rows are retained. If automatic summarization
fails before a new message, the turn reports an error and retains the original
context for a retry or a larger-context model. Expired image
tool references can be attached again without starting a new conversation.
Accounting reconciliation cannot turn incomplete text into a verified response.
Export with `sessions export` while the database can still be read. Unknown,
malformed or newer schemas are rejected; there is no general `sessions prune`
command or automatic destructive repair. Preserve the database for investigation.
[Versioning](versioning.md#local-database) describes supported migrations and the
account-scoped explicit reset path.

## Terminal display

The minimum useful viewport is 42×10 cells. `?` opens help, `/` the command menu,
and `Ctrl+F` history search. `Tab` focuses history; `Enter` toggles task details
and `v` opens its full viewer. `Esc` clears focus/closes an overlay. Two consecutive
`Ctrl+C` presses exit; the first displays confirmation.

Use `AXIOMCLI_COLOR=none`, `NO_COLOR=1`, `AXIOMCLI_ASCII=1` or
`AXIOMCLI_REDUCED_MOTION=1` for terminal compatibility/accessibility. Terminal
restoration is guarded for normal exit and panic.

## Trial and invoice capacity

If free trials are at capacity, use the displayed retry time or add paid credit.
A running response continues; this limit affects new trial-funded requests. Retry
times can move when other admitted requests settle.

If five invoices are active, open Balance → Other crypto → Recent payments and
cancel an unpaid invoice. This releases a slot immediately. Refreshed rate quotes
keep the same six-hour invoice deadline. Use Check payment status for an expired
or cancelled invoice if a transfer was already sent; do not resend to stale rates.

## Updates

Use `axiomcli update --check` or Settings → Updates to inspect a failure. Close
other installed AxiomCLI/ACP sessions and proxies before applying; a bundled TUI
also needs Desktop to close. The helper waits at most three minutes and does not
kill those processes. Keep using the installed Windows launcher for TUI updates.

Signature, unknown-key, changed-inventory and checksum errors fail closed. Retry
after connectivity is restored; do not bypass verification. Source builds require
a rebuild. A release without a matching platform/package is not substituted with
another product. Unsigned previews have stable updates disabled.

If installation or OS authorization fails, the helper records the error in the
per-installation `axiom/updates` directory under the OS user cache. The next app
check displays it. Retry the same verified installer from the public release to
repair program files; account data is outside the installation. Staging directories
can be removed once no update helper is running. Linux standalone keeps previous
version directories, but do not reactivate one against incompatible account data.

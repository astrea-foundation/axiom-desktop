# Troubleshooting

Start with `axiomcli doctor`. It reports the selected model, Axiom relay URL,
secure-provider mode, permission profile, SearXNG health, and local
configuration/session paths without printing credentials.

## Provider or model errors

- Run `/account` in the TUI or `axiomcli doctor` to validate the current login.
  If no native session is present, use `/login` and complete passkey, Google,
  email/password, or Ethereum-wallet sign-in in the system browser. Desktop's
  sign-in buttons open the same hosted authorization with a method hint.
- Run `axiomcli models` to test bounded model discovery.
- Set `AXIOM_API_KEY` only as an automation override. It prevents `/login` from
  starting a native account session until the environment variable is unset;
  Axiom Desktop deliberately removes it from the sidecar environment.
- Native sessions require the operating-system credential store. A locked,
  denied, or unavailable Keychain/Credential Manager leaves Axiom signed out;
  there is no plaintext file fallback.
- If browser approval succeeds but the loopback callback is blocked, leave the
  login window open. PKCE polling completes the authorization; the loopback
  callback only accelerates it.
- Authentication errors are not retried; rate limits and transient errors use
  bounded retries with a stable idempotency key.
- A truncated or malformed stream fails the turn. It is never treated as a
  complete answer.
- Attestation rejection, stale/mismatched nonces, model/key mismatches, TLS
  binding failures, and ciphertext authentication failures are security
  failures. AxiomCLI does not retry them through an insecure transport.
- An insufficient-credit response directs the user to create or inspect a
  Shieldz Zcash invoice. It never grants the renderer payment authority or
  weakens provider-E2EE verification.

## Local proxy fails

- Set `AXIOM_API_KEY` for the upstream service and generate an independent
  `AXIOM_PROXY_TOKEN` of at least 32 bytes for local clients.
- Start `axiom-proxy` and read its first stdout line as the bounded JSON
  readiness record. Logs are JSON metadata on stderr.
- Configure the OpenAI-compatible client with the readiness address plus `/v1`
  and the local token—not the Axiom credential.
- HTTP 401 means the local token is absent or wrong. HTTP 402 means the Axiom
  account needs credit. HTTP 502 can represent unavailable/rejected
  attestation, a network failure, invalid encrypted output, or another
  fail-closed upstream error; inspect the sanitized `request_failed` record.
- Only numeric loopback addresses are accepted. LAN exposure is intentionally
  unsupported in this version.

## A command will not run

On Linux, `/usr/bin/bwrap` is required. AxiomCLI fails closed if it is absent.
The default `confirm` profile asks before every tool invocation. `full_access`
removes those tool prompts, while `none`, `web`, and `observe` hide tools that
fall outside their declared purpose. Sandboxed processes receive no network and
only an allowlisted environment regardless of tool profile.

If Cargo cannot find an already installed toolchain inside the sandbox, verify
that the invoking account’s Cargo and Rustup directories exist. AxiomCLI mounts
those caches read-only into the sandbox.

## Search fails

The default `web_search_provider = "axiom"` uses the authenticated Axiom API,
backed by Decodo. Sign in with a native account session; automation keys do not
authorize free search. A 429 means the account used its 60 searches in the past
60 seconds; wait for `Retry-After` instead of retrying immediately. A 502/503
indicates an unavailable provider or hosted service. No fallback runs automatically.

To explicitly use the retained alternative, set `web_search_provider = "searxng"`.
Run `scripts/searxng-dev start` and then `scripts/searxng-dev query "test"` in a
source checkout, or point `searxng_url` at a SearXNG service you operate or
otherwise trust. The hosted search implementation lives in the Axiom backend repository.
Individual upstream engines can throttle or time out. Empty results and engine
warnings are returned honestly. `fetch_url` rejecting a loopback/private target
is expected security behavior.

## MCP fails or disconnects

MCP commands must be in trusted user configuration, not repository config. A
read-only tool is retried once after reconnect only when its name and schema are
unchanged. A side-effecting disconnect reports an unknown outcome and is never
replayed. Inspect the bounded error and the server’s own stderr without routing
MCP logs into ACP stdout.

## Session recovery

Use `axiomcli sessions list` and `axiomcli tui --resume SESSION_ID`. The saved
workspace must still exist, belong to the active account, and match any supplied
`--cwd`. Signing out closes the account store; signing into another account does
not expose the prior account's sessions. Interrupted tools are shown as unknown
and are not replayed. Use `sessions export` before pruning or manually repairing
data. Corrupt tails are reported and recovery stops before the damaged event
rather than deleting evidence.

## Terminal display

The minimum useful viewport is 42×10 cells. Press `?` for keyboard help. Set
`AXIOMCLI_COLOR=none`, `NO_COLOR=1`, `AXIOMCLI_ASCII=1`, or
`AXIOMCLI_REDUCED_MOTION=1` for accessibility/compatibility. `Esc` clears the
current focus or closes a temporary overlay. Press `Ctrl+C` twice consecutively
to exit; the first press displays a confirmation in the status line. Terminal
restoration is guarded across normal exit and panic. Use `Ctrl+F` to search the
work history, including folded task details; `/` opens the command menu. Press
`Tab` to focus the work history, then `Enter` to show or
hide a selected task's reasoning/tool details, or `v` for the full viewer.

## Permission was denied

Read the normalized effect shown by the prompt. `n` denies, `y` permits one
operation, and—when offered—`e`/`p` grant an exact or prefix scope for the
current session. Configured asks/denials and hard workspace boundaries cannot
be overridden by a session grant. Headless mode cannot answer approvals and
therefore fails them closed.

To persist a restrictive repository rule through the explicit configuration
flow, use `axiomcli permissions add-project-rule --action ask|deny --effect ...`.
This command can only add `ask` or `deny` rules; project configuration cannot
grant authority.

# Troubleshooting

Start with `axiomcli doctor`. It reports the selected model, Axiom relay URL,
secure-provider mode, permission profile, SearXNG health, and local
configuration/session paths without printing credentials.

## Provider or model errors

- Confirm `AXIOM_API_KEY` is present in the AxiomCLI process environment and
  the configured HTTPS Axiom service is reachable.
- Run `axiomcli models` to test bounded model discovery.
- Set `AXIOM_API_KEY` in the process environment only if the endpoint requires
  it. Authentication errors are not retried; rate limits and transient errors
  use bounded retries with a stable idempotency key.
- A truncated or malformed stream fails the turn. It is never treated as a
  complete answer.
- Attestation rejection, stale/mismatched nonces, model/key mismatches, TLS
  binding failures, and ciphertext authentication failures are security
  failures. AxiomCLI does not retry them through an insecure transport.
- An insufficient-credit response includes a link to `axiom.stream` for a top
  up and preserves the partial response already journaled before the failure.

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
The `workspace` profile automatically permits only its narrow development
command list; other structured programs require confirmation. Shell commands
always require confirmation. The process receives no network and only an
allowlisted environment.

If Cargo cannot find an already installed toolchain inside the sandbox, verify
that the invoking account’s Cargo and Rustup directories exist. AxiomCLI mounts
those caches read-only into the sandbox.

## Search fails

Run `scripts/searxng-dev start` and then `scripts/searxng-dev query "test"` in a
source checkout, or point `searxng_url` at a SearXNG service you operate or
otherwise trust. A hosted Axiom service is not part of this repository.
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
workspace must still exist and must match any supplied `--cwd`. Interrupted
tools are shown as unknown and are not replayed. Use `sessions export` before
pruning or manually repairing data. Corrupt tails are reported and recovery
stops before the damaged event rather than deleting evidence.

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
